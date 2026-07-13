use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("service '{name}' failed to start: {reason}")]
    StartupFailed { name: &'static str, reason: String },

    #[error("service '{name}' failed to stop: {reason}")]
    ShutdownFailed { name: &'static str, reason: String },
}

/// Base contract every service must implement.
/// Start/stop have default no-op implementations for stateless services.
#[async_trait]
pub trait Service: Any + Send + Sync + 'static {
    fn name(&self) -> &'static str;

    async fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), ServiceError> {
        Ok(())
    }
}

/// Central service registry — typed lookup + ordered lifecycle management.
///
/// Services are registered once at startup in dependency order.
/// `start_all` calls `start()` in registration order; `stop_all` calls `stop()` in reverse.
pub struct ServiceRegistry {
    typed: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    ordered: Mutex<Vec<Arc<dyn Service>>>,
}

impl ServiceRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            typed: RwLock::new(HashMap::new()),
            ordered: Mutex::new(Vec::new()),
        })
    }

    pub fn register<T: Service>(&self, service: Arc<T>) {
        self.typed
            .write()
            .insert(TypeId::of::<T>(), Box::new(service.clone()));
        self.ordered.lock().push(service);
    }

    /// Retrieves a registered service by type.
    /// Panics if T was not registered — this is always a startup-time programming error.
    #[allow(clippy::expect_used)]
    pub fn get<T: Service>(&self) -> Arc<T> {
        self.typed
            .read()
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<Arc<T>>())
            .cloned()
            .expect("service not registered — check startup construction order")
    }

    pub fn service_count(&self) -> usize {
        self.ordered.lock().len()
    }

    pub async fn start_all(&self) -> Result<(), ServiceError> {
        let services: Vec<Arc<dyn Service>> = self.ordered.lock().clone();
        for svc in services {
            svc.start().await?;
        }
        Ok(())
    }

    /// Stops services in reverse registration order (reverse of startup).
    pub async fn stop_all(&self) -> Result<(), ServiceError> {
        let services: Vec<Arc<dyn Service>> = self.ordered.lock().iter().rev().cloned().collect();
        for svc in services {
            svc.stop().await?;
        }
        Ok(())
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self {
            typed: RwLock::new(HashMap::new()),
            ordered: Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct AlphaService {
        started: AtomicBool,
        stopped: AtomicBool,
    }

    impl AlphaService {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                started: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
            })
        }
        fn is_started(&self) -> bool {
            self.started.load(Ordering::SeqCst)
        }
        fn is_stopped(&self) -> bool {
            self.stopped.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Service for AlphaService {
        fn name(&self) -> &'static str {
            "AlphaService"
        }
        async fn start(&self) -> Result<(), ServiceError> {
            self.started.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn stop(&self) -> Result<(), ServiceError> {
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct BetaService;

    impl BetaService {
        fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }

    #[async_trait]
    impl Service for BetaService {
        fn name(&self) -> &'static str {
            "BetaService"
        }
        // default no-op start/stop
    }

    #[tokio::test]
    async fn register_and_lookup() {
        let registry = ServiceRegistry::new();
        let svc = AlphaService::new();
        registry.register(svc.clone());

        let retrieved: Arc<AlphaService> = registry.get::<AlphaService>();
        assert_eq!(retrieved.name(), "AlphaService");
        assert_eq!(registry.service_count(), 1);
    }

    #[tokio::test]
    async fn start_all_calls_start_in_order() {
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(vec![]));

        struct OrderedService {
            label: &'static str,
            order: Arc<Mutex<Vec<&'static str>>>,
        }

        #[async_trait]
        impl Service for OrderedService {
            fn name(&self) -> &'static str {
                self.label
            }
            async fn start(&self) -> Result<(), ServiceError> {
                self.order.lock().push(self.label);
                Ok(())
            }
        }

        let registry = ServiceRegistry::new();
        let o1 = order.clone();
        let o2 = order.clone();
        registry.register(Arc::new(OrderedService {
            label: "first",
            order: o1,
        }));
        registry.register(Arc::new(OrderedService {
            label: "second",
            order: o2,
        }));

        registry.start_all().await.expect("start");
        let seq = order.lock().clone();
        assert_eq!(seq, vec!["first", "second"]);
    }

    #[tokio::test]
    async fn stop_all_calls_stop_in_reverse_order() {
        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(vec![]));

        struct OrderedService {
            label: &'static str,
            order: Arc<Mutex<Vec<&'static str>>>,
        }

        #[async_trait]
        impl Service for OrderedService {
            fn name(&self) -> &'static str {
                self.label
            }
            async fn stop(&self) -> Result<(), ServiceError> {
                self.order.lock().push(self.label);
                Ok(())
            }
        }

        let registry = ServiceRegistry::new();
        let o1 = order.clone();
        let o2 = order.clone();
        let o3 = order.clone();
        registry.register(Arc::new(OrderedService {
            label: "first",
            order: o1,
        }));
        registry.register(Arc::new(OrderedService {
            label: "second",
            order: o2,
        }));
        registry.register(Arc::new(OrderedService {
            label: "third",
            order: o3,
        }));

        registry.stop_all().await.expect("stop");
        let seq = order.lock().clone();
        assert_eq!(seq, vec!["third", "second", "first"]);
    }

    #[tokio::test]
    async fn start_and_stop_lifecycle() {
        let registry = ServiceRegistry::new();
        let alpha = AlphaService::new();
        registry.register(alpha.clone());
        registry.register(BetaService::new());

        assert!(!alpha.is_started());
        registry.start_all().await.expect("start");
        assert!(alpha.is_started());

        assert!(!alpha.is_stopped());
        registry.stop_all().await.expect("stop");
        assert!(alpha.is_stopped());
    }

    #[tokio::test]
    async fn default_noop_start_stop() {
        let registry = ServiceRegistry::new();
        registry.register(BetaService::new());
        registry.start_all().await.expect("start no-op");
        registry.stop_all().await.expect("stop no-op");
    }
}
