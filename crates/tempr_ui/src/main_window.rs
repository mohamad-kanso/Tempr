//! Root view of the application window.
//!
//! Layout: SQL `EditorView` (top) · `ResultGrid` (middle) · status bar (bottom) · `Palette` overlay.
//! Holds service handles + render state only (D6). Query execution happens
//! in `QueryService` on the tokio runtime; rows and bus events reach this
//! view through channels drained on the foreground executor.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription, Window,
    actions, div, prelude::*, px, rgb,
};
use tempr_domain::{
    Batch, ColumnSpec, Connection, ConnectionId, ConnectionState, QueryOutcome, QueryRunId,
};
use tempr_events::EventBus;
use tempr_services::{CommandService, ConnectionService, QueryService, RowSink, ServiceError};

use crate::commands;
use crate::components::{EditorEvent, EditorView, Palette, PaletteEvent, ResultGrid};
use crate::events::{self, UiEvent};
use crate::gpui_compat;
use crate::scroll_bench::ScrollBench;
use crate::theme;

actions!(
    main_window,
    [
        RunQuery,
        CancelQuery,
        DebugScrollBenchmark,
        TogglePalette,
        Quit
    ]
);

pub const KEY_CONTEXT: &str = "MainWindow";
/// Binding context for main-window commands: active only while no overlay
/// (anything adding `palette::MODAL_CONTEXT` to its key context) is open.
pub const KEY_CONTEXT_NOT_PALETTE: &str = "MainWindow && !Modal";

/// How many frames the scroll benchmark spreads the row sweep over.
const BENCH_TARGET_FRAMES: usize = 600;

/// Messages from the tokio-side [`RowSink`] to the grid.
enum GridMsg {
    Columns(Vec<ColumnSpec>),
    Batch(Batch),
}

struct ChannelSink(UnboundedSender<GridMsg>);

impl RowSink for ChannelSink {
    fn columns(&self, columns: &[ColumnSpec]) {
        let _ = self.0.unbounded_send(GridMsg::Columns(columns.to_vec()));
    }
    fn batch(&self, batch: Batch) {
        let _ = self.0.unbounded_send(GridMsg::Batch(batch));
    }
}

/// Developer knobs, resolved by the binary (env vars) — the UI crate never
/// reads the environment itself.
#[derive(Debug, Clone, Default)]
pub struct DevOptions {
    /// Statement to run as soon as the connection is up.
    pub startup_sql: Option<String>,
    /// After the first query completes, run the scroll benchmark, log the
    /// report, and quit. Any failure on the way also quits (non-zero log
    /// line), so a headless run never hangs.
    pub bench_scroll_then_exit: bool,
    /// Something the binary wants shown in the status bar at startup (e.g.
    /// "settings.toml ignored: …").
    pub startup_notice: Option<String>,
}

/// Service handles the window needs. Built by the binary before GPUI starts.
pub struct Services {
    pub bus: Arc<EventBus>,
    pub connection: Arc<ConnectionService>,
    pub query: Arc<QueryService>,
    pub command: Arc<CommandService>,
}

/// Outcome of the tokio-side query task, as seen by the view.
type QueryTaskOutcome = Result<Result<QueryRunId, ServiceError>, tokio::task::JoinError>;

pub struct MainWindow {
    services: Services,
    /// Connection to use for queries; `None` when no `DATABASE_URL` was given.
    connection: Option<Connection>,
    connection_state: ConnectionState,
    editor: Entity<EditorView>,
    grid: Entity<ResultGrid>,
    palette: Entity<Palette>,
    status: String,
    status_is_error: bool,
    /// The run in flight, allocated by the view before the task starts so
    /// `CancelQuery` can target it immediately. Cleared only by the task's
    /// outcome — the single owner of "is a query running".
    current_run: Option<QueryRunId>,
    dev: DevOptions,
    bench: Option<ScrollBench>,
    focus_handle: FocusHandle,
    _editor_subscription: Subscription,
    _palette_subscription: Subscription,
    _bus_subscription: tempr_events::Subscription,
}

impl MainWindow {
    pub fn new(
        services: Services,
        connection: Option<Connection>,
        dev: DevOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| EditorView::new("", cx));
        let grid = cx.new(|_| ResultGrid::new());
        let command_service = services.command.clone();
        let palette = cx.new(|cx| Palette::new(command_service, window, cx));

