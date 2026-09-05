# Tempr — Backlog

> Ideas and small tasks that surface mid-build but don't belong to the
> current phase. Move items here immediately (living-docs rule in
> CLAUDE.md); promote them into a phase via docs/PROGRESS.md when picked
> up. Now = should land within the current/next phase · Next = upcoming
> phases · Later = pre-release · Ideas = unscoped.

## Now

- [ ] Lazy wire streaming via `query_raw()` — `PostgresStream` currently buffers full result set via `client.query()`, then yields `batch_size` chunks. True `query_raw` streaming was attempted but fails with `Error { kind: Closed }` specifically when called through `QueryService::execute()` (works fine via raw driver or closures). Root cause undetermined — suspected `Box<dyn DriverConnection>` + async trait boundary interaction. Investigate as follow-up.

- [ ] `DriverConnection::ping` (active round-trip) on top of the existing `is_closed` recycle check (09-database-engine: idle ping every 30 s, reconnect with backoff → `Reconnecting`/`Failed` states, evict a connection whose query failed with a transport error)
- [ ] Per-connection `pool_max_size` from the workspace connection config instead of the service-wide `PoolConfig`
- [ ] `ThemeProvider` tokens replace the placeholder palette consts in `tempr_ui::theme` (no hard-coded colors rule, 11-gpui.md)
- [ ] Profile the 10% of bench frames > 20 ms (p95 23 ms, max 59 ms at 100k rows, release): suspects are text shaping of ~60 fresh cells per frame and per-frame `format_value` allocations — try a shaped-line cache keyed by (row, col) or pre-formatting strings on append; add a per-frame histogram to `ScrollBench`
- [ ] `ScrollBench`: detect a throttled compositor (e.g. > 25% of frames ≥ 500 ms) and mark the report invalid instead of reporting fps
- [ ] Workspace open + connection picker in `MainWindow` (workspace connection list from `workspace.toml`, secrets via OS keychain — new dependency → DECISIONS entry) replaces the `DATABASE_URL` stand-in; surface a malformed `DATABASE_URL` in the UI instead of exiting before the window opens; error toasts driven by `AppEvent`
- [ ] `ResultGrid`: column virtualization + resize, copy-on-select, 2D keyboard navigation (11-gpui.md Table row); columnar `RowStore` with spill-to-disk (13-result-grid.md) once result sizes demand it
- [ ] `Input`: undo/redo, auto-resize; multi-line SQL input arrives with the Phase 2 editor (rope + tree-sitter)

## Next

- [ ] TLS extras: per-connection CA file (`sslrootcert`), client certificate/key, CRL; make `verify-ca` genuinely skip hostname checks only if a user asks for it (D20 treats it as `verify-full`)
- [ ] macOS/Windows CI runners for the gpui build (Linux-only today; transitive `zed-font-kit` git source on macOS must pass `cargo deny`)

## Later

- [ ] Parse strategy for pathological files: a 10 MB dump of ~180k one-line statements re-parses in ~150 ms (tree-sitter re-walks the flat `program` sibling list); options — parse on a background thread from a `Tree` clone, or viewport-scoped `set_included_ranges`; realistic files (2.5k statements) are at 1.6 ms already
- [ ] Inner-statement execution inside `BEGIN … END` blocks / transactions: `StatementRange` reports the whole block as one range (kind `Block`/`Transaction`); descend into children when the cursor is inside
- [ ] Palette: highlight matched characters (`CommandMatch::indices`) in titles; show "no keybinding" hint; remember last query per session
- [ ] Apply the workspace keybinding layer (`WorkspaceManifest::keybindings`) when workspace open lands (`CommandService::set_keybinding_layers([user, workspace])`); rebind live on settings change (`cx.clear_key_bindings()` + `commands::install`)
- [ ] Plugin commands need a GPUI action shape (a generic `PluginCommand { id }` action or per-plugin `actions!`) before `CommandContribution` from 08-plugin-api can register through `CommandService`
- [ ] `EditorView` (multi-line, `tempr_ui`): render visible lines from `Buffer` with `highlights`, gutter line numbers, cursors/selections; keyboard editing through the command catalog (motions, edit ops, undo/redo, clipboard); replaces the single-line `Input` for SQL; then "execute statement under cursor" (`Buffer::statement_at` → `QueryService`), refusing `StatementKind::Error`
- [ ] Editing ops follow-ups: indent/outdent, join lines, transpose, select word/line, add cursor above/below, word motions across line breaks for `prev_word_boundary` when the previous line is empty
- [ ] `EditHistory` bounds: cap depth and coalesce typing bursts (today every keystroke stores its removed/inserted text forever; a select-all + paste on a 10 MB file retains full copies)
- [ ] Phase 2: Tree-sitter PostgreSQL grammar integration + incremental parse
- [ ] Phase 2: Statement boundary detector ($$ delimiters, comments, string literals)
- [ ] Phase 2: Command palette (fuzzy search, all registered commands, configurable keybindings)
- [ ] Phase 2: Keyboard-only audit — every user-facing action listed with keybinding
- [ ] Phase 3: Catalog cache (schema metadata from PostgreSQL, local cache, incremental refresh)
- [ ] Phase 3: Completion provider (context-aware, < 5 ms, 10,000 objects)
- [ ] Phase 3: Semantic analyzer (column ref resolution, ambiguity detection)
- [ ] Phase 3: Real-time diagnostics (syntax via tree-sitter + semantic via analyzer)
- [ ] Phase 3: Hover type information
- [ ] Phase 4: Plugin API stabilization (versioned, documented)
- [ ] Phase 4: Theme system (light/dark, configurable accent)
- [ ] Phase 4: Query history panel
- [ ] Phase 4: Layout persistence across restarts
- [ ] Phase 4: Platform-native packaging (.deb, .rpm, AppImage, .dmg, MSI)
- [ ] Resolve Open Decisions #1 (name/branding), #3 (telemetry), #4 (beta timing) before Phase 4
- [ ] SSH tunnel connections (user request 2026-09-03; sketched in 09-database-engine.md "SSH tunnels"): `ConnectionConfig` gains optional `ssh_host`/`ssh_port`/`ssh_user`/`ssh_key_ref`; `ConnectionService`'s `DriverManager::create` establishes the tunnel (keepalive/reconnect owned by the service) and hands the tunneled stream to the driver, TLS composing inside it; needs a pure-Rust SSH client crate (`russh` candidate → DECISIONS entry), key/passphrase via the keychain path, and a per-bastion vs per-connection tunnel decision

## Ideas

- [ ] WASM-based plugin execution sandbox (post-v1; requires ABI layer on top of stable plugin API)
- [ ] AI-assisted query generation via LLM provider abstraction (post-v1; Phase 3 catalog cache provides schema context)
- [ ] Real-time collaboration / multi-cursor shared SQL editing (post-v1; different architecture concern)
- [ ] MySQL/SQLite driver (post-v1; driver abstraction from Phase 1 supports this — just implement `DatabaseDriver`)
- [ ] Theme marketplace / plugin registry (post-v1; depends on Phase 4 plugin system stability)
- [ ] Cross-platform dark/light mode sync with OS appearance setting
- [ ] Query explain/analyze visualization panel
