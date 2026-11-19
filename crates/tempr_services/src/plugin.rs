//! `PluginService` — activation and the registries plugins contribute to
//! (docs/05-services.md, docs/08-plugin-api.md).
//!
//! Plugins are Rust crates linked into the binary (D8, D31): the service
//! receives them as a single list at startup, orders them by `requires`,
//! activates each inside a panic boundary, and drains what each one
//! registered. A failing plugin is reported and skipped — it can never take
//! the host down, and it can never leave a half-installed registration
//! behind, because nothing reaches the registries until `activate` returns
//! `Ok`.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tempr_db::{DatabaseDriver, EngineId};
use tempr_domain::{CommandId, PluginId};
use tempr_events::{AppEvent, EventBus};
use tempr_plugin::context::Registrations;
use tempr_plugin::{
    COMMAND_CONTEXT, CompletionProvider, PLUGIN_API_VERSION, Panel, Plugin, PluginCommandSpec,
    PluginContext, PluginError, ResultRenderer,
};

use crate::command::{CommandContribution, CommandService};
use crate::{Service, ServiceError};

/// What happened during startup activation. Logged by the binary and
/// readable by tests; the UI only needs `failed.is_empty()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivationReport {
    pub activated: Vec<PluginId>,
    pub failed: Vec<(PluginId, String)>,
    pub active_version: (u16, u16),
}

impl ActivationReport {
    pub fn all_activated(&self) -> bool {
        self.failed.is_empty()
    }
}

/// One registered command, tagged with its owner so deactivation can remove
/// exactly that plugin's contributions.
struct OwnedCommand {
    plugin: PluginId,
    spec: PluginCommandSpec,
}

pub struct PluginService {
    event_bus: Arc<EventBus>,
    commands: Arc<CommandService>,
    drivers: RwLock<Vec<(PluginId, Arc<dyn DatabaseDriver>)>>,
    completion_providers: RwLock<Vec<(PluginId, Arc<dyn CompletionProvider>)>>,
    result_renderers: RwLock<Vec<(PluginId, Arc<dyn ResultRenderer>)>>,
    panels: RwLock<Vec<(PluginId, Arc<dyn Panel>)>>,
    plugin_commands: RwLock<Vec<OwnedCommand>>,
    /// Activation order, so `deactivate_all` runs in reverse.
    active: RwLock<Vec<Box<dyn Plugin>>>,
    report: RwLock<ActivationReport>,
}