        // The palette restores the previous focus itself before emitting.
        let _palette_subscription =
            cx.subscribe_in(&palette, window, |this, _palette, event, window, cx| {
                if let PaletteEvent::Execute(id) = event
                    && !commands::dispatch(id, &this.services.command, window, cx)
                {
                    this.set_status(format!("Command {id} is not available here"), true, cx);
                }
                cx.notify();
            });

        let _editor_subscription = cx.subscribe_in(
            &editor,
            window,
            |this, _editor, event, window, cx| match event {
                EditorEvent::Run(sql) => this.run_sql(sql.clone(), window, cx),
                EditorEvent::Notice(text) => this.set_status(text.clone(), true, cx),
                EditorEvent::Changed => {}
            },
        );

        // Bus → UI: drain the bridge channel on the foreground executor.
        let (_bus_subscription, mut rx) = events::bridge(&services.bus);
        cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.on_ui_event(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        window.focus(&editor.focus_handle(cx), cx);

        let status = match &connection {
            Some(c) => format!(
                "Connecting to {}@{}:{}/{} …",
                c.username, c.host, c.port, c.database
            ),
            None => {
                "No connection: set DATABASE_URL (postgres://user:pass@host:port/db) and restart."
                    .to_string()
            }
        };

        let startup_notice = dev.startup_notice.clone();
        let mut this = Self {
            services,
            connection,
            connection_state: ConnectionState::Disconnected,
            editor,
            grid,
            palette,
            status,
            status_is_error: false,
            current_run: None,
            dev,
            bench: None,
            focus_handle: cx.focus_handle(),
            _editor_subscription,
            _palette_subscription,
            _bus_subscription,
        };
        this.connect(window, cx);
        if let Some(notice) = startup_notice {
            this.set_status(notice, true, cx);
        }
        this
    }

    fn connection_id(&self) -> Option<ConnectionId> {
        self.connection.as_ref().map(|c| c.id)
    }

    fn set_status(&mut self, text: impl Into<String>, is_error: bool, cx: &mut Context<Self>) {
        self.status = text.into();
        self.status_is_error = is_error;
        cx.notify();
    }

    /// Report a failure. In headless bench mode every failure is terminal:
    /// log it and quit so the run never hangs.
    fn fail(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if self.dev.bench_scroll_then_exit {
            tracing::error!("{text} — aborting headless run");
            cx.quit();
        }
        self.set_status(text, true, cx);
    }

