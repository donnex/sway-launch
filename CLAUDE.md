# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`sway-launch` is a CLI tool for the [Sway](https://swaywm.org/) window manager. It launches an
application, waits for its window to appear via the Sway IPC event stream, then optionally runs
follow-up actions against that window (floating, fullscreen, split, resize, move to new
row/column, mark).
Because it blocks until the window exists (and until each follow-up action completes), it's
designed to be chained in shell scripts to deterministically build up window layouts without
manual `sleep`s. See `README.md` for full CLI usage and layout-building examples.

**This repository is public** (confirmed with the user 2026-08-19). The Content section below's
strict, no-confirmation-step placeholder policy and the Issues section's issue-tracker sensitivity
rules both apply in full because of this — there's no "private repo" fallback to lean on.

## Commands

- Build: `cargo build`
- Run: `cargo run -- [OPTIONS] [COMMAND]` (e.g. `cargo run -- -a foot foot`)
- Format: `cargo fmt`
- Lint: `cargo clippy`
- Test: `cargo test` — runs the unit tests in `src/sway_launch.rs` and `src/main.rs` (covering all
  pure/logic functions; see the Testing bullet under Rust conventions for what's exempted and why)
  plus the integration tests in `tests/` (driving the compiled binary directly, for the couple of
  things that need a real subprocess — see Architecture below). This never touches a real Sway
  compositor, so it runs headlessly in CI.
  - Run a single test: `cargo test <test_name>`
  - Run with debug output: `cargo test -- --nocapture`
- Live-Sway test: `scripts/run-live-sway-tests` — starts a throwaway headless Sway compositor, runs
  `tests/live_sway.rs` (gated behind the `live-sway-tests` Cargo feature) against it, then tears it
  down. This is what actually exercises the IPC-touching functions `cargo test` exempts (see the
  Testing bullet under Rust conventions), plus anything else worth confirming against real Sway
  behavior rather than assuming it. Requires `sway`, `swaymsg`, and `foot` on `PATH`; runs in its
  own CI job (`live-sway-tests` in `.github/workflows/check.yml`) separate from the main `check`
  job, so a live-Sway hiccup doesn't block the fast unit-test feedback loop. Running the scripts in
  `examples/scripts/` against a live Sway session is still useful for eyeballing real layouts, but
  is no longer the only way these code paths get exercised.

See the CI section below for how GitHub Actions runs these same checks.

## Architecture

The crate is four source files plus five integration test files:

- `src/main.rs` — defines the `clap`-derived `Args` struct (CLI flags) and constructs a
  `sway_launch::SwayLaunch` (direct CLI mode) or dispatches to `run_layout()`/`layout.rs`
  (`--layout` mode) or `run_template()`/`template.rs` (`--template` mode) — both funnel into the
  shared `run_steps()` (see "`--template`" below). Argument validation itself (e.g.
  `--height`/`--width` must match `\d+(px|ppt)`) lives in `sway_launch.rs` as
  `pub fn validate_size_argument`/`validate_position_argument`, referenced from `main.rs`'s
  `#[clap(value_parser = ...)]` attributes, so both the CLI parser and `layout.rs`'s TOML steps
  validate the same way without duplicating the regexes.
- `src/sway_launch.rs` — all the core logic (see below), plus the two shared validators above.
- `src/layout.rs` — `--layout`'s TOML schema (`Layout`/`LayoutStep`) and
  `LayoutStep::to_sway_launch()`, which converts one step into a `sway_launch::SwayLaunch` (see
  "Layout files" below).
- `src/template.rs` — `--template`'s TOML schemas (`Template`/`TemplateStep`,
  `Bindings`/`Binding`) and `resolve()`, which converts a `Template` + `Bindings` into ordinary
  `layout::LayoutStep`s (see "Templates" below).
- `tests/completions.rs` — needed because `--completions` calls `process::exit(0)` inside
  `main()`, which a unit test can't call into directly.
- `tests/json_output.rs` — asserts `main()`'s actual stdout/stderr behavior (`--json` output,
  `--verbose` diagnostics going to stderr) by driving the compiled binary with `--con-id`, the one
  target mode that never touches the Sway socket, so this runs headless in CI. Also covers
  `--dry-run`'s direct-CLI path (`--dry-run` never touches Sway IPC by design — see "`--dry-run`"
  below — so unlike most other flags it doesn't need `--con-id` specifically to stay headless) and
  `--validate`'s "requires `--layout`/`--template`" check.
- `tests/layout.rs` — asserts `--layout`'s end-to-end behavior (file reading, TOML parsing, step
  iteration, `--json` output, error messages) the same way, using `con_id`-only steps. Also covers
  `--layout --dry-run` (including the `target_id`-placeholder-resolution case) and
  `--layout --validate` (success, a step error, and both `--json` shapes).
- `tests/template.rs` — the same headless approach applied to `--template`: `con_id`-based
  `Binding`s exercise resolution, `--apps`/`--bindings`, and error messages end to end without
  needing a live Sway session. Also covers `--template --dry-run` and `--template --validate`, the
  same cases as `tests/layout.rs`.
- `tests/live_sway.rs` — the odd one out: gated behind the `live-sway-tests` Cargo feature (so a
  plain `cargo test` skips it entirely) and needs a real, reachable Sway compositor, run via
  `scripts/run-live-sway-tests` rather than directly. Drives the compiled binary against real
  windows (`foot` — software-rendered, no GPU/EGL needed, so it runs headlessly; already pulled in
  as sway's own default-terminal dependency, and this project's example app throughout, so no
  stand-in substitution is needed anywhere in this file) and asserts on real tree state read back
  via `swayipc::Connection`, covering the IPC-touching functions the other four test files, and
  `cargo llvm-cov`, can't reach headlessly (see the Testing bullet under Rust conventions). Beyond
  the individual action/flag tests, `every_shipped_template_resolves_and_launches_successfully` and
  `dual_output_template_moves_windows_to_separate_outputs` drive every file under
  `templates/` directly (not a hand-written stand-in), and
  `quad_terminals_layout_launches_four_windows_in_a_grid`/
  `retarget_by_id_layout_floats_the_first_step_by_name` do the same for `examples/layouts/` — a
  broken shipped example is a live-Sway test failure, not just a manual-testing gap.
  `every_basic_example_script_launches_successfully` extends this to `examples/scripts/`'s six
  foot-only "basic" scripts (`dual-terminals`, `triple-row`, `column-split`, `quad-terminals`,
  `workspace-and-position`, `retarget-floating`): unlike the TOML files, each script invokes
  `sway-launch` by bare name via `PATH` rather than being passed as an argument to it, so the test
  copies the compiled binary into a temp directory named `sway-launch` and prepends that directory
  to `PATH` for the duration of each run, then runs the shipped script file directly — no temporary
  copy of the script itself is needed, since it already ships ready to run as-is. The five
  "advanced" scripts (`browser-comparison`,
  `dev-workspace`, `editor-with-floating-terminal`, `floating-file-manager`, `quad-mixed-apps`) are
  scoped out — none of Firefox/Chromium/Thunar/VS Code are installed in the `live-sway-tests` CI
  job. Several tests
  sleep briefly after their `sway-launch` invocation completes before asserting on tree state, on
  top of `--wait-time`: under this suite's cumulative load — many tests run in one shared
  compositor/workspace, and several call `create_output`, never removed — the last window's
  geometry can still be settling for a short while after the process itself has already exited.
  Every CLI flag is covered here against real Sway with one exception: `--class`/`-c` needs an
  XWayland/X11 client (a real window's `WM_CLASS`) to match against, and no such client (e.g.
  `xterm`) has been available in this project's dev/CI environments so far — `--class` stays
  unit-test-only (`window_class_match`, `matches_window_event`'s class arm) until one is.
  `--sticky` is covered by `sticky_sets_the_sticky_flag_even_on_a_tiled_window`/
  `sticky_confirms_via_poll_well_under_a_large_wait_time`.
  `--debug-events` (which runs until killed, unlike everything else here) is covered by
  `debug_events_prints_a_real_window_event`, spawned as a background child via the `KillChildOnDrop`
  guard (mirrors `KillOnDrop`, but for a `Child` rather than a container id) with its stdout read on
  a separate thread and forwarded through a channel — the same shape `sway_launch.rs`'s own event
  loop uses internally. `--scratchpad` is covered by
  `scratchpad_moves_a_tiled_window_to_the_scratchpad`/
  `scratchpad_is_a_no_op_when_already_in_the_scratchpad`; `--existing`'s reach into the scratchpad
  (documented in README.md's "Target an existing window" section, previously exercised only
  manually, never regression-tested here) is covered by `existing_matches_a_window_in_the_scratchpad`.
  `--rollback-on-error` is covered by
  `rollback_on_error_kills_earlier_launched_windows_when_a_later_step_fails` — the "requires
  `--layout`/`--template`" check and the `--json` error-output shape (`fail()`/
  `fail_with_rollback()` in `main.rs`) are both con_id-only and headless, so those live in
  `tests/json_output.rs`/`tests/layout.rs` instead (`rollback_on_error_without_layout_or_template_errors`,
  `con_id_json_error_output_is_a_structured_object`, `layout_json_error_output_is_a_structured_object`,
  `layout_rollback_on_error_reports_empty_rollback_when_nothing_was_launched`).

  **This file's coverage must stay complete, not just present.** It's the one place anything
  IPC-touching actually gets exercised against real Sway, and — same as `.github/workflows/`'s
  live-sway-tests job, which just runs it — it isn't something that self-maintains: nothing forces
  it to keep tracking the application as flags, actions, and shipped examples are added. Whenever a
  change adds or changes a CLI flag/action, an example script, a `--layout` file, or a
  `--template` file, add or update a `tests/live_sway.rs` case for it in the *same* change — driving
  the actual shipped file/flag against a real compositor, not a hand-written stand-in, per the
  precedent above. Treat a gap here exactly like CLAUDE.md drifting from the implementation, a CI
  workflow drifting from the tooling (see "Keeping a workflow up to date" under CI below), or a
  stale screenshot drifting from the template it's supposed to depict (see "Screenshots" below): a
  bug to fix immediately, not a follow-up.

All five integration test files need `CARGO_BIN_EXE_sway-launch` (to invoke the compiled binary as
a subprocess), which is only set for files under `tests/`, not for the bin crate's own unit test
harness — that's why these live here instead of as `#[cfg(test)]` modules in `src/main.rs`.

### Core model: `SwayAction`

Every CLI flag maps to a `SwayAction` enum variant (`Exec`, `Split`, `Floating`, `Sticky`,
`Fullscreen`, `Focus`, `NewColumn`, `NewRow`, `Workspace`, `Output`, `Mark`, `Height`, `Width`,
`Position`, `Scratchpad`). Each variant knows how to:

- render itself as a `swaymsg` command string (`sway_command()`) — `Mark`'s, `Workspace`'s, and
  `Output`'s values are wrapped through `quote_sway_string()` before interpolation, since Sway's
  command parser splits on unquoted `,`/`;` and an unescaped value could otherwise inject
  additional commands; `Height`, `Width`, and `Position` don't need this since they hold typed
  `Size`/`Position` values (see below), not arbitrary strings — `sway_command()` is their
  serialization point, formatting a `Size`/`Position` back into Sway's `<n>px`/`<n>ppt` or
  space-separated `move position <x> <y>` syntax, rather than interpolating a pre-validated string,
  and `Exec`'s command is passed through unquoted by design (the tool's whole job is to run it)
- declare which `WindowChange` event(s) would confirm it completed (`matching_window_change_events()`)
- report its `--timeout`/`--wait-time` value, whichever field the variant actually has
  (`duration()`)

