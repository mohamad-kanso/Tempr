use std::any::Any;
use tempr_domain::{
    ConnectionId, ConnectionState, PluginId, QueryOutcome, QueryRunId, SchemaSnapshotId, SqlFileId,
    WorkspaceId,
};

/// Opaque plugin payload — carries arbitrary data across the event bus.
/// Debug-prints as `<plugin payload>` to avoid requiring plugin types to implement Debug.
pub struct PluginPayload(pub Box<dyn Any + Send>);

impl std::fmt::Debug for PluginPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<plugin payload>")
    }
}

/// Every significant state change in Tempr flows through the event bus as an AppEvent.
/// Payloads carry IDs only — large data travels via dedicated handles.
#[derive(Debug)]
pub enum AppEvent {
    // Workspace lifecycle
    WorkspaceOpened {
        id: WorkspaceId,
    },
    WorkspaceClosed {
        id: WorkspaceId,
    },

    // Connection lifecycle
    ConnectionRequested {
        id: ConnectionId,
    },
    ConnectionStateChanged {
        id: ConnectionId,
        state: ConnectionState,
    },
    ConnectionClosed {
        id: ConnectionId,
    },

    // Query lifecycle
    QueryStarted {
        run: QueryRunId,
    },
    RowsReceived {
        run: QueryRunId,
        count: usize,
    },
    QueryFinished {
        run: QueryRunId,
        outcome: QueryOutcome,
    },

    // Schema
    SchemaRefreshed {
        connection: ConnectionId,
        snapshot: SchemaSnapshotId,
    },

    // Editor
    BufferChanged {
        file: SqlFileId,
    },
    BufferSaved {
        file: SqlFileId,
    },

    // Plugin-authored events — namespaced by plugin_id
    PluginEvent {
        plugin_id: PluginId,
        payload: PluginPayload,
    },
}

/// Discriminant enum for use in EventFilter — no payload data, cheap to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEventKind {
    WorkspaceOpened,
    WorkspaceClosed,
    ConnectionRequested,
    ConnectionStateChanged,
    ConnectionClosed,
    QueryStarted,
    RowsReceived,
    QueryFinished,
    SchemaRefreshed,
    BufferChanged,
    BufferSaved,
    PluginEvent,
}

impl AppEvent {
    pub fn kind(&self) -> AppEventKind {
        match self {
            AppEvent::WorkspaceOpened { .. } => AppEventKind::WorkspaceOpened,
            AppEvent::WorkspaceClosed { .. } => AppEventKind::WorkspaceClosed,
            AppEvent::ConnectionRequested { .. } => AppEventKind::ConnectionRequested,
            AppEvent::ConnectionStateChanged { .. } => AppEventKind::ConnectionStateChanged,
            AppEvent::ConnectionClosed { .. } => AppEventKind::ConnectionClosed,
            AppEvent::QueryStarted { .. } => AppEventKind::QueryStarted,
            AppEvent::RowsReceived { .. } => AppEventKind::RowsReceived,
            AppEvent::QueryFinished { .. } => AppEventKind::QueryFinished,
            AppEvent::SchemaRefreshed { .. } => AppEventKind::SchemaRefreshed,
            AppEvent::BufferChanged { .. } => AppEventKind::BufferChanged,
            AppEvent::BufferSaved { .. } => AppEventKind::BufferSaved,
            AppEvent::PluginEvent { .. } => AppEventKind::PluginEvent,
        }
    }
}

/// Filter applied before an event is pushed to a subscriber's handler.
/// Cheap to evaluate — runs before any allocation or clone.
#[derive(Debug, Clone)]
pub enum EventFilter {
    All,
    AnyOf(Vec<AppEventKind>),
    Not(AppEventKind),
}

impl EventFilter {
    pub fn matches(&self, event: &AppEvent) -> bool {
        match self {
            EventFilter::All => true,
            EventFilter::AnyOf(kinds) => kinds.contains(&event.kind()),
            EventFilter::Not(kind) => event.kind() != *kind,
        }
    }
}