impl PluginService {
    pub fn new(event_bus: Arc<EventBus>, commands: Arc<CommandService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            commands,
            drivers: RwLock::new(Vec::new()),
            completion_providers: RwLock::new(Vec::new()),
            result_renderers: RwLock::new(Vec::new()),
            panels: RwLock::new(Vec::new()),
            plugin_commands: RwLock::new(Vec::new()),
            active: RwLock::new(Vec::new()),
            report: RwLock::new(ActivationReport::default()),
        })
    }

    /// Validate, order and activate every plugin. Never panics: a plugin that
    /// panics or returns an error is recorded in the report and skipped.
    pub fn activate_all(self: &Arc<Self>, plugins: Vec<Box<dyn Plugin>>) -> ActivationReport {
        let mut plugins = plugins;
        // Deterministic order independent of the caller's list order, so a
        // dependency chain activates the same way across runs.
        plugins.sort_by_key(|plugin| plugin.manifest().id);

        let (order, mut failed) = activation_order(&plugins);
        let mut activated = Vec::new();
        let mut activated_plugins = Vec::new();
        let mut report = ActivationReport {
            activated: Vec::new(),
            failed: Vec::new(),
            active_version: PLUGIN_API_VERSION,
        };

        for index in order {
            let plugin = &mut plugins[index];
            let manifest = plugin.manifest().clone();
            let plugin_id = manifest.plugin_id();

            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let mut cx = PluginContext::new(plugin_id, manifest.id, &self.event_bus);
                let result = plugin.activate(&mut cx);
                (result, cx.into_registrations())
            }));

            match outcome {
                Ok((Ok(()), registrations)) => match self.install(plugin_id, registrations) {
                    Ok(()) => {
                        activated.push(plugin_id);
                        activated_plugins.push(index);
                    }
                    Err(e) => failed.push((plugin_id, e.to_string())),
                },
                Ok((Err(e), _)) => failed.push((plugin_id, e.to_string())),
                Err(_) => failed.push((
                    plugin_id,
                    PluginError::ActivationPanicked {
                        plugin: manifest.id.to_string(),
                    }
                    .to_string(),
                )),
            }
        }

        // Keep the boxes alive in activation order so `deactivate` can run.
        // `plugins` is consumed here; only the successful ones are retained
        // because a failed plugin's `activate` may have left partial state.
        let mut remaining: Vec<Option<Box<dyn Plugin>>> = plugins.into_iter().map(Some).collect();
        for index in activated_plugins {
            if let Some(plugin) = remaining[index].take() {
                self.active.write().push(plugin);
            }
        }

        report.activated = activated;
        report.failed = failed;
        *self.report.write() = report.clone();

        self.event_bus.publish(AppEvent::PluginsActivated {
            count: report.activated.len(),
            failed: report.failed.iter().map(|(id, _)| *id).collect(),
        });

        report
    }

    /// Install one plugin's registrations atomically: every cross-plugin
    /// check runs first, so a rejection leaves the registries untouched.
    fn install(&self, plugin: PluginId, registrations: Registrations) -> Result<(), PluginError> {
        {
            let drivers = self.drivers.read();
            for driver in &registrations.drivers {
                if drivers
                    .iter()
                    .any(|(_, existing)| existing.engine() == driver.engine())
                {
                    return Err(PluginError::DuplicateRegistration {
                        kind: "driver",
                        id: driver.engine().0,
                    });
                }
            }
        }
        {
            let commands = self.plugin_commands.read();
            for spec in &registrations.commands {
                if commands.iter().any(|owned| owned.spec.id == spec.id) {
                    return Err(PluginError::DuplicateRegistration {
                        kind: "command",
                        id: spec.id.to_string(),
                    });
                }
            }
        }
        {
            let panels = self.panels.read();
            for panel in &registrations.panels {
                if panels
                    .iter()
                    .any(|(_, existing)| existing.panel_id() == panel.panel_id())
                {
                    return Err(PluginError::DuplicateRegistration {
                        kind: "panel",
                        id: panel.panel_id().to_string(),
                    });
                }
            }
        }

        for spec in &registrations.commands {
            self.commands.register(CommandContribution {
                id: spec.id.clone(),
                title: spec.title.to_string(),
                category: spec.category.to_string(),
                context: Some(COMMAND_CONTEXT.to_string()),
                default_keystrokes: spec
                    .default_keystrokes
                    .iter()
                    .map(|k| k.to_string())
                    .collect(),
                hidden: false,
            });
        }

        self.drivers.write().extend(
            registrations
                .drivers
                .into_iter()
                .map(|driver| (plugin, driver)),
        );
        self.completion_providers.write().extend(
            registrations
                .completion_providers
                .into_iter()
                .map(|provider| (plugin, provider)),
        );
        self.result_renderers.write().extend(
            registrations
                .result_renderers
                .into_iter()
                .map(|renderer| (plugin, renderer)),
        );
        self.panels.write().extend(
            registrations
                .panels
                .into_iter()
                .map(|panel| (plugin, panel)),
        );
        self.plugin_commands.write().extend(
            registrations
                .commands
                .into_iter()
                .map(|spec| OwnedCommand { plugin, spec }),
        );

        Ok(())
    }

    /// The driver a connected engine resolves to. This is the only way the
    /// binary obtains a driver, so "a plugin provides PostgreSQL" is a fact
    /// rather than a comment.
    pub fn driver(&self, engine: &EngineId) -> Option<Arc<dyn DatabaseDriver>> {
        self.drivers
            .read()
            .iter()
            .find(|(_, driver)| driver.engine() == *engine)
            .map(|(_, driver)| driver.clone())
    }

    /// Every registered engine, in registration order.
    pub fn engines(&self) -> Vec<EngineId> {
        self.drivers
            .read()
            .iter()
            .map(|(_, driver)| driver.engine())
            .collect()
    }

    pub fn completion_providers(&self) -> Vec<Arc<dyn CompletionProvider>> {
        self.completion_providers
            .read()
            .iter()
            .map(|(_, provider)| provider.clone())
            .collect()
    }

    pub fn result_renderers(&self) -> Vec<(PluginId, Arc<dyn ResultRenderer>)> {
        self.result_renderers
            .read()
            .iter()
            .map(|(plugin, renderer)| (*plugin, renderer.clone()))
            .collect()
    }

    pub fn command(&self, id: &CommandId) -> Option<PluginCommandSpec> {
        self.plugin_commands
            .read()
            .iter()
            .find(|owned| &owned.spec.id == id)
            .map(|owned| PluginCommandSpec {
                id: owned.spec.id.clone(),
                title: owned.spec.title,
                category: owned.spec.category,
                default_keystrokes: owned.spec.default_keystrokes.clone(),
                dispatch: owned.spec.dispatch.clone(),
            })
    }

    pub fn command_specs(&self) -> Vec<PluginCommandSpec> {
        self.plugin_commands
            .read()
            .iter()
            .map(|owned| PluginCommandSpec {
                id: owned.spec.id.clone(),
                title: owned.spec.title,
                category: owned.spec.category,
                default_keystrokes: owned.spec.default_keystrokes.clone(),
                dispatch: owned.spec.dispatch.clone(),
            })
            .collect()
    }

    pub fn panels(&self) -> Vec<Arc<dyn Panel>> {
        self.panels
            .read()
            .iter()
            .map(|(_, panel)| panel.clone())
            .collect()
    }

    pub fn activation_report(&self) -> ActivationReport {
        self.report.read().clone()
    }

    /// Deactivate in reverse activation order, then drop every registration
    /// and unregister the command metadata. Navigation-safe: a plugin's
    /// `deactivate` runs before its data disappears.
    pub fn deactivate_all(&self) {
        let active = std::mem::take(&mut *self.active.write());
        for mut plugin in active.into_iter().rev() {
            let name = plugin.manifest().id;
            if catch_unwind(AssertUnwindSafe(|| plugin.deactivate())).is_err() {
                tracing::warn!(plugin = name, "plugin panicked while deactivating");
            }
        }

        for owned in self.plugin_commands.write().drain(..) {
            tracing::debug!(
                command = %owned.spec.id,
                plugin = %owned.plugin,
                "unregistering plugin command"
            );
            self.commands.unregister(&owned.spec.id);
        }
        self.drivers.write().clear();
        self.completion_providers.write().clear();
        self.result_renderers.write().clear();
        self.panels.write().clear();
    }
}

