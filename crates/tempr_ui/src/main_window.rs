//! Root view of the application window.
//!
//! Layout: SQL `Input` (top) · `ResultGrid` (middle) · status bar (bottom).
//! Holds service handles + render state only (D6). Query execution happens
//! in `QueryService` on the tokio runtime; rows and bus events reach this
//! view through channels drained with `cx.spawn`.

use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render, Subscription,
    Window, actions, div, prelude::*, px, rgb,
};
use tempr_domain::{Batch, ColumnSpec, Connection, ConnectionId, ConnectionState, QueryOutcome};
use tempr_events::EventBus;
use tempr_services::{ConnectionService, QueryService, RowSink};

use crate::components::{Input, InputEvent, ResultGrid};
use crate::events::{self, UiEvent};
use crate::gpui_compat;
use crate::scroll_bench::ScrollBench;
use crate::theme;

actions!(
    main_window,
    [RunQuery, CancelQuery, DebugScrollBenchmark, Quit]
);

/// How many frames the scroll benchmark spreads the row sweep over.
const BENCH_TARGET_FRAMES: usize = 600;

pub const KEY_CONTEXT: &str = "MainWindow";

/// Register global + main-window keybindings. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-enter", RunQuery, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-enter", RunQuery, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CancelQuery, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-b", DebugScrollBenchmark, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-shift-b", DebugScrollBenchmark, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    crate::components::input::bind_keys(cx);
}

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
    /// report, and quit. Used for headless frame-time measurement.
    pub bench_scroll_then_exit: bool,
}

/// Service handles the window needs. Built by the binary before GPUI starts.
pub struct Services {
    pub bus: Arc<EventBus>,
    pub connection: Arc<ConnectionService>,
    pub query: Arc<QueryService>,
}

pub struct MainWindow {
    services: Services,
    /// Connection to use for queries; `None` when no `DATABASE_URL` was given.
    connection: Option<Connection>,
    connection_state: ConnectionState,
    input: Entity<Input>,
    grid: Entity<ResultGrid>,
    status: String,
    status_is_error: bool,
    query_running: bool,
    dev: DevOptions,
    /// Set when a benchmark should start on the next rendered frame.
    bench_pending: bool,
    bench: Option<ScrollBench>,
    focus_handle: FocusHandle,
    _input_subscription: Subscription,
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
        let input = cx.new(|cx| Input::new("SELECT … — Enter runs the statement", cx));
        let grid = cx.new(|_| ResultGrid::new());

        let _input_subscription = cx.subscribe(&input, |this, _input, event, cx| {
            if let InputEvent::Submit(sql) = event {
                this.run_sql(sql.clone(), cx);
            }
        });

        // Bus → UI: drain the bridge channel on the foreground executor.
        let (_bus_subscription, mut rx) = events::bridge(&services.bus);
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| this.on_ui_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        window.focus(&input.focus_handle(cx), cx);

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