**`Height`/`Width` hold a `Size` (`Pixels(u32)`/`Percent(u32)`), `Position` holds a `Position`
(`Center`/`Coordinates { x: i32, y: i32 }`)** — both real types, not the validated strings this
project originally carried all the way from the CLI/TOML through to `sway_command()`. An external
review suggested this (typed values instead of repeatedly re-parsing/re-validating strings); the
CLI/TOML-facing surface didn't change at all (`--height 300px`, `position = "100,200"`, etc. still
parse identically) — only the internal representation, from the moment a `SwayLaunch` is
constructed (`main.rs`'s `Args`, `layout.rs`'s `LayoutStep::to_sway_launch()`) onward.
`validate_size_argument`/`validate_position_argument` still do the actual CLI/TOML-facing
validation (unchanged contract: `Result<String, String>`, still what `main.rs`'s `#[clap(value_parser
= ...)]` and `layout.rs`'s manual `to_sway_launch()` checks call) — `parse_size()`/`parse_position()`
are new, separate, **infallible** functions that convert an already-validated string into the typed
form, `.expect()`-ing that the digits parse as `u32`/`i32` cleanly. That `.expect()` is only sound
because both validators were tightened in the same change to also reject a value that matches the
`\d+` shape but overflows `u32`/`i32` (e.g. an absurd 11-digit pixel count) — without that, a
value passing validation but failing to parse would panic instead of erroring cleanly, the exact
failure mode this project treats as a bug elsewhere (see the Content/security-review discipline
generally). `sway_command()` becomes each variant's serialization point (`Size`/`Position` both
implement `Display`, rendering the identical `<n>px`/`<n>ppt`/`center`/`<x>,<y>` text the CLI/TOML
side accepts, so `SwayAction::Display`'s human-readable line and the actual Sway command text stay
in sync by construction rather than by two independently-written format strings), and
`SwayAction::poll_matches()`'s `Height`/`Width` arms match directly on `Size::Pixels`/`Size::Percent`
instead of calling the now-removed `parse_pixel_value()` on every poll iteration.
`LayoutStep`/`TemplateStep` themselves deliberately stayed `Option<String>` — they're TOML-facing
schema types where a plain string is the natural representation and keeps per-field error messages
(`"height: ..."`) exactly as before; only `SwayLaunch`/`SwayAction` needed to be typed for the
stated goal (safer internals, less repeated parsing) to actually land.

`SwayAction::run()` dispatches based on whether the action has a corresponding IPC event:

- **Has an event** (`Exec`, `Floating`, `Fullscreen`, `Focus`, `Workspace`, `Output`, `Mark`,
  `Scratchpad`) →
  `run_wait_matching_events()` for every variant except `Exec` (which gets its own
  `run_wait_matching_exec_event()`, below): connects to Sway, sends the command, then reads the
  event stream until a `Window` event matches (checked via `matches_window_event()`, container id
  for every one of these variants), or the `--timeout` is hit. `Workspace` and `Output` both use
  `WindowChange::Move`, since moving to a different workspace/output reparents the container in
  Sway's tree — confirmed reliable against a live Sway session by `tests/live_sway.rs`'s
  `workspace_moves_window_to_named_workspace` and `output_moves_window_to_named_output`
  respectively. `Focus` uses `WindowChange::Focus`, `Floating`/`Fullscreen` use
  `WindowChange::Floating`/`FullscreenMode`, confirmed reliable the same way by
  `focus_focuses_a_previously_unfocused_window`/`fullscreen_enables_fullscreen_mode` and
  `floating_with_width_and_height_applies_all_three`. `Scratchpad` (`[con_id] move scratchpad`)
  also uses `WindowChange::Move`, a third variant reusing that event type — harmless, since each
  `SwayAction::run()` waits on its own fresh event stream, so there's no cross-action ambiguity.
  Confirmed live that moving a still-*tiled* window to the scratchpad first fires a
  `WindowChange::Floating` event (Sway auto-floats it as part of the move) before the `Move` that
  actually confirms it; `matches_window_event()` needs no special handling for this — the
  `Floating` event's change type simply isn't in `Scratchpad`'s matching-events list, so it's
  skipped like any other non-matching event while `run_wait_matching_events()` keeps waiting for
  the `Move`. Confirmed by `tests/live_sway.rs`'s `scratchpad_moves_a_tiled_window_to_the_scratchpad`.

  Six of these eight — every one except `Exec` and `Mark` — *do* have an "already there" no-op
  case, though: re-applying a state the container already has doesn't fire the corresponding event
  either (moving to the workspace/output it's already on doesn't reparent anything; re-floating,
  re-fullscreening, re-focusing, or re-scratchpadding an already-floating/fullscreen/focused/
  scratchpadded window doesn't change anything for Sway to report). `SwayAction::run()` calls
  `already_at_target()` first to check for
  this and short-circuit with immediate success rather than waiting on an event Sway will never
  send. `Workspace`/`Output` were the first two found needing this (via
  `current_workspace()`/`current_output()`); `Floating`/`Fullscreen`/`Focus` were found live to have
  the identical failure mode later — re-running `--floating`/`--fullscreen`/`--focus` on a window
  already in that state hung the full `--timeout` and then errored, before this was added (checked
  via `find_container_node()`'s `floating`/`fullscreen_mode`/`focused` fields). `Floating`'s own
  check is `node_is_floating()`, not `node.floating` alone: a CI-failure investigation found Sway
  1.9 (still what `apt` installs on Ubuntu 24.04/`ubuntu-latest`, confirmed live against a headless
  compositor) never populates a floating container's own `floating` field — it stays `null` even
  though `floating enable` correctly changes the node's `type` to `floating_con` — while Sway 1.11
  populates both, so `node_is_floating()` checks `node_type == FloatingCon` first and falls back to
  the `floating` field. Without this, `already_at_target()` never short-circuited on Sway 1.9,
  reproducing the exact pre-fix hang/error this paragraph describes; confirmed live in both
  directions (broken on 1.9, fixed on both 1.9 and 1.11) during that investigation. `Scratchpad`
  was found to need this the same way, live: re-running `--scratchpad` on an already-scratchpadded
  window fires no event at all. Its own check is `container_is_in_scratchpad()`, which primarily
  checks the container's ancestor workspace name — Sway's scratchpad is always the fixed internal
  workspace named `__i3_scratch` — rather than `node_is_floating()`, deliberately, since the
  auto-float event described above would otherwise make `already_at_target()` misreport a window
  as already scratchpadded the moment it's floating, before the actual move has happened. This
  wasn't the first design: the initial version checked a node's own `scratchpad_state` field
  (`Some(ScratchpadState::Fresh)`/`Some(ScratchpadState::Changed)` once scratchpadded,
  `Some(ScratchpadState::None)` or the field entirely absent otherwise) exclusively — the same kind
  of Sway 1.9/1.11 split as `node_is_floating()`'s `floating` field above, just not caught locally
  before merging, since this project's own dev/CI environments so far had only ever run Sway 1.11
  until a real CI run against `ubuntu-latest`'s apt-installed 1.9 exposed it: `scratchpad_state`
  stays `Some(ScratchpadState::None)` on 1.9 even for a genuinely scratchpadded window, so
  `already_at_target()` never short-circuited there, reproducing the exact hang this paragraph
  describes. Fixed the same way `node_is_floating()` was: checking the version-independent signal
  (ancestor workspace name) first, keeping `node_is_in_scratchpad()`'s `scratchpad_state` read as a
  secondary, redundant OR condition rather than removing it outright. `Mark` was
  checked live too and found *not* to need this — re-applying a mark the container already has
  still fires
  `WindowChange::Mark`. Confirmed by `tests/live_sway.rs`'s
  `workspace_is_a_no_op_when_already_on_the_target_workspace`,
  `output_is_a_no_op_when_already_on_the_target_output`,
  `floating_is_a_no_op_when_already_floating`, `fullscreen_is_a_no_op_when_already_fullscreen`,
  `focus_is_a_no_op_when_already_focused`, and
  `scratchpad_is_a_no_op_when_already_in_the_scratchpad`.
- **`Exec`** → `run_wait_matching_exec_event()`. Matching purely on event content (app_id/class, or
  nothing at all with no filter) is ambiguous whenever more than one qualifying `New` window can
  appear around the same time — Sway broadcasts window events to every IPC connection, so a second,
  concurrently-running `sway-launch` process (or any other coincidentally-timed window) can be
  mistaken for the one this invocation itself launched, silently returning the wrong container id.
  To close this, the launched command's environment is tagged with a random, per-invocation marker
  (`exec env SWAY_LAUNCH_PID_MARKER=<token> <command>`, prepended without otherwise touching the
  user's command — sending the marker and command as raw, unquoted text like this, rather than
  wrapped in `quote_sway_string()`, was deliberate: Sway's own variable-substitution syntax
  (`$var`) mangles a literal `$` inside a quoted argument, which broke an earlier design that tried
  to capture the spawned shell's own pid via `echo "$$"`), and a content-matching event is only
  accepted outright once `process_has_env_var()` confirms that marker via
  `/proc/<event's pid>/environ`. A content-matching event whose marker doesn't confirm isn't
  rejected outright, though: some applications (browsers, editors) are single-instance and forward
  a second invocation's request to an already-running process before exiting, so the window that
  eventually appears is legitimately the right one, owned by a pid that was never given the marker.
  The first such event is kept as a fallback candidate, used once either `any_process_has_env_var()`
  shows the marked process (or a marked descendant) is no longer running — nothing marker-confirmed
  is coming — or `PID_MARKER_FALLBACK_GRACE` (2s; live testing under concurrent load showed a
  shorter 500ms cap occasionally forced a fallback before the real match — still coming, just
  slightly delayed by system load — arrived) elapses, whichever comes first, so a genuinely
  ambiguous case adds a bounded delay rather than the full `--timeout`. Confirmed live by
  `tests/live_sway.rs`'s `concurrent_exec_invocations_do_not_collide_on_the_same_container_id`
  (0 collisions across 90 manual trials during development, versus every trial colliding before
  this) and `exec_falls_back_to_a_content_match_when_its_own_process_already_exited`.
