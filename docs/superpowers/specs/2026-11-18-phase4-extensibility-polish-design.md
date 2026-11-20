# Phase 4 — Extensibility & Polish: design

**Date**: 2026-11-18
**Status**: draft, awaiting approval
**Phase**: 4 (Extensibility & Polish) — all eight exit criteria in [16-roadmap.md](../../16-roadmap.md)
**Governing docs**: [08-plugin-api.md](../../08-plugin-api.md), [07-storage.md](../../07-storage.md), [04-workspace.md](../../04-workspace.md), [05-services.md](../../05-services.md), [11-gpui.md](../../11-gpui.md), [14-project-layout.md](../../14-project-layout.md)
**Locked decisions honoured**: D2 (Rust-only plugins), D5 (workspace-first), D6 (no business logic in the UI), D8 (registration-based plugins), D15 (branch → PR), D21–D24 (editor, commands), D31 (API stays `0.x` until explicit sign-off), D32 (MSI)

---

## 1. Scope

Covers all of Phase 4: the plugin crate and service, the theme system, the dock that hosts panels, query history, layout persistence, the reference and demo plugins, and packaging. Implemented as five sequenced pull requests (§14).

**In scope**

- A `tempr_plugin` crate: manifest, capability declarations, lifecycle trait, registration context, extension-point traits, API version.
- `PluginService`: static-plugin activation in dependency order, typed capability registries, failure isolation.
- Completion fan-out from plugin providers into the existing ranked pipeline.
- One generic plugin command action, so plugin commands reach the palette *and* the keymap without per-plugin action types.
- Theme system: `ThemeData` tokens, light + dark defaults, accent override, replacement of the placeholder consts.
- A right-side dock with tabs, keyboard-resizable, hosting the history panel and plugin panels.
- Query history in `.tempr/history.db` (SQLite) with a browsable panel and re-run.
- Layout persistence in `.tempr/layout.json` (dock state, focus, editor caret/scroll).
- Two in-tree reference implementations migrated onto the API (PostgreSQL driver, CSV export command) and one out-of-core demo plugin (panel + completion provider).
- Linux, macOS and Windows packaging.

**Out of scope**

