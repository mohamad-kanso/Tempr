//! The PostgreSQL driver, contributed through the plugin API.
//!
//! The driver itself is unchanged — this is the registration a plugin makes,
//! proving the extension point is real: the binary no longer constructs
//! `PostgresDriver` directly, it asks `PluginService` for the engine.

use std::sync::Arc;

use tempr_db::DatabaseDriver;
use tempr_plugin::{Capability, Plugin, PluginContext, PluginError, PluginManifest};

use crate::driver::PostgresDriver;

pub struct PostgresPlugin;

impl PostgresPlugin {
    pub const MANIFEST: PluginManifest = PluginManifest {
        id: "dev.tempr.postgres",
        name: "PostgreSQL driver",
        version: env!("CARGO_PKG_VERSION"),
        api_version: tempr_plugin::PLUGIN_API_VERSION,
        requires: &[],
        capabilities: &[Capability::Driver {
            engine: "postgresql",
        }],
    };
}

impl Plugin for PostgresPlugin {
    fn manifest(&self) -> &PluginManifest {
        &Self::MANIFEST
    }

    fn activate(&mut self, cx: &mut PluginContext<'_>) -> Result<(), PluginError> {
        let driver = Arc::new(PostgresDriver::new()) as Arc<dyn DatabaseDriver>;
        cx.register_driver(driver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_declares_the_postgresql_engine() {
        PostgresPlugin::MANIFEST.validate().expect("valid manifest");
        assert!(matches!(
            PostgresPlugin::MANIFEST.capabilities,
            [Capability::Driver {
                engine: "postgresql"
            }]
        ));
    }

    #[test]
    fn activation_registers_one_driver() {
        let bus = tempr_events::EventBus::new();
        let mut cx = PluginContext::new(
            PostgresPlugin::MANIFEST.plugin_id(),
            PostgresPlugin::MANIFEST.id,
            &bus,
        );
        PostgresPlugin
            .activate(&mut cx)
            .expect("activation registers a driver");
        let registrations = cx.into_registrations();
        assert_eq!(registrations.drivers.len(), 1);
        assert_eq!(registrations.drivers[0].engine().0, "postgresql");
    }
}
