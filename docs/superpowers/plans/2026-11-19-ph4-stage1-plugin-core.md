# Phase 4 Stage 1 — Plugin Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** the plugin API exists and is proven by real consumers — a `tempr_plugin` crate (manifest, lifecycle, registration context, extension traits, API version), a `PluginService` in `tempr_services` that activates static plugins and owns their registrations, completion fan-out into the ranked pipeline, plugin commands that reach both the palette and the keymap, `ResultRenderer` wiring in the grid, the PostgreSQL driver re-registered through the API, and an `export-csv` reference plugin written against `tempr_plugin` only.

**Architecture:** `tempr_plugin` is a leaf crate depending on `tempr_domain` + `tempr_events` + `tempr_db` + `gpui` — never on `tempr_services` or `tempr_ui`, so anything a plugin needs must be re-exported from it. `PluginService` (in `tempr_services`) owns activation and typed registration vectors; other services pull from it during construction (`IntelligenceService` for providers, `ResultGrid` for renderers, the dock later). Static plugins are declared in one list in the binary, `crates/tempr/src/plugins.rs`. Because GPUI dispatches by action *type* and the host cannot name a plugin's types, every plugin command travels through one payload action, `plugin::PluginCommand { id }`, bound in the keymap and looked up in the service at dispatch time.

**Tech Stack:** Rust 1.97.1, `gpui` at the pinned rev (D17), `thiserror`, `async-trait`, existing `CommandService`/`CommandContribution` (D23), `tempr_intel::completion`'s ranking (weight × 1000 + quality × 4 − label length), `tempr_db` driver traits.

**Spec:** [2026-11-18-phase4-extensibility-polish-design.md](../specs/2026-11-18-phase4-extensibility-polish-design.md) §3 (API surface), §4 (`PluginService`), §5 (fan-out), §7.4 (plugin commands), §7.5 (renderers), §10 (reference implementations).

## Global Constraints

- **Branch, PR, review — never commit to main.** Branch: `feat/ph4-plugin-core`.
- **Every task ends green**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`.
- **No new third-party dependencies.** `tempr_plugin` uses `gpui` (in the graph), `thiserror`, `async-trait`. No `schemars`, no `serde` in the action.
- **API version stays `0.1`** (D31). `tempr_plugin` is the only stable surface; nothing from `tempr_services`/`tempr_ui` may leak into it.
- **No service access for plugins.** `PluginContext` exposes registrations and the bus, nothing else.
- **Registration failures are typed, never panics.** One broken plugin must not stop the app (spec §4.4).
- **Canonical names**: `Plugin`, `PluginManifest`, `Capability`, `PluginContext`, `PluginService`, `PluginError`, `CompletionProvider`, `ResultRenderer`, `Panel`, `PluginCommandSpec`, `PluginCompletionRequest`, `PLUGIN_API_VERSION`.
- **No `unwrap`/`expect` in library code**; tests may use them.
- **`register_theme` and `ThemeData` are NOT in this stage** — they ship with the theme system in stage 2 so the API never carries a type with no consumer. The context grows then, which `0.x` allows.

---

## Task 0: API-surface probe (record findings, delete scratch)

Three facts decide the shape below; verify them before writing code, and record the answers in the branch's first commit message.

1. **`CompletionRequest` cannot move to `tempr_domain`.** It holds `&'a mut Buffer`; `tempr_domain` has no `tempr_editor` dependency and adding one would create `editor → domain → editor`. → move only `CompletionItem`/`CompletionKind`; give plugins their own `PluginCompletionRequest` (text-based, no buffer).
2. **A payload action can avoid `serde`/`schemars`.** GPUI's `#[derive(Action)]` requires `serde::Deserialize` + `schemars::JsonSchema` *unless* `#[action(no_json)]`; `no_json` only disables JSON `build`, which Tempr never uses (keybindings are installed in code, not from JSON). → `#[derive(Clone, PartialEq, Action)] #[action(namespace = plugin, no_json)] pub struct PluginCommand { pub id: SharedString }`.
3. **`PluginService` must be constructed last but activated before the window.** It needs `CommandService` (to register metadata), `IntelligenceService` (providers), and the `EventBus`; the UI needs its accessors at `MainWindow::new`. → construct in the binary after the other services, `register` it last, and have the binary pass accessors into `Services` (the UI already receives a `Services` struct).

