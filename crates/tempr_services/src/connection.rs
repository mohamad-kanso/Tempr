//! `ConnectionService` — per-connection pools of driver connections, state
//! tracking, and the borrowing interface other services use.
//!
//! Pooling model (docs/09-database-engine.md → "Connection pooling model"):
//! every `Connection` gets a **user pool** (default max 8) borrowed by
//! `QueryService`, plus a **dedicated metadata slot** (a 1-connection pool)
//! reserved for `SchemaService`, so a long user query never starves a schema
//! refresh. Pools are `deadpool` managed pools over `Box<dyn DriverConnection>`
//! (D19) — driver-agnostic, lazily filled, returned on drop, and dead
//! connections are evicted on recycle via `DriverConnection::is_closed`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use deadpool::Runtime;
use deadpool::managed::{Manager, Metrics, Object, Pool, PoolError, RecycleError, RecycleResult};
use parking_lot::RwLock;
use tempr_db::{DatabaseDriver, DriverConnection, DriverError};
use tempr_domain::{Connection, ConnectionId, ConnectionState};
use tempr_events::{AppEvent, EventBus};

use crate::{Service, ServiceError};

/// Default user-pool size (09-database-engine: max 8).
pub const DEFAULT_MAX_POOL_SIZE: usize = 8;

/// Per-connection pool sizing and timeouts.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Max simultaneous user connections (`QueryService`). The metadata slot
    /// is always exactly one extra connection.
    pub max_size: usize,
    /// Max time a borrower waits for a free slot before failing.
    pub wait_timeout: Duration,
    /// Max time a new driver connection may take to be established.
    pub create_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_POOL_SIZE,
            wait_timeout: Duration::from_secs(30),
            create_timeout: Duration::from_secs(15),
        }
    }
}

/// `deadpool` manager: creates driver connections for one `Connection`.
pub struct DriverManager {
    driver: Arc<dyn DatabaseDriver>,
    connection: Connection,
}

impl Manager for DriverManager {
    type Type = Box<dyn DriverConnection>;
    type Error = DriverError;

    async fn create(&self) -> Result<Self::Type, DriverError> {
        self.driver.connect(&self.connection).await
    }

    /// Evict connections whose transport died while idle. An active ping is
    /// a follow-up (TODO: `DriverConnection::ping`).
    async fn recycle(
        &self,
        obj: &mut Self::Type,
        _metrics: &Metrics,
    ) -> RecycleResult<DriverError> {
        if obj.is_closed() {
            Err(RecycleError::message("connection closed"))
        } else {
            Ok(())
        }
    }
}

type DriverPool = Pool<DriverManager>;

/// A borrowed driver connection; returns to its pool on drop. Derefs to
/// `Box<dyn DriverConnection>`.
pub type PooledConnection = Object<DriverManager>;

struct PoolSet {
    user: DriverPool,
    metadata: DriverPool,
}

impl PoolSet {
    fn close(&self) {
        self.user.close();
        self.metadata.close();
    }
}

/// One entry per known connection: state and (when connected) its pools,
/// kept together under one lock so they cannot disagree.
struct ConnEntry {
    state: ConnectionState,
    pools: Option<PoolSet>,
}

pub struct ConnectionService {
    event_bus: Arc<EventBus>,
    drivers: RwLock<HashMap<String, Arc<dyn DatabaseDriver>>>,
    entries: RwLock<HashMap<ConnectionId, ConnEntry>>,
    pool_config: PoolConfig,
}

impl ConnectionService {
    pub fn new(event_bus: Arc<EventBus>) -> Arc<Self> {
        Self::with_pool_config(event_bus, PoolConfig::default())
    }

