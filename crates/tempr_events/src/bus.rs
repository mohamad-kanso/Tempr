use crate::event::{AppEvent, EventFilter};
use parking_lot::RwLock;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

type SharedHandler = Arc<dyn Fn(&AppEvent) + Send + Sync + 'static>;

struct SubscriberEntry {
    id: u64,
    filter: EventFilter,
    handler: SharedHandler,
}

type SubscriberList = RwLock<Vec<SubscriberEntry>>;

/// Central event bus — all services communicate state changes through here.
///
/// Publish is synchronous and ordered: handlers run in subscription order within a
/// single `publish` call, satisfying the per-publisher ordering guarantee. The
/// subscriber list lock is released before handlers execute, so handlers may
/// safely call `subscribe`/`unsubscribe` without deadlock.
pub struct EventBus {
    subscribers: Arc<SubscriberList>,
    next_id: AtomicU64,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(RwLock::new(Vec::new())),
            next_id: AtomicU64::new(0),
        }
    }

    /// Publish an event to all matching subscribers.
    /// Returns immediately after all matching handlers have been called in-order.
    pub fn publish(&self, event: AppEvent) {
        // Snapshot matching handlers before calling any — avoids holding the lock
        // during handler execution and prevents deadlock if a handler subscribes.
        let handlers: Vec<SharedHandler> = self
            .subscribers
            .read()
            .iter()
            .filter(|s| s.filter.matches(&event))
            .map(|s| s.handler.clone())
            .collect();
        for handler in handlers {
            handler(&event);
        }
    }

    /// Subscribe to events matching `filter`. Returns a `Subscription` that
    /// auto-unsubscribes on drop (RAII).
    pub fn subscribe(
        &self,
        filter: EventFilter,
        handler: impl Fn(&AppEvent) + Send + Sync + 'static,
    ) -> Subscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let handler: SharedHandler = Arc::new(handler);
        self.subscribers.write().push(SubscriberEntry {
            id,
            filter,
            handler,
        });
        Subscription {
            id,
            subscribers: Arc::downgrade(&self.subscribers),
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.read().len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard — unsubscribes from the EventBus when dropped.
pub struct Subscription {
    id: u64,
    subscribers: Weak<SubscriberList>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(subs) = self.subscribers.upgrade() {
            subs.write().retain(|e| e.id != self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AppEvent, AppEventKind, EventFilter};
    use std::sync::{Arc, Mutex};
    use tempr_domain::{QueryOutcome, QueryRunId, WorkspaceId};

    fn workspace_opened() -> AppEvent {
        AppEvent::WorkspaceOpened {
            id: WorkspaceId::new(),
        }
    }

    fn query_finished() -> AppEvent {
        AppEvent::QueryFinished {
            run: QueryRunId::new(),
            outcome: QueryOutcome::Success,
        }
    }

    #[test]
    fn subscriber_receives_matching_event() {
        let bus = EventBus::new();
        let received = Arc::new(Mutex::new(vec![]));
        let r = received.clone();

        let _sub = bus.subscribe(EventFilter::All, move |event| {
            r.lock().expect("lock").push(event.kind());
        });

        bus.publish(workspace_opened());
        bus.publish(query_finished());

        let events = received.lock().expect("lock");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], AppEventKind::WorkspaceOpened);
        assert_eq!(events[1], AppEventKind::QueryFinished);
    }

    #[test]
    fn filter_anyof_delivers_only_matching() {
        let bus = EventBus::new();
        let received = Arc::new(Mutex::new(vec![]));
        let r = received.clone();

        let _sub = bus.subscribe(
            EventFilter::AnyOf(vec![AppEventKind::QueryFinished]),
            move |event| {
                r.lock().expect("lock").push(event.kind());
            },
        );

        bus.publish(workspace_opened()); // should NOT reach subscriber
        bus.publish(query_finished()); // should reach subscriber

        let events = received.lock().expect("lock");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], AppEventKind::QueryFinished);
    }

    #[test]
    fn filter_not_excludes_one_kind() {
        let bus = EventBus::new();
        let received = Arc::new(Mutex::new(vec![]));
        let r = received.clone();

        let _sub = bus.subscribe(
            EventFilter::Not(AppEventKind::WorkspaceOpened),
            move |event| {
                r.lock().expect("lock").push(event.kind());
            },
        );

        bus.publish(workspace_opened()); // excluded
        bus.publish(query_finished()); // included

        let events = received.lock().expect("lock");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], AppEventKind::QueryFinished);
    }

    #[test]
    fn ordering_guaranteed_within_publish_sequence() {
        let bus = EventBus::new();
        let received = Arc::new(Mutex::new(vec![]));
        let r = received.clone();

        let _sub = bus.subscribe(EventFilter::All, move |event| {
            r.lock().expect("lock").push(event.kind());
        });

        bus.publish(AppEvent::WorkspaceOpened {
            id: WorkspaceId::new(),
        });
        bus.publish(AppEvent::WorkspaceClosed {
            id: WorkspaceId::new(),
        });
        bus.publish(query_finished());

        let events = received.lock().expect("lock");
        assert_eq!(events[0], AppEventKind::WorkspaceOpened);
        assert_eq!(events[1], AppEventKind::WorkspaceClosed);
        assert_eq!(events[2], AppEventKind::QueryFinished);
    }

    #[test]
    fn multiple_subscribers_all_receive_event() {
        let bus = EventBus::new();
        let count = Arc::new(Mutex::new(0u32));

        let c1 = count.clone();
        let c2 = count.clone();
        let c3 = count.clone();

        let _s1 = bus.subscribe(EventFilter::All, move |_| {
            *c1.lock().expect("lock") += 1;
        });
        let _s2 = bus.subscribe(EventFilter::All, move |_| {
            *c2.lock().expect("lock") += 1;
        });
        let _s3 = bus.subscribe(EventFilter::All, move |_| {
            *c3.lock().expect("lock") += 1;
        });

        bus.publish(workspace_opened());

        assert_eq!(*count.lock().expect("lock"), 3);
    }

    #[test]
    fn subscription_drop_unsubscribes() {
        let bus = EventBus::new();
        let received = Arc::new(Mutex::new(0u32));
        let r = received.clone();

        let sub = bus.subscribe(EventFilter::All, move |_| {
            *r.lock().expect("lock") += 1;
        });

        bus.publish(workspace_opened());
        assert_eq!(*received.lock().expect("lock"), 1);

        drop(sub); // should unsubscribe
        assert_eq!(bus.subscriber_count(), 0);

        bus.publish(workspace_opened());
        assert_eq!(*received.lock().expect("lock"), 1); // no additional delivery
    }
}
