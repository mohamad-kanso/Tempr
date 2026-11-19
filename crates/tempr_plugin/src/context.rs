//! Registration surface handed to a plugin during `activate`.
//!
//! Every check that can fail runs here, inside the plugin's own call, so the
//! plugin sees the error from its own `activate` instead of discovering later
//! that a registration vanished. The host drains the collected registrations
//! only after `activate` returns `Ok`, which is what makes "failed plugin
//! leaves no trace" true.

use std::sync::Arc;

use tempr_db::DatabaseDriver;
use tempr_domain::PluginId;
use tempr_events::EventBus;

use crate::error::PluginError;
use crate::extension::{CompletionProvider, Panel, PluginCommandSpec, ResultRenderer};

/// Registrations collected during one `activate` call.
#[derive(Default)]
pub struct Registrations {
    pub drivers: Vec<Arc<dyn DatabaseDriver>>,
    pub completion_providers: Vec<Arc<dyn CompletionProvider>>,
    pub result_renderers: Vec<Arc<dyn ResultRenderer>>,
    pub commands: Vec<PluginCommandSpec>,
    pub panels: Vec<Arc<dyn Panel>>,
}

impl Registrations {
    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
            && self.completion_providers.is_empty()
            && self.result_renderers.is_empty()
            && self.commands.is_empty()
            && self.panels.is_empty()
    }
}

pub struct PluginContext<'a> {
    plugin_id: PluginId,
    namespace: &'static str,
    events: &'a EventBus,
    registrations: Registrations,
}

impl<'a> PluginContext<'a> {
    pub fn new(plugin_id: PluginId, namespace: &'static str, events: &'a EventBus) -> Self {
        Self {
            plugin_id,
            namespace,
            events,
            registrations: Registrations::default(),
        }
    }

    pub fn plugin_id(&self) -> PluginId {
        self.plugin_id
    }