- [ ] Probe, record, delete the scratch file, commit `docs(ph4): record stage 1 API-surface findings` (one paragraph in the commit body is enough).

---

## File Structure

| File | Responsibility in this stage |
|---|---|
| `crates/tempr_domain/src/completion.rs` | **New.** `CompletionItem`, `CompletionKind` (moved from intel), `PluginCompletionRequest` |
| `crates/tempr_intel/src/completion.rs` | Re-export the moved types; `complete_with` gains a plugin-fan-out seam |
| `crates/tempr_plugin/Cargo.toml`, `src/lib.rs` | **New crate.** Manifest, capability, `Plugin`, `PluginError`, `PLUGIN_API_VERSION` |
| `crates/tempr_plugin/src/context.rs` | `PluginContext` + registration collection |
| `crates/tempr_plugin/src/extension.rs` | `CompletionProvider`, `ResultRenderer`, `Panel`, `PluginCommandSpec` |
| `crates/tempr_plugin/src/command.rs` | `PluginCommand` action |
| `crates/tempr_plugin/src/reexport.rs` | `gpui` + driver-trait re-exports (the plugin's only window onto GPUI) |
| `crates/tempr_services/src/plugin.rs` | **New.** `PluginService`: ordering, activation, registries, accessors, deactivate |
| `crates/tempr_services/src/lib.rs` | Export `PluginService`; `Service` impl |
| `crates/tempr_ui/src/commands.rs` | `install` takes plugin commands; `dispatch` gains the plugin path |
| `crates/tempr_ui/src/components/result_grid.rs` | Renderer chain per column |
| `crates/tempr_ui/src/main_window.rs` | `PluginCommand` handler; forwards plugin specs |
| `crates/tempr_db_postgres/src/plugin.rs` | **New.** `PostgresPlugin` implementing `Plugin` |
| `plugins/export-csv/` | **New crate.** `tempr_plugin_export_csv` — `CommandContribution` reference implementation |
| `crates/tempr/src/plugins.rs`, `main.rs` | Static plugin list; driver resolution through the service |
| `Cargo.toml` | Workspace members + `tempr_plugin` dependency entry |
| `docs/08-plugin-api.md`, `docs/05-services.md`, `docs/12-sql-intelligence.md`, `docs/13-result-grid.md`, `docs/14-project-layout.md`, `docs/PROGRESS.md`, `docs/DECISIONS.md`, `docs/TODO.md` | Living docs per CLAUDE.md |

---

## Task 1: Move the completion model into the domain

- [ ] `crates/tempr_domain/src/completion.rs`:
  ```rust
  pub enum CompletionKind { Keyword, Table, Column, Schema, Alias, Cte, Function, Snippet, Plugin }
  pub struct CompletionItem { pub label: String, pub kind: CompletionKind, pub detail: String,
                              pub insert: String, pub replace: Range<usize>, pub score: i32 }
  pub struct PluginCompletionRequest<'a> {
      pub file: SqlFileId, pub text: &'a str, pub offset: usize,
      pub prefix: &'a str, pub explicit: bool, pub connection: Option<ConnectionId>,
  }
  impl PluginCompletionRequest<'_> { pub fn replace_range(&self) -> Range<usize>; }
  ```
- [ ] `tempr_intel`: delete the local definitions, `pub use tempr_domain::completion::{CompletionItem, CompletionKind};`, keep `CompletionRequest` and `CompletionContext` local (buffer-bound and engine-internal).
- [ ] `cargo test --workspace` (no behaviour change) → commit `refactor(intel): move completion items into the domain`.

## Task 2: `tempr_plugin` skeleton

- [ ] Workspace member + `tempr_plugin` dependency entry; crate deps `tempr_domain`, `tempr_events`, `tempr_db`, `gpui`, `thiserror`, `async-trait`.
- [ ] `lib.rs`:
  ```rust
  pub const PLUGIN_API_VERSION: (u16, u16) = (0, 1);
  pub struct PluginManifest { pub id: PluginId, pub name: &'static str, pub version: &'static str,
                              pub api_version: (u16, u16), pub requires: &'static [&'static str],
                              pub capabilities: &'static [Capability] }
  pub enum Capability { Driver { engine: &'static str }, CompletionProvider, Theme,
                        CommandContribution, PanelContribution, ResultRenderer }
  pub trait Plugin: Send + Sync { fn manifest(&self) -> &PluginManifest;
      fn activate(&mut self, cx: &mut PluginContext<'_>) -> Result<(), PluginError>;
      fn deactivate(&mut self) {} }
  ```
- [ ] `PluginError` (`thiserror`): `IncompatibleApiVersion { plugin, plugin_api, host_api }`, `DuplicateRegistration { kind, id }`, `ForeignNamespace { id, plugin }`, `MissingDependency { plugin, missing }`, `DependencyCycle { plugins }`, `ActivationFailed { plugin, message }`, `ActivationPanicked { plugin }`.
- [ ] `reexport.rs`: `pub use gpui;` plus `pub use gpui::{AnyElement, App, FocusHandle, SharedString, Window};` and `pub use tempr_db::{DatabaseDriver, DriverConnection, DriverError, EngineId};` — plugins never name another crate.
- [ ] Tests: manifest `api_version` acceptance/rejection helper; capability list is advisory (a duplicate capability entry is not an error); namespace check accepts `dev.tempr.x::cmd`, rejects `other::cmd`.
- [ ] Green → commit `feat(plugin): manifest, lifecycle and error model`.

## Task 3: Registration context and extension traits

- [ ] `context.rs` — collection-backed, drained by the host:
  ```rust
  pub struct PluginContext<'a> { /* plugin id, EventBus, pending registrations */ }
  impl PluginContext<'_> {
      pub fn plugin_id(&self) -> &PluginId;
      pub fn events(&self) -> &EventBus;
      pub fn register_driver(&mut self, d: Arc<dyn DatabaseDriver>) -> Result<(), PluginError>;
      pub fn register_completion_provider(&mut self, p: Arc<dyn CompletionProvider>) -> Result<(), PluginError>;
      pub fn register_result_renderer(&mut self, r: Arc<dyn ResultRenderer>) -> Result<(), PluginError>;
      pub fn register_command(&mut self, c: PluginCommandSpec) -> Result<(), PluginError>;
      pub fn register_panel(&mut self, p: Arc<dyn Panel>) -> Result<(), PluginError>;
  }
  ```
  Id derivation: driver → `engine()`; provider/panel/command → their own id; renderers are positional (no id). Every check that can fail runs **inside** the context so the plugin sees the error from its own `activate`.
- [ ] `extension.rs`:
  ```rust
  pub trait CompletionProvider: Send + Sync {
      fn complete(&self, req: &PluginCompletionRequest<'_>) -> Vec<CompletionItem>;
  }
  pub trait ResultRenderer: Send + Sync {
      fn can_render(&self, col: &ColumnSpec) -> bool;
      fn render_cell(&self, value: &Value, col: &ColumnSpec) -> AnyElement;
  }
  pub trait Panel: Send + Sync {
      fn panel_id(&self) -> &'static str;
      fn title(&self) -> SharedString;
      fn render(&mut self, window: &mut Window, cx: &mut App) -> AnyElement;
      fn focus_handle(&self, _cx: &App) -> Option<FocusHandle> { None }
  }
  pub struct PluginCommandSpec { pub id: CommandId, pub title: &'static str, pub category: &'static str,
                                 pub default_keystrokes: Vec<&'static str>,
                                 pub dispatch: Arc<dyn Fn(&mut Window, &mut App, &CommandContext) -> CommandOutcome + Send + Sync> }
  pub struct CommandContext { pub last_run: Option<Arc<QueryRun>>, pub workspace_root: Option<PathBuf> }
  pub enum CommandOutcome { Done(String), Failed(String) }
  ```
  `CommandContext` is how a plugin reaches data: the host fills it, the plugin never gets a service or the bus.
- [ ] `command.rs`: the payload action from Task 0 finding 2; `impl PluginCommand { pub fn new(id: impl Into<SharedString>) -> Self }`.
- [ ] Tests: duplicate driver engine rejected; duplicate command id rejected; foreign namespace rejected; two providers accepted; `PluginCommand` round-trips through `Action::name`/`boxed_clone`.
- [ ] Green → commit `feat(plugin): registration context and extension traits`.

## Task 4: `PluginService`

- [ ] `tempr_services/src/plugin.rs`:
  ```rust
  impl PluginService {
      pub fn new(events: Arc<EventBus>, command: Arc<CommandService>) -> Arc<Self>;
      pub fn activate_all(self: &Arc<Self>, plugins: Vec<Box<dyn Plugin>>) -> ActivationReport;
      pub fn driver(&self, engine: &EngineId) -> Option<Arc<dyn DatabaseDriver>>;
      pub fn completion_providers(&self) -> Vec<Arc<dyn CompletionProvider>>;
      pub fn result_renderers(&self) -> Vec<(PluginId, Arc<dyn ResultRenderer>)>;
      pub fn command(&self, id: &CommandId) -> Option<PluginCommandSpec>;
      pub fn command_specs(&self) -> Vec<PluginCommandSpec>;
      pub fn panels(&self) -> Vec<Arc<dyn Panel>>;
      pub fn activation_report(&self) -> ActivationReport;
  }
  ```
  `ActivationReport { activated: Vec<PluginId>, failed: Vec<(PluginId, String)>, active_version: (u16, u16) }`.
- [ ] Ordering: topological sort by `requires` (unknown → `MissingDependency`, cycle → `DependencyCycle`, self-reference is a cycle).
- [ ] `activate` wrapped in `catch_unwind`; failure discards that plugin's registrations and continues. On success, drain the context into the typed vectors, tag each with the owning `PluginId`, and register command metadata with `CommandService` (`CommandContribution` conversion at `category`, `default_keystrokes`, `context = Some("MainWindow && !Modal")`).
- [ ] `Service::stop` → `deactivate_all()` in reverse activation order, then clear the vectors.
- [ ] Publish `AppEvent::PluginsActivated { count, failed: Vec<PluginId> }` — add the variant to `tempr_events` (payload: counts only, no plugin types leak).
- [ ] Tests (fake plugins in the module): three plugins with a dependency chain activate in order; a failing plugin does not stop the others; a panicking plugin is reported and isolated; duplicate engine across two plugins fails the second; `driver()` resolves; `command()`/`command_specs()`; stop deactivates in reverse.
- [ ] Green → commit `feat(services): PluginService activates static plugins`.

## Task 5: Completion fan-out

- [ ] `IntelligenceService::complete` keeps its signature; internally, after the engine's list, append provider results:
  - build `PluginCompletionRequest { file, text: buffer text, offset, prefix: word under cursor, explicit, connection }` (the connection is the one the intelligence service was built with);
  - each provider's items are filtered to `label` matching the prefix through the existing `match_quality`, scored as `WEIGHT_PLUGIN_BASE (100) + priority.clamp(0, 300)` where `priority` is encoded by the provider in `item.score` (documented as a *weight hint*, 0..=300) — final `score = weight * 1000 + quality * 4 - label.len()`, so a plugin item can never outrank a schema object (weight 450) but can outrank keywords (100);
  - dedupe on `(label, kind)` against engine items, keeping the higher score; truncate to 200;
  - `Instant` timing: `tracing::warn!` above 1 ms, `error!` above 5 ms, naming the slowest provider when it is identifiable (wrap each provider call in its own timer).
- [ ] Tests: a provider contributes items; a provider's item is deduped against a catalog item of the same label+kind; 250 returned items are capped to 200; priority mapping is monotonic; an empty catalog still fans out (offline); a panicking provider is not caught (documented) — instead test that a provider returning zero items is a no-op.
- [ ] Green → commit `feat(intel): fan out plugin completion providers`.

## Task 6: Plugin commands

- [ ] `commands::install(cx, service, plugin_specs: &[PluginCommandSpec])`: register metadata (done by the service), then bind every default keystroke that parses to `PluginCommand { id }` under `"MainWindow && !Modal"`, honouring user overrides through `service.keystrokes_for(&spec.id)` exactly like core specs (invalid keystroke logs, empty override unbinds, fallback to defaults when nothing usable).
- [ ] `commands::dispatch`: when `find(id)` returns `None`, resolve through `PluginService::command(id)` — call the closure with `(window, cx, &context)`, set the status bar from the returned `CommandOutcome`, and `service.record_executed(id)`; return `false` when the id is unknown to both. The `CommandContext` is built by `MainWindow` (it owns the last-run handle) and passed in, so `dispatch` gains one parameter and stays free of service lookups.
- [ ] `MainWindow`: `.on_action` for `PluginCommand` → resolve id → call closure; a missing id logs once and sets the status bar to `Unknown plugin command`.
- [ ] Tests (`#[gpui::test]`): a registered plugin command appears in palette search; enter dispatches it (counter increments); its default keystroke dispatches it; an unknown id is a no-op with no `CommandExecuted` recorded.
- [ ] Green → commit `feat(ui): dispatch plugin commands from palette and keymap`.

## Task 7: `ResultRenderer` wiring

- [ ] `ResultGrid::new` takes `Vec<(PluginId, Arc<dyn ResultRenderer>)>` (default empty for tests); per result set, compute `column_renderer: Vec<Option<usize>>` once, first `can_render` wins.
- [ ] `render_row`: when a column has a renderer, call `render_cell(value, col)` and wrap as `AnyElement`; otherwise keep the existing `format_value` text path. Nulls stay on the text path even when a renderer matches (`Value::Null` is never handed to a plugin).
- [ ] Test: a test-only renderer that renders `int` values as `"<n>"` proves the chain selects it and that a `text` column falls back; a second renderer registered earlier wins.
- [ ] Green → commit `feat(ui): pluggable result-cell renderers`.

## Task 8: The PostgreSQL driver becomes a plugin

- [ ] `tempr_db_postgres/src/plugin.rs`:
  ```rust
  pub struct PostgresPlugin;
  impl Plugin for PostgresPlugin { /* manifest: id dev.tempr.postgres, capabilities [Driver { engine: "postgresql" }] */ }
  ```
  `activate` registers `Arc::new(PostgresDriver::new())`.
- [ ] `crates/tempr/src/plugins.rs`:
  ```rust
  pub fn static_plugins() -> Vec<Box<dyn Plugin>> { vec![Box::new(PostgresPlugin), Box::new(ExportCsvPlugin::default())] }
  ```
- [ ] `main.rs`: build `CommandService`, then `PluginService`, `activate_all(static_plugins())`, then resolve the driver with `plugin.driver(&EngineId("postgresql".into()))` before `ConnectionService::register_driver`. Remove the direct `PostgresDriver` import from the binary. Log the `ActivationReport`; if the driver is missing, fail startup with the report in the message (no silent no-driver state).
- [ ] Test: `static_plugins()` activates and yields exactly one `postgresql` driver (in the binary's test module, no DB needed).
- [ ] Green → commit `feat(postgres): register the driver through the plugin API`.

## Task 9: `export-csv` reference plugin

- [ ] `plugins/export-csv/` (`tempr_plugin_export_csv`), deps: `tempr_plugin` only. Write CSV by hand (`std::fs::File` + `writeln!`, RFC 4180 quoting in ~20 lines) — **no new dependency**.
- [ ] **How a plugin reads data without services.** `PluginCommandSpec::dispatch` receives a host-supplied context, so plugin logic stays in the plugin and data access is read-only:
  ```rust
  // tempr_plugin::extension
  pub struct CommandContext { pub last_run: Option<Arc<QueryRun>>, pub workspace_root: Option<PathBuf> }
  pub struct PluginCommandSpec { /* id, title, category, default_keystrokes, */
      pub dispatch: Arc<dyn Fn(&mut Window, &mut App, &CommandContext) + Send + Sync> }
  ```
  `MainWindow` owns the `Arc<Mutex<Option<QueryRun>>>` filled from the outcome task (`QueryService::completed_run(run)`), so the plugin never touches `QueryService`, `Storage`, or the connection. `QueryRun.result_set` is the domain's `ResultSet`.
- [ ] Command `dev.tempr.export-csv::export_last_run`, default `ctrl-alt-e`, category `Plugin`: writes columns + rows of `ctx.last_run.result_set` to `<workspace_root>/last-run.csv` (quote `"` by doubling, wrap fields containing `,`, `"`, `\r` or `\n`), then sets the status bar through a `CommandOutcome` returned to the host — the closure's return value, not a service call:
  ```rust
  pub type DispatchFn = dyn Fn(&mut Window, &mut App, &CommandContext) -> CommandOutcome + Send + Sync;
  pub enum CommandOutcome { Done(String), Failed(String) }   // → MainWindow::set_status
  ```
- [ ] Tests: RFC 4180 quoting (comma, quote, newline, CRLF), zero rows writes a header-only file, `last_run: None` produces `CommandOutcome::Failed("No completed query to export")` and writes nothing, `/` in the workspace root is created if missing.
- [ ] Green → commit `feat(plugins): add the CSV export reference plugin`.

## Task 10: Exit sweep

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check` all green; count tests before/after in the PROGRESS row.
- [ ] Manual (live PostgreSQL 16, headless X server as in the Phase 3 sweep): app starts with both plugins activated; palette lists `Export last run as CSV`; `ctrl-alt-e` writes `last-run.csv` and the status bar reports the row count; a query still returns rows through the grid with no renderer registered.
- [ ] Docs: `08-plugin-api.md` (shipped types, drop `AppContext`/`AiProvider`, mark `register_theme` as stage 2, `CommandContext`/`CommandOutcome`), the Stage 4 spec's §3.5/§7.4 (dispatch signature now carries the host-supplied context — a refinement of the approved design, recorded rather than silently diverged), `05-services.md` (`PluginService`), `12-sql-intelligence.md` (fan-out weights), `13-result-grid.md` (renderer chain), `14-project-layout.md` (`tempr_plugin` + `plugins/`), `PROGRESS.md` (stage row + decisions log), `DECISIONS.md` (entries for spec decisions 2, 3, 5, 6, 7, now that they shipped), `TODO.md` (anything discovered).
- [ ] Commit `docs(ph4): record stage 1` and open the PR against `main`.

---

## Exit criteria for this stage

- A plugin crate outside `crates/` (`plugins/export-csv`) registers a command and works with zero core edits beyond `crates/tempr/src/plugins.rs`.
- The PostgreSQL driver reaches `ConnectionService` only through `PluginService`.
- Completion providers fan out and are ranked by the one existing score.
- Plugin commands work from the palette and from their keybinding.
- `ResultRenderer` is wired and tested, with no shipped consumer (deliberate, spec §3.5).
- `PLUGIN_API_VERSION` is `(0, 1)` and enforced at activation.