- **No event exists in Sway IPC for it** (`Split`, `Sticky`, `NewColumn`, `NewRow`, `Height`,
  `Width`, `Position`) → `run_wait_time()`: sleeps for `--wait-time` *before* sending the command
  unconditionally (there's no signal yet to poll for), then sends it, since Sway doesn't emit an
  event to confirm any of these. `Sticky` was confirmed live to have no dedicated `WindowChange`
  variant at all — subscribing to window events while toggling `sticky enable`/`sticky disable`
  against a live container fired nothing, unlike `Floating`/`Fullscreen`/`Focus` above, each of
  which has its own variant — so it was never a candidate for the event-confirmed path to begin
  with. `Position` has no dedicated event because moving a floating window
  doesn't reparent it in the tree. `NewColumn`/`NewRow` (`move right`/`move down`) used to be
  event-confirmed via `WindowChange::Move`, but live-Sway testing showed `move right` doesn't fire
  that event when the window is already at the tree's rightmost position — the ordinary two-window
  case — so it hung until `--timeout` every time; both moved to this wait-time pattern instead. On a
  multi-monitor setup, "move right"/"move down" on a window with no sibling to move past within its
  workspace don't always no-op the way they do with a single output — Sway's own move-direction
  semantics can instead escalate and relocate the whole workspace to the next output in that
  direction. `SwayLaunch::run()` guards against this (see below) rather than silently moving the
  window to a different monitor. Before actually running its command, `run_wait_time()` also calls
  `container_exists()` (a `get_tree()` lookup) and errors clearly if the container is gone —
  without this, a `[con_id=N]` criteria matching zero containers is treated by Sway as success
  rather than a failure, so a container that closed between an earlier action resolving it and this
  one running used to silently no-op instead of erroring; confirmed by `tests/live_sway.rs`'s
  `wait_time_action_errors_clearly_when_its_container_already_closed`. That "Sway treats a missing
  `[con_id=N]` as success" behavior is itself Sway-version-dependent, the same kind of split
  `node_is_floating()` (below) documents for a different field: confirmed live, Sway 1.9 (still
  what `apt` installs on Ubuntu 24.04/CI) silently no-ops, while Sway 1.11 already errors clearly
  with `"No matching node."` on its own. `container_exists()` is therefore redundant on 1.11 but
  still required for 1.9 — don't remove it on the strength of testing against a newer Sway alone.

  After sending the command, what happens next depends on `SwayAction::poll_matches()`, which now
  has a matcher for every one of these seven variants (per
  `docs/plan-poll-based-wait-time-actions.md`, now fully landed): `run_wait_time()` hands off to
  `run_poll_then_fallback()`, which polls `get_tree()` every `WAIT_TIME_POLL_INTERVAL` for up to
  `WAIT_TIME_POLL_GRACE` for `poll_matches()` to confirm, returning immediately once it does (the
  fast path) — mirroring `run_wait_matching_exec_event()`'s `PID_MARKER_FALLBACK_GRACE` pattern of
  a short, bounded confirmation window rather than a hang. If the grace period elapses without a
  match, `run_poll_then_fallback()` falls back to today's original behavior — assume success and
  sleep out the rest of `--wait-time` — the same never-regress-into-a-hang principle as the `Exec`
  PID-marker fallback, since several of these have a legitimate case where the expected state never
  arrives at all (a solo-window resize clamp, a tiled `NewColumn`/`NewRow` already at the tree's
  edge). Each variant's own confirmation:

  - **`Split`** — `parent_node_layout()` (`[con_id]`'s *parent* node's `layout` field, not the
    container's own — confirmed live: splitting a window with siblings wraps it in a new split
    container one level up, and splitting a solo window sets the workspace it's already the sole
    child of directly, but a leaf window's own `layout` is always unset either way) matching the
    requested `Split::H`/`V`. No known no-op case — even re-applying an already-set direction
    matches on the very first poll. Confirmed live by `tests/live_sway.rs`'s
    `split_confirms_via_poll_well_under_a_large_wait_time` (a real parent-layout change, confirmed
    well under `2 * --wait-time`) and `split_is_idempotent_and_still_confirms_promptly_when_already_set`.
  - **`Height`/`Width`** — matches directly on the held `Size`: `Size::Percent` opts out of polling
    entirely (`None`, not `Some(false)`) since there's no pixel figure to poll for without also
    resolving the reference dimension it's a percentage of, while `Size::Pixels` opts in
    unconditionally.
    `height_matches()`/`width_matches()` then compare the container's own `rect`/`deco_rect`/
    `current_border_width` (via `node_by_id()`) against the requested pixel value. Height has one
    consistent formula (`rect.height + deco_rect.height`, the decoration-inclusive outer height,
    confirmed live for both a freshly-floated and a plain tiled resize). Width needed two candidate
    formulas, not one — confirmed live that a window resized while it's been floating since the
    very command that floated it matches `rect.width` exactly, but a window that had already been
    tiled for a while before being floated and resized comes out `2 * current_border_width` short
    on `rect.width` alone (this project never found a single deterministic rule for the discrepancy,
    so `width_matches()` accepts either). The grace-period fallback is what handles the solo-window
    clamp (resizing a window that's the sole occupant of its workspace is silently clamped to
    100%): the poll can never confirm it, so it always falls back, same as before this feature
    existed. Confirmed live by `tests/live_sway.rs`'s `height_confirms_via_poll_when_resized_with_a_sibling`/
    `width_confirms_via_poll_when_resized_with_a_sibling` (fast path) and
    `height_and_width_fall_back_gracefully_when_solo_window_clamps_the_resize` (fallback path).
  - **`Position`** — `position_matches()` compares `deco_rect.x`/`deco_rect.y` (the
    decoration-inclusive frame `move position` actually targets — confirmed live, and that
    `deco_rect.x`/`rect.x` are always equal in this project's testing, so there's no width-style
    formula ambiguity here) against either the parsed `<x>,<y>` or, for `center`, a computed target
    (`compute_center_position()`: the output's own rect, read via a second `get_outputs()` call
    since `get_tree()` alone doesn't carry output geometry, centered against the window's current
    outer footprint). Unlike Height/Width, a tiled window's `move position` isn't a silent no-op
    that falls back gracefully — Sway rejects it outright ("Only floating containers can be moved
    to an absolute position"), which `run_sway_command()`'s `?` propagates as an error *before*
    `poll_matches()` is ever reached, confirmed live by `tests/live_sway.rs`'s
    `position_errors_clearly_for_a_tiled_window`. `position_confirms_via_poll_for_a_floating_window`
    covers the fast path. A *fullscreen* window is a second exception to the `deco_rect.x`/`rect.x`
    equality above: confirmed live that a fullscreen window's `deco_rect` stays `{0, 0, 0, 0}`
    permanently (stable across a multi-second sweep, not a transient settle race), since Sway never
    computes decoration geometry for a window with no border/titlebar to draw — `move position`
    still succeeds immediately against a fullscreen container (`rect.x`/`rect.y` land on the
    requested target right away), but comparing only `deco_rect` meant this could never be confirmed
    via poll, always burning the full grace period before falling back. `position_matches()` falls
    back to `rect.x`/`rect.y` whenever `deco_rect` is unset (both `width`/`height` zero), the same
    dual-formula-tolerance shape as `width_matches()` above for a different geometry quirk. Confirmed
    live by `tests/live_sway.rs`'s `position_confirms_via_poll_for_a_fullscreen_window`.
  - **`Sticky`** — a direct read of the container's own `sticky` field on `Node` (via
    `node_by_id()`), unlike `Floating`'s `node_type`/`floating`-field split (see
    `node_is_floating()`'s doc comment) — confirmed live that `sticky` is a plain `bool` with no
    Sway-1.9/1.11 quirk found, and that `sticky enable` sets it directly and immediately,
    regardless of the container's floating state (it succeeds — and the field flips — even against
    a still-*tiled* window, confirmed live by deliberately not floating it first; Sway's own docs
    describe sticky as floating-only, but that wasn't observed as an enforced restriction here).
    No known no-op case, same as `Split` — re-applying an already-set sticky state matches on the
    very first poll. Confirmed live by `tests/live_sway.rs`'s
    `sticky_sets_the_sticky_flag_even_on_a_tiled_window` and
    `sticky_confirms_via_poll_well_under_a_large_wait_time`.
  - **`NewColumn`/`NewRow`** — the one pair with no fixed target to check against (a successful move
    can land the window almost anywhere in the tree), so these are the only variants that also use
    `poll_baseline()`: `run_wait_time()` snapshots the container's own `rect` (via `node_by_id()`)
    *before* sending the command, and `poll_matches()` compares the *current* `rect` against that
    snapshot afterward — any real move changes it, a no-op doesn't. This deliberately does **not**
    compare parent/sibling structure — an earlier design that snapshotted the parent's full
    children-id list was found live to false-positive on exactly the documented "already at the
    edge" no-op case, because Sway can still incidentally restructure *other* siblings there (wrapping
    one in a new split container) even though the target window's own `rect` never changes.
    Confirmed live by `tests/live_sway.rs`'s `new_column_confirms_via_poll_when_swapping_past_a_sibling`/
    `new_row_confirms_via_poll_when_swapping_past_a_sibling` (fast path, a real sibling swap) and
    `new_column_falls_back_gracefully_at_the_edge_with_a_large_wait_time` (fallback path).

### Orchestration: `SwayLaunch::build_actions()`/`run()`

`SwayLaunch` no longer always launches a new window — it has a `target: Target` field
(`Target::Exec { command }`, `Target::ConId(i64)`, or `Target::Existing`), and
`resolve_container_id()` turns whichever one is set into a `container_id`:

- `Exec` — today's original behavior: build and run a `SwayAction::Exec`.
- `ConId(id)` — the id directly, no IPC call at all.
- `Existing` — `get_tree()` + `matching_container_ids()` (a recursive walk over `Node::nodes`/
  `Node::floating_nodes`, reusing `window_app_id_match`/`window_class_match` — refactored to take
  `&Node` instead of `&WindowEvent`, since `WindowEvent.container` already *is* a `Node`) +
  `resolve_matches()`, which errors on zero or more than one match rather than guessing. The walk
  covers the entire tree returned by `get_tree()`, which includes the `__i3_scratch` scratchpad
  workspace — a hidden scratchpad window matching the criteria is just as eligible as a visible
  one (documented for the user in README.md's "Target an existing window" section).

`run()` used to conditionally build *and immediately run* the other actions inline, one `if
self.foo { SwayAction::Foo { .. }.run()?; }` block per flag. An external review flagged that shape
as a code-structure risk (every future action inflates one already-large function, and there's no
way to inspect the planned actions without running them) — split into two methods instead:
`build_actions(container_id)` builds the fixed-order `Vec<SwayAction>` (`NewColumn` → `NewRow` →
`Workspace` → `Output` → `Split` → `Floating` → `Sticky` → `Fullscreen` → `Focus` → `Height` →
`Width` → `Position` → `Mark` → `Scratchpad`) based on which CLI flags were set, without running any
of them, and `run()` itself is now just `resolve_container_id()` + `for action in
self.build_actions(container_id)? { action.run()?; }`. A mechanical extraction, not a redesign —
`SwayAction<'a>`'s existing lifetime already matched `SwayLaunch<'a>`'s, so no data ever needed to
change shape, only *when* each `SwayAction` gets constructed. This is also what makes
`build_actions()` itself unit-testable headlessly for the first time (see its tests in
`sway_launch.rs`, `build_actions_includes_every_flag_in_the_documented_fixed_order` in particular,
which pins the exact order above) — before the split, testing the order meant testing `run()`
end-to-end against live Sway, since building and running were the same inseparable step. `Sticky`
runs immediately after `Floating` — a conventional pairing (sticky is most useful on a small
floating utility window), not a hard dependency: live testing confirmed `--sticky` alone, with no
`--floating`, works identically (see `SwayAction::poll_matches()`'s `Sticky` arm above), so this
ordering is about grouping related flags together, not about one requiring the other to run first.
`Scratchpad` runs last deliberately — it's the one action that hides the window away, so every
other action (size, position, mark) gets a chance to apply to it first while it's still
visible/tiled, and a `--mark` set earlier in the same invocation is still there to retarget the
window by later (e.g. `swaymsg 'mark dropdown-term scratchpad show'`), the classic "dropdown
terminal" scripting pattern. The final container id is printed to stdout (`main.rs`, as a bare
integer, or as part of a richer object under `--json` — see "`--json`'s richer schema" below) —
this is what makes commands chainable/scriptable (see README examples).

`NewColumn`/`NewRow` running *before* `Workspace`/`Output` in this order means `--new-column
--workspace 3` (or `--new-row --output ...`) restructures the window relative to its *origin*
workspace's siblings before the subsequent move relocates it — an external review flagged this as a
potential semantic surprise (the window arguably "should" restructure on the target workspace, not
the origin one). Investigated live rather than assumed: confirmed harmless in every case tried
(solo origin workspace, non-solo origin workspace, and the multi-output case where
`relocates_to_another_output()` above already applies) — whatever `move right`/`move down` did on
the origin workspace is entirely superseded once `move workspace`/`move container to output`
relocates the window elsewhere, so it always lands as an ordinary new sibling in the target's
existing layout with the origin workspace's other windows completely undisturbed. No reorder
needed. Confirmed by `tests/live_sway.rs`'s
`new_column_combined_with_workspace_lands_on_the_target_workspace_correctly` and
`new_column_output_guard_still_applies_when_combined_with_output`.

Before including `NewColumn`/`NewRow` in the plan at all, `build_actions()` calls
`relocates_to_another_output(container_id, direction)`, which checks `get_outputs()` (skipping the
guard entirely when there's only one output) and, if more than one exists, `get_tree()` +
`is_at_the_trailing_workspace_edge()` to see whether `container_id` is at risk of the relocation
described above. This originally checked only "is `container_id` the only window in its
workspace" — live testing found that too narrow: a *non-solo* workspace can escalate too, whenever
`container_id` is already the trailing child of a workspace whose own `layout` already matches the
move's axis (confirmed live: two windows side by side, `move right` on the rightmost relocated it
to a different output, not a same-workspace no-op, even with a sibling to its left). The current
check — direct child of its workspace (not nested in a sub-container), workspace `layout` matches
the axis (`SplitH` for `NewColumn`, `SplitV` for `NewRow`), and last child in that list — subsumes
the original solo-window case (trivially both direct- and last-child of its workspace) while also
catching the multi-window case that check alone missed; a solo window whose workspace layout
*doesn't* match the axis (e.g. stacked via `splitv`, then moved right) was confirmed live to
restructure in place rather than escalate, so checking layout too (not just child count) also
avoids skipping a move that would actually have been safe. A window nested inside a sub-container
is conservatively never flagged; unlike the direct-child cases above, this was initially left
unconfirmed live in either direction, but later confirmed safe rather than just untested — in both
an axis-mismatched nesting (a `splitv` sub-container under a `splith` workspace) and the
axis-matched worst case (a `splith` sub-container under a `splith` workspace, target as its
trailing child), `move right` never escalated the nested target to a different output; it simply
popped the target out to become a new direct child of the workspace, staying on the same output.
When flagged, the action is skipped (logged under `--verbose`) rather than run, trading a silent
cross-monitor relocation for a silent no-op — confirmed by `tests/live_sway.rs`'s
`new_column_does_not_relocate_a_solo_window_to_a_different_output`,
`new_column_does_not_relocate_a_non_solo_window_at_the_trailing_edge`, and (the nested case)
`new_column_does_not_relocate_a_nested_window_to_a_different_output`.

Each Sway IPC call opens its own fresh `Connection` (`new_connection()` in `sway_launch.rs`) — there
is no persistent/shared connection across actions.

### Output streams

stdout is reserved for that one result value (bare id or the `--json` object) and nothing else, so
`container_id="$(sway-launch ...)"`-style capture always gets exactly one clean line. Every
diagnostic/debug `println!` behind `if self.verbose()` in `sway_launch.rs` is `eprintln!`, not
`println!`, for this reason — `--verbose` output goes to stderr. Two exceptions:
`SwayLaunch::debug_events()`'s event dump and `--dry-run`'s planned-command listing are each the
command's actual output in that mode, so both stay on stdout — neither is meant to be captured as a
single clean value the way a real run's container id is, so the "exactly one line" property simply
doesn't apply to them.

Errors always go to stderr regardless of `--json` (`main.rs`'s `fail()`/`fail_with_rollback()`), but
their *shape* on stderr does follow `--json`: a `{"error": "...", "rolled_back": [...]}` object
instead of a plain-text line, mirroring the structured success shapes below rather than leaving a
`--json` caller to also parse plain-text stderr on failure. `rolled_back` is only ever non-empty
when `run_steps()`'s `--rollback-on-error` (see "`--layout`" below) actually killed something
first — every other error path passes an empty slice. This is deliberately scoped to *runtime*
failures (file I/O, TOML parsing, a `SwayLaunch`/step actually failing against Sway) — a bad CLI
invocation (missing/conflicting flags, `Args::command().error(...).exit()`) is still reported via
clap's own usage-error formatting regardless of `--json`, the same as `--help` itself isn't JSON
either.

### `--json`'s richer schema

An external review suggested expanding `--json`'s success shapes beyond a bare id/id-list — since
no version has ever shipped (`Cargo.toml` is still `0.1.0`, no `v*` tags exist), there was no
existing consumer to stay compatible with, so this was a free redesign rather than an additive one.

`SwayLaunch::run()` itself now returns `Result<RunOutcome, String>` instead of `Result<i64,
String>` — `RunOutcome { container_id: i64, actions: Vec<String> }`, where `actions` is every
action's `sway_command_verb()` (the same container-id-free text `--dry-run` prints — see above),
collected in the order it actually ran, immediately before each one's own `.run()` call. Because
`run()` still stops and returns `Err` at the first action that fails (unchanged), a successful
`Ok(RunOutcome)`'s `actions` is always the *complete* planned list, never a partial one — there's
no per-action "confirmed"/"failed" status to report the way the review's own illustrative example
showed, because a failed action never reaches the `Ok` return at all; it's `run()`'s `Err` instead,
exactly as before this existed. Plain (non-`--json`) output is unaffected — still just the bare
`container_id`.

A single invocation's `--json` output is now `{"container_id": N, "actions": [...]}`. `run_steps()`
(`--layout`/`--template`) already tracked a `resolved_ids: HashMap<String, i64>` of every named
step's (`id`, or a template `slot`, which resolves to the same name — see "`--template`" below)
container id, purely to resolve later `target_id` references — that same map is now serialized
directly as `--json`'s new `"containers"` field, alongside the existing `"container_ids"` array
(every step's id positionally, named or not). No new bookkeeping was needed for this specifically
because `resolved_ids` already existed for an unrelated reason.

### `--debug-events`

`SwayLaunch::debug_events()` subscribes to all Sway IPC event types and prints every event until
killed. Useful for discovering event shapes when adding a new action.

### `--dry-run`

Prints the planned sequence of Sway commands instead of running them, never touching Sway IPC or
launching anything — the review's suggestion, and the reason Phase 2's `build_actions()`/
`sway_command()` split (see "Orchestration" above and the `sway_command()` bullet under "Core
model" above) was worth doing first: both were designed with this in mind, not retrofitted for it.

`SwayLaunch::build_actions_for_preview()` calls `build_actions(0, false)` — the `false` is
`check_relocation`, a new parameter on `build_actions()` that, when `false`, skips
`relocates_to_another_output()`'s live `get_outputs()`/`get_tree()` call entirely and always
includes `NewColumn`/`NewRow` in the plan. `run()` itself passes `true` (unchanged behavior — it's
about to run the action for real, so it needs the real answer); a preview has no real
`container_id` yet (nothing has launched) to check the guard against anyway, so skipping it is what
makes previewing fully IPC-free, not just IPC-light. The placeholder `container_id: 0` passed to
`build_actions()` is never actually shown: `main.rs`'s dry-run printing calls
`SwayAction::sway_command_verb()` (new — `sway_command()`'s own building block, `sway_command()`
itself now just `format!("[con_id={}] {}", container_id, self.sway_command_verb())` for every
variant except `Exec`, which has no target container yet either way), not `sway_command()`, so the
placeholder id is constructed but never rendered anywhere.

`main.rs`'s `DryRunStep { target, actions }` is the shared preview representation for both the
direct-CLI path and `run_steps_dry_run()` (`--layout`/`--template`, dispatched from `run_steps()`
before any real execution begins) — `describe_target()` renders `SwayLaunch.target` as `"launch
<command>"`/`"target existing container"`/`"target existing window (app_id=...)"`, deliberately
never naming a container id even for `Target::ConId` (its id *is* already known, but showing it on
that one target mode while every action line stays id-free would be an inconsistent preview
format). `run_steps_dry_run()` still needs `to_sway_launch()`'s `target_id` lookups to resolve, so
a step's own 1-based index is inserted into `resolved_ids` as a synthetic placeholder wherever a
step has an `id` — never rendered either, same reasoning as `container_id: 0`. Plain output is one
continuously-numbered line per target/action across every step (matching the external review's own
illustrative example); `--json` is a single `{"steps": [{"target": ..., "actions": [...]}, ...]}`
object.

### `--validate`

Parses and validates a `--layout`/`--template` file without launching anything or touching Sway
IPC, exiting 0 (`valid: N step(s)`, or `{"valid": true, "steps": N}` under `--json`) or 1 (the same
`step N: <message>`/structured-`--json`-error shape every other runtime failure uses). Requires
`--layout` or `--template` (the manual `Args::command().error(...)` check mirrors
`--rollback-on-error`'s existing one exactly, same reasoning: this needs to fire in `main()`'s
direct-CLI fallthrough path, since `--layout`/`--template` themselves always exit before reaching
it).

Turned out to need less new machinery than `--dry-run`: `LayoutStep::to_sway_launch()` already
*is* the validation — height/width/position formats, target-field consistency, `target_id`
resolution — with no Sway IPC call of its own (only `SwayAction::run()` touches the socket), so
`run_steps_validate()` is just `run_steps()`'s loop with the `SwayLaunch`/`.run()` call dropped
entirely: convert every step, propagate the first error, done. A `--template`'s own
`--bindings`/`--apps` resolution (slot count, duplicate slots, binding correctness) already
happens unconditionally before `run_steps()` is ever reached (`run_template()`'s own
`template::resolve()` call) — by the time `--validate` sees `steps`, that part already succeeded,
so there's nothing left for it to repeat. Uses the same synthetic-`target_id`-placeholder approach
`run_steps_dry_run()` does, for the same reason.

### `--completions`

Standalone mode handled entirely in `main.rs`, before any of the `SwayLaunch`/`Target` logic:
`clap_complete::generate()` writes the completion script for the given `clap_complete::Shell` to
stdout and exits 0. Doesn't touch Sway IPC at all, so — unlike `--debug-events`, which still builds
a `SwayLaunch` first — it's checked and short-circuits right after `Args::parse()`.

### `--layout`

Another standalone mode, short-circuiting after `--completions`/`--list-templates` (before the
command/`--con-id`/`--existing` validation, since a layout file satisfies that requirement on its
own). `main.rs`'s `run_layout()` reads the file, parses it via `layout::parse()`
(`toml::from_str::<layout::Layout>`), then hands `parsed_layout.step` to `run_steps()` (shared with
`--template`, see below), which converts each `[[step]]` in order to a `sway_launch::SwayLaunch` via
`LayoutStep::to_sway_launch()` (reusing `sway_launch::validate_size_argument`/
`validate_position_argument` — the same validators the CLI's `--height`/`--width`/`--position`
flags use — plus the same `command`/`con_id`/`existing` one-of-four-required rule `main.rs`
enforces for the direct-CLI case, `target_id` being the fourth, layout-only option) and calls
`.run()` on it, stopping at the first error. Prints one container id per line as each step
completes, or (if `--json` is set) collects them into one `{"container_ids": [...], "containers":
{...}}` object printed at the end instead (see "`--json`'s richer schema" above). Every top-level
per-window flag `conflicts_with_all`-conflicts with `--layout` in `Args`, since a step's own fields
are what apply, not a top-level flag with no specific step to attach to.

**Named/aliased steps (`id`/`target_id`)**: a step's `id` names it for later reference; a later
step's `target_id` resolves to that named step's container id instead of launching/matching its
own window — the only way to unambiguously retarget one specific earlier step when several share
the same `app_id`/`class` (`existing = true` would be ambiguous between them). `run_steps()`
maintains a `resolved_ids: HashMap<String, i64>` alongside `container_ids: Vec<i64>`: before
converting a step, errors if its `id` was already used by an earlier step; after a step runs
successfully, if it has an `id`, inserts `id → container_id`. `to_sway_launch()` takes
`resolved_ids: &HashMap<String, i64>` and, when `target_id` is set, looks it up
(`Target::ConId(*id)`) or errors clearly if not found — both `id` and `target_id` are layout-only,
with no CLI flag equivalent, since a single `sway-launch` invocation only ever has one step to
name or reference.

**`--rollback-on-error`**: `run_steps()` stops at the first error by default, leaving whatever
earlier steps already launched open — `--rollback-on-error` (requires `--layout` or `--template`,
manually checked in `main()` the same way `--bindings`/`--apps` requires `--template`, for the same
`clap` `requires`-reliability reason documented under "`--template`" below) makes that cleanup
automatic instead. `run_steps()` tracks `launched_container_ids: Vec<i64>` alongside
`container_ids` — only ids a step resolved via `Target::Exec`, never one retargeted via
`con_id`/`existing`/`target_id`, since those windows already existed before this invocation started
and weren't this run's to close. Read directly off the step's own `command` field
(`step.command.is_some()`) rather than matching the resolved `SwayLaunch`'s `target` — equivalent,
since `to_sway_launch()` having just succeeded guarantees exactly one of
`command`/`con_id`/`existing`/`target_id` was set, but avoids a local `sway_launch` binding
shadowing the `sway_launch` module path in the same scope. On any failure — an id collision, a step failing to convert, or a
step's own `.run()` failing — `fail_step()` calls `rollback()` first (if the flag is set), which
best-effort `[con_id] kill`s every tracked id, most-recently-launched first, via the new
`sway_launch::kill_container()` (a thin `pub` wrapper around the existing private
`run_sway_command()`, added specifically so `main.rs` can reach it — every other Sway-IPC-touching
function in `sway_launch.rs` stays private, called only from within that module). A kill that
itself fails (e.g. the window already closed on its own) is logged and skipped rather than treated
as fatal — it doesn't stop the rest of the rollback, and the original step failure stays the error
actually reported. `fail_step()` is shared by all three failure branches specifically so an id
collision or conversion error on step *N* still rolls back steps *1..N-1*'s real windows, not just
a failure inside `SwayAction::run()` itself. Not handled: a step whose *own* window launched
successfully but a later action *within that same step* then failed — `SwayLaunch::run()`'s
`Result<i64, String>` doesn't carry a container id on the error path, so there's nothing for
`run_steps()` to roll back in that specific case; accepted as a known gap rather than reworking
`SwayLaunch::run()`'s error type for it, since it needs `resolve_container_id()` to have already
succeeded on the *failing* step itself, a narrower window than the earlier-steps case this feature
was written for.

### `--template`

A layer on top of `--layout`, not a replacement: `--template <FILE>` decouples a layout's shape
(what to do) from its application identity (which window), so the same template can be reused
across different applications, or shared/bundled independently of any specific one. `main.rs`'s
`run_template()` reads and parses the template via `template::parse()`
(`toml::from_str::<template::Template>`), builds a `template::Bindings` from either `--bindings`
(reads + `template::parse_bindings()`) or `--apps` (`bindings_from_apps()`: splits the
comma-separated list, zips it 1:1 onto the template's distinct `slot` names in first-appearance
order, erroring on a count mismatch), then calls `template::resolve()` and hands the result to the
same `run_steps()` `--layout` uses — a resolved template is just a `Vec<layout::LayoutStep>`, so
nothing downstream needs to know a template was involved. `--template` requires exactly one of
`--bindings`/`--apps`, and the reverse also holds: `--bindings`/`--apps` require `--template`. Both
directions are manual checks in `main()`, not `clap`'s declarative `requires`/`conflicts_with` —
`requires = "template"` on the `bindings`/`apps` fields was tried first, but a live-Sway-review
investigation found `clap`'s `derive` macro only reliably enforces it when `--bindings`/`--apps` is
combined with a narrow subset of other flags (`--json`/`--verbose`/`--timeout`/`--wait-time`);
combined with almost anything else (`--con-id`, `--existing`, any per-window flag, `--completions`,
`--list-templates`, no flags at all beyond a bare `command`), it silently parsed clean with no
`--template` and no error, falling through to the ordinary direct-CLI dispatch with `--bindings`/
`--apps` discarded — confirmed reproducible independent of this project's own `main.rs` changes, so
a pre-existing `clap` interaction, not a regression. The manual check (mirroring the existing
"exactly one of `--bindings`/`--apps`" check already in `main()`) replaced `requires` entirely,
rather than being layered alongside it, so there's exactly one code path and one error message
for "missing `--template`" regardless of which other flags are also present. `completions` and
`list_templates` additionally list `bindings`/`apps` in their own `conflicts_with_all`, since both
short-circuit via `process::exit(0)` before the manual check would otherwise run — without that,
`--completions ... --apps ...` would still silently ignore `--apps`. `--template` itself
`conflicts_with_all`-conflicts with `--layout` and every per-window flag, same reasoning as
`--layout`.

`TemplateStep` has the same action fields `LayoutStep` has, but only two target-selection fields
instead of `LayoutStep`'s five: `slot` (needs a binding) and `target_id` (retargets an earlier
step), exactly one required per step. `Binding` has the same target-selection fields `LayoutStep`
has, minus `target_id` (which only makes sense on a `TemplateStep`, referencing another slot).
`template::resolve()` turns each `slot` step into a `LayoutStep` with `id` set to the slot name and
the binding's target fields filled in, and each `target_id` step into a `LayoutStep` with no `id`
and only `target_id` set — deliberately producing an ordinary `layout::LayoutStep` in both cases, so
two mechanisms fall out for free without any template-specific code: a `target_id` step can
reference an earlier `slot` step via the **existing** `id`/`target_id`/`resolved_ids` mechanism
above (since the slot name *is* the id), and two `TemplateStep`s accidentally sharing a `slot` name
trip `run_steps()`'s **existing** "id already used by an earlier step" check, rather than needing a
separate duplicate-slot check in `resolve()` itself.

**`LayoutStep`/`TemplateStep` mirror `Args` by design and nothing keeps any of them in sync
automatically** — no compiler check, no test. When adding a new flag to `main.rs`'s `Args`, add the
matching field to `layout.rs`'s `LayoutStep` *and* `template.rs`'s `TemplateStep` in the same
change, wire it into `to_sway_launch()`/`resolve()`, and add it to README.md's "Layout files" and
"Templates" field lists — otherwise `--layout`/`--template` mode silently lacks that capability
with no signal to anyone that they've drifted apart. `id`/`target_id`/`slot` are the one exception:
layout/template-only, so they never get an `Args` field to mirror. `TemplateStep`'s action fields
are a plain duplicate of `LayoutStep`'s rather than a shared `#[serde(flatten)]`ed struct — flatten
has a known history of interacting badly with `#[serde(deny_unknown_fields)]` on the outer struct
(unknown fields can silently pass through instead of erroring), which isn't worth risking against
this project's explicit typo-catching regression tests (see `parse_rejects_misspelled_step_field`
in both `layout.rs` and `template.rs`).