    pub fn with_pool_config(event_bus: Arc<EventBus>, pool_config: PoolConfig) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            drivers: RwLock::new(HashMap::new()),
            entries: RwLock::new(HashMap::new()),
            pool_config,
        })
    }

    pub fn register_driver(&self, driver: Arc<dyn DatabaseDriver>) {
        let engine = driver.engine().0.clone();
        self.drivers.write().insert(engine, driver);
    }

    /// Set `state`, drop (and close) the pools when the new state is not
    /// `Connected`, publish the change. Returns the displaced pools so the
    /// caller can decide what to do with them (they are already closed).
    fn transition(&self, id: ConnectionId, state: ConnectionState, pools: Option<PoolSet>) {
        let displaced = {
            let mut entries = self.entries.write();
            let entry = entries
                .entry(id)
                .or_insert(ConnEntry { state, pools: None });
            entry.state = state;
            std::mem::replace(&mut entry.pools, pools)
        };
        if let Some(old) = displaced {
            old.close();
        }
        self.event_bus
            .publish(AppEvent::ConnectionStateChanged { id, state });
    }

    fn build_pool(
        &self,
        driver: &Arc<dyn DatabaseDriver>,
        connection: &Connection,
        max_size: usize,
    ) -> Result<DriverPool, ServiceError> {
        Pool::builder(DriverManager {
            driver: driver.clone(),
            connection: connection.clone(),
        })
        .max_size(max_size)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(self.pool_config.wait_timeout))
        .create_timeout(Some(self.pool_config.create_timeout))
        .build()
        .map_err(|e| ServiceError::StartupFailed {
            name: "ConnectionService",
            reason: format!("pool build failed: {e}"),
        })
    }

    /// Build the pools for `connection` and establish one connection eagerly
    /// so credentials/reachability are validated now, not on first query.
    /// Any previous pools for the same id are closed and replaced.
    pub async fn connect(&self, connection: &Connection) -> Result<(), ServiceError> {
        let id = connection.id;
        let engine = connection.driver.engine_name();
        self.transition(id, ConnectionState::Connecting, None);
        // If this future is dropped mid-flight, do not leave the entry stuck
        // in `Connecting` with no pools.
        let mut guard = ConnectingGuard {
            service: self,
            id,
            armed: true,
        };

        let driver = match self.drivers.read().get(engine).cloned() {
            Some(d) => d,
            None => {
                guard.disarm();
                self.transition(id, ConnectionState::Failed, None);
                return Err(ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: format!("no driver registered for engine '{engine}'"),
                });
            }
        };

        let user = self.build_pool(&driver, connection, self.pool_config.max_size.max(1))?;
        let metadata = self.build_pool(&driver, connection, 1)?;

        // Warm-up: one real connection through the user pool.
        if let Err(e) = user.get().await {
            guard.disarm();
            self.transition(id, ConnectionState::Failed, None);
            return Err(ServiceError::StartupFailed {
                name: "ConnectionService",
                reason: pool_error_reason(e),
            });
        }

        // A `disconnect` that raced the warm-up wins: keep its outcome.
        let still_connecting = self
            .entries
            .read()
            .get(&id)
            .is_some_and(|e| e.state == ConnectionState::Connecting);
        guard.disarm();
        if !still_connecting {
            user.close();
            metadata.close();
            return Err(ServiceError::StartupFailed {
                name: "ConnectionService",
                reason: "connection was disconnected while connecting".to_string(),
            });
        }
        self.transition(
            id,
            ConnectionState::Connected,
            Some(PoolSet { user, metadata }),
        );
        Ok(())
    }

    /// Close the pools and mark the connection `Disconnected`.
    pub async fn disconnect(&self, id: ConnectionId) -> Result<(), ServiceError> {
        self.transition(id, ConnectionState::Disconnected, None);
        Ok(())
    }

    pub fn state(&self, id: ConnectionId) -> ConnectionState {
        self.entries
            .read()
            .get(&id)
            .map(|e| e.state)
            .unwrap_or(ConnectionState::Disconnected)
    }

    /// Ids of connections that currently hold pools (i.e. are `Connected`).
    pub fn connected_ids(&self) -> Vec<ConnectionId> {
        self.entries
            .read()
            .iter()
            .filter(|(_, e)| e.pools.is_some())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Pool occupancy `(in_use, idle)` of the user pool, for status UIs and
    /// tests. `None` when the connection holds no pools.
    pub fn pool_status(&self, id: ConnectionId) -> Option<(usize, usize)> {
        self.entries.read().get(&id).and_then(|e| {
            e.pools.as_ref().map(|set| {
                let s = set.user.status();
                (s.size - s.available, s.available)
            })
        })
    }

    /// Borrow a user-pool connection and run `f` with it. The connection
    /// returns to the pool when the `PooledConnection` is dropped — move it
    /// into the future if the body needs it.
    pub async fn with_connection_fn<F, Fut, R>(
        &self,
        id: ConnectionId,
        f: F,
    ) -> Result<R, ServiceError>
    where
        F: FnOnce(PooledConnection) -> Fut,
        Fut: std::future::Future<Output = Result<R, DriverError>>,
    {
        self.with_pool(id, |set| set.user.clone(), f).await
    }

    /// Borrow the dedicated metadata slot (`SchemaService` only).
    pub async fn with_metadata_connection_fn<F, Fut, R>(
        &self,
        id: ConnectionId,
        f: F,
    ) -> Result<R, ServiceError>
    where
        F: FnOnce(PooledConnection) -> Fut,
        Fut: std::future::Future<Output = Result<R, DriverError>>,
    {
        self.with_pool(id, |set| set.metadata.clone(), f).await
    }

    async fn with_pool<F, Fut, R>(
        &self,
        id: ConnectionId,
        select: impl FnOnce(&PoolSet) -> DriverPool,
        f: F,
    ) -> Result<R, ServiceError>
    where
        F: FnOnce(PooledConnection) -> Fut,
        Fut: std::future::Future<Output = Result<R, DriverError>>,
    {
        let pool = {
            let entries = self.entries.read();
            let entry = entries.get(&id);
            let state = entry
                .map(|e| e.state)
                .unwrap_or(ConnectionState::Disconnected);
            if state != ConnectionState::Connected {
                return Err(ServiceError::NotConnected {
                    id: id.to_string(),
                    state: state_name(state),
                });
            }
            entry
                .and_then(|e| e.pools.as_ref())
                .map(select)
                .ok_or_else(|| ServiceError::ConnectionNotFound { id: id.to_string() })?
        };

        let conn = pool.get().await.map_err(|e| ServiceError::QueryFailed {
            name: "ConnectionService",
            reason: pool_error_reason(e),
        })?;

        f(conn).await.map_err(|e| ServiceError::QueryFailed {
            name: "ConnectionService",
            reason: e.to_string(),
        })
    }
}

