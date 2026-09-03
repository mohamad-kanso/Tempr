//! Root view of the application window.
//!
//! Layout: SQL `Input` (top) · `ResultsGrid` (middle) · status bar (bottom).
//! Holds service handles + render state only (D6). Query execution happens
//! in `QueryService` on the tokio runtime; rows and bus events reach this
//! view through channels drained with `cx.spawn`.

use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, Render, Subscription,
    Window, actions, div, prelude::*, px, rgb,
};
use tempr_domain::{Batch, ColumnSpec, Connection, ConnectionId, ConnectionState, QueryOutcome};
use tempr_events::EventBus;
use tempr_services::{ConnectionService, QueryService, RowSink};

use crate::components::{Input, InputEvent, ResultsGrid};
use crate::events::{self, UiEvent};
use crate::gpui_compat;
use crate::theme;

actions!(main_window, [RunQuery, Quit]);

pub const KEY_CONTEXT: &str = "MainWindow";

/// Register global + main-window keybindings. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-enter", RunQuery, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-enter", RunQuery, Some(KEY_CONTEXT)),
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
    grid: Entity<ResultsGrid>,
    status: String,
    status_is_error: bool,
    query_running: bool,
    focus_handle: FocusHandle,
    _input_subscription: Subscription,
    _bus_subscription: tempr_events::Subscription,
}

impl MainWindow {
    pub fn new(
        services: Services,
        connection: Option<Connection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| Input::new("SELECT … — Enter runs the statement", cx));
        let grid = cx.new(|_| ResultsGrid::new());

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
                    Ok(Ok(_run)) => {
                        let rows = this.grid.read(cx).row_count();
                        let cols = this.grid.read(cx).column_count();
                        if cols == 0 {
                            this.grid.update(cx, |g, cx| {
                                g.set_message("Statement executed; no rows returned.");
                                cx.notify();
                            });
                        }
                        this.set_status(format!("Done — {rows} rows"), false, cx);
                    }
                    Ok(Err(e)) => this.set_status(format!("Query failed: {e}"), true, cx),
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
            }
            UiEvent::RowsReceived { .. } if self.query_running => {
                let rows = self.grid.read(cx).row_count();
                self.set_status(format!("Running… {rows} rows"), false, cx);
            }
            UiEvent::QueryFinished {
                outcome: QueryOutcome::Cancelled,
                ..
            } => self.set_status("Query cancelled", false, cx),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::run_query_action))
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
