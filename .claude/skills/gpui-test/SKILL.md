---
name: gpui-test
description: >-
  Use when writing, debugging, or reproducing GPUI tests in Tempr, including
  gpui::test arguments, TestAppContext parameters, scheduler seeds,
  ITERATIONS/SEED reproduction, parking failures, and pending task traces.
---

# GPUI Test Debugging (Tempr)

Use this skill when the task involves `#[gpui::test]`, GPUI test seeds or iterations, deterministic scheduler failures, parking/pending task failures, reproducing a flaky GPUI test, or exercising a `tempr_ui` view/render/scroll path without a real window.

Adapted from Zed's `.agents/skills/gpui-test/SKILL.md` at the pinned gpui rev, minus Zed's `ZED_HEADLESS` flag: Tempr's `test-support` platform is already headless, so no environment switch is needed.

## Tempr setup

- `gpui` is a dev-dependency of `crates/tempr_ui` with `features = ["test-support"]` (see its `Cargo.toml`). That feature supplies `#[gpui::test]`, `TestAppContext`, `TestApp`, the simulated platform and the 1920×1080 test display.
- Only `tempr_ui` may use it. Other crates' UI-facing logic is tested through pure helpers, not gpui.
- Cargo unifies features per package, so `test-support` also reaches `cargo bench` for `tempr_ui`. Benches that must stay free of it belong in their own crate (see `docs/TODO.md`, gpui-bench item).
- Tests live in the component's own `#[cfg(test)] mod tests`, next to the code, like the existing pure tests.
- `cargo test -p tempr_ui` is the fast loop; `cargo clippy -p tempr_ui --all-targets -- -D warnings` includes the test targets.

## What `#[gpui::test]` does

`#[gpui::test]` expands to a normal Rust `#[test]`, so it runs under standard Rust test runners such as `cargo test` and `cargo nextest`.

It wraps the body in GPUI's deterministic test dispatcher/scheduler and can run the same test multiple times with different seeds. The seed controls scheduler task interleavings and any `StdRng` argument injected into the test.

The macro supports both synchronous and asynchronous tests.

### Supported function arguments

The macro recognizes arguments by type name:

| Test kind | Supported arguments |
| --- | --- |
| Sync and async | `&TestAppContext`, `&mut TestAppContext`, `StdRng` |
| Async only | `BackgroundExecutor` |
| Sync only | `&App`, `&mut App` |

`StdRng` is seeded from the current GPUI test seed, and `BackgroundExecutor` is backed by the same deterministic test dispatcher.

### Attribute arguments

Use these forms on `#[gpui::test(arguments)]`:

- No arguments: runs once with seed `0`, unless `SEED` is set.
- `seed = N`: adds a single explicit seed.
- `seeds(...)`: adds multiple explicit seeds.
- `iterations = N`: runs sequential seeds starting at `0` by default.
- `retries = N`: retries a failing run up to `N` times before surfacing the failure.
- `on_failure = "path::to::function"`: calls the function after final failure, before resuming the panic.
- `iterations` can be combined with explicit `seed` / `seeds`; explicit seeds are appended to the `0..iterations` range.
- If the `SEED` environment variable is set, it takes precedence over explicit seeds.
- With `SEED=N` and `ITERATIONS=M` or `iterations = M`, the harness runs seeds `N..N+M`.

## Environment variables

### GPUI test macro / scheduler execution

- `SEED=<u64>` — chooses the scheduler seed. Use this to reproduce a failure printed as `failing seed: N`. It also seeds injected `StdRng` arguments. For `#[gpui::property_test]`, it controls the scheduler seed and GPUI applies it to the proptest config for deterministic case generation.
- `ITERATIONS=<usize>` — overrides the `iterations = ...` value at runtime. Use to sweep many seeds without editing the test.
- `PENDING_TRACES=1` or `PENDING_TRACES=true` — captures and prints pending task traces when the test scheduler panics with `Parking forbidden`. Use this when `run_until_parked()` or teardown reports pending work.
- `GPUI_RUN_UNTIL_PARKED_LOG=1` — logs when `allow_parking()` is enabled. Use to find tests that explicitly permit parking/pending work.
- `DEBUG_SCHEDULER=1` — prints scheduler clock/timer debugging from `scheduler::TestScheduler`.

### Lower-level scheduler tests

- `SCHEDULER_NONINTERACTIVE=1` — suppresses interactive seed progress output in `scheduler::TestScheduler::many`. This does not affect the `#[gpui::test]` harness path.

### General Rust test debugging vars often useful with GPUI tests

- `RUST_BACKTRACE=1` or `RUST_BACKTRACE=full` — show panic backtraces.
- `RUST_LOG=<filter>` — enable logs when the test initializes logging.

## Reproducing a specific GPUI test

1. Identify the crate/package and test name.

2. Run the narrowest test filter first, skip to 3. if a failing seed is known.

   ```sh
   cargo -q test -p tempr_ui <test_name> -- --nocapture
   ```

3. If the failure mentions a seed, rerun exactly that seed.

   ```sh
   SEED=<seed> cargo -q test -p tempr_ui <test_name> -- --nocapture
   ```