    pub fn namespace(&self) -> &'static str {
        self.namespace
    }

    /// Read-only event bus. Plugins may publish their own namespaced events
    /// and subscribe to anything the host publishes.
    pub fn events(&self) -> &'a EventBus {
        self.events
    }

    pub fn register_driver(&mut self, driver: Arc<dyn DatabaseDriver>) -> Result<(), PluginError> {
        let engine = driver.engine();
        if let Some(existing) = self
            .registrations
            .drivers
            .iter()
            .find(|d| d.engine() == engine)
        {
            let _ = existing;
            return Err(PluginError::DuplicateRegistration {
                kind: "driver",
                id: engine.0,
            });
        }
        self.registrations.drivers.push(driver);
        Ok(())
    }

    pub fn register_completion_provider(
        &mut self,
        provider: Arc<dyn CompletionProvider>,
    ) -> Result<(), PluginError> {
        if self
            .registrations
            .completion_providers
            .iter()
            .any(|p| p.provider_name() == provider.provider_name())
        {
            return Err(PluginError::DuplicateRegistration {
                kind: "completion provider",
                id: provider.provider_name().to_string(),
            });
        }
        self.registrations.completion_providers.push(provider);
        Ok(())
    }

    /// Renderers are positional: order decides precedence, so duplicates are
    /// meaningless and always accepted.
    pub fn register_result_renderer(
        &mut self,
        renderer: Arc<dyn ResultRenderer>,
    ) -> Result<(), PluginError> {
        self.registrations.result_renderers.push(renderer);
        Ok(())
    }

    pub fn register_command(&mut self, command: PluginCommandSpec) -> Result<(), PluginError> {
        self.check_namespace(command.id.as_str())?;
        if self
            .registrations
            .commands
            .iter()
            .any(|c| c.id == command.id)
        {
            return Err(PluginError::DuplicateRegistration {
                kind: "command",
                id: command.id.to_string(),
            });
        }
        self.registrations.commands.push(command);
        Ok(())
    }

    pub fn register_panel(&mut self, panel: Arc<dyn Panel>) -> Result<(), PluginError> {
        self.check_namespace(panel.panel_id())?;
        if self
            .registrations
            .panels
            .iter()
            .any(|p| p.panel_id() == panel.panel_id())
        {
            return Err(PluginError::DuplicateRegistration {
                kind: "panel",
                id: panel.panel_id().to_string(),
            });
        }
        self.registrations.panels.push(panel);
        Ok(())
    }

    /// Consumed by `PluginService` after a successful `activate`.
    pub fn into_registrations(self) -> Registrations {
        self.registrations
    }

    fn check_namespace(&self, id: &str) -> Result<(), PluginError> {
        let owned = id
            .strip_prefix(self.namespace)
            .and_then(|rest| rest.strip_prefix("::"))
            .is_some_and(|local| !local.is_empty());
        if owned {
            Ok(())
        } else {
            Err(PluginError::ForeignNamespace {
                plugin: self.namespace.to_string(),
                id: id.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{CommandOutcome, Panel};
    use gpui::{AnyElement, App, IntoElement, SharedString, Window};
    use tempr_db::{DatabaseDriver, DriverConnection, DriverError, EngineId};
    use tempr_domain::{CommandId, Connection};
    use tempr_events::EventBus;

    struct FakeDriver(&'static str);

    #[async_trait::async_trait]
    impl DatabaseDriver for FakeDriver {
        fn engine(&self) -> EngineId {
            EngineId(self.0.to_string())
        }

        async fn connect(
            &self,
            _connection: &Connection,
        ) -> Result<Box<dyn DriverConnection>, DriverError> {
            Err(DriverError::Unsupported("not a real driver".into()))
        }
    }

    struct FakePanel(&'static str);

    impl Panel for FakePanel {
        fn panel_id(&self) -> &'static str {
            self.0
        }

        fn title(&self) -> SharedString {
            SharedString::from("Fake")
        }

        fn render(&mut self, _window: &mut Window, _cx: &mut App) -> AnyElement {
            gpui::div().into_any_element()
        }
    }

    fn command(id: &str) -> PluginCommandSpec {
        PluginCommandSpec::new(
            CommandId::new(id),
            "Do it",
            "Plugin",
            &["ctrl-alt-y"],
            |_w, _cx, _ctx| CommandOutcome::done("ok"),
        )
    }

    fn context<'a>(bus: &'a EventBus, namespace: &'static str) -> PluginContext<'a> {
        PluginContext::new(PluginId::derived(namespace), namespace, bus)
    }

    #[test]
    fn a_plugin_may_not_register_another_plugins_command() {
        let bus = EventBus::new();
        let mut cx = context(&bus, "dev.tempr.demo");
        let err = cx
            .register_command(command("dev.tempr.other::run"))
            .expect_err("foreign namespace");
        assert!(matches!(err, PluginError::ForeignNamespace { .. }));
    }

    #[test]
    fn duplicate_command_ids_within_a_plugin_are_rejected() {
        let bus = EventBus::new();
        let mut cx = context(&bus, "dev.tempr.demo");
        cx.register_command(command("dev.tempr.demo::run"))
            .expect("first");
        let err = cx
            .register_command(command("dev.tempr.demo::run"))
            .expect_err("duplicate");
        assert_eq!(err.registration_kind(), Some("command"));
    }

    #[test]
    fn duplicate_driver_engines_are_rejected() {
        let bus = EventBus::new();
        let mut cx = context(&bus, "dev.tempr.demo");
        cx.register_driver(Arc::new(FakeDriver("postgresql")))
            .expect("first");
        let err = cx
            .register_driver(Arc::new(FakeDriver("postgresql")))
            .expect_err("duplicate");
        assert_eq!(err.registration_kind(), Some("driver"));
    }

    #[test]
    fn two_renderers_are_allowed_because_order_decides() {
        struct R;
        impl ResultRenderer for R {
            fn can_render(&self, _col: &tempr_domain::ColumnSpec) -> bool {
                true
            }
            fn render_cell(
                &self,
                _value: &tempr_domain::Value,
                _col: &tempr_domain::ColumnSpec,
            ) -> AnyElement {
                gpui::div().into_any_element()
            }
        }
        let bus = EventBus::new();
        let mut cx = context(&bus, "dev.tempr.demo");
        cx.register_result_renderer(Arc::new(R)).expect("first");
        cx.register_result_renderer(Arc::new(R)).expect("second");
        assert_eq!(cx.into_registrations().result_renderers.len(), 2);
    }

    #[test]
    fn panels_are_namespace_scoped() {
        let bus = EventBus::new();
        let mut cx = context(&bus, "dev.tempr.demo");
        cx.register_panel(Arc::new(FakePanel("dev.tempr.demo::stats")))
            .expect("own panel");
        let err = cx
            .register_panel(Arc::new(FakePanel("dev.tempr.other::stats")))
            .expect_err("foreign panel");
        assert!(matches!(err, PluginError::ForeignNamespace { .. }));
    }
}
