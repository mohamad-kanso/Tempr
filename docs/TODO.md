# Tempr — Backlog

> Ideas and small tasks that surface mid-build but don't belong to the
> current phase. Move items here immediately (living-docs rule in
> CLAUDE.md); promote them into a phase via docs/PROGRESS.md when picked
> up. Now = should land within the current/next phase · Next = upcoming
> phases · Later = pre-release · Ideas = unscoped.

## Now

- [ ] Lazy wire streaming via `query_raw()` — `PostgresStream` currently buffers full result set via `client.query()`, then yields `batch_size` chunks. True `query_raw` streaming was attempted but fails with `Error { kind: Closed }` specifically when called through `QueryService::execute()` (works fine via raw driver or closures). Root cause undetermined — suspected `Box<dyn DriverConnection>` + async trait boundary interaction. Investigate as follow-up.

- [ ] `DriverConnection::ping` (active round-trip) on top of the existing `is_closed` recycle check (09-database-engine: idle ping every 30 s, reconnect with backoff → `Reconnecting`/`Failed` states, evict a connection whose query failed with a transport error)
- [ ] The `schema_fingerprints` sweep does not filter by `has_function_privilege` and does not exclude extension-owned objects, consistent with the other catalog queries in `snapshot_schema` — revisit if a catalog gets noisy with objects the user cannot use or did not create
- [ ] Per-connection `pool_max_size` from the workspace connection config instead of the service-wide `PoolConfig`
- [ ] `ThemeProvider` tokens replace the placeholder palette consts in `tempr_ui::theme` (no hard-coded colors rule, 11-gpui.md)
- [ ] Profile the 10% of bench frames > 20 ms (p95 23 ms, max 59 ms at 100k rows, release): suspects are text shaping of ~60 fresh cells per frame and per-frame `format_value` allocations — try a shaped-line cache keyed by (row, col) or pre-formatting strings on append; add a per-frame histogram to `ScrollBench`
- [ ] `ScrollBench`: detect a throttled compositor (e.g. > 25% of frames ≥ 500 ms) and mark the report invalid instead of reporting fps
- [ ] Workspace open + connection picker in `MainWindow` (workspace connection list from `workspace.toml`, secrets via OS keychain — new dependency → DECISIONS entry) replaces the `DATABASE_URL` stand-in and the `TEMPR_WORKSPACE`/cwd manifest lookup from D24 (the keybinding layering itself stays); surface a malformed `DATABASE_URL` in the UI instead of exiting before the window opens; error toasts driven by `AppEvent`
- [ ] `ResultGrid`: column virtualization + resize, copy-on-select, 2D keyboard navigation (11-gpui.md Table row); columnar `RowStore` with spill-to-disk (13-result-grid.md) once result sizes demand it
- [ ] `Input`: undo/redo, auto-resize; multi-line SQL input arrives with the Phase 2 editor (rope + tree-sitter)
- [ ] Catalog cache eviction: `max_cache_size` setting + LRU by last access (07-storage.md's own follow-up from the `.tcat` cache landing in Phase 3 stage 2) — today a `.tempr/cache/catalog/` entry is never removed once written
- [ ] Per-connection catalog scope setting: `SchemaService::refresh`/`refresh_incremental` now always scope to `SchemaScope::SearchPath` (Phase 3 stage 2); a manual "refresh all schemas" action (`SchemaScope::All`) as the explicit escape hatch, and a per-connection default sticking with `SearchPath` vs `All`, should live on the connection config
- [ ] Today an added object always forces `refresh_incremental` into a full refresh (`RefreshPath::FullRefresh(FullRefreshReason::UnknownObject)`), because the fingerprint sweep reports only `(kind, oid, xmin)` — no schema name — so a brand-new key can't be scoped to a targeted re-read. Extending the sweep to report each object's schema would let additions take the incremental path too, the same way changes and drops already do
- [ ] `Storage::save_manifest` derives its temp file name from the destination path (`workspace.toml.tmp`, fixed) rather than a per-call unique name, so two concurrent saves race the same way the catalog cache did before `FileCatalogCache::save` was fixed to use a `uuid`-suffixed temp name — apply the same fix here
- [ ] Domain `SchemaObject`s carry no `native_id` field, so `SchemaService`'s incremental-refresh diff has to reverse-derive `SchemaObjectId`s from a dropped relation's fingerprint and try both `SchemaObjectKind::Table` and `::View` (a `pg_class` sweep can't say which it was) — carrying the native id on `SchemaObject` would remove that ambiguity entirely
- [ ] `cargo deny` carries an ignore for RUSTSEC-2025-0141 (bincode unmaintained, D27) — revisit at the catalog load probe (07-storage.md OD#1), with `postcard` as the candidate replacement
- [ ] `refresh_incremental` copies the cached `keywords` verbatim into the spliced snapshot and never refreshes them — risk: a server upgraded in place (new reserved words) keeps serving the old keyword list from cache until a full refresh happens to run
- [ ] The write-skip dedup in `SchemaService::save_cached` hashes the object `Vec` in the order the catalog queries return it, but those queries carry no `ORDER BY` and a splice's output order (cached objects minus dropped/touched, then refreshed ones appended) differs structurally from a full refresh's — risk: two content-identical refreshes can hash differently and rewrite the cache file anyway. `save_cached_skips_rewrite_when_content_is_unchanged` and `identical_snapshots_hash_identically` both hash clones of one already-built `Vec`, so neither test would catch a reordering-induced hash mismatch
- [ ] `save_cached` reads and fully decodes the existing `.tcat` file just to compare content hashes before deciding whether to skip the write — storing the content hash in the file's header would turn the skip check into a 16-byte read instead (needs a `CATALOG_FORMAT_VERSION` bump)
- [ ] `SchemaService` has no per-connection concurrency guard on `refresh`/`refresh_incremental` — risk: two overlapping refreshes for the same connection can each read the same stale cached snapshot, splice/refresh independently, and clobber each other's write, with both publishing `AppEvent::SchemaRefreshed`. No production caller triggers this today, but stage 6 (per docs/superpowers/specs/2026-09-07-phase3-sql-intelligence-design.md §9) wires `schema::Refresh` and `schema::RefreshIncremental` to keybindings, which lets a user double-press one and create exactly this race
- [ ] `refresh_incremental` returns `(Arc<SchemaSnapshot>, RefreshPath)` — a path label, not the delta itself. Stage 3's in-memory `tempr_intel` catalog will want to know which ids to invalidate, not just that a splice happened; rebuilding the whole in-memory catalog from a spliced snapshot on every refresh discards the exact reason the splice exists (avoiding a full rebuild)

## Next

- [ ] TLS extras: per-connection CA file (`sslrootcert`), client certificate/key, CRL; make `verify-ca` genuinely skip hostname checks only if a user asks for it (D20 treats it as `verify-full`)
- [ ] macOS/Windows CI runners for the gpui build (Linux-only today; transitive `zed-font-kit` git source on macOS must pass `cargo deny`)

## Later

- [ ] Parse strategy for pathological files: a 10 MB dump of ~180k one-line statements re-parses in ~150 ms (tree-sitter re-walks the flat `program` sibling list); options — parse on a background thread from a `Tree` clone, or viewport-scoped `set_included_ranges`; realistic files (2.5k statements) are at 1.6 ms already
- [ ] Inner-statement execution inside `BEGIN … END` blocks / transactions: `StatementRange` reports the whole block as one range (kind `Block`/`Transaction`); descend into children when the cursor is inside
- [ ] `EditorView` scroll: `set_selections` picks Top/Bottom from last frame's `visible_lines`; `ScrollStrategy::Nearest` (used by the palette since 2026-09-07) does the same with no bookkeeping — swap it and drop the side pick
- [ ] Palette: highlight matched characters (`CommandMatch::indices`) in titles; show "no keybinding" hint; remember last query per session
- [ ] `format_version` is never checked when a manifest is read (`Storage::load_manifest` and the sync `load_manifest_from` both ignore it): a future v2 `workspace.toml` loads silently in an old binary, unknown fields dropped — validate and report a typed error before workspace open ships
- [ ] Rebind live when `settings.toml` / `workspace.toml` change (`cx.clear_key_bindings()` + `commands::install`) and add a settings validation command; today the three layers resolve once at startup (→ D24, restart required), invalid keystrokes are logged with a fallback to defaults, and a parse error shows once in the status bar
- [ ] Plugin commands need a GPUI action shape (a generic `PluginCommand { id }` action or per-plugin `actions!`) before `CommandContribution` from 08-plugin-api can register through `CommandService`
- [ ] `EditorView` follow-ups: horizontal scroll / soft wrap (long lines are clipped today), find/replace, add cursor above/below and select-next-occurrence, gutter run buttons per statement, `BufferChanged` publisher so other views can observe the buffer (10-editor data flow), shaped-line cache keyed by (line, text, highlights) if profiling shows re-shaping visible lines each frame matters
- [ ] `AppEvent::CommandExecuted` keymap semantics: today it fires only for palette-dispatched commands; key-driven actions bypass the service, so listeners (history, plugins) see a partial stream — either record from a global action observer or document it as palette-only
- [ ] Editing ops follow-ups: indent/outdent, join lines, transpose, select word/line, add cursor above/below, word motions across line breaks for `prev_word_boundary` when the previous line is empty
- [ ] `EditHistory` bounds: cap depth and coalesce typing bursts (today every keystroke stores its removed/inserted text forever; a select-all + paste on a 10 MB file retains full copies)
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
