use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::{DatabaseDriver, DriverConnection, DriverError};
use tempr_domain::{Connection, ConnectionId, ConnectionState};
use tempr_events::{AppEvent, EventBus};

use crate::ServiceError;

struct PoolSlot {
    connection: Option<Box<dyn DriverConnection>>,
}

pub struct ConnectionService {
    event_bus: Arc<EventBus>,
    drivers: RwLock<HashMap<String, Arc<dyn DatabaseDriver>>>,
    pools: RwLock<HashMap<ConnectionId, PoolSlot>>,
    states: RwLock<HashMap<ConnectionId, ConnectionState>>,
}

impl ConnectionService {
    pub fn new(event_bus: Arc<EventBus>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            drivers: RwLock::new(HashMap::new()),
            pools: RwLock::new(HashMap::new()),
            states: RwLock::new(HashMap::new()),
        })
    }

    pub fn register_driver(&self, driver: Arc<dyn DatabaseDriver>) {
        let engine = driver.engine().0.clone();
        self.drivers.write().insert(engine, driver);
    }

    pub async fn connect(&self, connection: &Connection) -> Result<(), ServiceError> {
        let id = connection.id;
        let engine = connection.driver.engine_name();

        self.states.write().insert(id, ConnectionState::Connecting);
        self.event_bus.publish(AppEvent::ConnectionStateChanged {
            id,
            state: ConnectionState::Connecting,
        });

        let driver = {
            let drivers = self.drivers.read();
            drivers
                .get(engine)
                .cloned()
                .ok_or_else(|| ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: format!("no driver registered for engine '{engine}'"),
                })?
        };

        match driver.connect(connection).await {
            Ok(conn) => {
                self.pools.write().insert(
                    id,
                    PoolSlot {
                        connection: Some(conn),
                    },
                );
                self.states.write().insert(id, ConnectionState::Connected);
                self.event_bus.publish(AppEvent::ConnectionStateChanged {
                    id,
                    state: ConnectionState::Connected,
                });
                Ok(())
            }
            Err(e) => {
                let state = match &e {
                    DriverError::AuthFailed(_) => ConnectionState::Failed,
                    DriverError::ConnectionRefused(_) => ConnectionState::Failed,
                    _ => ConnectionState::Failed,
                };
                self.states.write().insert(id, state);
                self.event_bus
                    .publish(AppEvent::ConnectionStateChanged { id, state });
                Err(ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: e.to_string(),
                })
            }
        }
    }

    pub async fn disconnect(&self, id: ConnectionId) -> Result<(), ServiceError> {
        self.pools.write().remove(&id);
        self.states
            .write()
            .insert(id, ConnectionState::Disconnected);
        self.event_bus.publish(AppEvent::ConnectionStateChanged {
            id,
            state: ConnectionState::Disconnected,
        });
        Ok(())
    }

    pub fn state(&self, id: ConnectionId) -> ConnectionState {
        self.states
            .read()
            .get(&id)
            .copied()
            .unwrap_or(ConnectionState::Disconnected)
    }

    /// Take the connection out of the pool, pass it to the closure, and put it back.
    /// This ensures exclusive access while the closure runs.
    pub async fn with_connection<F, R>(&self, id: ConnectionId, f: F) -> Result<R, ServiceError>
    where
        F: std::future::Future<Output = Result<R, DriverError>>,
    {
        let state = self.state(id);
        if state != ConnectionState::Connected {
            return Err(ServiceError::StartupFailed {
                name: "ConnectionService",
                reason: format!("connection {id} is in state {state:?}, not Connected"),
            });
        }

        let conn = {
            let mut pools = self.pools.write();
            let slot = pools
                .get_mut(&id)
                .ok_or_else(|| ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: format!("no pool for connection {id}"),
                })?;
            slot.connection
                .take()
                .ok_or_else(|| ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: "connection already borrowed".to_string(),
                })?
        };

        let result = f.await;

        if let Some(slot) = self.pools.write().get_mut(&id) {
            slot.connection = Some(conn);
        }

        result.map_err(|e| ServiceError::StartupFailed {
            name: "ConnectionService",
            reason: e.to_string(),
        })
    }

    /// Take the connection, run a fallible future that needs the connection,
    /// and return the connection back along with the result.
    pub async fn with_connection_fn<F, Fut, R>(
        &self,
        id: ConnectionId,
        f: F,
    ) -> Result<R, ServiceError>
    where
        F: FnOnce(Box<dyn DriverConnection>) -> Fut,
        Fut: std::future::Future<Output = (Box<dyn DriverConnection>, Result<R, DriverError>)>,
    {
        let state = self.state(id);
        if state != ConnectionState::Connected {
            return Err(ServiceError::StartupFailed {
                name: "ConnectionService",
                reason: format!("connection {id} is in state {state:?}, not Connected"),
            });
        }

        let conn = {
            let mut pools = self.pools.write();
            let slot = pools
                .get_mut(&id)
                .ok_or_else(|| ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: format!("no pool for connection {id}"),
                })?;
            slot.connection
                .take()
                .ok_or_else(|| ServiceError::StartupFailed {
                    name: "ConnectionService",
                    reason: "connection already borrowed".to_string(),
                })?
        };

        let (conn, result) = f(conn).await;

        if let Some(slot) = self.pools.write().get_mut(&id) {
            slot.connection = Some(conn);
        }

        result.map_err(|e| ServiceError::StartupFailed {
            name: "ConnectionService",
            reason: e.to_string(),
        })
    }
}
