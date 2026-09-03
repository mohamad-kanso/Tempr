//! Bridge from the service-layer `EventBus` (published on tokio threads) to
//! the GPUI main thread.
//!
//! `AppEvent` is not `Clone` (plugin payloads are opaque), so the bridge maps
//! the events the UI cares about into [`UiEvent`] and forwards them over a
//! channel that a view drains with `cx.spawn`.

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use tempr_domain::{ConnectionId, ConnectionState, QueryOutcome, QueryRunId};
use tempr_events::{AppEvent, AppEventKind, EventBus, EventFilter, Subscription};

/// UI-facing projection of the bus events a view reacts to.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    ConnectionStateChanged {
        id: ConnectionId,
        state: ConnectionState,
    },
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
}

impl UiEvent {
    pub fn from_app(event: &AppEvent) -> Option<Self> {
        Some(match event {
            AppEvent::ConnectionStateChanged { id, state } => Self::ConnectionStateChanged {
                id: *id,
                state: *state,
            },
            AppEvent::QueryStarted { run } => Self::QueryStarted { run: *run },
            AppEvent::RowsReceived { run, count } => Self::RowsReceived {
                run: *run,
                count: *count,
            },
            AppEvent::QueryFinished { run, outcome } => Self::QueryFinished {
                run: *run,
                outcome: outcome.clone(),
            },
            _ => return None,
        })
    }
}

/// Subscribe to `bus` and forward matching events into a channel. Keep the
/// returned `Subscription` alive for as long as the receiver is drained.
pub fn bridge(bus: &EventBus) -> (Subscription, UnboundedReceiver<UiEvent>) {
    let (tx, rx) = unbounded();
    let filter = EventFilter::AnyOf(vec![
        AppEventKind::ConnectionStateChanged,
        AppEventKind::QueryStarted,
        AppEventKind::RowsReceived,
        AppEventKind::QueryFinished,
    ]);
    let sub = bus.subscribe(filter, move |event| {
        if let Some(ui) = UiEvent::from_app(event) {
            // Receiver gone means the view is gone; nothing to do.
            let _ = tx.unbounded_send(ui);
        }
    });
    (sub, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{FutureExt, StreamExt};
    use tempr_domain::WorkspaceId;

    #[test]
    fn maps_query_lifecycle_and_ignores_unrelated() {
        let run = QueryRunId::new();
        assert_eq!(
            UiEvent::from_app(&AppEvent::RowsReceived { run, count: 7 }),
            Some(UiEvent::RowsReceived { run, count: 7 })
        );
        assert_eq!(
            UiEvent::from_app(&AppEvent::WorkspaceOpened {
                id: WorkspaceId::new()
            }),
            None
        );
    }

    #[test]
    fn bridge_forwards_in_publish_order() {
        let bus = EventBus::new();
        let (_sub, mut rx) = bridge(&bus);
        let run = QueryRunId::new();
        bus.publish(AppEvent::QueryStarted { run });
        bus.publish(AppEvent::RowsReceived { run, count: 3 });
        bus.publish(AppEvent::QueryFinished {
            run,
            outcome: QueryOutcome::Success,
        });

        let mut got = Vec::new();
        while let Some(Some(ev)) = rx.next().now_or_never() {
            got.push(ev);
        }
        assert_eq!(got.len(), 3);
        assert!(matches!(got[0], UiEvent::QueryStarted { .. }));
        assert!(matches!(got[2], UiEvent::QueryFinished { .. }));
        // Channel still open (subscription alive) and nothing else pending.
        assert!(rx.next().now_or_never().is_none());
    }
}