### Built-in templates (`--template <name>`, `--list-templates`)

`--template`'s argument can be either a path to a template file ending in `.toml` (the original
behavior above) or a bare name with no extension, resolved against a built-in copy of every file
under `templates/` (at the repo root, not under `examples/` — see "Example layout scripts" below
for why) embedded directly into the binary at compile time via the
`include_dir` crate (`template.rs`'s `BUILTIN_TEMPLATES:
Dir<'_>`, `static`, built from `include_dir!("$CARGO_MANIFEST_DIR/templates")`) — the
single source of truth for both the shipped example files and the built-ins, so there's nothing to
keep in sync between the two: a new file under `templates/` becomes a built-in
automatically, with no code change. `main.rs`'s `resolve_template_contents()` is the dispatch
point, checked before `run_template()` reads anything: a `.toml`-suffixed value is read from disk
exactly as before this existed; anything else is looked up via `template::builtin()`
(`BUILTIN_TEMPLATES.get_file()` + `contents_utf8()`), erroring clearly (naming `--list-templates`)
if no such built-in exists. This extension-based split was chosen over inspecting the filesystem
(e.g. "try a built-in first, fall back to a file") specifically so there's no TOCTOU-ish ambiguity
and no silent shadowing between the two: every shipped template file already ends in `.toml` by
convention (see "Example layout scripts" below), so requiring external files to do the same costs
nothing while making the two paths mutually exclusive by construction — confirmed by
`tests/template.rs`'s `template_toml_suffixed_name_is_never_treated_as_a_builtin` (a `.toml`-suffixed
value that happens to share a real built-in's name still fails as a file read, not a lookup) and
`template_unknown_builtin_name_errors_clearly`.

`--list-templates` (`main.rs`'s `print_builtin_templates()`) is a standalone mode — doesn't touch
Sway IPC, so it's checked and short-circuits right after `--completions`, before the `--layout`/
`--template` dispatch — printing every built-in's name and a one-line description
(`template::builtin_templates()`: each file's *first* header-comment line, stripped of its leading
`#` and space), sorted by name, or (`--json`) the same as a `{"templates": [...]}` array. Because this
description is extracted programmatically, every template file's header comment must lead with one
complete, self-contained sentence (ending in `.`) before any further rationale — several existing
headers didn't (a single sentence wrapped across the first two lines) and were rewritten alongside
this feature so `--list-templates` never prints a sentence truncated mid-thought;
`template.rs`'s `builtin_templates_every_description_is_a_complete_sentence` test guards against
this regressing, so keep it true of any new template file's header too.

`tests/live_sway.rs`'s `builtin_template_name_resolves_and_launches_without_a_toml_extension`
drives the built-in dispatch path against a real compositor (`--template quad-grid`, no path or
extension), alongside the existing `every_shipped_template_resolves_and_launches_successfully`
(which still drives every file by its `.toml` path, unchanged) — proving the embedded copy a bare
name resolves to is the genuine, working template content, not just that the lookup compiles.

## Example layout scripts

`examples/` splits into two subdirectories by what each file actually is, not just topic; a third
kind of shipped layout file, `--template` files, lives at the repo root instead, in `templates/`
(see below for why):

- `examples/scripts/` — tracked, user-facing example scripts, each a small standalone shell script
  built out of `sway-launch` calls that demonstrates one layout. Basic examples (`dual-terminals`,
  `triple-row`, `column-split`, `quad-terminals`, `workspace-and-position`, `retarget-floating`)
  use only `foot`; advanced examples (`dev-workspace`, `floating-file-manager`,
  `browser-comparison`, `quad-mixed-apps`, `editor-with-floating-terminal`) combine multiple
  applications (Firefox, Chromium, Thunar, VS Code) and exercise more of the CLI surface
  (`--class` matching, `--floating`, `--mark`, `--width`/`--height`). README.md's "Recreatable
  layouts" section links to and groups all of these; they are full scripts a user runs directly,
  so they follow every Scripts/Shell convention below, including `-h`/`--help`. Keep this set and
  README's list of them in sync when either changes. The six basic scripts are a confirmed
  exception to the Scripts conventions' "every script needs a `die` error-reporting mechanism by
  default" rule: `foot` is the one dependency they launch, and this project already treats it as
  always present (see `tests/live_sway.rs`'s note above on why it needs no stand-in substitution),
  so none of these six ever has an error condition to report beyond bad CLI usage, which `usage 1`
  already covers — unlike the five advanced scripts, each of which checks for an app
  (Firefox/Chromium/Thunar/VS Code) that isn't guaranteed present and so does need `die`. Revisit
  this if a basic script ever grows a real failure path of its own.
- `examples/layouts/` (`quad-terminals.toml`, `retarget-by-id.toml`) — `--layout` files, run via
  `sway-launch --layout <file>` rather than executed directly.

`templates/` (`quad-grid.toml` and a wider library of other app-agnostic shapes — see README.md's
"Templates" section for the full, grouped list) holds `--template` files, run via
`sway-launch --template <file> --apps ...`/`--bindings <file>`. Named for the shape alone, never
an application: spelled-out count words for even splits/grids (`dual-row.toml`,
`triple-column.toml`, `six-grid.toml`, ...) and descriptive compound names for special-purpose
shapes (`master-dual-stack.toml`, `sidebar-left.toml`, `floating-overlay.toml`, ...). Every file
here is also embedded into the binary as a built-in `--template <name>` — see "Built-in templates
(`--template <name>`, `--list-templates`)" under "`--template`" above for how. It lives at the repo
root rather than under `examples/` precisely because of that: once a directory's contents are
compiled into the shipped binary, calling it merely an "example" undersells it — unlike
`examples/layouts/` and `examples/scripts/`, which really are just illustrative and never embedded.

The files under `examples/layouts/` and `templates/` are plain data (not executable, no
`-h`/`--help`), so the Scripts/Shell conventions don't apply to them.

There is no separate ad-hoc/scratch scripts directory — a prior `layout-tests/` served that
purpose (untracked, personal iteration history) but was removed once its useful layouts had all
been polished into tracked `examples/` scripts. If a similar need for throwaway manual-verification
scripts comes up again, recreate it under the same untracked-scratch-space conventions this section
used to document, rather than letting one-off verification scripts accumulate in `examples/`
unpolished.

## Screenshots

`scripts/generate-layout-screenshots` is a maintainer-run Python tool (not invoked by anything
else, including CI) that generates a labeled screenshot of every shipped `--template` shape, for
visual reference when writing docs or checking a shape actually looks like its description claims.
It's Python, not POSIX sh like the rest of `scripts/` — the reasoning being it needs to parse TOML
(`tomllib`, stdlib) and `swaymsg`'s JSON tree output, and orchestrate several subprocesses (`sway`,
`swaymsg`, `sway-launch`, `figlet`, `grim`), which gets painful in POSIX sh without pulling in
`jq`/similar as another new dependency. Requires `sway`, `swaymsg`, `foot`, `figlet`, `grim`, and
`cargo` on `PATH` — `figlet`/`grim` are new dependencies introduced solely for this script, not
used anywhere else in the project. Confirmed with the user (2026-08-19) per the Shell conventions'
"confirm before depending on a new external command" rule — Python scripts follow the same rule by
extension, since the reasoning (a new tool the project didn't already require) applies regardless
of language.

It reuses `run-live-sway-tests`'s throwaway-headless-Sway recipe (same `WLR_BACKENDS=headless`
setup/teardown), but loops over every `templates/*.toml` file on one long-lived compositor instead
of restarting one per test — each template gets its own Sway workspace
(`workspace screenshot-<name>`), so `grim`ing the current output after switching to it never picks
up a previous template's leftover windows, without needing to kill/relaunch the compositor itself
between iterations. `dual-output.toml` (needs a second real output) and `workspace-spread.toml`
(moves every window to its own separate workspace by design) are excluded — a single-output
screenshot can't meaningfully depict either.

Unlike `run-live-sway-tests`, the compositor isn't started with `-c /dev/null` — it's started
against a temp config file containing a single `gaps inner <--gaps>` directive (default `6`), so
adjacent windows stay visually distinct in the screenshot instead of touching edge-to-edge.
Confirmed live that this has to be a config directive rather than a `gaps inner all set <N>`
runtime command sent once at startup: the runtime command's `all` scope only covers workspaces that
already exist at the moment it runs, not ones a later `switch_workspace()` call creates — which is
every screenshot's own workspace — so it silently produced zero gap before this was caught.

Each slot is filled with a `foot` window given a distinct background color and its own slot name
rendered via `figlet` (`--font`, default the `standard` font — `mini` was tried first but found too
cramped to read at a glance; `standard` reads clearly even in a 3x3 grid's small cells, at the cost
of hitting `MAX_FONT_SIZE`'s clamp rather than growing further in wide panes), so the shape and
slot names are both
readable directly from the image without needing the template's own source alongside it. The font
size is **not**
a fixed guess — a template's shape (a 2x2 grid's equal quadrants vs. a sidebar's narrow column)
isn't known until `sway-launch` actually lays it out, so a fixed size that looked fine in one
template wrapped ugly in another. Instead, each template is launched twice: a first pass at a
uniform default size purely to measure every slot's real pixel rect via `get_tree` (matched by
window title, set via `foot -T <slot>`, since every slot shares `app_id=foot`); then, per slot, the
largest font size that still fits that measured rect is computed from empirically-calibrated
`foot` monospace-cell metrics (`PX_PER_COLUMN_PER_PT`/`PX_PER_ROW_PER_PT` in the script, with a
comment there on how to recalibrate them — there are no separate derivation notes elsewhere) and
clamped to a sane range — and only then is the template relaunched for
real and captured. `figlet`'s own `-c` (center) flag is deliberately not used: it pads to figlet's
own default 80-column canvas rather than the pane's real width, which reintroduced the same
wrapping bug this two-pass measurement exists to avoid; labels are left-aligned instead.

Screenshots are written to `--output-dir` (default `screenshots/` at the repo root) as one
`<template-name>.png` per template; `--only <name>` (repeatable) limits a run to specific
templates, useful when iterating on one shape. Unlike most other generated output in this repo,
`screenshots/` **is** committed: these images are user-facing documentation, embedded as thumbnails
in README.md's "Templates" table, not a build artifact — so they need to exist in the repo for that
table to render for anyone who hasn't run the script themselves.

**Screenshots must always stay in sync with the actual `templates/` files.** Whenever a template
file is added, removed, or has its shape changed (a step's `split`/`floating`/`height`/`width`/
`workspace`/`output`/etc. — anything that changes what the rendered layout actually looks like,
not just prose in its header comment), regenerate the affected screenshot(s) via
`scripts/generate-layout-screenshots --only <name>` (or a full run with no `--only` after a
broader change) *and commit the updated PNG(s)* in the *same* piece of work, the same discipline
`tests/live_sway.rs`'s own coverage rule above and "Keeping a workflow up to date" under CI below
already hold this project to elsewhere. A screenshot depicting a shape the template no longer
produces is exactly as much a bug as a stale doc or a CI workflow drifting from the tooling — fix
it immediately, not as a follow-up. A new template file needs a row added to README.md's
"Templates" table, thumbnail included, in the same change too, for the same reason.

## Changelog

- `CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/): dated
  `## [X.Y.Z]` sections (newest first), each broken into `Added`/`Changed`/`Deprecated`/`Removed`/
  `Fixed`/`Security` subsections as needed, plus a `## [Unreleased]` section at the top for work
  already on `dev` but not yet released.
- Whenever a user-facing change lands — a new flag/action, a new built-in template, a behavior
  change, a bug fix a user would notice — add a bullet under `## [Unreleased]` in the **same
  commit** as the change, the same sync-in-the-same-work discipline this file already holds
  `tests/live_sway.rs`, CI workflows, and `templates/`'s screenshots/README table to (see the
  Architecture, CI, and Screenshots sections above). Write it for a user reading release notes, not
  as a copy of the commit title: describe the effect, not the implementation.
- Purely internal changes (refactors, test-only additions, CI/tooling, contributor-facing doc
  updates to this file) don't get a changelog entry — `CHANGELOG.md` is for users, not
  contributors.
- At release time, see "Merging to master" below: `## [Unreleased]` is renamed to the new version
  heading and a fresh empty `## [Unreleased]` is added above it, as part of the version-bump
  commit.

## Python conventions

`scripts/generate-layout-screenshots` is currently the only Python file in the project, so these
conventions are scoped to it rather than a general `bin/`/`lib/` project layout. Managed with `uv`
via the repo-root `pyproject.toml`/`uv.lock` (`uv run scripts/generate-layout-screenshots ...`
works the same as running it directly, using the same pinned tool versions). `black` (formatter)
and `ruff` (linter) are dev-only dependencies (`[dependency-groups].dev`, never imported at
runtime) — both must pass with no findings before committing. Adopting `uv` added `/.venv` to
`.gitignore` (confirmed with the user, 2026-08-19, per the Git conventions' "never modify
`.gitignore` without explicit confirmation" rule):

```shell
uv run black scripts/generate-layout-screenshots
uv run ruff check scripts/generate-layout-screenshots
```

Formatting and linting only — no `mypy`/`ty` type-checking and no requirement that every function
be annotated, a deliberate, narrower adoption for this one maintainer script rather than the full
type-checked convention a larger Python codebase would use. `pyproject.toml`'s `[tool.ruff.lint]`
`select` list is the canonical rule selection minus `ANN` (flake8-annotations, which exists to
enforce the typing requirement this project has opted out of) and `D` (pydocstyle, which would
require a docstring on every function — in tension with this project's own "default to no
comments, name things clearly instead" style). Two rule-specific overrides:

- `UP036` (outdated-version-block) is ignored: the `sys.version_info < (3, 11)` guard at the top of
  the script is a runtime UX check (a friendly error for whichever Python actually ends up running
  it) — `requires-python` in `pyproject.toml` only documents the minimum, it doesn't enforce it at
  runtime, so this isn't the dead code the rule assumes it is.
- `[tool.ruff.lint.pylint]`'s `max-args` is raised to 6, to fit `generate_screenshot()`'s six plain
  config values — bundling them into a dataclass for a single ~400-line maintainer script isn't
  worth the added indirection.

## Rust conventions

- Always target the latest stable Rust release, and verify the toolchain is current before
  committing. Check with `rustup check`.
- Only use a nightly toolchain, or a pre-release dependency version, if it is strictly necessary —
  confirm with the user first.
- Use the latest stable release versions of dependencies.
- For a program a user runs directly, use color and clear presentation — structured formatting,
  tables, progress/status indicators, etc. — for its terminal output when it makes sense: status
  messages that should stand out (success/warning/error), progress, or tabular data. Skip it for
  something too small to warrant it, e.g. a program whose entire output is a single line.
- Default to no comments in code — clear naming and control flow should carry the intent by
  themselves. Doc comments (`///`, `//!`), where the project requires them (e.g. via
  `#![warn(missing_docs)]`), are a separate, always-required matter, not part of this rule. Add a
  regular `//` comment only when the code is genuinely hard to follow without one: a non-obvious
  constraint, a workaround for a specific bug or quirk, or logic that isn't self-evident from
  reading it — in that case, add the comment rather than leaving a future reader to puzzle it out.
- Use `cargo fmt` for formatting and `cargo clippy` for linting. Both must pass with no errors
  before committing.
- Aim for high test coverage on every new feature or fix; agree the target threshold with the user
  per project and keep coverage at or above it as code is added, rather than letting it slip. It's
  fine for genuinely untestable paths (e.g. something that can't run headless) to stay uncovered —
  note why in the project's issue tracker rather than forcing a brittle test.
  - This project's agreed target: cover every pure/logic function (command-string building, event
    dispatch tables, window-match logic, CLI argument validation/parsing) via `cargo test`. The
    functions that open, read, or write the Sway IPC socket directly (`new_connection`,
    `event_loop`, `run_sway_command`'s connection call, `kill_container`, `run_wait_time`,
    `run_wait_matching_events`, `run_wait_matching_exec_event`, `run_poll_then_fallback`,
    `container_exists`, `parent_node_layout`, `node_by_id`, `find_container_node`,
    `container_is_in_scratchpad`,
    `position_matches`, `node_and_output_name`, `output_rect`,
    `expected_position`'s `"center"` arm (only reachable once `output_rect()` succeeds, so it's
    exempt for the same reason `output_rect` itself is; the rest of `expected_position` — explicit
    coordinates, and `"center"` without a live socket — is ordinary pure logic and stays
    coverage-measured),
    `find_existing_container_id`'s connection call,
    `SwayAction::run`, `SwayAction::already_at_target`, `SwayAction::poll_baseline`'s
    `NewColumn`/`NewRow` arm, `current_workspace`, `current_output`, `containing_node_name`,
    `relocates_to_another_output`, `SwayLaunch::run`, `SwayLaunch::build_actions`'s
    `NewColumn`/`NewRow` arms (their `relocates_to_another_output()` call — the rest of
    `build_actions()` is ordinary pure logic and stays coverage-measured, same reasoning as
    `expected_position` above; see "Orchestration" below for why it's split out from `run()` at
    all), `SwayLaunch::debug_events`) are exempted from
    the `cargo llvm-cov` line-
    coverage measurement — they require a live Sway compositor, so `cargo test`/`cargo llvm-cov`
    (which run headlessly, without one) never execute them. They're no longer *unverifiable*,
    though: `tests/live_sway.rs` (see the Testing section below) exercises them for real against a
    throwaway headless Sway compositor via `scripts/run-live-sway-tests`, both in CI and locally —
    it's just a separate test tier from `cargo test`'s coverage-measured suite, gated behind the
    `live-sway-tests` Cargo feature so it stays fully opt-in. No mocking layer has been introduced
    for these on the judgment that a trait-based abstraction purely to unit-test thin IPC wiring
    isn't worth the added indirection for a tool this size; revisit if the IPC-touching logic grows
    more complex than it is today.
    `SwayLaunch::resolve_container_id`'s `Target::ConId` branch is the one exception — it never
    touches the socket, so it's covered headlessly by `tests/json_output.rs` driving the compiled
    binary with `--con-id` instead.
- Measure coverage with `cargo llvm-cov` (requires the `cargo-llvm-cov` subcommand and the
  `llvm-tools-preview` rustup component):
  - `cargo llvm-cov --summary-only --ignore-filename-regex 'main\.rs'` — summary
  - `cargo llvm-cov --html --ignore-filename-regex 'main\.rs'` — line-by-line HTML report
  - Exclude `main.rs` (or an equivalent thin entry-point file) from coverage accounting if it's
    mostly wiring that's better exercised by integration tests than unit tests.
- UI/rendering tests should assert on rendered output content, not styling or presentation
  details, so tests survive cosmetic tweaks.

### Rust workflow

- After making changes, run `cargo fmt` and `cargo clippy` and fix all findings before committing.
- Run the full test suite (`cargo test`) before committing.
- Integration tests that drive a compiled binary (e.g. under a pseudo-terminal) require the binary
  to be built first — run `cargo build` before running them.
- When making an architectural or behavioral change (new module, new data flow, changed data
  types, new dependency, new major UI/output element), update this file's Architecture section in
  the same piece of work. Don't let it drift out of sync with the implementation.
- When adding or changing a CLI flag/action, an example script, or a `--layout`/`--template` file,
  add or update the matching `tests/live_sway.rs` case in the same piece of work, run it via
  `scripts/run-live-sway-tests`, and confirm it passes before considering the change done — see the
  "This file's coverage must stay complete" note under `tests/live_sway.rs` in the Architecture
  section above.

## Scripts

These rules apply to every script in this repo, regardless of language.

- Always make executable scripts executable with `chmod +x`.
- Scripts a user runs directly must implement `-h`/`--help`. When passed, print usage to stdout
  and exit 0. When called due to invalid usage, print to stderr and exit 1.
  - Scripts whose intended caller is another program — hooks, daemons, a status bar's `exec`
    target — are exempt. The test is who invokes it in normal use, not whether a human *could*.
    Being runnable by hand for testing doesn't make a script user-run; if it did, nothing would
    ever be exempt.
  - Such a script still needs its header comment to explain how it's invoked and by what, since it
    has no `--help` to carry that. Configuration it reads from the environment belongs there too.
- For a script a user runs directly, use clear presentation — structured formatting, tables,
  progress/status indicators, etc. — for its terminal output when it makes sense: status messages
  that should stand out (success/warning/error), progress, or tabular data. Skip it for something
  too small to warrant it, e.g. a script whose entire output is a single line. Do not use ANSI
  color codes — plain text only.
- Always include a brief description at the top of every file:
  - Shell: three comment lines directly below the shebang — a blank `#`, a one-line summary, a
    closing blank `#`.
  - Python: a module docstring at the very top of the file with a one-line summary.
- Default to no comments in code — clear naming and control flow should carry the intent by
  themselves. The file-header description above (and a docstring, where the language's own
  convention requires one) is a separate, always-required matter, not part of this rule. Add an
  inline comment only when the code is genuinely hard to follow without one: a non-obvious
  constraint, a workaround for a specific bug or quirk, or logic that isn't self-evident from
  reading it — in that case, add the comment rather than leaving a future reader to puzzle it out.
- Use long and descriptive names — avoid abbreviations (e.g. `command` not `cmd`, `character` not
  `char`).
- Handle errors explicitly and exit early — check results and surface a clear message as soon as a
  failure is detected. Never let a script continue in a broken or partially-completed state.
- Never hardcode a secret, credential, API key, or token in a script. Read it from the environment
  or from a local config file excluded via `.gitignore`, and never print it to stdout/stderr or
  write it into a log, commit, issue, or any other document. Prefer a mechanism that doesn't place
  it in a subprocess's argument list when the tool supports one (a value read from a file, stdin,
  or an env var passed to the subprocess) — a bare CLI argument is visible to other processes on
  the host for the argument's lifetime (e.g. via `ps`).
- When an operation may fail transiently (e.g. a network call), implement retry logic rather than
  failing on the first attempt. If it's unclear whether the operation is safe to retry (e.g. it
  isn't idempotent), confirm with the user before adding retry behavior.
- All intermediate files must be created inside a temporary directory and cleaned up on exit,
  including on errors and signals:
  - Shell: use `mktemp -d` and clean up with `trap cleanup EXIT`.
  - Python: use `tempfile.TemporaryDirectory()` as a context manager.
- After making changes to a script, review the help output and verify it accurately reflects the
  current arguments, options, and behaviour.

## Shell scripts

Run a script directly with `sh <script>`; make it executable first with `chmod +x <script>`.

### Shell conventions

- Only POSIX sh should be used. When a non-POSIX feature would simplify the code, confirm with the
  user before using it. If the POSIX alternative is significantly more complex, or the script does
  not need to run on multiple OSes, suggest the simpler non-POSIX solution and ask the user to
  confirm before falling back to it.
- Always use `printf` instead of `echo`
- Always pass a literal format string to `printf` — never `printf "$var"` (a `%` in the value
  causes silent bugs). Use `printf '%s\n' "$var"` instead.
- Quote every variable expansion and command substitution by default — `"$var"`, `"$(cmd)"` —
  including on the right-hand side of an assignment (`host="$(hostname -f 2>/dev/null ||
  hostname)"`). POSIX assignment doesn't word-split, so this one is for consistency, not
  correctness, but keep it uniform. The sole exception is deliberate word-splitting — e.g.
  iterating a space-separated list (`for recipient in $RECIPIENTS`) — which must stay unquoted;
  `shellcheck` is the backstop for catching the unintentional cases.
- Always use `read -r` — without `-r`, backslashes in input are interpreted, which is almost never
  the intent.
- Use single quotes `''` for static strings that don't need variable expansion
- All scripts must have a `usage()` function and an error-reporting mechanism (`die` by default,
  per the Error helper pattern). Three exceptions: scripts so small that these would add more
  noise than value (confirm with the user before omitting them); scripts exempt from `-h`/`--help`
  under the Scripts conventions above — those have no usage to print, so they need only the
  error-reporting mechanism; and a harness-invoked hook/daemon/action script whose stdout or exit
  code is itself part of a contract with its caller (e.g. a status line that must always exit 0
  and never write to stderr, or an action script whose plain stdout is surfaced as another tool's
  failure message) — `die`'s stderr-and-exit-1 behavior would break that contract, so this third
  category is exempt from both `usage()` and the error-reporting mechanism.
- Always use `set -eu` at the top of every script. Be aware that `set -e` does not catch failures
  inside pipelines — in `cmd1 | cmd2` only `cmd2`'s exit code is checked. Handle pipeline errors
  explicitly by storing intermediate output in a variable or temp file rather than piping
  directly.
- Never parse the output of `ls` — use glob patterns (`for file in ./*.txt`) or `find` instead.
- Use 2 spaces for indentation
- Use uppercase variable names only for static settings and constants, not for regular variables.
  Declare them with `readonly`: `readonly VAR=value`
- Prefer a single-line `&&`/`||` guard over a full `if`/`then`/`fi` block for a simple,
  single-action condition (e.g. `[ "$var" = 1 ] && printf 'ok'`) — it keeps the script shorter and
  cuts down on block count. Only do this when it stays readable; fall back to a full `if` block
  once the condition or action gets complex.
- For a guard like `... || die ...`, let line length decide the layout. When the command before
  the guard is short, keep the whole thing on one line (`command -v jq >/dev/null 2>&1 || die "jq
  not found"`). When the command is long or complex — a pipeline, a command substitution, an
  interactive prompt — put the `||` at the end of the command line and the action on the next
  line, indented, so the guard doesn't run past a comfortable width.
- Do not cram multiple statements into a one-line `{ }` block (e.g. `foo() { cmd1; cmd2; cmd3;
  }`) — it hurts readability. Keep one-liner `{ }` blocks to a single statement in regular script
  code; once a function needs more than one statement, write it as a normal multi-line function.
- This project does not use a reusable-snippet-file convention (a shared helper copied verbatim
  into each script that needs it) — with as few scripts as this repo has, the overhead of a
  separate snippet source file plus sync-auditing isn't worth it. Write each script's helpers
  (error reporting, usage, retry/poll loops, color setup, etc.) inline in the script itself. If
  this repo's script count grows enough that duplication across them becomes a real maintenance
  burden, revisit this and reintroduce a snippet convention rather than letting copies silently
  drift apart.
- Before a script comes to depend on a new external command not already used elsewhere in the
  project, confirm the choice with the user first. This excludes standard POSIX/base-OS utilities
  guaranteed present on the target system (e.g. `sort`, `cut`, `awk`); confirmation is for
  genuinely new tooling (e.g. `jq`, `httpie`, `fzf`), not coreutils.
- Do not use a `.sh` file extension
- Name scripts in lowercase. Use `-` as word separator for scripts run directly by the user (e.g.
  `run-commands`). No separator for scripts called from other scripts (e.g. `runcommands`).

### Shell workflow

- After making changes to a script, always review the summary comment below the shebang and update
  it if it no longer accurately describes what the script does.
- Run `shellcheck --shell=sh <script>` after making changes and fix any findings before committing.
- Run `shfmt -i 2 -w <script>` after making changes to format the script.
- Neither tool is guaranteed to be present in a fresh environment. Install them via the OS's
  package manager if missing (e.g. `sudo apt-get install -y shellcheck shfmt` on Debian/Ubuntu,
  `brew install shellcheck shfmt` on macOS) rather than skipping the checks.

### Testing

Most scripts here are small enough that careful manual testing during development is enough — a
dedicated test suite would cost more than it's worth. Once a script grows large or complex (many
options, non-trivial branching logic, behavior that's easy to regress silently without noticing),
ask the user whether to set up a shell test framework (e.g. `bats-core`) rather than assuming
either way.

## Git

### Branching

- Default to the `dev` branch for all work, unless a different branch has already been checked out
  or explicitly set for the current task — in that case, keep working on that branch instead of
  switching to `dev`.
- Never commit, amend, rebase onto, or push directly to `master`, and never push `dev` to trigger
  anything master-facing on your own initiative. `master` only moves via an explicit
  user-requested merge (see "Merging to master" below).
- Before starting work on a tracked issue or a larger/multi-commit piece of work, ask whether to
  create and switch to a new topic branch for it, rather than committing straight to `dev`.
- When the work is tied to a tracked issue — whether a local file-based checklist (e.g.
  `ISSUES.md`) or an external tracker (Gitea, GitHub, or similar) — name the topic branch with
  that issue's identifier: `issue-<number>-<short-kebab-case-description>` (e.g.
  `issue-123-fix-broken-parser`). This mirrors the `(#N)` suffix used in commit titles, so the
  branch, its commits, and the tracker entry are all traceable to each other, regardless of where
  the issue itself lives.
