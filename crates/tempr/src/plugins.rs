//! The static plugin list: every plugin linked into this binary (D8, D31).
//!
//! Order does not matter — `PluginService` activates by manifest `requires`
//! and rejects cycles. This file is the whole registry: adding a plugin is
//! one line here plus the crate dependency, and nothing else in the host
//! changes.

use tempr_db_postgres::PostgresPlugin;
use tempr_plugin::Plugin;
use tempr_plugin_export_csv::ExportCsvPlugin;

/// Plugins compiled into Tempr.
pub fn static_plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(PostgresPlugin),
        Box::new(ExportCsvPlugin::default()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_db::EngineId;
    use tempr_events::EventBus;
    use tempr_services::{CommandService, PluginService};

    #[test]
    fn the_static_list_provides_exactly_one_postgresql_driver() {
        let bus = std::sync::Arc::new(EventBus::new());
        let command = CommandService::new(bus.clone());
        let plugin = PluginService::new(bus, command);
        let report = plugin.activate_all(static_plugins());

        assert!(report.all_activated(), "{:?}", report.failed);
        assert_eq!(plugin.engines(), vec![EngineId("postgresql".into())]);
    }
}