/// Flips a still-`Connecting` entry to `Failed` if `connect` is dropped
/// before completing (e.g. the driving task was aborted).
struct ConnectingGuard<'a> {
    service: &'a ConnectionService,
    id: ConnectionId,
    armed: bool,
}

impl ConnectingGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ConnectingGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let stuck = self
            .service
            .entries
            .read()
            .get(&self.id)
            .is_some_and(|e| e.state == ConnectionState::Connecting);
        if stuck {
            self.service
                .transition(self.id, ConnectionState::Failed, None);
        }
    }
}

fn state_name(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Connecting => "Connecting",
        ConnectionState::Connected => "Connected",
        ConnectionState::Disconnected => "Disconnected",
        ConnectionState::Failed => "Failed",
        ConnectionState::Reconnecting => "Reconnecting",
    }
}

fn pool_error_reason(e: PoolError<DriverError>) -> String {
    match e {
        PoolError::Backend(e) => e.to_string(),
        PoolError::Timeout(kind) => format!("pool timeout ({kind:?})"),
        PoolError::Closed => "pool closed".to_string(),
        PoolError::NoRuntimeSpecified => "pool has no async runtime".to_string(),
        PoolError::PostCreateHook(e) => format!("pool post-create hook failed: {e}"),
    }
}

#[async_trait]
impl Service for ConnectionService {
    fn name(&self) -> &'static str {
        "ConnectionService"
    }

    /// Drain every pool. Connections are (re)established by `connect`, which
    /// the workspace open sequence drives — not by `start`.
    async fn stop(&self) -> Result<(), ServiceError> {
        for id in self.connected_ids() {
            self.disconnect(id).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempr_db::EngineId;
    use tempr_domain::{DriverKind, SecretRef};

    struct NoopCancelHandle;

    #[async_trait::async_trait]
    impl tempr_db::CancelHandle for NoopCancelHandle {
        async fn cancel(&self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    fn make_connection_id() -> ConnectionId {
        ConnectionId::new()
    }

    fn make_connection(id: ConnectionId) -> Connection {
        Connection {
            id,
            name: "test".to_string(),
            driver: DriverKind::Postgres,
            host: "localhost".to_string(),
            port: 5432,
            database: "test".to_string(),
            username: "user".to_string(),
            password: "pass".to_string(),
            secret_ref: SecretRef {
                vault_key: "test".to_string(),
            },
        }
    }

    /// Mock driver registered under the PostgreSQL engine name; counts
    /// connections created and can mark all of them closed.
    struct MockDriver {
        created: Arc<AtomicUsize>,
        closed: Arc<AtomicBool>,
        fail: bool,
    }

    impl MockDriver {
        fn ok() -> (Arc<Self>, Arc<AtomicUsize>, Arc<AtomicBool>) {
            let created = Arc::new(AtomicUsize::new(0));
            let closed = Arc::new(AtomicBool::new(false));
            (
                Arc::new(Self {
                    created: created.clone(),
                    closed: closed.clone(),
                    fail: false,
                }),
                created,
                closed,
            )
        }
    }

    #[async_trait::async_trait]
    impl DatabaseDriver for MockDriver {
        fn engine(&self) -> EngineId {
            EngineId(DriverKind::Postgres.engine_name().to_string())
        }
        async fn connect(
            &self,
            _connection: &Connection,
        ) -> Result<Box<dyn DriverConnection>, DriverError> {
            if self.fail {
                return Err(DriverError::AuthFailed("nope".into()));
            }
            self.created.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(MockConnection {
                closed: self.closed.clone(),
            }))
        }
    }

    struct MockConnection {
        closed: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl DriverConnection for MockConnection {
        async fn execute(
            &mut self,
            _sql: &str,
            _params: &[tempr_domain::Value],
        ) -> Result<tempr_db::QueryStream, DriverError> {
            unimplemented!()
        }
        async fn cancel(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
        fn cancel_handle(&self) -> Box<dyn tempr_db::CancelHandle> {
            Box::new(NoopCancelHandle)
        }
        async fn snapshot_schema(
            &mut self,
            _scope: tempr_db::SchemaScope,
        ) -> Result<Vec<tempr_db::SchemaSnapshotEntry>, DriverError> {
            Ok(Vec::new())
        }
    }

    async fn connected() -> (
        Arc<ConnectionService>,
        ConnectionId,
        Arc<AtomicUsize>,
        Arc<AtomicBool>,
    ) {
        let svc = ConnectionService::new(make_event_bus());
        let (driver, created, closed) = MockDriver::ok();
        svc.register_driver(driver);
        let id = make_connection_id();
        svc.connect(&make_connection(id)).await.unwrap();
        (svc, id, created, closed)
    }

    async fn borrow_noop(svc: &ConnectionService, id: ConnectionId) -> Result<(), ServiceError> {
        svc.with_connection_fn(id, |conn| async move {
            let _held = conn;
            Ok::<_, DriverError>(())
        })
        .await
    }

    #[tokio::test]
    async fn state_defaults_to_disconnected() {
        let svc = ConnectionService::new(make_event_bus());
        assert_eq!(
            svc.state(make_connection_id()),
            ConnectionState::Disconnected
        );
    }

    #[tokio::test]
    async fn connect_with_no_driver_fails() {
        let svc = ConnectionService::new(make_event_bus());
        let id = make_connection_id();
        assert!(svc.connect(&make_connection(id)).await.is_err());
        assert_eq!(svc.state(id), ConnectionState::Failed);
        assert!(svc.pool_status(id).is_none());
    }

    #[tokio::test]
    async fn connect_success_sets_connected_and_warms_one_connection() {
        let (svc, id, created, _) = connected().await;
        assert_eq!(svc.state(id), ConnectionState::Connected);
        assert_eq!(created.load(Ordering::SeqCst), 1, "warm-up connection");
        assert_eq!(svc.pool_status(id), Some((0, 1)));
        assert_eq!(svc.connected_ids(), vec![id]);
    }

    #[tokio::test]
    async fn connect_failure_sets_failed_state_without_pools() {
        let svc = ConnectionService::new(make_event_bus());
        svc.register_driver(Arc::new(MockDriver {
            created: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
            fail: true,
        }));
        let id = make_connection_id();
        let err = svc.connect(&make_connection(id)).await.unwrap_err();
        assert!(err.to_string().contains("authentication failed"), "{err}");
        assert_eq!(svc.state(id), ConnectionState::Failed);
        assert!(svc.pool_status(id).is_none());
        assert!(svc.connected_ids().is_empty());
    }

    #[tokio::test]
    async fn reconnect_replaces_and_closes_previous_pools() {
        let (svc, id, created, _) = connected().await;
        // Grab a handle on the first user pool to observe it being closed.
        let first_pool = svc
            .entries
            .read()
            .get(&id)
            .and_then(|e| e.pools.as_ref())
            .map(|p| p.user.clone())
            .unwrap();
        svc.connect(&make_connection(id)).await.unwrap();
        assert!(first_pool.is_closed(), "displaced pool must be closed");
        assert_eq!(created.load(Ordering::SeqCst), 2);
        assert_eq!(svc.state(id), ConnectionState::Connected);
        assert_eq!(svc.pool_status(id), Some((0, 1)));
    }

    #[tokio::test]
    async fn borrowed_connection_returns_to_pool_and_is_reused() {
        let (svc, id, created, _) = connected().await;
        for _ in 0..3 {
            borrow_noop(&svc, id).await.unwrap();
        }
        assert_eq!(created.load(Ordering::SeqCst), 1, "one connection reused");
        assert_eq!(svc.pool_status(id), Some((0, 1)));
    }

    #[tokio::test]
    async fn closed_connections_are_evicted_on_recycle() {
        let (svc, id, created, closed) = connected().await;
        closed.store(true, Ordering::SeqCst);
        // The idle warm-up connection is now dead: the next borrow must
        // evict it and create a fresh one (which the mock also reports
        // closed, but it is handed out freshly created, not recycled).
        borrow_noop(&svc, id).await.unwrap();
        assert_eq!(
            created.load(Ordering::SeqCst),
            2,
            "dead connection replaced"
        );
    }

    #[tokio::test]
    async fn concurrent_borrows_grow_pool_up_to_max() {
        let bus = make_event_bus();
        let svc = ConnectionService::with_pool_config(
            bus,
            PoolConfig {
                max_size: 2,
                ..PoolConfig::default()
            },
        );
        let (driver, created, _) = MockDriver::ok();
        svc.register_driver(driver);
        let id = make_connection_id();
        svc.connect(&make_connection(id)).await.unwrap();

        let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let svc2 = svc.clone();
        let holder = tokio::spawn(async move {
            svc2.with_connection_fn(id, |conn| async move {
                let _held = conn;
                acquired_tx.send(()).ok();
                release_rx.await.ok();
                Ok::<_, DriverError>(())
            })
            .await
        });
        acquired_rx.await.unwrap();
        borrow_noop(&svc, id).await.unwrap();
        release_tx.send(()).unwrap();
        holder.await.unwrap().unwrap();

        assert_eq!(created.load(Ordering::SeqCst), 2);
        assert_eq!(svc.pool_status(id), Some((0, 2)));
    }

    #[tokio::test]
    async fn borrow_times_out_when_pool_is_exhausted() {
        let bus = make_event_bus();
        let svc = ConnectionService::with_pool_config(
            bus,
            PoolConfig {
                max_size: 1,
                wait_timeout: Duration::from_millis(50),
                ..PoolConfig::default()
            },
        );
        let (driver, _, _) = MockDriver::ok();
        svc.register_driver(driver);
        let id = make_connection_id();
        svc.connect(&make_connection(id)).await.unwrap();

        let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let svc2 = svc.clone();
        let holder = tokio::spawn(async move {
            svc2.with_connection_fn(id, |conn| async move {
                let _held = conn;
                acquired_tx.send(()).ok();
                release_rx.await.ok();
                Ok::<_, DriverError>(())
            })
            .await
        });
        acquired_rx.await.unwrap();
        let err = borrow_noop(&svc, id).await.unwrap_err();
        assert!(err.to_string().contains("pool timeout"), "{err}");
        release_tx.send(()).unwrap();
        holder.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn metadata_slot_is_separate_from_user_pool() {
        let (svc, id, created, _) = connected().await;
        svc.with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(tempr_db::SchemaScope::All).await
        })
        .await
        .unwrap();
        // user warm-up (1) + metadata slot (1)
        assert_eq!(created.load(Ordering::SeqCst), 2);
        assert_eq!(svc.pool_status(id), Some((0, 1)), "user pool untouched");
    }

    #[tokio::test]
    async fn disconnect_closes_pools_and_sets_disconnected() {
        let (svc, id, _, _) = connected().await;
        svc.disconnect(id).await.unwrap();
        assert_eq!(svc.state(id), ConnectionState::Disconnected);
        assert!(svc.pool_status(id).is_none());
        let err = borrow_noop(&svc, id).await.unwrap_err();
        assert!(matches!(err, ServiceError::NotConnected { .. }));
    }

    #[tokio::test]
    async fn stop_disconnects_only_pooled_connections() {
        let (svc, id, _, _) = connected().await;
        // A failed connection must not receive a spurious Disconnected event.
        let failed_id = make_connection_id();
        svc.entries.write().insert(
            failed_id,
            ConnEntry {
                state: ConnectionState::Failed,
                pools: None,
            },
        );
        let events: Arc<Mutex<Vec<(ConnectionId, ConnectionState)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let _sub = svc
            .event_bus
            .subscribe(tempr_events::EventFilter::All, move |ev| {
                if let AppEvent::ConnectionStateChanged { id, state } = ev {
                    e.lock().push((*id, *state));
                }
            });

        Service::stop(&*svc).await.unwrap();

        assert_eq!(svc.state(id), ConnectionState::Disconnected);
        assert_eq!(svc.state(failed_id), ConnectionState::Failed);
        assert_eq!(*events.lock(), vec![(id, ConnectionState::Disconnected)]);
        assert_eq!(svc.name(), "ConnectionService");
    }

    #[tokio::test]
    async fn events_published_on_connect_attempt() {
        let bus = make_event_bus();
        let received: Arc<Mutex<Vec<ConnectionState>>> = Arc::new(Mutex::new(Vec::new()));
        let r = received.clone();
        let _sub = bus.subscribe(tempr_events::EventFilter::All, move |event| {
            if let AppEvent::ConnectionStateChanged { state, .. } = event {
                r.lock().push(*state);
            }
        });

        let svc = ConnectionService::new(bus);
        let _ = svc.connect(&make_connection(make_connection_id())).await;

        assert_eq!(
            *received.lock(),
            vec![ConnectionState::Connecting, ConnectionState::Failed]
        );
    }

    #[tokio::test]
    async fn with_connection_fn_not_connected_fails() {
        let svc = ConnectionService::new(make_event_bus());
        let result = borrow_noop(&svc, make_connection_id()).await;
        assert!(matches!(result, Err(ServiceError::NotConnected { .. })));
    }

    #[tokio::test]
    async fn with_connection_fn_connected_without_pools_is_not_found() {
        let svc = ConnectionService::new(make_event_bus());
        let id = make_connection_id();
        svc.entries.write().insert(
            id,
            ConnEntry {
                state: ConnectionState::Connected,
                pools: None,
            },
        );
        let result = borrow_noop(&svc, id).await;
        assert!(matches!(
            result,
            Err(ServiceError::ConnectionNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn register_driver_stores_driver() {
        let svc = ConnectionService::new(make_event_bus());
        let (driver, _, _) = MockDriver::ok();
        svc.register_driver(driver);
        assert!(svc.drivers.read().contains_key("postgresql"));
    }
}