#[async_trait]
impl Service for PluginService {
    fn name(&self) -> &'static str {
        "PluginService"
    }

    async fn stop(&self) -> Result<(), ServiceError> {
        self.deactivate_all();
        Ok(())
    }
}

/// Marks used by the dependency walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Unvisited,
    Visiting,
    Done,
    Failed,
}

/// Topological order plus one error per plugin that cannot activate.
///
/// Failures are attributed, never merged: a cycle names every plugin in the
/// cycle, and a plugin whose dependency failed says so instead of pretending
/// the dependency is missing.
fn activation_order(plugins: &[Box<dyn Plugin>]) -> (Vec<usize>, Vec<(PluginId, String)>) {
    let mut by_id: HashMap<&'static str, usize> = HashMap::new();
    let mut marks = vec![Mark::Unvisited; plugins.len()];
    let mut order = Vec::new();
    let mut failures: Vec<(usize, PluginError)> = Vec::new();
    let mut stack = Vec::new();

    for (index, plugin) in plugins.iter().enumerate() {
        let id = plugin.manifest().id;
        if let Some(previous) = by_id.insert(id, index) {
            let error = PluginError::DuplicateRegistration {
                kind: "plugin",
                id: format!("{id} (also linked at index {previous})"),
            };
            marks[index] = Mark::Failed;
            failures.push((index, error));
        }
    }

    // Sorted indices keep the walk deterministic even when the caller's list
    // order changes.
    let mut roots: Vec<usize> = (0..plugins.len()).collect();
    roots.sort_by_key(|&index| plugins[index].manifest().id);

    for root in roots {
        visit(
            root,
            plugins,
            &by_id,
            &mut marks,
            &mut order,
            &mut failures,
            &mut stack,
        );
    }

    let failures = failures
        .into_iter()
        .map(|(index, error)| (plugins[index].manifest().plugin_id(), error.to_string()))
        .collect();
    (order, failures)
}