- Dynamic plugin loading (dylib or WASM) — ADR-0008 and D31 defer it past v1.
- A plugin registry or marketplace (PRODUCT out-of-scope; open decision #6 stays post-v1).
- Plugin settings schema, capability permissions, hot reload (08's open questions 2, 3, 5).
- `AiProvider` — PRODUCT puts AI assistance out of scope for v1, so v1 publishes no trait for a feature that does not exist (§3.5).
- Cross-workspace history (07 open question 4).
- Multiple open files — there is still no file-open flow, so layout persists one buffer's state (§9).
- Any theme marketplace or downloadable theme.

---

## 2. Decisions taken in brainstorming

| # | Decision | Rationale |
|---|---|---|
| 1 | Five stages: plugin core → themes → dock + history → layout + demo plugin → packaging | Each stage leaves the app working; plugin core lands first so later stages build on the real API instead of a sketch |
| 2 | Plugins are static crates that depend on `tempr_plugin` only | ADR-0002/0008. `tempr_plugin` re-exports the GPUI types a panel needs, so no plugin ever names `tempr_ui` |
| 3 | Completion types (`CompletionRequest`, `CompletionItem`, `CompletionKind`) move to `tempr_domain` | Both `tempr_intel` and `tempr_plugin` need them; domain is the only crate they may both depend on without a cycle |
| 4 | Themes are **data**, not a trait: `register_theme(ThemeData)` | A static theme has no behaviour. A trait adds a vtable and a name method for nothing, and `ThemeData` is what the UI actually needs. Deviates from 08's `ThemeProvider` trait — recorded |
| 5 | One generic GPUI action `PluginCommand { id }` carries every plugin command | GPUI dispatches by action type; the host cannot name a plugin's types. One action with a payload serves the keymap and the palette, and avoids per-plugin `actions!` registration |
| 6 | Panels are a trait (`Panel`), not a closure returning an element | Panels hold state and need `&mut Window, &mut App` per frame; a closure cannot own state and a plugin cannot create a GPUI entity during `activate` (no window exists yet) |
| 7 | `ResultRenderer::render_cell` returns `AnyElement`, not `Box<dyn Element>` | GPUI's `Element` has associated types and is not object-safe; `AnyElement` is the sanctioned erased form |
| 8 | Query history lives in SQLite via `sqlx` (new dependency, staged) | 07-storage specifies it: append-heavy, indexed search, cheap pruning. A JSONL file would need its own index and rewrite-on-prune |
| 9 | History retention is a hard cap of the newest 10 000 entries, pruned at startup and after every 500 inserts | Deterministic and bounded; a time window makes storage depend on usage rate. 07's open question 2 (age-based pruning) stays deferred |
| 10 | Layout restores **after** the first frame, asynchronously | NFR says no synchronous I/O before first frame except the manifest; a post-first-frame apply costs a brief default layout instead of an exception to the NFR |
| 11 | Panel widths/states change only through commands (no drag handle required) | No-mouse-only rule: `layout::WidenDock`/`NarrowDock` must exist even if a drag handle ships later |
| 12 | Packaging is one stage, Linux first; macOS and Windows jobs are gated on runners that do not exist yet | MSI needs Windows, `.dmg` needs macOS (TODO "Now"); sequencing them after the OS-independent work keeps the phase shippable on Linux |
| 13 | The plugin API stays `0.x` for the whole phase, and after it | D31: `1.0` is an explicit user call, not an exit consequence |

Decisions 2, 3, 5, 6, 8 and 9 are MAJOR and get DECISIONS.md entries when their PR lands, not before.

---

## 3. Crate layout and the plugin surface

### 3.1 Crate graph

```
tempr_domain ──┬─ tempr_events
               ├─ tempr_db ── tempr_db_postgres ──┐
               ├─ tempr_workspace                │
               ├─ tempr_editor                   │
               ├─ tempr_intel                    │
               └─ tempr_plugin ◄─────────────────┘   (re-exports gpui, domain types)
                        ▲
       tempr_services ──┘        (PluginService consumes registrations)
                        ▲
              tempr_ui ─┘        (dock renders Panel trait objects)
                        ▲
                    tempr  (binary: declares the static plugin list)
```

`tempr_plugin` depends on: `tempr_domain`, `tempr_events`, `tempr_db` (driver traits), `gpui` (panels, commands, elements), `async-trait`, `thiserror`. It does **not** depend on `tempr_services`, `tempr_ui`, `tempr_workspace` or `tempr_intel`. That last rule is what makes the API honest: anything a plugin needs must be re-exported here, so the plugin author never reaches into core internals.

A plugin crate depends on `tempr_plugin` (+ whatever else it likes that is not a Tempr core crate).

### 3.2 Types that move

| Type | From | To | Why |
|---|---|---|---|
| `CompletionRequest` | `tempr_intel::completion` | `tempr_domain::completion` | Plugins produce items without depending on the engine |
| `CompletionItem` | `tempr_intel::completion` | `tempr_domain::completion` | same |
| `CompletionKind` | `tempr_intel::completion` | `tempr_domain::completion` | same; `Plugin` variant already exists |
| `ThemeData` | — | `tempr_plugin::theme` | Plugin-registered themes need it; `tempr_ui` consumes it |

`tempr_intel` re-exports all three so existing call sites keep compiling (`pub use tempr_domain::completion::*`).

### 3.3 Manifest, capability, plugin

```rust
pub struct PluginManifest {
    pub id: PluginId,                 // reverse-domain: "dev.tempr.postgres"
    pub name: &'static str,
    pub version: &'static str,        // the plugin's own semver
    pub api_version: (u16, u16),      // the API major.minor it was written against
    pub capabilities: &'static [Capability],
}

pub enum Capability {
    Driver { engine: &'static str },
    CompletionProvider,
    Theme,
    CommandContribution,
    PanelContribution,
    ResultRenderer,
}

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;
    fn activate(&mut self, cx: &mut PluginContext<'_>) -> Result<(), PluginError>;
    fn deactivate(&mut self) {}
}
```

`capabilities` stays advisory (08's rule): activation registers for real, and a manifest that declares a capability the plugin never registers is logged, not fatal. `id` is namespaced — a plugin cannot register under another plugin's prefix, and every registration is recorded with its owning plugin so deactivation can remove exactly its own.

### 3.4 Registration context

```rust
pub struct PluginContext<'a> {
    pub fn register_driver(&mut self, driver: Arc<dyn DatabaseDriver>) -> Result<(), PluginError>;
    pub fn register_completion_provider(&mut self, p: Arc<dyn CompletionProvider>) -> Result<(), PluginError>;
    pub fn register_result_renderer(&mut self, r: Arc<dyn ResultRenderer>) -> Result<(), PluginError>;
    pub fn register_theme(&mut self, t: ThemeData) -> Result<(), PluginError>;
    pub fn register_command(&mut self, c: PluginCommandSpec) -> Result<(), PluginError>;
    pub fn register_panel(&mut self, p: Arc<dyn Panel>) -> Result<(), PluginError>;
    pub fn events(&self) -> &EventBus;
    pub fn plugin_id(&self) -> &PluginId;
}
```

Every method returns `Result` so a duplicate is a compile-time-visible branch in the plugin, not a panic. The context exposes no service registry and no storage handle (08's "why plugins never get `ServiceRegistry` access"): a plugin that needs data subscribes to the bus.

### 3.5 Extension points

```rust
// Already exists; unchanged, re-exported so plugin crates need one dependency.
pub use tempr_db::{DatabaseDriver, DriverConnection};

pub trait CompletionProvider: Send + Sync {
    fn provider_name(&self) -> &'static str;
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
    fn focus_handle(&self, cx: &App) -> Option<FocusHandle> { None }
}

pub struct PluginCommandSpec {
    pub id: CommandId,                       // "plugin_id::command"
    pub title: &'static str,
    pub category: &'static str,              // palette grouping; "Plugin" by default
    pub default_keystrokes: Vec<&'static str>,
    pub dispatch: Arc<dyn Fn(&mut Window, &mut App, &CommandContext) -> CommandOutcome + Send + Sync>,
}

pub struct CommandContext {
    pub last_run: Option<Arc<QueryRun>>,     // host-built, read-only
    pub workspace_root: Option<PathBuf>,
}

pub enum CommandOutcome { Done(String), Failed(String) }
```

**As shipped (stage 1):** `CompletionProvider::complete` takes `PluginCompletionRequest` (buffer text, offset, prefix, explicit, connection) rather than the engine's `CompletionRequest`, which holds `&mut Buffer` and stays in `tempr_intel` (→ D34). `provider_name` exists so the host can name a slow provider in the latency log. `PluginCommandSpec::dispatch` receives a host-built `CommandContext` and returns a `CommandOutcome` instead of taking no arguments and returning nothing — a plugin never resolves a service, and the status bar gets one line of feedback (→ D35). The action is `PluginCommand { id }` declared `#[action(namespace = plugin, no_json)]`; `no_json` keeps `serde`/`schemars` out of the plugin crate because every binding is installed in code. Theme registration is **not** in the stage-1 surface; it arrives with `register_theme(ThemeData)` in stage 2.

`AiProvider` is deliberately absent: PRODUCT lists AI-assisted generation as out of scope for v1, and publishing a trait with no host implementation would be an API surface nobody can test. docs/08 loses it from the v1 list (§16).

`ResultRenderer` is wired but has no shipped consumer (§7.5): the demo plugin does not need one and the core grid already formats every decoded type. It is published because the wiring is ~20 lines and the trait's shape is the part that is hard to change later.

### 3.6 Versioning

`tempr_plugin` exports `pub const PLUGIN_API_VERSION: (u16, u16) = (0, 1);` and bumps the minor on every API change during Phase 4. Activation rejects a plugin whose `api_version.0` differs from the host's with `PluginError::IncompatibleApiVersion`. With static plugins this cannot fail in practice; the check exists so the dynamic-loading future starts from a contract, and so a stale vendored plugin fails loudly instead of subtly (→ D31).

---

## 4. `PluginService`

### 4.1 Lifecycle

`PluginService` lives in `tempr_services` alongside the others and is constructed **last** (05-services.md's order), before the window opens. It owns:

```rust
pub struct PluginService {
    plugins: Vec<PluginSlot>,                     // id, instance, state
    drivers: Vec<(PluginId, Arc<dyn DatabaseDriver>)>,
    completion_providers: Vec<(PluginId, Arc<dyn CompletionProvider>)>,
    result_renderers: Vec<(PluginId, Arc<dyn ResultRenderer>)>,
    themes: Vec<(PluginId, ThemeData)>,
    commands: Vec<(PluginId, PluginCommandSpec)>,
    panels: Vec<(PluginId, Arc<dyn Panel>)>,
    events: Arc<EventBus>,
}
```

Order: sort plugins topologically by a declared `requires: &'static [&'static str]` field on the manifest (dependency by plugin id; unknown id → `PluginError::MissingDependency`). Cycles and self-references are rejected the same way. Then `activate` each in order; on `Err`, mark `Failed`, log, drop that plugin's registrations, and continue — one broken plugin must not cost the user their editor.

`deactivate_all` runs from `stop_all` before `HistoryService::flush` and before the window closes, so a panel holding a repository handle is not dropped after its dependencies. After the last `deactivate`, all registries are cleared.

Events: publish `AppEvent::PluginsActivated { count, failed }` after the loop, and `AppEvent::PluginEvent { plugin_id, payload }` passthrough for plugin-published data. Nothing else in the app may depend on activation ordering.

### 4.2 Registration rules

| Registration | Duplicate policy | Why |
|---|---|---|
| Driver | `PluginError::DuplicateRegistration` | Two drivers for one engine name is a configuration bug with no sane winner |
| Completion provider | Allowed — many providers | Fan-out is the point |
| Result renderer | Allowed; registration order decides | First `can_render` win is predictable and documented |
| Theme | `DuplicateRegistration` on the same `name` | Theme selection is by name; two "tempr-dark" is a bug |
| Command | `DuplicateRegistration` on the `CommandId` | The palette keys on id; 08's leaning, now fixed |
| Panel | `DuplicateRegistration` on `panel_id` | The dock keys on id |

An id outside the plugin's own namespace is `PluginError::ForeignNamespace { id, plugin }`.

### 4.3 Accessors and consumer wiring

| Consumer | Call |
|---|---|
| `tempr` (binary) driver lookup | `PluginService::driver(engine)` |
| `intel::IntelligenceService` | `completion_providers()` each request |
| `ResultGrid` | `result_renderers()` when a result set arrives (cached per column) |
| Theme resolution | `themes()` at startup, before the window is created |
| `CommandService` + palette | `commands()` merged into search metadata; dispatch by id |
| Dock | `panels()` to build tab entries |

Nothing else. Panels are handed to the dock by the `MainWindow` at construction, not looked up per frame.

### 4.4 Failure isolation

- `activate` returning `Err`, or panicking: wrapped in `catch_unwind`, plugin marked `Failed`, registrations discarded, `PluginError::ActivationPanicked` logged. The app starts.
- `Panel::render`, `CompletionProvider::complete`, `ResultRenderer::render_cell` panics are **not** caught: GPUI's render path is not unwind-safe, and catching per frame would hide a real bug every 16 ms. Documented as a 0.x risk; dynamic loading is where sandboxing arrives.
- A plugin that returns more than `COMPLETION_PROVIDER_MAX_ITEMS` (200) items has the tail dropped, so one provider cannot unbalance ranking.

---

## 5. Completion fan-out

`IntelligenceService::complete` currently returns the engine's ranked list. It becomes:

1. Engine candidates (unchanged).
2. For each registered provider, in registration order: `complete(req)`, accumulate `CompletionItem`s tagged `CompletionKind::Plugin`.
3. Merge, dedupe by `(label, kind)` keeping the higher score, rank with the existing score (`context_weight * 1000 + match_quality * 4 - label len`), truncate to 200.
4. Measure the fan-out with `Instant`; log at `warn` above 1 ms and at `error` above 5 ms, with the slowest plugin's id. No provider is disabled automatically — the budget is asserted in the probe, not enforced by killing user code.

The request type gains a `connection: Option<ConnectionId>` field (already sketched in 08) and `visible_bindings: &[Binding]` is *not* exposed: bindings are engine internals, and a plugin that wants schema context can read the catalog through… nothing. Plugin completions are therefore text/snippet providers plus whatever the plugin itself caches from the event bus. That limit is intentional for v1: exposing the analysis tree would freeze the semantic engine's internal types into the public API at `0.1`.

---

## 6. Theme system

### 6.1 `ThemeData`

```rust
pub struct ThemeData {
    pub name: SharedString,
    pub appearance: Appearance,          // Light | Dark
    pub colors: ThemeColors,
    pub highlight: [[u32; 3]; HighlightKind::COUNT],  // fg for each capture, per theme
    pub fonts: ThemeFonts,
}

pub struct ThemeColors {
    pub surface: u32, pub surface_raised: u32, pub surface_input: u32,
    pub border: u32, pub text: u32, pub text_dim: u32,
    pub accent: u32, pub selection: u32,
    pub error: u32, pub warning: u32, pub success: u32,
    pub gutter: u32, pub current_line: u32,
}

pub struct ThemeFonts {
    pub ui_family: SharedString,
    pub ui_size: f32,
    pub mono_family: SharedString,
    pub mono_size: f32,
}
```

Colours are `0xRRGGBB` `u32`, the same representation `theme::rgb`-style helpers already take, so migration is mechanical. Syntax colours keep the exhaustive `HighlightKind` match: with a data-driven table, `highlight_color` becomes an array index and the compile-time exhaustiveness guard moves to `HighlightKind::COUNT` + a test that `COUNT` matches the last variant.

### 6.2 Defaults

Two themes ship in `tempr_ui::theme::builtin`: `tempr-dark` (today's Catppuccin-derived palette) and `tempr-light` (same structure, inverted surfaces, WCAG AA body contrast ≥ 4.5:1 on `surface` and `surface_raised`). Selection is by name; `tempr-dark` is the default.

### 6.3 Settings and resolution

```toml
[theme]
name = "tempr-dark"          # any registered theme name
accent = "#89b4fa"           # optional; overrides ThemeColors::accent only
```

Resolution runs once, after plugin activation and before the window is created: builtin defaults → plugin themes → settings. Unknown name or malformed hex: log, show once in the status bar on first paint, fall back to `tempr-dark`. Same policy as keybindings (never lock the user out of a working UI).

Live switching is a command (`theme::CycleTheme`, ctrl-alt-t) rather than a settings reload: it walks registered themes in name order, applies the data to the global, and writes the choice back through the settings layer so the next launch keeps it.

### 6.4 Migration off the const palette

`tempr_ui::theme` keeps `pub fn theme(cx: &App) -> &Theme` and the module-level consts are deleted. The 64 `theme::` call sites (editor 22, main window 18, result grid 10, palette 8, input 6) change to read the resolved struct at `render` time. Components must not take a `&Theme` parameter — that would thread a lifetime through every component for no gain; a GPUI global (`cx.global::<Theme>()`) is the seam, set before the first frame and cloned per render.

---

## 7. Dock, panels, commands

### 7.1 Model

The window becomes: header bar → editor (resizable height) → grid (fill) → **right dock** (tabs, resizable width) → status bar. Bar heights and dock width are `DockState`:

```rust
pub struct DockState {
    pub visible: bool,
    pub width_px: f32,          // clamped 200..=640, default 320
    pub active_tab: Option<SharedString>,
    pub tabs: Vec<DockTab>,     // id + title, from the panel registry
}
```

The dock is a sibling of the grid (`flex_row` around the editor/grid column and the dock). Panels render top to bottom inside it: tab strip, panel body (`AnyElement`), and nothing else — no per-panel chrome, so every panel looks the same by construction.

### 7.2 Tab set

Built at startup from `PluginService::panels()` plus built-in panels (`History`). A plugin panel registers; it does not choose its position. Duplicate ids are rejected at registration (§4.2), so the tab strip cannot flicker.

### 7.3 Keyboard model

New commands (all in the catalog, all with default keystrokes, per the no-mouse-only rule):

| Command | Default | Context |
|---|---|---|
| `layout::ToggleDock` | ctrl-alt-b | `MainWindow && !Modal` |
| `layout::FocusNextPanel` | ctrl-alt-right | `MainWindow && !Modal` |
| `layout::WidenDock` | ctrl-alt-= | `MainWindow && !Modal` |
| `layout::NarrowDock` | ctrl-alt-- | `MainWindow && !Modal` |
| `layout::CyclePanel` | ctrl-alt-tab | `MainWindow && !Modal` |
| `history::Focus` | ctrl-alt-h | `MainWindow && !Modal` |
| `history::Rerun` | enter | `History Panel` |
| `history::Delete` | delete | `History Panel` |
| `theme::CycleTheme` | ctrl-alt-t | `MainWindow && !Modal` |

`ctrl-alt-tab` collides with some window managers; `CyclePanel` also ships on its own context-free alias if the audit finds the collision in practice. The palette exposes every command, and `every_command_has_a_default_keystroke` keeps failing if any of them lacks one.

### 7.4 Plugin commands

Registered `PluginCommandSpec`s enter `CommandService` as ordinary metadata (id, title, category, default keystrokes) so palette search finds them with no special case. Dispatch:

- Keybinding: `commands::install` binds every plugin command's keystrokes to `PluginCommand { id }` in `MainWindow && !Modal`. `MainWindow` handles `PluginCommand` by looking the id up in `PluginService::command(id)` and calling its closure. One action, one handler, no per-plugin compile-time registration. The context string is `tempr_plugin::COMMAND_CONTEXT`, the single source both the installer and the test read.
- Palette: the palette dispatches `PluginCommand { id }` too (the core and plugin paths share one action), then `MainWindow::dispatch_plugin` resolves the spec, builds `CommandContext` from the last completed run and the workspace root, calls the closure and puts the `CommandOutcome` in the status bar. `CommandExecuted` is recorded as for any other command.

**As shipped (stage 1):** the plugin binding uses `ctrl-alt-e` for `dev.tempr.export-csv::export_last_run`; the action lives in `tempr_plugin` (not `tempr_ui`) so a plugin crate can name it without depending on the UI crate.

### 7.5 Result grid wiring

`ResultGrid` holds a `Vec<(PluginId, Arc<dyn ResultRenderer>)>` handed in at construction. On result arrival it computes, per column, the first renderer whose `can_render` is true, and stores `Option<usize>` per column. Cells without one keep the existing text path. No shipped plugin uses this in Phase 4; the wiring is unit-tested with a test-only renderer (§13).

---

## 8. Query history

### 8.1 Storage

`.tempr/history.db`, SQLite via `sqlx` (`sqlite`, `runtime-tokio`, `migrate` off — plain `query` calls, no macro compile-time DB access):

```sql
CREATE TABLE IF NOT EXISTS history (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id        TEXT NOT NULL,
    connection_id TEXT NOT NULL,
    sql           TEXT NOT NULL,
    started_ms    INTEGER NOT NULL,     -- unix epoch millis
    duration_ms   INTEGER NOT NULL,
    rows          INTEGER,              -- NULL when the run failed or was cancelled
    outcome       TEXT NOT NULL,        -- 'ok' | 'error' | 'cancelled'
    error         TEXT
);
CREATE INDEX IF NOT EXISTS history_started ON history (started_ms DESC);
```

SQL text is stored verbatim, connection by id (never the URL), errors as text. WAL mode on; one connection pool of 1 (append-heavy, single writer). The database lives in the workspace, so `.tempr/` stays out of the user's manifest by construction (07-storage's security invariant).

### 8.2 `HistoryService`

Subscribes to `QueryFinished` (which already carries run id, sql, outcome, rows, duration — the view ignores it today but the bus publishes it). Writes on a tokio task; the UI never awaits a write. Interface:

```rust
impl HistoryService {
    pub async fn record(&self, r: &QueryRun) -> Result<(), HistoryError>;
    pub async fn recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, HistoryError>;
    pub async fn search(&self, q: &str, limit: u32) -> Result<Vec<HistoryEntry>, HistoryError>;
    pub async fn delete(&self, id: HistoryEntryId) -> Result<(), HistoryError>;
    pub async fn clear(&self) -> Result<(), HistoryError>;
    pub async fn flush(&self) -> Result<(), HistoryError>;
}
```

The panel queries through `recent`/`search` on focus and after each insert (debounced 200 ms) — it does not keep a full mirror in memory. `HistoryEntry` already exists in `tempr_domain`; it gains `duration_ms` and `error`, and its `QueryRunId` field is joined by the persisted `run_id` so "re-run" can reuse the recorded SQL without re-reading the file.

### 8.3 Retention

Prune to the newest 10 000 rows at startup and after every 500 inserts, in one `DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY started_ms DESC LIMIT 10000)`. Configurable later; the constant lives in `HistoryService`, not in the manifest.

### 8.4 Panel

`HistoryPanel` in `tempr_ui::components` (not a plugin — it is core UI that uses the service directly, like the result grid). List rows: relative time, outcome badge, duration, row count, first line of SQL. Selection drives a preview pane below the list showing the full SQL. Enter re-runs the selected entry into the grid through `MainWindow::run_sql`; delete removes the row; ctrl-c copies the SQL to the clipboard. Empty state shows "No queries yet — ctrl-enter runs the statement under the cursor."

Panel state (filter text, selection index, scroll) lives in the entity; nothing persists across restarts except the entries themselves (layout persistence covers the panel being open, §9).

---

## 9. Layout persistence

`.tempr/layout.json`, versioned:

```json
{
  "version": 1,
  "dock": { "visible": true, "width_px": 320, "active_tab": "History" },
  "editor": { "height_px": 260, "caret": 42, "scroll_top_line": 0 },
  "focus": "Editor"
}
```

`LayoutService` owns the file: `load()` returns `LayoutState::default()` on missing/corrupt/unknown version (derived data — never an error dialog), `save(&LayoutState)` debounced 500 ms with a trailing flush on quit and on `on_app_quit`. One atomic write (temp file with a UUID suffix, then rename) — the same fix TODO asks for in `save_manifest`, applied here first.

Startup: the window opens with defaults, `LayoutService::load` resolves on tokio, and the `MainWindow` applies the state when it lands (single `LayoutLoaded` bus event). The apply is skipped if the user has already moved the caret (a `layout_dirty` flag set by the first input event), so a fast typist is never yanked back to a restored caret. Nothing in the layout path is synchronous before the first frame, so no NFR exception is needed.

Explicitly **not** persisted yet: open files (none exist), per-file carets, grid column widths, palette state. The `LayoutState` shape is `#[serde(default)]` throughout so adding them later is not a version bump.

---

## 10. Reference implementations and the demo plugin

| Crate | Location | Proves |
|---|---|---|
| `tempr_db_postgres` | existing core crate, now activated as a plugin | `register_driver`; the binary stops hard-coding `PostgresDriver` and resolves `"postgresql"` through `PluginService` |
| `tempr_plugin_export_csv` | `plugins/export-csv/` | `register_command` + a typed `CommandContribution` closure that writes the last completed run to a workspace CSV and reports via the status bar |
| `tempr_demo_plugin` | `plugins/demo/` | `register_panel` + `register_completion_provider`, depending on `tempr_plugin` only |

The demo plugin is the exit-criterion demo and lives outside the core crates: its `Cargo.toml` lists `tempr_plugin` and nothing from `crates/`. It adds a "Demo" dock tab showing the host API version, the plugin's manifest, and a live count of completion requests it has served; its provider offers three snippet items (`demo_row_count`, `demo_now`, `demo_pg_version`) ranked as `Plugin` kind. Installing it is a build-time step — add the path dependency and one entry in `crates/tempr/src/plugins.rs`:

```rust
pub fn static_plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(tempr_db_postgres::PostgresPlugin::default()),
        Box::new(tempr_plugin_export_csv::ExportCsvPlugin::default()),
        Box::new(tempr_demo_plugin::DemoPlugin::default()),
    ]
}
```

The README documents that v1 "installation" means recompiling with the crate linked, which is the honest consequence of ADR-0002's native-only rule. No dynamic loading is added.

---

## 11. Packaging

Per-platform, one CI job each; all produce artifacts under `target/dist/`.

| Target | Tool | Notes |
|---|---|---|
| `.deb` | `cargo-deb` | Depends on `libxkbcommon`, `libwayland-client`, `libvulkan1`; `Recommends: fonts-inter` |
| `.rpm` | `cargo-generate-rpm` | Same dependency set |
| AppImage | `linuxdeploy` + `appimagetool` | Bundles the binary + `.desktop` + icon; fails the job if the AppImage cannot launch with `--help` (no display in CI) |
| `.dmg` | `hdiutil`/`create-dmg` | Needs a macOS runner — not present today |
| `.msi` | `cargo-wix` (WiX 3) | Needs a Windows runner — not present today (D32) |

Shared inputs: `packaging/tempr.desktop`, `packaging/icons/tempr-{16,32,48,128,256}.png`, `packaging/wix/main.wxs`, and metadata (product name, manufacturer, upgrade code) resolved from one `packaging/metadata.toml` so branding changes land in one file. Icons and product name depend on PRODUCT open decision #1 — until it is answered the jobs run with the working name and the criterion stays unchecked (§17).

CI: a `package` job per OS, `workflow_dispatch`-able and on tags. Linux jobs run on the existing runner; macOS/Windows jobs are written but `if: false` until runners exist, so the workflow is reviewable and the gaps are visible rather than implied.

---

## 12. Error handling

| Failure | Behaviour |
|---|---|
| Plugin `activate` error/panic | Log, mark `Failed`, discard registrations, app continues (§4.4) |
| Duplicate or foreign-namespace registration | Typed `PluginError`, plugin marked `Failed` (a partial plugin is worse than none) |
| API major mismatch | `PluginError::IncompatibleApiVersion`, plugin marked `Failed` |
| Unknown theme name / bad accent hex | Fall back to `tempr-dark`, one status-bar notice |
| Missing/corrupt `layout.json` | Defaults, silently (derived data) |
| History DB unopenable | Log once, status-bar notice, history panel shows "History unavailable"; queries still run |
| History write fails | Log; never block or fail a query |
| Packaging metadata missing branding | Job fails loudly with the missing key |

---

## 13. Testing

**Unit (no GPUI, no DB)**

- `tempr_plugin`: manifest/namespace validation, duplicate registration policy, API-compatibility check, topological ordering (deps, missing dep, cycle, self-reference).
- Completion fan-out: provider ordering, dedupe, rank merge, 200-item cap per provider and overall, `Plugin` kind survives ranking.
- Theme: both builtin themes parse into `ThemeData`, `HighlightKind::COUNT` matches the variant count, accent override applies to `accent` only, unknown name falls back, contrast assertions for `tempr-light` (≥ 4.5:1).
- `LayoutState`: round-trip, unknown version → default, missing fields → default, debounce coalesces bursts.
- `HistoryService` against a temp-dir SQLite: record/recent/search/delete/clear, prune at 10 001 entries, failure path leaves the DB usable.

**`#[gpui::test]` (deterministic harness, per 11-gpui.md)**

- Dock: `layout::ToggleDock` hides/shows; `WidenDock`/`NarrowDock` clamp at 200/640; tab cycling wraps; focus lands in the panel and returns.
- Plugin command: a test-registered `PluginCommandSpec` appears in palette search, dispatches from the palette, and fires from its keystroke via the generic action.
- Plugin panel: a test panel renders its title in the tab strip and its body when active.
- History panel: rows render from a fake service, enter re-runs through a mock driver (reusing `main_window.rs`'s test mock), delete removes, empty state renders.
- Theme switching: `CycleTheme` changes `cx.global::<Theme>()` and persists the choice.
- Layout restore: `LayoutLoaded` applies dock width and tab; a caret-typed-first session ignores the restored caret.

**Release probe**

- 10 000-object catalog with one builtin provider + 3 plugin providers: fan-out p99 stays under the 5 ms budget, asserted (extending the existing Phase 3 probe).

**Manual (live PostgreSQL 16)**

- Demo plugin panel visible, provider completions appear, CSV export writes a file, theme switch repaints everything with no leftover placeholder colour, history survives a restart, dock width/caret restored after restart.

**Packaging smoke**

- `.deb` installs in a container and `tempr --version` runs; AppImage runs `--help`; artifact metadata lists the expected deps. MSI/dmg verified when runners exist.

---

## 14. Implementation stages

| Stage | PR | Contents | Exit check |
|---|---|---|---|
| 1 | `feat/ph4-plugin-core` | `tempr_plugin` crate, domain move (§3.2), `PluginService`, completion fan-out, plugin commands, `ResultRenderer` wiring, Postgres driver as a plugin, `export-csv` reference plugin | App runs with all three plugin crates linked; palette + keymap dispatch a plugin command; fan-out probe |
| 2 | `feat/ph4-themes` | `ThemeData`, builtins, accent setting, `cx.global::<Theme>`, const migration, `theme::CycleTheme` | Light/dark switch repaints every component; no `theme::` consts left |
| 3 | `feat/ph4-dock-history` | Dock + tabs + commands, `sqlx` history store, `HistoryService`, history panel, `history.*` commands | History survives restart; panel re-runs a query |
| 4 | `feat/ph4-layout-demo` | `LayoutService`, `layout.json`, restore-after-first-frame, demo plugin crate, README install recipe | Demo plugin panel + completions work with no core edit; dock/caret restored |
| 5 | `chore/ph4-packaging` | Packaging assets, metadata, CI jobs, smoke checks | `.deb`/`.rpm`/AppImage artifacts; wiring in place for `.dmg`/`.msi` |

Each stage lands with its own PROGRESS row; the phase's exit sweep follows stage 5 as in Phase 3.

---

## 15. Resolved questions and deferred work

**Resolved here**

- Plugin API stability: `0.x`, explicit user sign-off for `1.0` (D31).
- Windows artifact: MSI (D32).
- Themes are data (decision 4); panels are traits (decision 6); commands are one generic action (decision 5).
- History retention: newest 10 000 (decision 9) — supersedes 07's open question 2 for v1.
- Layout restoration is asynchronous, after the first frame (decision 10) — resolves the NFR conflict noted in the Phase 4 review without an exception.

**Deferred (recorded in TODO.md)**

- Dynamic loading (dylib/WASM), plugin registry, settings schema, capability permissions, hot reload.
- `AiProvider` and everything AI (post-v1 by PRODUCT).
- Cross-workspace history; age-based retention; history search ranking.
- Dock drag handles, split panes inside the dock, floating panels.
- Multiple open files, per-file layout, grid column widths in layout.
- macOS/Windows CI runners (blocking their packaging criteria).
- Plugin-contributed keymap contexts beyond `MainWindow`; per-plugin settings UI.

---

## 16. Documentation impact

| Doc | Change |
|---|---|
| 08-plugin-api.md | Replace stale sketches with the shipped types: real `DatabaseDriver` signature, `PluginCommandSpec`, `Panel` trait, `AnyElement`, drop `AppContext`/`AiProvider`, add the versioning section (already landed) |
| 07-storage.md | `history.db` schema + retention; layout semantics (async restore, no version error dialog); `sqlx` dependency |
| 04-workspace.md | Layout no longer restored before the workspace is interactive |
| 05-services.md | `PluginService`/`HistoryService`/`LayoutService` contracts as built; construction/deactivation order |
| 11-gpui.md | Theming via `cx.global::<Theme>()`; dock + panel conventions; plugin command action |
| 12-sql-intelligence.md | Fan-out is real: provider ordering, cap, budget measurement |
| 13-result-grid.md | `ResultRenderer` chain in the grid |
| 14-project-layout.md | `tempr_plugin` + `plugins/` crates; the "depends on domain only" rule |
| 15-coding-standards.md | Plugin crates follow the same standards; API-surface review checklist |
| 16-roadmap.md | Phase 4 criteria reflect what shipped |
| PRODUCT.md | Phase 4 markers, AC evidence, MSI (already landed) |
| PROGRESS/DECISIONS/TODO | Per stage, as always |

---

## 17. Open items blocking stages

| # | Item | Blocks |
|---|---|---|
| 1 | PRODUCT open decision #1 (name/branding): MSI product name, manufacturer, upgrade code, `.desktop` name, icon set | Stage 5 only — stages 1–4 are branding-independent |
| 2 | PRODUCT open decision #3 (telemetry): PRODUCT says it blocks Phase 4. v1 ships no telemetry, so the phase can proceed; the opt-in policy text and toggle still need a decision before any beta | Phase 4 exit sign-off, not implementation |
| 3 | PRODUCT open decision #6 (plugin distribution): D31 records that it does not block static plugins and stays post-v1 | Nothing in this phase |
| 4 | macOS/Windows runners | Stage 5's `.dmg`/`.msi` criteria |
| 5 | GPUI on macOS/Windows is unverified (Phase 1 shipped Linux only) | The v1 cross-platform criterion, independent of packaging |

---

## Appendix — doc-vs-code corrections folded into this design

| 08-plugin-api.md says | Reality |
|---|---|
| `DatabaseDriver::engine_name() -> &'static str`, `connect(&ConnectionConfig)` | `engine() -> EngineId`, `connect(&Connection) -> Box<dyn DriverConnection>` |
| `CommandContribution { id, title, keybinding, handler }` and a `Command { id, title, keybinding }` | Real metadata type is `CommandContribution { id, title, category, context, default_keystrokes, hidden }`; dispatch needs `PluginCommandSpec` |
| `PanelContribution { render: Box<dyn Fn() -> Box<dyn Element>> }` | `Panel` trait; `AnyElement` instead of `Box<dyn Element>` |
| `ResultRenderer::render_cell -> Box<dyn Element>` | `-> AnyElement` (GPUI's `Element` is not object-safe) |
| Handlers receive `&mut AppContext` | GPUI `&mut Window, &mut App` |
| `ThemeProvider` trait + undefined `ThemeData` | `ThemeData` value, §6.1 |
| `PluginService::drivers()`-style accessors used by every service | Services consume capabilities during activation; only the table in §4.3 is wired |
| `AiProvider` is a v1 extension point | Not published in v1 (PRODUCT scope) |
