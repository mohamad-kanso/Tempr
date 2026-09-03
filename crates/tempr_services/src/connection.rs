//! `ConnectionService` — per-connection pools of driver connections, state
//! tracking, and the borrowing interface other services use.
//!
//! Pooling model (docs/09-database-engine.md → "Connection pooling model"):
//! every `Connection` gets a **user pool** (default max 8) borrowed by
//! `QueryService`, plus a **dedicated metadata slot** (a 1-connection pool)
//! reserved for `SchemaService`, so a long user query never starves a schema
//! refresh. Pools are `deadpool` managed pools over `Box<dyn DriverConnection>`
//! (D19) — driver-agnostic, lazily filled, returned on drop.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use deadpool::managed::{Manager, Metrics, Object, Pool, PoolError, RecycleResult};
use parking_lot::RwLock;
use tempr_db::{DatabaseDriver, DriverConnection, DriverError};
use tempr_domain::{Connection, ConnectionId, ConnectionState};
use tempr_events::{AppEvent, EventBus};

use crate::{Service, ServiceError};

/// Default user-pool size (09-database-engine: max 8).
pub const DEFAULT_MAX_POOL_SIZE: usize = 8;

/// Per-connection pool sizing.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Max simultaneous user connections (`QueryService`). The metadata slot
    /// is always exactly one extra connection.
    pub max_size: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_POOL_SIZE,
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

    async fn recycle(
        &self,
        _obj: &mut Self::Type,
        _metrics: &Metrics,
    ) -> RecycleResult<DriverError> {
        // Health check (ping) needs a `DriverConnection::ping`; tracked in TODO.
        Ok(())
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

pub struct ConnectionService {
    event_bus: Arc<EventBus>,
    drivers: RwLock<HashMap<String, Arc<dyn DatabaseDriver>>>,
    pools: RwLock<HashMap<ConnectionId, PoolSet>>,
    states: RwLock<HashMap<ConnectionId, ConnectionState>>,
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
            pools: RwLock::new(HashMap::new()),
            states: RwLock::new(HashMap::new()),
            pool_config,
        })
    }

    pub fn register_driver(&self, driver: Arc<dyn DatabaseDriver>) {
        let engine = driver.engine().0.clone();
        self.drivers.write().insert(engine, driver);
    }

    fn set_state(&self, id: ConnectionId, state: ConnectionState) {
        self.states.write().insert(id, state);
        self.event_bus
            .publish(AppEvent::ConnectionStateChanged { id, state });
    }

    /// Build the pools for `connection` and establish one connection eagerly
    /// so credentials/reachability are validated now, not on first query.
    pub async fn connect(&self, connection: &Connection) -> Result<(), ServiceError> {
        let id = connection.id;
        let engine = connection.driver.engine_name();
        self.set_state(id, ConnectionState::Connecting);

        let driver = match self.drivers.read().get(engine).cloned() {
            Some(d) => d,
            None => {
                self.set_state(id, ConnectionState::Failed);
                return Err(ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: format!("no driver registered for engine '{engine}'"),
                });
            }
        };

        let build = |max_size: usize| {
            Pool::builder(DriverManager {
                driver: driver.clone(),
                connection: connection.clone(),
            })
            .max_size(max_size)
            .build()
            .map_err(|e| ServiceError::StartupFailed {
                name: "ConnectionService",
                reason: format!("pool build failed: {e}"),
            })
        };
        let user = build(self.pool_config.max_size.max(1))?;
        let metadata = build(1)?;

        // Warm-up: one real connection through the user pool.
        match user.get().await {
            Ok(conn) => drop(conn),
            Err(e) => {
                self.set_state(id, ConnectionState::Failed);
                return Err(ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: pool_error_reason(e),
                });
            }
        }

        self.pools.write().insert(id, PoolSet { user, metadata });
        self.set_state(id, ConnectionState::Connected);
        Ok(())
    }

    pub async fn disconnect(&self, id: ConnectionId) -> Result<(), ServiceError> {
        if let Some(set) = self.pools.write().remove(&id) {
            set.user.close();
            set.metadata.close();
        }
        self.set_state(id, ConnectionState::Disconnected);
        Ok(())
    }

    pub fn state(&self, id: ConnectionId) -> ConnectionState {
        self.states
            .read()
            .get(&id)
            .copied()
            .unwrap_or(ConnectionState::Disconnected)
    }

    /// Ids of every connection this service knows about.
    pub fn connection_ids(&self) -> Vec<ConnectionId> {
        self.states.read().keys().copied().collect()
    }

    /// Pool occupancy `(in_use, available)` of the user pool, for status UIs
    /// and tests.
    pub fn pool_status(&self, id: ConnectionId) -> Option<(usize, usize)> {
        self.pools.read().get(&id).map(|set| {
            let s = set.user.status();
            (s.size - s.available, s.available)
        })
    }

    /// Borrow a user-pool connection and run `f` with it. The connection
    /// returns to the pool when the `PooledConnection` is dropped.
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
        let state = self.state(id);
        if state != ConnectionState::Connected {
            return Err(ServiceError::NotConnected {
                id: id.to_string(),
                state: state_name(state),
            });
        }
        let pool = self
            .pools
            .read()
            .get(&id)
            .map(select)
            .ok_or_else(|| ServiceError::ConnectionNotFound { id: id.to_string() })?;

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
        for id in self.connection_ids() {
            self.disconnect(id).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// connections created so pooling behaviour is observable.
    struct MockDriver {
        created: Arc<AtomicUsize>,
        fail: bool,
    }

    impl MockDriver {
        fn ok() -> (Arc<Self>, Arc<AtomicUsize>) {
            let created = Arc::new(AtomicUsize::new(0));
            (
                Arc::new(Self {
                    created: created.clone(),
                    fail: false,
                }),
                created,
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
            Ok(Box::new(MockConnection))
        }
    }

    struct MockConnection;

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

    async fn connected() -> (Arc<ConnectionService>, ConnectionId, Arc<AtomicUsize>) {
        let svc = ConnectionService::new(make_event_bus());
        let (driver, created) = MockDriver::ok();
        svc.register_driver(driver);
        let id = make_connection_id();
        svc.connect(&make_connection(id)).await.unwrap();
        (svc, id, created)
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
        let result = svc.connect(&make_connection(id)).await;
        assert!(result.is_err());
        assert_eq!(svc.state(id), ConnectionState::Failed);
    }

    #[tokio::test]
    async fn connect_success_sets_connected_and_warms_one_connection() {
        let (svc, id, created) = connected().await;
        assert_eq!(svc.state(id), ConnectionState::Connected);
        assert_eq!(created.load(Ordering::SeqCst), 1, "warm-up connection");
        assert_eq!(svc.pool_status(id), Some((0, 1)));
    }

    #[tokio::test]
    async fn connect_failure_sets_failed_state() {
        let svc = ConnectionService::new(make_event_bus());
        svc.register_driver(Arc::new(MockDriver {
            created: Arc::new(AtomicUsize::new(0)),
            fail: true,
        }));
        let id = make_connection_id();
        let err = svc.connect(&make_connection(id)).await.unwrap_err();
        assert!(err.to_string().contains("authentication failed"), "{err}");
        assert_eq!(svc.state(id), ConnectionState::Failed);
        assert!(svc.pool_status(id).is_none());
    }

    #[tokio::test]
    async fn borrowed_connection_returns_to_pool_and_is_reused() {
        let (svc, id, created) = connected().await;
        for _ in 0..3 {
            svc.with_connection_fn(id, |_conn| async move { Ok::<_, DriverError>(()) })
                .await
                .unwrap();
        }
        assert_eq!(created.load(Ordering::SeqCst), 1, "one connection reused");
        assert_eq!(svc.pool_status(id), Some((0, 1)));
    }

    #[tokio::test]
    async fn concurrent_borrows_grow_pool_up_to_max() {
        let bus = make_event_bus();
        let svc = ConnectionService::with_pool_config(bus, PoolConfig { max_size: 2 });
        let (driver, created) = MockDriver::ok();
        svc.register_driver(driver);
        let id = make_connection_id();
        svc.connect(&make_connection(id)).await.unwrap();

        // Hold one connection while borrowing another → a second is created.
        let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let svc2 = svc.clone();
        let holder = tokio::spawn(async move {
            svc2.with_connection_fn(id, |conn| async move {
                // Keep the pooled connection alive for the whole body; a
                // closure parameter that the future does not capture would
                // be dropped (returned to the pool) before the body runs.
                let _held = conn;
                acquired_tx.send(()).ok();
                release_rx.await.ok();
                Ok::<_, DriverError>(())
            })
            .await
        });
        acquired_rx.await.unwrap();
        svc.with_connection_fn(id, |_conn| async move { Ok::<_, DriverError>(()) })
            .await
            .unwrap();
        release_tx.send(()).unwrap();
        holder.await.unwrap().unwrap();

        assert_eq!(created.load(Ordering::SeqCst), 2);
        assert_eq!(svc.pool_status(id), Some((0, 2)));
    }

    #[tokio::test]
    async fn metadata_slot_is_separate_from_user_pool() {
        let (svc, id, created) = connected().await;
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
        let (svc, id, _) = connected().await;
        svc.disconnect(id).await.unwrap();
        assert_eq!(svc.state(id), ConnectionState::Disconnected);
        assert!(svc.pool_status(id).is_none());
        let err = svc
            .with_connection_fn(id, |_conn| async move { Ok::<_, DriverError>(()) })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotConnected { .. }));
    }

    #[tokio::test]
    async fn stop_disconnects_everything() {
        let (svc, id, _) = connected().await;
        Service::stop(&*svc).await.unwrap();
        assert_eq!(svc.state(id), ConnectionState::Disconnected);
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
        let result = svc
            .with_connection_fn(make_connection_id(), |_conn| async move {
                Ok::<_, DriverError>(())
            })
            .await;
        assert!(matches!(result, Err(ServiceError::NotConnected { .. })));
    }

    #[tokio::test]
    async fn with_connection_fn_connection_not_found() {
        let svc = ConnectionService::new(make_event_bus());
        let id = make_connection_id();
        svc.states.write().insert(id, ConnectionState::Connected);
        let result = svc
            .with_connection_fn(id, |_conn| async move { Ok::<_, DriverError>(()) })
            .await;
        assert!(matches!(
            result,
            Err(ServiceError::ConnectionNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn register_driver_stores_driver() {
        let svc = ConnectionService::new(make_event_bus());
        let (driver, _) = MockDriver::ok();
        svc.register_driver(driver);
        assert!(svc.drivers.read().contains_key("postgresql"));
    }
}