#[allow(clippy::too_many_arguments)]
fn visit(
    index: usize,
    plugins: &[Box<dyn Plugin>],
    by_id: &HashMap<&'static str, usize>,
    marks: &mut [Mark],
    order: &mut Vec<usize>,
    failures: &mut Vec<(usize, PluginError)>,
    stack: &mut Vec<usize>,
) {
    match marks[index] {
        Mark::Done | Mark::Failed => return,
        Mark::Visiting => {
            let start = stack.iter().position(|&i| i == index).unwrap_or(0);
            let members: Vec<String> = stack[start..]
                .iter()
                .map(|&i| plugins[i].manifest().id.to_string())
                .collect();
            for &member in &stack[start..] {
                if marks[member] != Mark::Failed {
                    marks[member] = Mark::Failed;
                    failures.push((
                        member,
                        PluginError::DependencyCycle {
                            plugins: members.clone(),
                        },
                    ));
                }
            }
            return;
        }
        Mark::Unvisited => {}
    }

    let manifest = plugins[index].manifest().clone();
    marks[index] = Mark::Visiting;

    if let Err(error) = manifest.validate() {
        marks[index] = Mark::Failed;
        failures.push((index, error));
        return;
    }

    stack.push(index);
    let mut failed = None;
    for dependency in manifest.requires {
        match by_id.get(dependency) {
            None => {
                failed = Some(PluginError::MissingDependency {
                    plugin: manifest.id.to_string(),
                    missing: (*dependency).to_string(),
                });
                break;
            }
            Some(&dependency_index) => {
                visit(
                    dependency_index,
                    plugins,
                    by_id,
                    marks,
                    order,
                    failures,
                    stack,
                );
                if marks[dependency_index] == Mark::Failed {
                    failed = Some(PluginError::DependencyFailed {
                        plugin: manifest.id.to_string(),
                        dependency: (*dependency).to_string(),
                    });
                    break;
                }
            }
        }
    }
    stack.pop();

    match failed {
        Some(error) => {
            if marks[index] != Mark::Failed {
                marks[index] = Mark::Failed;
                failures.push((index, error));
            }
        }
        None => {
            marks[index] = Mark::Done;
            order.push(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempr_db::{DriverConnection, DriverError};
    use tempr_domain::{CommandId, Connection};
    use tempr_plugin::{Capability, CommandOutcome, PluginCommandSpec, PluginManifest};

    struct FakePlugin {
        manifest: PluginManifest,
        activated: Arc<AtomicUsize>,
        fail: bool,
        panics: bool,
        registers_driver: bool,
        deactivated: Arc<AtomicUsize>,
        requires_ok: Arc<AtomicUsize>,
        dependency_marker: Option<Arc<AtomicUsize>>,
    }

    impl FakePlugin {
        fn new(id: &'static str) -> Self {
            Self {
                manifest: PluginManifest {
                    id,
                    name: "Fake",
                    version: "0.1.0",
                    api_version: PLUGIN_API_VERSION,
                    requires: &[],
                    capabilities: &[],
                },
                activated: Arc::default(),
                fail: false,
                panics: false,
                registers_driver: false,
                deactivated: Arc::default(),
                requires_ok: Arc::default(),
                dependency_marker: None,
            }
        }

        fn requiring(mut self, plugin: &'static str, marker: Arc<AtomicUsize>) -> Self {
            self.manifest.requires = Box::leak(vec![plugin].into_boxed_slice());
            self.dependency_marker = Some(marker);
            self
        }
    }

    impl Plugin for FakePlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn activate(&mut self, cx: &mut PluginContext<'_>) -> Result<(), PluginError> {
            if self.panics {
                panic!("boom");
            }
            if self.fail {
                return Err(PluginError::ActivationFailed {
                    plugin: self.manifest.id.to_string(),
                    message: "nope".into(),
                });
            }
            if let Some(marker) = &self.dependency_marker
                && marker.load(Ordering::SeqCst) == 0
            {
                return Err(PluginError::ActivationFailed {
                    plugin: self.manifest.id.to_string(),
                    message: "dependency not activated yet".into(),
                });
            }
            if self.registers_driver {
                cx.register_driver(Arc::new(FakeDriver("postgresql")))?;
            }
            cx.register_command(PluginCommandSpec::new(
                CommandId::new(format!("{}::run", self.manifest.id)),
                "Run",
                "Plugin",
                &[],
                |_window, _cx, _context| CommandOutcome::done("ok"),
            ))?;
            self.activated.fetch_add(1, Ordering::SeqCst);
            self.requires_ok.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn deactivate(&mut self) {
            self.deactivated.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeDriver(&'static str);

    #[async_trait]
    impl DatabaseDriver for FakeDriver {
        fn engine(&self) -> EngineId {
            EngineId(self.0.to_string())
        }

        async fn connect(
            &self,
            _connection: &Connection,
        ) -> Result<Box<dyn DriverConnection>, DriverError> {
            Err(DriverError::Unsupported("fake".into()))
        }
    }

    fn service() -> Arc<PluginService> {
        let bus = Arc::new(EventBus::new());
        let commands = CommandService::new(bus.clone());
        PluginService::new(bus, commands)
    }

    #[test]
    fn a_dependency_chain_activates_in_order() {
        let base = FakePlugin::new("dev.tempr.base");
        let base_activated = base.activated.clone();
        let dependent =
            FakePlugin::new("dev.tempr.dependent").requiring("dev.tempr.base", base_activated);

        let service = service();
        let report = service.activate_all(vec![Box::new(dependent), Box::new(base)]);

        assert!(report.all_activated(), "{:?}", report.failed);
        assert_eq!(report.activated.len(), 2);
    }

    #[test]
    fn a_failing_plugin_does_not_stop_the_others() {
        let mut broken = FakePlugin::new("dev.tempr.broken");
        broken.fail = true;
        let healthy = FakePlugin::new("dev.tempr.healthy");
        let healthy_activated = healthy.activated.clone();

        let service = service();
        let report = service.activate_all(vec![Box::new(broken), Box::new(healthy)]);

        assert_eq!(report.activated.len(), 1);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(healthy_activated.load(Ordering::SeqCst), 1);
        assert_eq!(service.command_specs().len(), 1);
    }

    #[test]
    fn a_panicking_plugin_is_reported_and_isolated() {
        let mut panicky = FakePlugin::new("dev.tempr.panic");
        panicky.panics = true;
        let healthy = FakePlugin::new("dev.tempr.healthy");

        let service = service();
        let report = service.activate_all(vec![Box::new(panicky), Box::new(healthy)]);

        assert_eq!(report.activated.len(), 1);
        assert!(report.failed[0].1.contains("panicked"));
    }

    #[test]
    fn a_plugin_whose_dependency_failed_does_not_activate() {
        let mut broken = FakePlugin::new("dev.tempr.base");
        broken.fail = true;
        let marker = Arc::new(AtomicUsize::new(0));
        let dependent = FakePlugin::new("dev.tempr.dependent").requiring("dev.tempr.base", marker);

        let service = service();
        let report = service.activate_all(vec![Box::new(broken), Box::new(dependent)]);

        assert!(report.activated.is_empty());
        assert_eq!(report.failed.len(), 2);
        assert!(
            report
                .failed
                .iter()
                .any(|(_, message)| message.contains("failed to activate"))
        );
    }

    #[test]
    fn an_unlinked_dependency_is_reported() {
        let marker = Arc::new(AtomicUsize::new(1));
        let dependent =
            FakePlugin::new("dev.tempr.dependent").requiring("dev.tempr.absent", marker);

        let service = service();
        let report = service.activate_all(vec![Box::new(dependent)]);

        assert!(report.activated.is_empty());
        assert!(report.failed[0].1.contains("not linked"));
    }

    #[test]
    fn a_cycle_is_reported_once_per_member() {
        let a = FakePlugin::new("dev.tempr.a");
        let b = FakePlugin::new("dev.tempr.b");
        let a_marker = a.activated.clone();
        let a = a.requiring("dev.tempr.b", a_marker);
        let b_marker = b.activated.clone();
        let b = b.requiring("dev.tempr.a", b_marker);

        let service = service();
        let report = service.activate_all(vec![Box::new(a), Box::new(b)]);

        assert!(!report.all_activated());
        assert_eq!(report.failed.len(), 2);
        assert!(
            report
                .failed
                .iter()
                .all(|(_, message)| message.contains("cycle"))
        );
    }

    #[test]
    fn a_duplicate_engine_across_plugins_fails_the_second() {
        let mut first = FakePlugin::new("dev.tempr.first");
        first.registers_driver = true;
        let mut second = FakePlugin::new("dev.tempr.second");
        second.registers_driver = true;

        let service = service();
        let report = service.activate_all(vec![Box::new(first), Box::new(second)]);

        assert_eq!(report.activated.len(), 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].1.contains("duplicate driver"));
        assert_eq!(service.engines().len(), 1);
    }

    #[test]
    fn registered_commands_reach_the_command_service() {
        let plugin = FakePlugin::new("dev.tempr.demo");
        let service = service();
        service.activate_all(vec![Box::new(plugin)]);

        let id = CommandId::new("dev.tempr.demo::run");
        assert!(service.command(&id).is_some());
        assert_eq!(service.command_specs().len(), 1);
        assert!(
            service
                .command(&CommandId::new("dev.tempr.demo::other"))
                .is_none()
        );
    }

    #[test]
    fn a_failed_plugin_leaves_no_registrations_behind() {
        let mut broken = FakePlugin::new("dev.tempr.broken");
        broken.registers_driver = true;
        broken.fail = true;

        let service = service();
        service.activate_all(vec![Box::new(broken)]);

        assert!(service.engines().is_empty());
        assert!(service.command_specs().is_empty());
    }

    #[test]
    fn stop_deactivates_in_reverse_and_clears_registrations() {
        let base = FakePlugin::new("dev.tempr.base");
        let base_deactivated = base.deactivated.clone();
        let dependent = FakePlugin::new("dev.tempr.dependent");
        let dependent_deactivated = dependent.deactivated.clone();

        let service = service();
        service.activate_all(vec![Box::new(dependent), Box::new(base)]);
        assert_eq!(service.command_specs().len(), 2);

        service.deactivate_all();

        assert_eq!(base_deactivated.load(Ordering::SeqCst), 1);
        assert_eq!(dependent_deactivated.load(Ordering::SeqCst), 1);
        assert!(service.command_specs().is_empty());
        assert!(service.engines().is_empty());
    }

    #[test]
    fn activation_publishes_a_summary_event() {
        let bus = Arc::new(EventBus::new());
        let seen = Arc::new(AtomicUsize::new(0));
        let seen_clone = seen.clone();
        let _subscription = bus.subscribe(
            tempr_events::EventFilter::AnyOf(vec![tempr_events::AppEventKind::PluginsActivated]),
            move |event| {
                if let AppEvent::PluginsActivated { count, .. } = event {
                    seen_clone.store(*count, Ordering::SeqCst);
                }
            },
        );
        let commands = CommandService::new(bus.clone());
        let service = PluginService::new(bus, commands);
        service.activate_all(vec![Box::new(FakePlugin::new("dev.tempr.demo"))]);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn capabilities_are_advisory_only() {
        let mut plugin = FakePlugin::new("dev.tempr.demo");
        plugin.manifest.capabilities = &[
            Capability::CommandContribution,
            Capability::CompletionProvider,
        ];
        let service = service();
        let report = service.activate_all(vec![Box::new(plugin)]);
        assert!(report.all_activated());
    }
}