4. If the failure is flaky and no seed is known, sweep seeds.

   ```sh
   ITERATIONS=100 cargo -q test -p tempr_ui <test_name> -- --nocapture
   ```

   When the harness prints `failing seed: <seed>`, switch to `SEED=<seed>` for all future debugging.

5. If the failure is `Parking forbidden`, rerun with pending traces.

   ```sh
   PENDING_TRACES=1 cargo -q test -p tempr_ui <test_name> -- --nocapture
   ```

   If a failing seed was printed or is already known, include it too:

   ```sh
   SEED=<seed> PENDING_TRACES=1 cargo -q test -p tempr_ui <test_name> -- --nocapture
   ```

   Inspect the pending traces for a task that was spawned but not awaited, detached, completed, or intentionally allowed to park.

6. If timing or timer advancement is involved, prefer GPUI scheduler timers in tests:

   ```rust
   cx.background_executor().timer(duration).await;
   ```

   Avoid `smol::Timer::after(...)` in GPUI tests that rely on `run_until_parked()`, because GPUI's scheduler may not track it.

7. Minimize the reproduction.
   - Keep the failing `SEED` fixed.
   - Reduce `ITERATIONS` to `1` or remove it once a seed is known.
   - Remove unrelated setup only after confirming the same seed still fails.
   - Preserve scheduler-sensitive awaits/yields; removing them can mask the bug.
   - If randomness is test-controlled via `StdRng`, log or assert the generated scenario after fixing the scheduler seed.

8. Validate the fix.
   - Run the fixed seed.
   - Run a modest seed sweep, e.g. `ITERATIONS=20`, if the failure was scheduler-sensitive.
   - Run the relevant crate's test filter or broader suite if the touched code has shared behavior.

## Rendering and input injection

A plain `#[test]` plus `TestAppContext::single()` is enough for a view test; the macro is only needed for deterministic executors/fake time. Pattern used by `ResultGrid` (`crates/tempr_ui/src/components/result_grid.rs`, `wheel_axes_split_between_the_rows_and_the_columns`):

1. Build the entity: `cx.update(|cx| cx.new(|_| MyView::new()))`.
2. Wrap it in a `Render` root (usually `div().size_full().child(view.clone())`) and open a window: `cx.add_window(|_, _| Harness { view })` — maximized, i.e. a 1920×1080 test display.
3. **Draw once before dispatching input.** `cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx)).unwrap();` — hitboxes and scroll listeners only exist on the rendered frame, so a simulated event before this is a no-op.
4. Inject input on the same window. `ScrollWheelEvent { position, delta: ScrollDelta::Pixels(point(px(0.), px(-240.))), modifiers: Default::default(), touch_phase: TouchPhase::Moved }.to_platform_input()` (needs `InputEvent as _` in scope); `window.simulate_mouse_move`, `simulate_click`, `simulate_keystrokes` cover the rest.
5. Read state back through the entity (`cx.read(|cx| entity.read(cx)... )`) or draw again and assert on a counter/last-frame cache.
6. When the test is a regression pin, prove it fails first: temporarily revert the fix, watch the assertion fail for the right reason, restore.

Vertical wheel deltas arrive on `delta.y`, shift-wheel/tilt on `delta.x`; a pure vertical delta is `(0, -N)` and scrolls content down. Pointer position must be inside the element's hitbox — center of the window works for a full-size view.

## Common diagnosis patterns

### Seed-dependent assertion failure

Likely caused by a scheduler interleaving or by `StdRng`-driven test data. Fix `SEED`, reproduce, and inspect which task or generated scenario differs.

### `Parking forbidden`

Usually means a foreground/background task is still pending when the scheduler expected the test to make progress or finish. Look for:

- A task that should be awaited but was dropped.
- A task that should be detached with error logging.
- A timer or receiver that is waiting forever.
- A missing `cx.run_until_parked()` after triggering async work in a test.
- A missing `cx.advance_clock(...)` to wait for debounced work in a test.
- Use of non-GPUI timers or executors that the test scheduler cannot drive.

Rerun with `PENDING_TRACES=1` before changing code.

### Event simulated but nothing moved

The frame was never drawn, the pointer is outside the hitbox, or the element's handler is restricted to the other axis (`restrict_scroll_to_axis`, docs/11-gpui.md hard rule 6). Draw first, assert the pointer is over the element, then check which axes the element handles.

### Non-determinism / wrong thread

The scheduler can report activity from an unexpected thread. Look for work escaping GPUI's foreground/background executors, direct thread spawns, or external async runtimes not controlled by the test dispatcher.

### Tests pass alone but fail in sweeps

Use the failing seed from sweep output. Avoid assuming test order unless the runner is explicitly serial. Check globals, leaked entities/tasks, and state not reset by test initialization.

## Writing GPUI tests

- Prefer `#[gpui::test]` for tests that need `TestAppContext`, deterministic executors, fake time, or scheduler interleaving coverage.
- Add `iterations = N` when the test is intentionally checking interleavings.
- Use `StdRng` as a test argument when randomized test data should follow the same seed as the scheduler.
- Use `cx.background_executor().timer(duration).await` for delays/timeouts in GPUI tests.
- Do not add or increase `retries` while fixing a test unless the user explicitly asks or the test already documents why probabilistic tolerance is intentional. Retries can mask the failure instead of fixing it.
