# Tempr — Progress archive (2026)

> Rows moved out of docs/PROGRESS.md at the Phase 1 → Phase 2 boundary
> (2026-09-03) per the living-docs archival rule in CLAUDE.md. Nothing here is
> edited after archival; MAJOR decisions remain in docs/DECISIONS.md, which is
> never archived.

## Decisions log (archived rows)

| Date | Decision | Why |
|---|---|---|
| 2026-07-13 | GPUI selected as sole UI framework | → D1 |
| 2026-07-13 | MIT license chosen as working default; `license.workspace = true` in all crates; OD#2 closed | → D12 |
| 2026-07-13 | Rust-only constraint locked | → D2 |
| 2026-07-13 | Custom SQL editor, no embedded editors | → D3 |
| 2026-07-13 | PostgreSQL first via driver abstraction | → D4 |
| 2026-07-13 | Workspace-first scope model | → D5 |
| 2026-07-13 | Service-oriented architecture | → D6 |
| 2026-07-13 | Internal event bus for service communication | → D7 |
| 2026-07-13 | Plugin-extensible from day one | → D8 |
| 2026-07-13 | Internal semantic engine, not LSP | → D9 |
| 2026-07-13 | Capability-gated roadmap phases over date-based milestones | → D10 |
| 2026-07-13 | Lorekeeper living-docs adopted | → D11 |
| 2026-07-13 | PR review-based workflow adopted — branch → PR → /code-review → user approval | → D13 |
| 2026-07-13 | GPUI dependency strategy: upstream git pin, no fork; OD#5 closed | → D14 |
| 2026-07-13 | No direct commits to main ever — judgment exception in D13 eliminated | → D15 |
| 2026-08-17 | GPUI dependency = `gpui` + `gpui_platform` at one rev; Apache-2.0 Zed crates only (no GPL `ui`/`theme`/`markdown`/`editor`); tokio↔GPUI bridge required | → D16 |
| 2026-09-03 | GPUI rev pinned to Zed `main` HEAD `ed8d600` (2026-09-03), not a release tag — every tag ≤ v1.18.0 carries GPL `zlog`/`ztracing` under gpui | → D17 |
| 2026-09-03 | `deny.toml` allows Zlib, CC0-1.0, bzip2-1.0.6, NCSA (permissive, pulled by gpui's graph); `cargo deny check licenses` is the D16 gate | → D17 |
| 2026-09-03 | `gpui_tokio` adopted as the tokio↔GPUI bridge (Apache-2.0 verified) | → D17 |
| 2026-09-03 | `rust-toolchain.toml` pins `1.97.1`, minimal profile + rustfmt/clippy; CI installs it with `rustup show` (no action input duplicates the pin) | → D17 |
| 2026-09-03 | `.cargo/config.toml` sets `net.git-fetch-with-cli = true` | libgit2 fetched 3 MB of the Zed repo in 10 min; system git completes the clone |
| 2026-09-03 | `QueryService::execute_streaming(sql, id, Arc<dyn RowSink>)` is the UI query path; `execute` collects via the same `run()` and stays for tests/scripting; `RowsReceived` published per batch | Service never holds the rows the grid displays (memory pillar, 13-result-grid); one code path, two sinks |
| 2026-09-03 | Bus→UI via `tempr_ui::events::bridge` mapping `AppEvent` → `Clone` `UiEvent` over a futures channel drained with `cx.spawn` | `AppEvent` cannot be `Clone` (`PluginPayload` is `Box<dyn Any>`); views need owned events on the main thread |
| 2026-09-03 | Phase 1 grid stores `Vec<Vec<Value>>`; the columnar `RowStore` + spill from 13-result-grid.md is deferred | 100k rows × few columns is well under the 500 MB NFR; virtualization (the render-side risk) is in place now |
| 2026-09-03 | Connection comes from `DATABASE_URL` until the workspace connection list exists; userinfo/path percent-decoded | Same source the integration tests use; no UI for secrets yet; `url` returns encoded parts |
| 2026-09-03 | `QueryService::cancel` flags an in-flight run; `finish()` records `Cancelled` and returns `Ok(run_id)` (partial rows stay valid); unknown runs still get an immediate `QueryFinished{Cancelled}` | Review: cancelling used to surface as `QueryFailed` with no completed run |