    // ── connection ───────────────────────────────────────────────────────

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            if self.dev.bench_scroll_then_exit {
                self.fail("No connection configured", cx);
            }
            return;
        };
        let service = self.services.connection.clone();
        let task = gpui_compat::spawn_tokio(cx, async move { service.connect(&connection).await });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            this.update_in(cx, |this, _window, cx| match outcome {
                Ok(Ok(())) => {}
                Ok(Err(e)) => this.fail(format!("Connection failed: {e}"), cx),
                Err(join) => this.fail(format!("Connection task failed: {join}"), cx),
            })
            .ok();
        })
        .detach();
    }

    // ── actions ──────────────────────────────────────────────────────────

    /// Run the statement under the editor's cursor (or its selection); the
    /// editor emits `EditorEvent::Run`, handled above.
    fn run_query_action(&mut self, _: &RunQuery, _: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.run_statement_under_cursor(cx));
    }

    fn cancel_query_action(&mut self, _: &CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.current_run else {
            self.set_status("No query running.", false, cx);
            return;
        };
        self.set_status("Cancelling…", false, cx);
        let query_service = self.services.query.clone();
        gpui_compat::spawn_tokio(cx, async move {
            if let Err(e) = query_service.cancel(run).await {
                tracing::warn!(error = %e, "cancel failed");
            }
        })
        .detach();
    }

    fn bench_action(
        &mut self,
        _: &DebugScrollBenchmark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_bench(window, cx);
    }

    fn toggle_palette_action(
        &mut self,
        _: &TogglePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.palette.read(cx).is_open();
        self.palette.update(cx, |p, cx| {
            if open {
                p.close(window, cx)
            } else {
                p.open(window, cx)
            }
        });
    }

    // ── query lifecycle ──────────────────────────────────────────────────

    fn run_sql(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let sql = sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        let Some(connection_id) = self.connection_id() else {
            self.fail("No connection configured (DATABASE_URL).", cx);
            return;
        };
        if self.connection_state != ConnectionState::Connected {
            self.fail(
                format!(
                    "Not connected ({:?}); cannot run query.",
                    self.connection_state
                ),
                cx,
            );
            return;
        }
        if self.current_run.is_some() {
            self.set_status("A query is already running.", true, cx);
            return;
        }

        let run_id = QueryRunId::new();
        self.current_run = Some(run_id);
        self.grid.update(cx, |grid, cx| {
            grid.reset();
            cx.notify();
        });
        self.set_status("Running…", false, cx);

        let (tx, mut rx) = unbounded::<GridMsg>();
        let sink: Arc<dyn RowSink> = Arc::new(ChannelSink(tx));
        let query_service = self.services.query.clone();

        // 1. DB work on tokio. The sink (and its sender) is dropped when the
        //    service finishes, which closes the channel and ends the loop below.
        let task = gpui_compat::spawn_tokio(cx, async move {
            query_service
                .execute_streaming_with_id(run_id, &sql, connection_id, sink)
                .await
        });

        // 2. Rows → grid on the foreground executor, one notify per batch.
        let grid = self.grid.clone();
        cx.spawn(async move |_this, cx| {
            while let Some(msg) = rx.next().await {
                grid.update(cx, |grid, cx| {
                    match msg {
                        GridMsg::Columns(columns) => grid.set_columns(columns),
                        GridMsg::Batch(batch) => grid.append(batch),
                    }
                    cx.notify();
                });
            }
        })
        .detach();

        // 3. Final outcome → status; runs after (2) has drained (FIFO on the
        //    main thread), so row counts are final.
        cx.spawn_in(window, async move |this, cx| {
            let outcome = task.await;
            this.update_in(cx, |this, window, cx| {
                this.on_query_outcome(run_id, outcome, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn on_query_outcome(
        &mut self,
        run_id: QueryRunId,
        outcome: QueryTaskOutcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.current_run == Some(run_id) {
            self.current_run = None;
        }
        match outcome {
            Ok(Ok(run)) => {
                let rows = self.grid.read(cx).row_count();
                let cols = self.grid.read(cx).column_count();
                if cols == 0 {
                    self.grid.update(cx, |g, cx| {
                        g.set_message("Statement executed; no rows returned.");
                        cx.notify();
                    });
                }
                let cancelled = self
                    .services
                    .query
                    .completed_run(run)
                    .is_some_and(|r| r.outcome == QueryOutcome::Cancelled);
                if cancelled {
                    self.set_status(
                        format!("Cancelled — {rows} {} (partial)", row_word(rows)),
                        false,
                        cx,
                    );
                } else {
                    self.set_status(format!("Done — {rows} {}", row_word(rows)), false, cx);
                }
                if self.dev.bench_scroll_then_exit {
                    self.start_bench(window, cx);
                }
            }
            Ok(Err(e)) => {
                self.grid.update(cx, |g, cx| {
                    g.set_message(format!("Query failed: {e}"));
                    cx.notify();
                });
                self.fail(format!("Query failed: {e}"), cx);
            }
            Err(join) => self.fail(format!("Query task failed: {join}"), cx),
        }
    }

    fn on_ui_event(&mut self, event: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            UiEvent::ConnectionStateChanged { id, state } if Some(id) == self.connection_id() => {
                self.connection_state = state;
                let text = match state {
                    ConnectionState::Connecting => "Connecting…".to_string(),
                    ConnectionState::Connected => match &self.connection {
                        Some(c) => format!(
                            "Connected to {}@{}:{}/{}",
                            c.username, c.host, c.port, c.database
                        ),
                        None => "Connected".to_string(),
                    },
                    ConnectionState::Reconnecting => "Reconnecting…".to_string(),
                    ConnectionState::Failed => "Connection failed".to_string(),
                    ConnectionState::Disconnected => "Disconnected".to_string(),
                };
                if state == ConnectionState::Failed {
                    self.fail(text, cx);
                } else {
                    self.set_status(text, false, cx);
                }
                if state == ConnectionState::Connected
                    && let Some(sql) = self.dev.startup_sql.take()
                {
                    self.editor
                        .update(cx, |editor, cx| editor.set_text(&sql, cx));
                    self.run_sql(sql, window, cx);
                }
            }
            UiEvent::RowsReceived { run, .. } if self.current_run == Some(run) => {
                let rows = self.grid.read(cx).row_count();
                self.set_status(format!("Running… {rows} {}", row_word(rows)), false, cx);
            }
            // QueryStarted/QueryFinished: the outcome task owns final status.
            _ => {}
        }
    }

    // ── scroll benchmark ─────────────────────────────────────────────────

    /// Sweep the grid from top to bottom, one `on_next_frame` per step, and
    /// report frame times. Diagnostic for the 60 fps acceptance criterion.
    fn start_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.grid.read(cx).row_count();
        if rows == 0 {
            self.fail("Scroll benchmark: no rows to scroll.", cx);
            return;
        }
        if self.bench.is_some() {
            return;
        }
        let bench = ScrollBench::new(rows, BENCH_TARGET_FRAMES);
        tracing::info!(rows, step = bench.step(), "scroll bench: starting");
        self.set_status(
            format!(
                "Scroll benchmark: {rows} rows, {} rows/frame…",
                bench.step()
            ),
            false,
            cx,
        );
        self.bench = Some(bench);
        cx.on_next_frame(window, |this, window, cx| this.bench_tick(window, cx));
    }

    fn bench_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.as_mut() else {
            return;
        };
        match bench.tick(Instant::now()) {
            Some(row) => {
                if bench.frames_recorded().is_multiple_of(100) {
                    let rendered = self.grid.read(cx).last_rendered_rows();
                    tracing::info!(
                        row,
                        frames = bench.frames_recorded(),
                        rendered_rows = ?rendered,
                        "scroll bench: progress"
                    );
                }
                self.grid.update(cx, |grid, cx| {
                    grid.scroll_to_row(row);
                    cx.notify();
                });
                cx.on_next_frame(window, |this, window, cx| this.bench_tick(window, cx));
            }
            None => {
                let report = bench.report();
                self.bench = None;
                let summary = report.summary();
                tracing::info!(
                    frames = report.frames,
                    avg_ms = report.avg.as_secs_f64() * 1e3,
                    p95_ms = report.p95.as_secs_f64() * 1e3,
                    max_ms = report.max.as_secs_f64() * 1e3,
                    dropped = report.dropped,
                    "{summary}"
                );
                self.set_status(summary, report.dropped > 0, cx);
                if self.dev.bench_scroll_then_exit {
                    cx.quit();
                }
            }
        }
    }

    fn connection_dot_color(&self) -> u32 {
        match self.connection_state {
            ConnectionState::Connected => theme::SUCCESS,
            ConnectionState::Connecting | ConnectionState::Reconnecting => theme::ACCENT,
            ConnectionState::Failed => theme::ERROR,
            ConnectionState::Disconnected => theme::TEXT_DIM,
        }
    }
}

/// "row"/"rows" for a status line count.
fn row_word(rows: usize) -> &'static str {
    if rows == 1 { "row" } else { "rows" }
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::run_query_action))
            .on_action(cx.listener(Self::cancel_query_action))
            .on_action(cx.listener(Self::bench_action))
            .on_action(cx.listener(Self::toggle_palette_action))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme::SURFACE))
            .text_color(rgb(theme::TEXT))
            .text_size(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .bg(rgb(theme::SURFACE_RAISED))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(div().text_color(rgb(theme::ACCENT)).child("SQL"))
                    .child(
                        div()
                            .flex_1()
                            .text_color(rgb(theme::TEXT_DIM))
                            .text_size(px(12.))
                            .child("ctrl-enter runs the statement under the cursor · ctrl-shift-enter runs all · ctrl-shift-p commands"),
                    ),
            )
            .child(
                div()
                    .h(px(260.))
                    .border_b_1()
                    .border_color(rgb(theme::BORDER))
                    .child(self.editor.clone()),
            )
            .child(div().flex_1().min_h_0().child(self.grid.clone()))
            .child(self.palette.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .px_3()
                    .bg(rgb(theme::SURFACE_RAISED))
                    .border_t_1()
                    .border_color(rgb(theme::BORDER))
                    .text_size(px(12.))
                    .child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .bg(rgb(self.connection_dot_color())),
                    )
                    .child(
                        div()
                            .text_color(rgb(if self.status_is_error {
                                theme::ERROR
                            } else {
                                theme::TEXT_DIM
                            }))
                            .child(self.status.clone()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::row_word;

    #[test]
    fn row_word_is_singular_only_for_one() {
        assert_eq!(row_word(0), "rows");
        assert_eq!(row_word(1), "row");
        assert_eq!(row_word(2), "rows");
    }
}