        let mut this = Self {
            services,
            connection,
            connection_state: ConnectionState::Disconnected,
            input,
            grid,
            status,
            status_is_error: false,
            query_running: false,
            dev,
            bench_pending: false,
            bench: None,
            focus_handle: cx.focus_handle(),
            _input_subscription,
            _bus_subscription,
        };
        this.connect(cx);
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

    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let service = self.services.connection.clone();
        let task = gpui_compat::spawn_tokio(cx, async move { service.connect(&connection).await });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| match outcome {
                Ok(Ok(())) => {}
                Ok(Err(e)) => this.set_status(format!("Connection failed: {e}"), true, cx),
                Err(join) => this.set_status(format!("Connection task failed: {join}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    fn run_query_action(&mut self, _: &RunQuery, _: &mut Window, cx: &mut Context<Self>) {
        let sql = self.input.read(cx).text().to_string();
        self.run_sql(sql, cx);
    }

    fn cancel_query_action(&mut self, _: &CancelQuery, _: &mut Window, cx: &mut Context<Self>) {
        if !self.query_running {
            self.set_status("No query running.", false, cx);
            return;
        }
        // One connection → at most one active run; cancel whatever is in flight.
        let query_service = self.services.query.clone();
        let runs = query_service.active_runs();
        if runs.is_empty() {
            return;
        }
        self.set_status("Cancelling…", false, cx);
        gpui_compat::spawn_tokio(cx, async move {
            for run in runs {
                if let Err(e) = query_service.cancel(run).await {
                    tracing::warn!(error = %e, "cancel failed");
                }
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

    /// Sweep the grid from top to bottom, one `on_next_frame` per step, and
    /// report frame times. Diagnostic for the 60 fps acceptance criterion.
    fn start_bench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.grid.read(cx).row_count();
        if rows == 0 {
            self.set_status("Scroll benchmark: no rows to scroll.", true, cx);
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
                    tracing::info!(
                        row,
                        frames = bench.frames_recorded(),
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

    fn run_sql(&mut self, sql: String, cx: &mut Context<Self>) {
        let sql = sql.trim().to_string();
        if sql.is_empty() {
            return;
        }
        let Some(connection_id) = self.connection_id() else {
            self.set_status("No connection configured (DATABASE_URL).", true, cx);
            return;
        };
        if self.connection_state != ConnectionState::Connected {
            self.set_status(
                format!(
                    "Not connected ({:?}); cannot run query.",
                    self.connection_state
                ),
                true,
                cx,
            );
            return;
        }
        if self.query_running {
            self.set_status("A query is already running.", true, cx);
            return;
        }

        self.query_running = true;
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
                .execute_streaming(&sql, connection_id, sink)
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

        // 3. Final outcome → status (QueryFinished also arrives via the bus).
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.query_running = false;
                match outcome {
                    Ok(Ok(run)) => {
                        let rows = this.grid.read(cx).row_count();
                        let cols = this.grid.read(cx).column_count();
                        if cols == 0 {
                            this.grid.update(cx, |g, cx| {
                                g.set_message("Statement executed; no rows returned.");
                                cx.notify();
                            });
                        }
                        let cancelled = this
                            .services
                            .query
                            .completed_run(run)
                            .is_some_and(|r| r.outcome == QueryOutcome::Cancelled);
                        if cancelled {
                            this.set_status(
                                format!("Cancelled — {rows} rows (partial)"),
                                false,
                                cx,
                            );
                        } else {
                            this.set_status(format!("Done — {rows} rows"), false, cx);
                        }
                        if this.dev.bench_scroll_then_exit && this.bench.is_none() {
                            // No `Window` here; `render` picks this up next frame.
                            this.bench_pending = true;
                            cx.notify();
                        }
                    }
                    Ok(Err(e)) => {
                        this.grid.update(cx, |g, cx| {
                            g.set_message(format!("Query failed: {e}"));
                            cx.notify();
                        });
                        this.set_status(format!("Query failed: {e}"), true, cx);
                    }
                    Err(join) => this.set_status(format!("Query task failed: {join}"), true, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_ui_event(&mut self, event: UiEvent, cx: &mut Context<Self>) {
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
                let is_error = matches!(state, ConnectionState::Failed);
                self.set_status(text, is_error, cx);
                if state == ConnectionState::Connected
                    && let Some(sql) = self.dev.startup_sql.take()
                {
                    self.input
                        .update(cx, |input, cx| input.set_text(sql.clone(), cx));
                    self.run_sql(sql, cx);
                }
            }
            UiEvent::RowsReceived { .. } if self.query_running => {
                let rows = self.grid.read(cx).row_count();
                self.set_status(format!("Running… {rows} rows"), false, cx);
            }
            UiEvent::QueryFinished {
                outcome: QueryOutcome::Cancelled,
                ..
            } => {
                self.query_running = false;
                self.set_status("Query cancelled", false, cx)
            }
            _ => {}
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

impl Focusable for MainWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.bench_pending) {
            self.start_bench(window, cx);
        }
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::run_query_action))
            .on_action(cx.listener(Self::cancel_query_action))
            .on_action(cx.listener(Self::bench_action))
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
                    .child(div().flex_1().child(self.input.clone())),
            )
            .child(div().flex_1().min_h_0().child(self.grid.clone()))
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