- Keep `dev` linear on top of `master`: rebase `dev` onto `master` (never merge `master` into
  `dev`), so `dev` stays a fast-forwardable descendant of `master`.

#### History rewriting

- History rewriting — amending, rebasing, force-pushing — is allowed on `dev` (and topic branches
  based on it), but only for commits not yet merged into `master`, and only as long as `dev` stays
  linear on top of `master` per the rule above. This overrides the general "always create new
  commits, never amend/force-push" default, but *only* for `dev`/topic branches — never rewrite
  `master` history.
- Aim to keep `dev`'s commit count low. Whenever a later commit would touch the same change as one
  already on `dev` and not yet merged to `master` — most often a fix or refinement found via
  testing or review of something just committed — rewrite the earlier commit(s) instead of
  stacking a new one on top: `git commit --amend` for the tip commit, or a soft-reset-and-recommit
  for an earlier one (Claude Code's tooling disallows interactive git flags, so `git rebase -i`
  isn't an option). Do not carry multiple commits into a merge to `master` that are really just
  successive fixes or changes to the same not-yet-merged work — combine them into the commit(s)
  they fix before merging. This is about the same change accruing fixes over time, not about
  splitting distinct work: the one-commit-per-concern rule under Commits still applies to
  genuinely separate concerns. Keep each commit correct and self-contained, since it hasn't
  shipped yet; re-run lint and push with `--force-with-lease` afterward per the rule below. Once a
  commit is merged to `master`, this no longer applies — fix it forward with a new commit as
  usual.
- A topic branch gets the same treatment relative to `dev` that `dev` gets relative to `master`:
  rewrite its commits — amend, or soft-reset and recommit — to fold in fixes and refinements found
  while working on it, rather than stacking new fix-up commits on top. Clean it up before merging
  into `dev`, the same way `dev` gets cleaned up before merging into `master`, so what lands on
  `dev` is already tidy rather than a commit plus a trail of its own fixes. This is not optional —
  see "Mandatory pre-merge history check" below, which applies to this merge exactly as much as it
  applies to merging `dev` into `master`.
- Always push `dev` after committing or rewriting its history, so the remote never lags local. The
  same applies to a topic branch: push it after every commit made on it, not only once it's ready
  to merge, so it's never sitting local-only. When history was rewritten (amend/rebase), push with
  `--force-with-lease` (never a bare `--force`), so the push fails safely instead of clobbering
  anything unexpectedly added to the remote branch since the last fetch.

#### Mandatory pre-merge history check

**Before merging `dev` into `master`, or a topic branch into `dev`, the commit history being
merged MUST be checked and rewritten if needed. This is not optional cleanup, not a judgment call,
and not something to skip because the commits "look fine" — it is a required gate, every single
time, with no exceptions. This has been missed before, and a missed check means every messy
in-between commit ships permanently, since the destination branch's history is never rewritten
after the fact.**

The check: walk every commit being merged (`git log --oneline master..dev`, or
`master..<topic-branch>` for a topic branch) and compare each one against the History rewriting
rules above. Ask, for every commit: *is this really just a fix, refinement, typo correction, or
follow-up to an earlier not-yet-merged commit in the same range?* If so, it must not survive as its
own commit — squash it into the commit it fixes (`git commit --amend` for the tip, or a
soft-reset-and-recommit for an earlier one) before doing anything else. The goal is that what lands
on the destination branch reads as if it had been written correctly the first time, not as a live
recording of the back-and-forth it took to get there. This is about collapsing accrued fixes to the
*same* change, not about squashing genuinely separate concerns into one commit — the
one-commit-per-concern rule under Commits still applies.

Do this check — and any resulting rewrite plus `--force-with-lease` push — *before* presenting a
merge summary to the user, not after. A merge summary should already describe the clean, final
history, not a history that's about to be rewritten out from under it.

#### Merging to master

Merging to `master` happens only on explicit request, as a sequence:

1. Perform the mandatory pre-merge history check above. Do not proceed to the next step until
   `dev`'s history is already clean.
2. Recommend a `Cargo.toml` `version` bump based on what's landed on `dev` since the last release
   (semver: patch for fixes/internal work, minor for a backward-compatible feature, major for a
   breaking change) — then ask for confirmation before applying it. Apply it together with
   finalizing `CHANGELOG.md` (see "Changelog" above): rename `## [Unreleased]` to
   `## [X.Y.Z] - YYYY-MM-DD` with the new version and today's date, and add a fresh empty
   `## [Unreleased]` above it. This version-bump-and-changelog commit must be the last commit on
   `dev` before the fast-forward merge to `master` — nothing else lands on `dev` after it.
3. Present a summary of what's about to land — the commit range (`git log --oneline
   master..dev`) and a nutshell of what changed.
4. Get explicit confirmation on that summary. Asking for the merge and confirming its contents are
   two separate steps; don't collapse them just because the user already said "merge."
5. Once confirmed, fast-forward `master` to `dev`: `git merge --ff-only dev` — no merge commit, no
   squash, since `dev`'s history is already linear and clean. If there's no local `master` checkout
   to merge into, push `dev`'s tip directly to `master` on the remote instead.
6. If a true fast-forward isn't possible (something moved `master` independently), stop and ask
   rather than falling back to a merge commit or force-push.
7. Once `master` is updated, create and push an annotated tag matching the new version, `vX.Y.Z`,
   so `.github/workflows/release.yml` (which triggers on a pushed `v*` tag; see the CI section
   above) picks it up and builds/publishes the release archive.

Remind the user to merge when it seems due: when a large feature/fix on `dev` looks finished, or
`dev` has accumulated a lot of commits ahead of `master`, say so and suggest merging. This is a
reminder only — never merge to `master` automatically or without explicit confirmation, no matter
how done the work looks or how many commits have piled up.

#### Branch cleanup

Once a topic branch has been merged into `dev`, delete it — both the local branch and its `origin`
remote counterpart (`git branch -d <branch>`, then `git push origin --delete <branch>`) — no need
to ask first. A merged branch is fully redundant the moment its commits live on `dev`; its only
purpose was getting them there. `dev` and `master` themselves are never deleted — this applies
only to topic branches.

### Commits

- Every commit Claude Code creates must end with a `Co-Authored-By:` trailer identifying the
  active model, e.g. `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`. Never omit this —
  if a commit is later found to be missing it and hasn't been merged to `master` yet, fix it via
  the history-rewriting rules above rather than leaving it out.
- One commit = one concern, not necessarily one file. Bundle files that share the same concern
  into a single commit — a source change routinely spans several files, and this applies to
  documentation too: if a single change is documented across more than one file (e.g. a design doc
  and `README.md` both describing the same change), commit them together rather than splitting per
  file. Keep genuinely separate concerns in separate commits even when they land together for the
  same task — don't bundle work together just because it happened at the same time if the concerns
  themselves are distinct.
- Prefix commit titles with the **scope** of the change, followed by a colon and an uppercase
  description. Match the prefix casing to the actual file or directory name. One file → its exact
  path (`src/main.rs:`, `CLAUDE.md:`). Several files sharing one concern → the directory that
  contains them (`src:`). End the title with the issue number when the work is tied to a tracked
  issue: `(#12)`.
- Keep the title under 80 characters. Only exceed this if absolutely unavoidable.
- Put detailed descriptions in the commit body, not the title. For a commit spanning several
  files, use a one- or two-line prose summary followed by per-file bullets (`- src/main.rs: ...`)
  saying what each file's part does.
- Examples: `src/main.rs: Add retry prompt`, `CLAUDE.md: Add indentation rule`
- When adding a new file with no meaningful description, use `filename: Add file`. If there is a
  reason worth stating, describe it instead: `src/sway_launch.rs: Add height/width resize action`
- Never modify `.gitignore` files without explicit confirmation from the user.
- Before every commit, all lint/format checks required by this file (`cargo fmt`/`cargo clippy`
  per Rust conventions, `shellcheck`/`shfmt` per Shell scripts, and `markdownlint` per Workflow
  below) must pass with zero errors. Never commit with outstanding failures — fix them, or ask the
  user how to handle a check that genuinely should be skipped.

## Workflow

- This repo is public (see the note under "Project Overview"), so `README.md` must open with the
  AI-assistance disclaimer directly below the title. If the README is ever rewritten, or the
  disclaimer is accidentally removed, restore it.
- Commits to `dev` or any other in-progress branch do not need confirmation before committing —
  proceed directly once the change is ready. After the commit completes, show a summary of what
  changed and why. If the work produces multiple commits, commit each as its own concern per Git
  conventions above, then show one summary covering the full set once they are all done.
- Before committing any `.md` file, run `markdownlint <file>` and fix all findings. The file must
  pass with no errors before it is committed.
- Disable MD013 (line-length) in `.markdownlint.json` — prose and instruction files can't
  reasonably be wrapped at 80 characters. Once set, don't remove it or attempt to reformat files to
  satisfy it.
- If the project generates a review report (e.g. a code-review agent instruction that writes
  `REVIEW.md`), leave that file intentionally untracked (not gitignored) in the repo root. Its
  presence signals that a review has been generated and needs to be processed. Never add it to
  `.gitignore`.
- Before running a prompt that generates `REVIEW.md` (e.g. a deep-code-review or an
  agent-instructions audit prompt), check whether an untracked `REVIEW.md` already exists in the
  repo root. If it does, ask whether it has already been processed — its presence signals
  unfinished review work per the rule above. If the user confirms it's been processed, proceed
  (the new prompt will overwrite it); otherwise stop and let the user decide how to handle the
  existing report before running the new prompt.
- When a large task wraps up, or context usage is running high (roughly 20% or more used), remind
  the user to run `/clear` to start the next task with a fresh context. Output the reminder as a
  `> [!WARNING]` markdown callout so it stands out from surrounding prose. This is a reminder
  only — `/clear` is a built-in CLI command, not something invocable through a tool, so it can
  never be run on the user's behalf.

## CI

### Suggesting CI setup

If this project has no CI workflow configured yet, suggest setting one up — don't wait to be
asked. This is a suggestion, not something to set up unprompted: propose it and let the user
decide.

- Check `git remote -v` to see which host(s) the project actually uses. Suggest a workflow for
  each one present: GitHub Actions (`.github/workflows/`) for a `github.com` remote, Gitea Actions
  (`.gitea/workflows/`) for a self-hosted Gitea remote. If the project is pushed to both, suggest
  both — the two are close enough in syntax that a workflow's content is largely shareable between
  them as-is.
- The workflow must run every check the project actually enforces before a commit or change is
  considered clean — derive this from this file's own conventions (the Rust conventions and Shell
  scripts sections above), not a generic template. For example: `cargo fmt --check`, `cargo
  clippy`, `cargo test`, `shellcheck`, `shfmt -d`, and any build step — run linters/formatters in
  check mode, never autofix mode, in CI. If the project has no automated checks at all yet,
  there's nothing to wire up yet either — say so instead of inventing checks that don't exist.
- If the user declines CI setup (not now, not wanted, whatever the reason), note that decision —
  and the date — in this file, then drop it for the rest of the conversation. In a later session,
  once this file carries that note, treat it as a standing prompt to ask again: whether real time
  has clearly passed, or the project has grown enough that the case for CI is stronger than when it
  was declined.

### Keeping a workflow up to date

A CI workflow is a second copy of "what needs to pass before this is clean" — the same failure
mode as any other duplicated logic applies: whenever the project's tools or checks change (a new
linter, a new required check, a build step added or removed), the workflow file(s) must change
with it, in the same change that changed the tooling. Don't let this slip to a follow-up — treat an
out-of-date workflow as a bug, the same way a stale doc would be. This applies to what the
`live-sway-tests` job actually *runs*, not just its own YAML: that job is only as good as
`tests/live_sway.rs`'s coverage, so an application feature, flag, or shipped example with no
live-Sway case is exactly as much a bug as a missing lint step — see the coverage note under
`tests/live_sway.rs` in the Architecture section and the matching Rust workflow bullet above.

GitHub Actions is set up: `.github/workflows/check.yml` runs `cargo fmt --check`, `cargo clippy`,
`cargo build`, `cargo test`, and a `cargo llvm-cov` coverage-regression gate (`--fail-under-lines
82 --fail-under-regions 80 --fail-under-functions 90`, calibrated to this project's actual measured
baseline — see the comment above that step in `check.yml` for why a flat number this project didn't
choose arbitrarily still coexists with the qualitative "cover every pure/logic function" target
under Rust conventions above) in its `check` job; a separate `lint-scripts-and-docs` job that runs
`shellcheck`/`shfmt` on every script under `scripts/`/`examples/scripts/`, `markdownlint '**/*.md'`
(every Markdown file in the repo, not a fixed list, so a newly added one like `CHANGELOG.md` is
covered automatically), and `black --check`/`ruff check` on the one Python script; a separate
`live-sway-tests` job that installs `sway`+`foot` and runs `scripts/run-live-sway-tests` (kept as
its own job so a live-Sway hiccup doesn't block the fast unit-test feedback loop); and a `cargo
audit` job via the `actions-rust-lang/audit` action (switched from `rustsec/audit-check`, which has
had no release since September 2024 and still declares the now-deprecated `node20` runtime with
nothing newer to bump to; `actions-rust-lang/audit` is a composite action with no Node runtime of
its own, so it sidesteps the issue rather than just deferring it).

`.github/workflows/release.yml` re-runs `check.yml`'s `check` and `lint-scripts-and-docs` checks
combined into one job (not the coverage gate, not `live-sway-tests` or `audit` — a release build
doesn't need a compositor or a coverage measurement, and the security audit runs independently on
every push regardless) against the exact tagged commit, then builds and publishes a release archive
when a `v*` tag is pushed. Release notes come from `CHANGELOG.md` rather than `gh`'s own
auto-generated notes: an `awk` script extracts the section matching the tag's version (see the
Changelog section above) and fails the release outright if that section is missing or empty — a
release can't ship without the version-bump-and-changelog commit having actually finalized
`CHANGELOG.md` first. Keep both workflow files in sync with this file's Rust/Shell/Python
conventions above whenever the checks change.

## Content

Applies to every file in this repository — not just test fixtures, but source code, comments,
commit messages, and the repo's own documentation (`CLAUDE.md`, `README.md`, any design doc,
etc).

Real content must never be committed, full stop — no confirmation step, no exception, no "just
this once." Always use placeholders or made-up content instead. Real data leaking into the
repository at all is the failure mode to guard against here, not just an unconfirmed one — this
matters more once the repo is public.

- Never write a real person's name anywhere in the repository. Use a generic placeholder instead
  (e.g. "the user", "example-user").
- Never write a real hostname, URL, IP address, command, or any other identifying detail. Use a
  made-up placeholder instead (e.g. `example-host`, `example.com`, or a documentation-range
  address like `203.0.113.10` — never a real one).
- This applies even when writing a rule *about* real data: cite a placeholder, never an actual
  value, even as an illustrative example.
- If the user supplies a real, non-generic example (a real hostname, a real command, a real name)
  to illustrate a request, generalize it into a placeholder before writing it down anywhere in the
  repository — never commit it verbatim, no matter who provided it or why.
- There is no confirm-and-proceed path here: if something might be real data, treat it as real
  data and replace it with a placeholder. When in doubt, default to a placeholder rather than
  asking whether the real value is fine to use.

**Approved exception:** `README.md`'s Installation section links directly to this repo's own
GitHub Releases page. That's a real URL, but it's self-referential (the repo linking to its own
page) rather than a leak of unrelated real-world data, and the user explicitly signed off on it
after being asked. Don't flag it in a future content-policy review, and don't use it as precedent
for adding other real URLs without the same explicit confirmation.

**Approved exception:** `README.md:8`/`CLAUDE.md:7` (`https://swaywm.org/`, the window manager
this tool is built for), `CLAUDE.md:1296` (`https://cli.github.com/`, the `gh` CLI's own site), and
`Cargo.toml:9` (`https://doc.rust-lang.org/cargo/reference/manifest.html`, `cargo init`'s
boilerplate "see more keys" comment pointing at Cargo's own manifest reference docs) are real
URLs, but each is a necessary reference to the specific upstream project/tool the surrounding text
is about, not a leak of unrelated real-world data — the user explicitly signed off on these after
being asked (2026-08-15). Don't flag these specific ones in a future content-policy review, and
don't use this as precedent for adding other real URLs without the same explicit confirmation.

**Approved exception:** `Cargo.toml:7`'s `repository = "https://github.com/donnex/sway-launch"` is
a real URL, but — like `README.md`'s Releases-page link above — it's self-referential (the repo
describing its own location), not a leak of unrelated real-world data, and the user explicitly
signed off on it after being asked (2026-08-19). Don't flag it in a future content-policy review.

**Standing exception:** `Cargo.lock` and `uv.lock` (package registry index URLs — `crates.io-index`,
`pypi.org`, `pythonhosted.org`) are tool-generated lockfile content, not authored by anyone, and
necessary for reproducible builds (`cargo build --locked`, `uv run` both depend on them). Out of
scope for this policy categorically, not as a one-off approval — a future `cargo update`/`uv lock`
regenerating either file with different URLs needs no re-confirmation and shouldn't be flagged by a
future content-policy review.

**Approved exception:** `CHANGELOG.md`'s header and this file's "Changelog" section
(`CLAUDE.md:734`) link to `https://keepachangelog.com/en/1.1.0/` and
`https://semver.org/spec/v2.0.0.html`. Both are real URLs, but each is a necessary reference to the
specific upstream spec the surrounding text is about, the same reasoning as the `swaywm.org`/
`cli.github.com`/`doc.rust-lang.org` exception above — the user explicitly signed off on these
after being asked (2026-08-20). Don't flag these two in a future content-policy review, and don't
use this as precedent for adding other real URLs without the same explicit confirmation.

**Approved exception:** `LICENSE:3`'s `Copyright (c) 2026, donnex` uses the maintainer's real
handle. Unlike the placeholder-name rule above, this isn't an illustrative example — it's the
actual copyright holder of this actual repository, the same self-referential reasoning as the
`github.com/donnex/sway-launch` URL exceptions above (a placeholder name in a real LICENSE file
would be legally meaningless). The user explicitly signed off on this after being asked
(2026-08-21). Don't flag it in a future content-policy review, and don't use this as precedent for
writing any other real name without the same explicit confirmation.

## Issues

For tracking issues on GitHub (github.com or a GitHub Enterprise Server host), managed with the
`gh` CLI (<https://cli.github.com/>) rather than the web UI for routine operations. This repo now
has a GitHub `origin` remote configured, so these conventions are active.

**GitHub issues are frequently public, and even a private repo's issues can be read by anyone with
access to it — treat every issue title, body, comment, and label as content that could leak beyond
the intended audience.** Never write any of the following into an issue: private or confidential
data, real system information (hostnames, IP addresses, internal file paths, internal URLs,
infrastructure details), credentials, tokens or secrets of any kind, real personal names, or any
other identifying or sensitive detail. Use a placeholder or generic description instead — the same
discipline as the Content conventions above.

**If it is ever unclear whether something counts as sensitive, stop and ask the user for explicit
confirmation before creating or posting anything** — do not guess, and do not proceed on the
assumption that a repo or issue is private enough to relax this.

Before actually running `gh issue create` or `gh issue comment`, re-read the fully drafted
title/body a second time as a distinct check, looking specifically for anything that violates the
rule above. This second pass happens after the content is written and before the command runs —
never skip it, even for a small or seemingly obvious issue. Only submit once that second read
confirms it's clean.

- Install `gh` via the OS's package manager (e.g. `apt install gh`, `brew install gh`) or from
  GitHub's own release page.
- Authenticate once per machine: `gh auth login`, following its interactive prompts (browser or
  token; add `--hostname <host>` for a GitHub Enterprise Server host). The resulting credentials
  live only in `gh`'s own local config (`~/.config/gh/hosts.yml`) on the machine running it — never
  commit a token or write one into the repo.
- Run `gh` from inside the project's repo; it auto-detects the repository from the git remote, so
  `--repo <owner>/<name>` isn't needed unless operating on a different repo than the current
  checkout.
- List issues: `gh issue list`
- View a single issue: `gh issue view <number>`
- Create an issue: `gh issue create --title "..." --body "..."`
- Comment on an issue: `gh issue comment <number> --body "..."`
- Close an issue via `gh issue close <number>`, once it's actually fixed and the fix is committed
  (and pushed, if that's part of the workflow in play).
- Label an issue: `gh issue edit <number> --add-label "..."`
- Unlike some other issue-tracker CLIs, `gh`'s issue subcommands cover the full set of routine
  operations natively (list, view, create, comment, close, label) — there's no need to fall back to
  the API or the web UI for any of these.
