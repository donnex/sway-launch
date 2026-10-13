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
- Test: `cargo test` — runs the unit tests in `src/sway_launch/`'s modules and `src/main.rs` (covering all
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

The crate is `main.rs`, two TOML-schema modules, the `sway_launch` module (itself nine files, see
below) plus five integration test files:

- `src/main.rs` — defines the `clap`-derived `Args` struct (CLI flags) and constructs a
  `sway_launch::SwayLaunch` (direct CLI mode) or dispatches to `run_layout()`/`layout.rs`
  (`--layout` mode) or `run_template()`/`template.rs` (`--template` mode) — both funnel into the
  shared `run_steps()` (see "`--template`" below). Argument validation itself (e.g.
  `--height`/`--width` must match `\d+(px|ppt)`) lives in `sway_launch/values.rs` as
  `pub fn validate_size_argument`/`validate_position_argument`/
  `validate_sway_string_argument`, referenced from `main.rs`'s
  `#[clap(value_parser = ...)]` attributes, so both the CLI parser and `layout.rs`'s TOML steps
  validate the same way without duplicating the regexes. `validate_sway_string_argument` is the
  third of these, covering `--mark`/`--mark-match`/`--workspace`/`--output` — every value
  interpolated into a Sway command as a quoted string: it rejects a blank value, and both
  characters `quote_sway_string()` escapes (`"` and `\`), since Sway was confirmed live to store
  the escape character literally rather than unescaping it, silently corrupting the value instead
  of round-tripping it (see that function's own doc comment); it rejects a newline and a NUL too,
  both of which Sway rewrites to `;`. `validate_exec_command` covers the launch command (the
  positional CLI command and a step's/binding's `command`, checked in `to_sway_launch()`), which is
  sent to Sway raw rather than quoted (see "`Exec`" below), so it goes through Sway's own command
  parser before the shell: it rejects an unquoted, unescaped `,`/`;` (scanned with the same
  quote/backslash rules as Sway's `argsep()`, so a quoted or escaped one passes through to the
  shell intact) and a newline/CR/NUL anywhere, since Sway rewrites those to `;` before it looks at
  quotes. `run_wait_matching_exec_event()` backs this up by sending the `exec` through
  `ipc.rs`'s `run_single_sway_command()`, which treats a reply with more than one outcome as an
  error, so a split that somehow got past the validator can't report success.
  `validate_non_blank_argument` is the CLI-side check for `--app-id`/`--class`: those values are
  only ever compared against a window, never interpolated into a command, so it rejects nothing but
  a blank value. `pub fn require_non_blank` is its schema-side counterpart, the generic "this named field must not be empty or whitespace-only" check the layout/template
  schemas apply to free-form identifiers (`id`, `target_id`, `slot`, a binding's `command`, a
  template's `description`/`category`) that have no dedicated format of their own. A fifth pair,
  `validate_timeout_argument` (CLI text) / `validate_timeout_seconds` (a TOML step's `u64`), bounds
  `--timeout`/`timeout` to `1..=MAX_TIMEOUT_SECONDS` (`u32::MAX`): zero because every
  timeout-bounded action sends its command *before* waiting, so `0` could only report a timeout
  over a change already applied (a launched window left open with no id printed), and the ceiling
  only because `Instant` has no published maximum and a value near `u64::MAX` overflowed the
  deadline — a representability bound, not a policy cap (see `--wait-time`'s declined cap below).
  The deadline itself is also computed (`confirmation.rs`'s `deadline_after()`) before the command
  is sent in both event-confirmed drivers, so even an unvalidated caller fails before any side
  effect.
- `src/sway_launch.rs` plus `src/sway_launch/` — all the core logic (see below), plus the shared
  validators above. The file itself is a roughly 100-line façade: module declarations, the public surface
  the three files above use, and the poll-timing constants shared between modules. Everything else
  lives in one of nine submodules, layered so that what needs a live compositor is confined to two
  of them:

  | Module | Holds |
  | --- | --- |
  | `values.rs` | `Split`/`Size`/`Position`, the `validate_*` functions and their `parse_*` pairs |
  | `ipc.rs` | connections, socket discovery, command dispatch, the event subscription, `kill_container` |
  | `tree.rs` | every pure `&Node` predicate — matching, geometry, `ContainerState::from_tree` |
  | `query.rs` | fetch-a-tree-then-delegate wrappers, and nothing else |
  | `process_marker.rs` | the PID marker and its `/proc` reads |
  | `action.rs` | `SwayAction`/`ActionKind`/`ExecAction` themselves: fields, `Display`, `sway_command_verb()`, `flag_name()` |
  | `confirmation.rs` | how an action's effect is established: event matching, state checks, polling, the `run_*` drivers |
  | `outcome.rs` | `RunOutcome`/`RunError`/`UnmatchedWindow`/`ActionRecord`/`ActionStatus`/`LaunchOwnership` and their internal counterparts |
  | `launch.rs` | `SwayLaunch`, `Target`, `build_actions()`/`run()`, `--debug-events` |

  Split out of a single 5,847-line file on 2026-09-03, on an external review's recommendation, one
  module per commit with `cargo test` green at every step. The review proposed a single `state.rs`;
  it was split into `tree.rs`/`query.rs` instead, because merging fetching with interpretation is
  the exact arrangement an earlier finding from the same review was about. **`impl SwayAction` (and
  `impl ExecAction`) is deliberately split across `action.rs` and `confirmation.rs`** — legal in Rust, since an inherent
  impl needs the same crate rather than the same module — along the line of what an action *is*
  versus how its effect is confirmed; either file would otherwise be ~1,400 lines. Unit tests moved
  with their code, each module's into a sibling `<module>/tests.rs` (`#[cfg(test)] mod tests;`)
  rather than an inline `mod tests { ... }` block — the same applies to `layout.rs`/`template.rs`
  (`src/layout/tests.rs`, `src/template/tests.rs`) — so the coverage gate can exclude test code by
  filename (see the Rust conventions' coverage bullet); `test_support.rs` (`#[cfg(test)]`) holds
  the `Node`/`WindowEvent` JSON fixtures more than one module's tests need. A new module's unit
  tests go in its own `tests.rs` the same way, never inline. Adding to this module means picking the layer first: a new
  predicate belongs in `tree.rs` unless it genuinely cannot answer from a `&Node`, in which case
  the fetching half goes in `query.rs` and the deciding half still goes in `tree.rs`.
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
  `--layout --validate` (success, a step error, and both `--json` shapes), and runs `--validate`
  against every shipped `examples/layouts/*.toml` file
  (`every_shipped_example_layout_passes_validate`), so a broken shipped example fails the fast
  `check` job and the release gate, not only the live-Sway job.
- `tests/template.rs` — the same headless approach applied to `--template`: `con_id`-based
  `Binding`s exercise resolution, `--apps`/`--bindings`, and error messages end to end without
  needing a live Sway session. Also covers `--template --dry-run` and `--template --validate`, the
  same cases as `tests/layout.rs`, plus `--show-template` (built-in name, file path, `--json`,
  unknown-name error, and its conflicts with a command/`--template`).
- `tests/live_sway.rs` — the odd one out: gated behind the `live-sway-tests` Cargo feature (so a
  plain `cargo test` skips it entirely) and needs a real, reachable Sway compositor, run via
  `scripts/run-live-sway-tests` rather than directly. **It refuses to run any other way**:
  `live_socket()`, which every connection and every spawned `sway-launch` goes through, panics
  unless the runner's `SWAY_LAUNCH_LIVE_TESTS_SOCKET` sentinel is set, `SWAYSOCK` names that same
  socket, `I3SOCK` is unset or names it too, and every output is `HEADLESS-*`. The suite kills
  every foot window, switches workspaces and creates outputs, so a plain `cargo test
  --all-features` from a desktop terminal (where `SWAYSOCK`/`I3SOCK` both name the real session)
  must fail rather than act on it; the runner exports `SWAYSOCK`/`I3SOCK` pointing at its headless
  instance for the same reason. Drives the compiled binary against real
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
  `every_shipped_template_resolves_and_launches_successfully` checks more than exit status: it runs
  each template on a fresh workspace of its own, reduces the result to a normalized outline (each
  split's layout and its children in order, each child's share of its parent as a whole
  percentage, each window by its slot name from `--json`'s `containers` map, floating windows by
  slot name only), and compares that against a recorded golden file,
  `tests/template-shapes/<name>.txt`. Exiting 0 says nothing about a template's shape — a
  reordered action, a step that silently no-ops, or a misplaced `split` all still exit 0 — so this
  is what catches a shape regression. `UPDATE_TEMPLATE_SHAPES=1 scripts/run-live-sway-tests`
  rewrites the files from what the templates build now, so an intended change is a reviewed diff
  rather than a silent pass; a recorded file with no matching template fails the test too.
  Normalization choices, each made so the outline stays the same across environments: a split
  holding a single child is collapsed into that child (whether Sway wraps a window in one is a
  release-dependent detail with no visible effect), and floating geometry is left out entirely
  (it's in pixels, and depends on the output size and titlebar font, both of which differ between
  local and CI compositors — the dedicated floating/width/height/position tests cover it). Tiled
  shares are safe to record because every shipped template sizes tiled windows in `ppt`;
  a tiled `px` size would make them output-dependent. The recorded files capture current behavior,
  not intended behavior, so a known-wrong shape is recorded as it is until the fix for it lands
  and updates the file. `builtin_template_name_resolves_and_launches_without_a_toml_extension`
  compares the built-in `quad-grid` against the same recorded file.
  `every_basic_example_script_launches_successfully` extends this to `examples/scripts/`'s six
  foot-only "basic" scripts (`dual-terminals`, `triple-row`, `column-split`, `quad-terminals`,
  `workspace-and-position`, `retarget-floating`): unlike the TOML files, each script invokes
  `sway-launch` by bare name via `PATH` rather than being passed as an argument to it, so the test
  copies the compiled binary into a temp directory named `sway-launch` and prepends that directory
  to `PATH` for the duration of each run, then runs the shipped script file directly — no temporary
  copy of the script itself is needed, since it already ships ready to run as-is.
  `retarget-floating` launches its terminal with its own app_id (`sway-launch-example`) so its
  `--existing` step matches only that window, and the test opens a decoy `foot` window before
  running it — standing in for the foot terminal a user normally runs the script from, which
  previously made `--existing` match two windows and fail. The five
  "advanced" scripts (`browser-comparison`,
  `dev-workspace`, `editor-with-floating-terminal`, `floating-file-manager`, `quad-mixed-apps`) are
  scoped out — none of Firefox/Chromium/Thunar/VS Code are installed in the `live-sway-tests` CI
  job. Several tests
  sleep briefly after their `sway-launch` invocation completes before asserting on tree state, on
  top of `--wait-time`: the last window's geometry can still be settling for a short while after
  the process itself has already exited.

  **Every test starts from the same compositor state, whatever ran before it.** The file runs in
  one compositor with `--test-threads=1`, so libtest runs the tests in name order, and state one
  test leaves behind used to change what later ones exercised — not just their timing. Outputs
  were the worst of it: seven tests called `create_output` and never removed the output, so every
  test sorting after `dual_output_template_*` ran with two or more outputs, enough to make
  `relocates_to_another_output()` silently skip the `move right` that
  `new_column_and_new_row_complete_promptly_when_already_at_the_edge` exists to send, while it kept
  passing on elapsed time alone. Three rules now hold for every test: it calls `fresh_workspace()`
  first (focuses the compositor's original output and switches to an empty workspace named after
  the test); it adds an output only through the `CreatedOutput` guard, which runs
  `output <name> unplug` on drop; and it kills its windows via `KillOnDrop`. All three hold on
  assertion panic too, so running a single test alone exercises the same paths as the full run.
  Removing outputs also retired the old "only seven `create_output` calls — the eighth crashes the
  headless backend" budget: that crash came from outputs accumulating, and 12 create/unplug cycles
  in one compositor were confirmed live (Sway 1.11) to run clean. A test that's about one specific
  path should also assert it through `--json`'s per-action status (as that test now does for
  `"skipped"`), so taking a different path is a failure rather than a pass.
  Every CLI flag is covered here against real Sway with one exception: `--class`/`-c` needs an
  XWayland/X11 client (a real window's `WM_CLASS`) to match against, and no such client (e.g.
  `xterm`) has been available in this project's dev/CI environments so far — `--class` stays
  unit-test-only (`window_class_match`, `matches_window_event`'s class arm) until one is.
  `--sticky` is covered by `sticky_sets_the_sticky_flag_even_on_a_tiled_window`/
  `sticky_confirms_via_poll_well_under_a_large_wait_time`.
  `--debug-events` (which runs until killed, unlike everything else here) is covered by
  `debug_events_prints_a_real_window_event` and `debug_events_prints_a_real_output_event`, each
  spawned as a background child by `spawn_debug_events()` via the `KillChildOnDrop` guard (mirrors
  `KillOnDrop`, but for a `Child` rather than a container id) with its stdout read on a separate
  thread and forwarded through a channel — the same shape `confirmation.rs`'s own event loop uses
  internally. The output test rescales the original output to make Sway send an output event,
  rather than spending a `create_output` call, and restores the scale on drop. `--scratchpad` is covered by
  `scratchpad_moves_a_tiled_window_to_the_scratchpad`/
  `scratchpad_is_a_no_op_when_already_in_the_scratchpad`/
  `scratchpad_hides_a_window_currently_shown_from_the_scratchpad`; `--existing`'s reach into the scratchpad
  (documented in README.md's "Target an existing window" section, previously exercised only
  manually, never regression-tested here) is covered by `existing_matches_a_window_in_the_scratchpad`.
  `--rollback-on-error` is covered by
  `rollback_on_error_kills_earlier_launched_windows_when_a_later_step_fails` — the "requires
  `--layout`/`--template`" check and the `--json` error-output shape (`fail()`/
  `fail_with_rollback()` in `main.rs`) are both con_id-only and headless, so those live in
  `tests/json_output.rs`/`tests/layout.rs` instead (`rollback_on_error_without_layout_or_template_errors`,
  `con_id_json_error_output_is_a_structured_object`, `layout_json_error_output_is_a_structured_object`,
  `layout_rollback_on_error_reports_empty_rollback_when_nothing_was_launched`).
  `--timeout`'s failure half is covered for launches: `launch_timeout_names_the_window_its_app_id_filter_rejected`
  asserts a wrong `--app-id` exits 1 at `--timeout` (not early, not much later), and
  `layout_launch_timeout_reports_its_step_and_rolls_back_earlier_steps` the same inside a layout —
  `step 2: ...` in the error and step 1's window in `rolled_back` and closed. An event-confirmed
  action's timeout (`run_wait_matching_events()`) has no live test yet: the only reliable way found
  to make Sway withhold a confirming state is the case-insensitive/keyword `--workspace`/`--output`
  move tracked in #5, and pinning that as a timeout would pin the bug. Revisit once #5 is decided.
  `rollback_on_error_handles_a_launched_window_that_already_closed_itself` covers the scenario
  where a step-1-launched window closes on its own before a later step's failure triggers
  rollback — `rollback()`'s own `kill_container()` call against that already-gone container is
  confirmed to log-and-skip rather than hang or panic. Deterministic, not racy: step 2 retargets
  step 1's window with a `NewColumn`/large-`wait_time` action, and `run_wait_time()`'s *second*
  `container_exists()` check runs after the sleep, so the window is reliably already gone by the
  time that check runs, without needing an external process to race a kill against it. (Step 2's
  own window is still alive for the *first*, fast-fail check — see `run_wait_time()`'s own section
  below for why there are two.)
  `rollback_on_error_undoes_launches_not_mutations_to_existing_windows` pins the semantic README.md
  states: rollback closes windows this run launched and leaves mutations to retargeted windows in
  place. Intentional and already documented, but untested until an external review raised it as
  something a user could read the wrong way — a documented guarantee with no test is one a future
  change can widen or narrow without noticing.
  `exec_runs_in_sway_s_directory_and_environment_not_the_caller_s` pins another documented
  semantic the same way: a launched command is forked by Sway's own `exec`, so it inherits Sway's
  working directory and environment, not `sway-launch`'s — README.md's "Basic usage" tells users
  so, and a change to how the command is launched (a `--cwd`/`--env` flag, say) has to change that
  test and that paragraph deliberately rather than by accident.

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

Helpers more than one of them needs live in `tests/common/mod.rs`, pulled in with `mod common;`:
`TempToml` (a TOML fixture removed on drop), `test_temp_dir()` (the per-process scratch directory
it writes into) and `sway_launch_without_a_compositor()` (the binary aimed at a socket that cannot
exist, for tests that need an IPC call to fail). Add a shared helper there rather than copying it
into a second test file. `tests/live_sway.rs` still carries its own copies of a few `tree.rs`
predicates (`find_node`, `node_is_floating`, the workspace/output ancestor walks, `parent_layout`):
the crate is a binary only, so an integration test can't call them, and sharing them needs a
library/binary split, which hasn't been done.

### Core model: `SwayAction`

Two types, not one. **`ExecAction`** launches the window (`command`, `app_id_match`,
`class_match`, `verbose`, `timeout`) and has no container yet. **`SwayAction`** acts on an
already-resolved container: a struct holding what every such action shares — `container_id`,
`verbose`, and `timing` — plus a `kind: ActionKind`, one variant per remaining CLI flag (`Split`,
`Floating`, `Sticky`, `Fullscreen`, `Focus`, `NewColumn`, `NewRow`, `Workspace`, `Output`, `Mark`,
`Height`, `Width`, `Position`, `Scratchpad`), each holding only what its own flag specifies.
`timing` is `--timeout` for an event-confirmed kind and `--wait-time` for a polled one, chosen once
by `SwayAction::new()` from `ActionKind::is_event_confirmed()`.

This used to be a single 15-variant `SwayAction` enum with `Exec` as one of its variants, every
variant repeating `verbose` and all but `Exec` repeating `container_id` and a `timeout` or
`wait_time` — so three 15-arm accessor matches (`verbose()`, `duration()`, `container_id()`)
existed only to read those fields back, every `build_actions()` branch re-stated them, and "this
isn't `Exec`" was enforced at runtime by nine `unreachable!`/`expect` calls instead of by the type
system. A code review (#21) suggested the split; behavior didn't change, and every one of those
panics is gone because an `ExecAction` and a `SwayAction` are different types. `Display` also now
says `container id:` for every kind (it used to mix in `container_id:` for some).

A `SwayAction` knows how to:

- render itself as a `swaymsg` command string (`sway_command()`, `[con_id=N] <verb>`) — `Mark`'s,
  `Workspace`'s, and `Output`'s values are wrapped through `quote_sway_string()` before
  interpolation, since Sway's command parser splits on unquoted `,`/`;` and an unescaped value
  could otherwise inject additional commands. That quoting is defense in depth rather than the only
  guard: since `validate_sway_string_argument` (see the `src/main.rs` bullet above) rejects `"` and
  `\` up front, `quote_sway_string()`'s own `replace()` calls can no longer fire for any value this
  tool actually accepts — they're kept so the function stays correct for any future caller, not
  because a validated value can still reach them. `Height`, `Width`, and `Position` don't need this
  since they hold typed `Size`/`Position` values (see below), not arbitrary strings —
  `sway_command()` is their serialization point, formatting a `Size`/`Position` back into Sway's
  `<n>px`/`<n>ppt` or space-separated `move position <x> <y>` syntax, rather than interpolating a
  pre-validated string.
- declare which `WindowChange` event(s) would confirm it completed
  (`ActionKind::matching_window_change_events()`)

`ExecAction::sway_command(token)` renders the launch command instead, taking the per-invocation PID
marker as an argument since it doesn't exist until the action runs. The command itself is passed
through unquoted by design (the tool's whole job is to run it). A separate rendering of
`exec <command>` that nothing ever sent once lived alongside the real one (built inline at the call
site, marker included), so a change made there would have passed its own test and altered nothing;
a code review caught it, which is why the one real rendering lives in `action.rs` with every other
command's.

**`Height`/`Width` hold a `Size` (`Pixels(i32)`/`Percent(i32)`), `Position` holds a `Position`
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
form, `.expect()`-ing that the digits parse as `i32` cleanly. That `.expect()` is only sound
because both validators were tightened in the same change to also reject a value that matches the
`\d+` shape but overflows (e.g. an absurd 11-digit pixel count) — without that, a
value passing validation but failing to parse would panic instead of erroring cleanly, the exact
failure mode this project treats as a bug elsewhere (see the Content/security-review discipline
generally).

`Size`'s bound was `u32` at first, and that was the wrong invariant to promise: `swayipc::Rect` is
`i32`, so `poll_matches()` had to cast (`*pixels as i32`), and a value in `i32::MAX+1..=u32::MAX`
validated, reached Sway as a real `resize set` command, and *then* wrapped negative — accepted by
the front door, unrepresentable by the time anything tried to confirm it. An external review caught
the mismatch. Fixed by moving `Size` itself to `i32` (which deletes the cast rather than relocating
the mismatch) and bounding the validator to match; negative values can't arrive regardless, since
the `\d+` regex rejects the sign. The lesson generalizes: a validator's bound has to be the
narrowest domain anything downstream uses, not the widest one the parser happens to accept — pinned
by `validate_size_argument_accepts_the_largest_representable_pixel_value`/
`validate_size_argument_rejects_one_past_the_largest_representable_value`.

`sway_command()` becomes each variant's serialization point (`Size`/`Position` both
implement `Display`, rendering the identical `<n>px`/`<n>ppt`/`center`/`<x>,<y>` text the CLI/TOML
side accepts; for `Size`, `sway_command_verb()` interpolates that same `Display`, so the
human-readable line and the actual Sway command text stay in sync by construction. `Position`
can't: Sway's `move position` takes space-separated `<x> <y>`, not the CLI's `<x>,<y>`, so
`sway_command_verb()` formats it with its own match, pinned by `action.rs`'s tests), and
`SwayAction::poll_matches()`'s `Height`/`Width` arms match directly on `Size::Pixels`/`Size::Percent`
instead of calling the now-removed `parse_pixel_value()` on every poll iteration.
`LayoutStep`/`TemplateStep` themselves deliberately stayed `Option<String>` — they're TOML-facing
schema types where a plain string is the natural representation and keeps per-field error messages
(`"height: ..."`) exactly as before; only `SwayLaunch`/`SwayAction` needed to be typed for the
stated goal (safer internals, less repeated parsing) to actually land.

`SwayAction::run()` dispatches based on whether the action has a corresponding IPC event
(`ExecAction::run()` always waits on one — see "`Exec`" below):

- **Has an event** (`Exec`, `Floating`, `Fullscreen`, `Focus`, `Workspace`, `Output`, `Mark`,
  `Scratchpad`) →
  `run_wait_matching_events()` for every kind except `Exec` (which is an `ExecAction` and gets its
  own `run_wait_matching_exec_event()`, below): confirms the container still exists, subscribes to the
  event stream, checks `already_at_target()` (see below), sends the command, then reads the event
  stream until a `Window` event matches (checked via `matches_window_event()`, container id for
  every one of these variants) **and** `state_satisfied()` confirms the requested state is actually
  in effect, or the `--timeout` is hit.

  That second check is the difference between "Sway emitted this event type for this container" and
  "what this action asked for is what applied". Container id gives object identity, not causality:
  when something else drives the same window — another `sway-launch`, a keybinding, a `swaymsg` in
  the same script — its events are indistinguishable from ours, so two invocations sending
  `move workspace 2` and `move workspace 3` at once each see a `Move` for their container and, on
  the event alone, both report success while the window is on exactly one of them. `Focus` is the
  most exposed, focus being inherently global. An event that doesn't bring the requested state with
  it is logged under `--verbose` and the loop keeps waiting. Requiring the state rather than
  preferring it is safe because Sway applies a command before emitting its event and serves IPC
  requests in order, so a `get_tree()` issued after receiving the event already reflects it — not a
  settle race, and confirmed by the live suite running in the same 42s it did before the check
  existed. `state_satisfied()` is also what `already_at_target()` now delegates to, so the
  pre-command no-op check and the post-event confirmation can't drift apart; `Mark` is the one
  variant deliberately excluded from the *pre*-command half (re-applying a mark does fire the
  event, so there's no hang to avoid, and short-circuiting would misreport a real re-apply as
  `AlreadySatisfied`). `Workspace` and `Output` both use
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
  scratchpadded window doesn't change anything for Sway to report). `run_wait_matching_events()`
  calls `already_at_target()` to check for
  this and short-circuit with immediate success rather than waiting on an event Sway will never
  send.

  **That check runs after the subscription is open, and the ordering is load-bearing rather than
  incidental.** It reads state another client can change a moment later, and the no-op it exists to
  detect is exactly the case Sway fires no event for — so while the check ran first (in
  `SwayAction::run()`, before dispatching here), a competing `sway-launch`/keybinding/`swaymsg`
  satisfying the state in that gap left this invocation's own command a no-op, no event ever
  arrived, and the action waited out the full `--timeout` before erroring about a state that had
  been in effect the whole time. Not theoretical: measured against a live compositor by sweeping a
  competing `floating enable` across a 0–20ms window after spawn, 5 of 40 trials hung, clustered
  2.5–4ms in (a debug build's own startup puts the gap there); the same sweep against the fixed
  ordering hung 0 of 160. Subscribing first *closes* the window rather than narrowing it — a change
  landing before the check is caught by the check, one landing after it is caught by the
  subscription, and no instant belongs to neither. The cost is one Sway IPC connection opened and
  immediately dropped whenever the short-circuit does fire.

  The `container_exists()` check stays *ahead* of the subscription, though. That ordering used to
  be load-bearing for a second reason, found by moving it: `subscribe()` is itself a
  request/response handshake, and while it ran on an unbounded socket, a wedged compositor hung
  there forever unless a bounded read came first. The launch path had no such read, so
  `sway-launch --timeout 2 foot` against a stalled socket never returned (#51); `event_loop()` now
  bounds the handshake itself (see the `new_connection()` paragraph below), which protects every
  subscriber — launches and `--debug-events` included — and leaves the existence check's position
  about failing fast on a closed window rather than about hangs. The state check must still come
  *after* the subscription.

  Rejected alternative: treating a
  post-command state read as an accepted completion path (an external review's suggestion, and it
  does also close the race) — confirmed live that Sway applies a command before replying to it, so
  that read would succeed on essentially every invocation, making the event wait dead code and
  quietly replacing event confirmation with a single tree read. `SwayAction::run()` therefore no
  longer calls `already_at_target()` at all; every variant it routes elsewhere answers `false` from
  `state_satisfied()` unconditionally, so nothing else changed behavior. Pinned by
  `tests/live_sway.rs`'s `floating_still_succeeds_when_another_client_wins_the_race`.

  `Workspace`/`Output` were the first two found needing this (via the containing workspace/output
  name); `Floating`/`Fullscreen`/`Focus` were found live to have
  the identical failure mode later — re-running `--floating`/`--fullscreen`/`--focus` on a window
  already in that state hung the full `--timeout` and then errored, before this was added (checked
  via the container node's own `floating`/`fullscreen_mode`/`focused` fields). `Floating`'s own
  check is `node_is_floating()`, not `node.floating` alone: a CI-failure investigation found Sway
  1.9 (still what `apt` installs on Ubuntu 24.04/`ubuntu-latest`, confirmed live against a headless
  compositor) never populates a floating container's own `floating` field — it stays `null` even
  though `floating enable` correctly changes the node's `type` to `floating_con` — while Sway 1.11
  populates both, so `node_is_floating()` checks `node_type == FloatingCon` first and falls back to
  the `floating` field. Without this, `already_at_target()` never short-circuited on Sway 1.9,
  reproducing the exact pre-fix hang/error this paragraph describes; confirmed live in both
  directions (broken on 1.9, fixed on both 1.9 and 1.11) during that investigation. `Scratchpad`
  was found to need this the same way, live: re-running `--scratchpad` on an already-scratchpadded
  window fires no event at all. Its own check is `tree_shows_container_in_scratchpad()`, which
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
  (ancestor workspace name) first, initially keeping the `scratchpad_state` read as a secondary OR
  condition — later dropped entirely, because that field means "belongs to the scratchpad", not
  "hidden in it": Sway 1.11 keeps it `fresh`/`changed` while the window is *shown* on an ordinary
  workspace via `scratchpad show`, so the OR made a shown dropdown window look already-hidden and
  `--scratchpad` short-circuited as `already_satisfied` without sending anything. The ancestor
  workspace name alone decides now, pinned by `tree.rs`'s
  `tree_shows_container_in_scratchpad_false_for_a_shown_scratchpad_window` and `tests/live_sway.rs`'s
  `scratchpad_hides_a_window_currently_shown_from_the_scratchpad`. `Mark` was
  checked live too and found *not* to need this — re-applying a mark the container already has
  still fires
  `WindowChange::Mark`. Confirmed by `tests/live_sway.rs`'s
  `workspace_is_a_no_op_when_already_on_the_target_workspace`,
  `output_is_a_no_op_when_already_on_the_target_output`,
  `floating_is_a_no_op_when_already_floating`, `fullscreen_is_a_no_op_when_already_fullscreen`,
  `focus_is_a_no_op_when_already_focused`, and
  `scratchpad_is_a_no_op_when_already_in_the_scratchpad`.

  **`state_satisfied()` fetches; `state_satisfies()` decides.** The first reads one `get_tree()`
  into a `ContainerState` (`workspace`/`output`/`floating`/`fullscreen`/`focused`/`in_scratchpad`/
  `marks` — everything any action needs to answer for, and nothing else) via
  `ContainerState::from_tree()`; the second is pure, takes an `Option<&ContainerState>`, and holds
  the actual per-variant comparisons. Before this split each arm called its own IPC helper
  (`current_workspace()`, `current_output()`, `find_container_node()`,
  `container_is_in_scratchpad()`, all now gone), which put the *interpretation* behind a socket and
  left it reachable only by the live-Sway suite — the same problem `expected_position()`/
  `position_matches()` were already split to avoid, so this follows that precedent rather than
  inventing one (an external review's suggestion, scoped to this function rather than the wholesale
  `sway_client.rs`/`state.rs`/`executor.rs` split it proposed). Two things fell out: every arm is
  now covered headlessly (`state_satisfies_*` in `confirmation.rs`, `container_state_from_tree_*`
  in `tree.rs`),
  measured at +1.3 line/+2.1 function coverage points, and a state check costs one tree read
  instead of one per arm. `None` — the container isn't in the tree — is deliberately never
  satisfied: a closed window isn't floating, focused, or on the target workspace, and
  short-circuiting an action into claiming success over a container that no longer exists would be
  worse than letting its command fail. Prefer this shape for any new state check, the same way the
  Testing bullet under Rust conventions asks for it for any new poll matcher.
- **`Exec`** → `run_wait_matching_exec_event()`. Matching purely on event content (app_id/class, or
  nothing at all with no filter) is ambiguous whenever more than one qualifying `New` window can
  appear around the same time — Sway broadcasts window events to every IPC connection, so a second,
  concurrently-running `sway-launch` process (or any other coincidentally-timed window) can be
  mistaken for the one this invocation itself launched, silently returning the wrong container id.
  To close this, the launched command's environment is tagged with a unique, per-invocation marker
  — this process's own pid plus a nanosecond timestamp, a correlation tag rather than a secret
  (`exec export SWAY_LAUNCH_PID_MARKER=<token> && <command>`, prepended without otherwise touching
  the user's command — an `export ... &&` prefix rather than the original `env VAR=value` one,
  since Sway runs an `exec` line through `sh -c` and `env` can only run an executable, so a command
  starting with a builtin/keyword (`cd dir && app`, `exec app`) launched nothing and timed out;
  confirmed live by `tests/live_sway.rs`'s `exec_runs_a_command_that_starts_with_a_shell_builtin` —
  sending the marker and command as raw, unquoted text like this, rather than
  wrapped in `quote_sway_string()`, was deliberate: Sway's own variable-substitution syntax
  (`$var`) mangles a literal `$` inside a quoted argument, which broke an earlier design that tried
  to capture the spawned shell's own pid via `echo "$$"`), and a content-matching event is only
  accepted outright once `process_has_env_var()` confirms that marker via
  `/proc/<event's pid>/environ`. A content-matching event whose marker doesn't confirm isn't
  rejected outright, though: some applications (browsers, editors) are single-instance and forward
  a second invocation's request to an already-running process before exiting, so the window that
  eventually appears is legitimately the right one, owned by a pid that was never given the marker.
  The first such event is kept as a fallback candidate, used once either the marked process (or a
  marked descendant) is known to be gone — nothing marker-confirmed is coming — or
  `PID_MARKER_FALLBACK_GRACE` (2s; live testing under concurrent load showed a shorter 500ms cap
  occasionally forced a fallback before the real match — still coming, just slightly delayed by
  system load — arrived) elapses, whichever comes first, so a genuinely ambiguous case adds a
  bounded delay rather than the full `--timeout`. Confirmed live by `tests/live_sway.rs`'s
  `concurrent_exec_invocations_do_not_collide_on_the_same_container_id` (0 collisions across 90
  manual trials during development, versus every trial colliding before this) and
  `exec_falls_back_to_a_content_match_when_its_own_process_already_exited`.
  A `New` window the `app_id`/`class` filter rejects isn't silently dropped: it's recorded as an
  `UnmatchedWindow` (con_id, app_id, class, and whether its pid carries this run's marker) and
  logged under `--verbose`. If the wait times out, `exec_timeout_message()` — pure, unit-tested —
  builds the error from what was seen: the rejected windows with a "check `--app-id`/`--class`"
  hint, or, with none, a fresh `scan_marker_presence()` deciding between "the launched command
  exited without opening a window" (check `PATH`) and "still running, may be slow". Before this, a
  typo'd command and a wrong `--app-id` both ended in a bare `N sec timeout reached`. The rejected
  windows also travel structurally: `ExecAction::run()`/`SwayLaunch::run()` return
  `Err(RunError { message, unmatched_windows })` rather than `Err(String)` (`From<String>` covers
  every other failure), and `main.rs`'s `fail_with_rollback()` renders them as the `--json` error's
  `unmatched_windows` field, omitted when empty — they're real windows left open, and a script
  needs their ids without parsing prose. Both `run()`s also prefix every error with what the
  action was (`launch "<command>":` for an `ExecAction`, `sway_command_verb()` otherwise), so a timeout
  in a multi-action invocation or a layout's `step N: ...` names the action that failed. Covered
  live by `launch_timeout_names_the_window_its_app_id_filter_rejected` and
  `launch_timeout_says_when_the_command_exited_without_a_window`, and headlessly (the prefix) by
  `tests/json_output.rs`'s `a_failing_action_names_itself_in_the_error`.
  Which of the two ends the wait is reported back as a `LaunchOwnership` (`Launched` for the
  marked-process-gone branch, `Adopted` for the grace-elapsed one), the distinction
  `--rollback-on-error` keys its destructive cleanup off — see its own section below.

  **"Gone" means seen, then gone — never merely absent.** `process_marker.rs`'s
  `scan_marker_presence()` scans `/proc` for the marker every `MARKER_SCAN_INTERVAL` (25ms) from the
  moment the command is sent until it's been seen, and, while a fallback candidate is held, until
  it disappears; `MarkerTracker` accumulates the answers and only reports `is_gone()` once a scan
  that found the marker is followed by one that doesn't. The first version read a single "no
  process carries the marker" scan, taken when a fallback candidate arrived, as "the marked process
  exited", and that's wrong in three ways: before the marked process exists at all (Sway's `exec`
  returns once it has forked), for a launcher that drops the environment (`systemd-run`, an app
  launcher, `env -u`), and when `/proc` can't be read. Each claimed whichever matching window
  arrived first as `Launched` — every follow-up action applied to someone else's window, and
  `--rollback-on-error` killing it. The first case wasn't exotic: two concurrent `--template quad-grid
  --apps foot,foot,foot,foot` runs reported a shared container id in 4 of 7 trials, each run having
  taken the other's `New` window during its own startup gap. A marker that's never seen now always
  ends through the grace period, as `Adopted`. Pinned deterministically by
  `rollback_on_error_leaves_a_window_opened_while_an_env_dropping_wrapper_waits`; the concurrent
  case by `concurrent_template_runs_report_disjoint_container_ids`, which is timing-dependent and
  didn't reproduce the old bug in its three trials when tried, so it guards the invariant rather
  than the rule. The residual case is a launcher that is seen carrying the marker, exits, and only
  then has its real window appear: indistinguishable from a single-instance forwarding client by
  the marker alone, and treated the same way, as `Launched`.

  The scan is tri-state per process (`read_process_marker()`: `Present`/`Absent`/`Unreadable`),
  because a failed `environ` read isn't evidence of absence — a non-dumpable process
  (`prctl(PR_SET_DUMPABLE, 0)`, setuid/setgid, file capabilities) has a root-owned `environ` even
  for its own user. Only `NotFound`/`ESRCH` count as gone. `classify_marker_reads()` lets an
  unreadable process block an `Absent` answer (`MarkerPresence::Unknown`) only when it's one already
  seen carrying the marker; counting *every* unreadable process would make `Absent` unreachable on
  an ordinary desktop, where root's processes and the user's own ssh-agent/gpg-agent are unreadable
  on every scan. A `read_dir("/proc")` failure is `MarkerPresence::Unavailable`: logged once under
  `--verbose`, scanning stops, and the launch ends as `Adopted` — which is what README.md promises
  for a system without `/proc`. Not unit-tested: reliably making `/proc` unreadable from within a
  test isn't portable/safe to simulate. A forwarding client that lives less than one scan interval
  is never seen and so is adopted after the grace period rather than claimed — the safe direction
  to be wrong in.
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

  It's called **twice**, on either side of the pre-command sleep, and both calls earn their keep.
  The one after the sleep is the correctness check above: it's what guarantees the container is
  still there at the moment the command is actually sent, covering a window that closes *during*
  the wait. The one before it is a fast fail, added after an external review pointed out that
  `--wait-time` is an unbounded caller-supplied number slept *before* anything is validated — so a
  step with `wait_time = 3600000` against an already-closed window sat for an hour before reporting
  a foregone answer. Declined in the same review: capping `--wait-time`. It's the caller's own
  knob, `--timeout` isn't policy-capped either (its only bound is representability), and any
  specific ceiling would be arbitrary; the real complaint was the pointless wait, which the early
  check removes. Pinned by `tests/live_sway.rs`'s
  `wait_time_action_does_not_sleep_out_the_wait_before_noticing_its_container_is_gone`, alongside
  the original `wait_time_action_errors_clearly_when_its_container_already_closed` for the
  post-sleep half. Note that
  `rollback_on_error_handles_a_launched_window_that_already_closed_itself` depends on the *second*
  check specifically (its window is still alive when the first one runs, and closes during the
  1.2s wait) — so removing either one breaks a test that documents why the other exists.

  After sending the command, what happens next depends on `SwayAction::polls()`/`poll_matches()`,
  which between them cover every one of these seven variants (per
  `docs/plan-poll-based-wait-time-actions.md`, now fully landed). The two are deliberately split:
  `polls()` is the pure "does this variant confirm by polling at all" decision (no IPC, so
  `run_wait_time()` can ask before opening a connection, and so it stays unit-testable headlessly),
  while `poll_matches()` is the tree read itself and takes a `&mut Connection` from its caller
  rather than opening one. That connection is created once per action in `run_wait_time()` and
  shared by the baseline snapshot and every poll iteration — previously each iteration opened one
  or two fresh connections (`Position` two: `get_tree()` plus `get_outputs()`), so a single action
  could cost ~40 connects and handshakes inside the 200ms grace window. Failing to open it isn't
  fatal: there's simply nothing to poll with, so the action falls back to the unconditional sleep
  exactly as a variant with no matcher does. `run_wait_time()` hands off to
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
    entirely (`polls()` returns `false`) since there's no pixel figure to poll for without also
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

    **This is the project's weakest confirmation predicate, and it's weak on purpose — but it's
    bounded to `MOVE_POLL_GRACE` (25ms) rather than the usual 200ms.** "The rect changed" is
    evidence that geometry changed, not that *this command* changed it: if the move was a no-op and
    another client resizes or moves the same window while the poll is still looking, that change
    gets credited here. An external review raised this, correctly, and its suggested fix —
    snapshotting parent/sibling index and requiring a direction-consistent structural transition —
    was measured against a live compositor and rejected: a genuine `move down` left the target's
    parent *and* index unchanged (Sway wrapped the *sibling* in a new container and resized the
    target in place, 640x695 → 1280x335), so that predicate false-*negatives* on a real move, the
    mirror image of the children-id-list attempt above. Of everything tried, `rect` is the only
    signal that separated all four cases (real `move right`, real `move down`, and both edge
    no-ops). The review's own fallback — always report `Unconfirmed` — would discard a signal
    that's correct in essentially every real case.
    So the exposure is narrowed instead of closed: the same measurements found a real move's `rect`
    change visible on the very first tree read after Sway's reply (0.29-0.59ms across five moves,
    one 3.42ms outlier), so 25ms keeps ~7x margin while cutting the window ~8x. Note this only
    bites when `--wait-time` exceeds it — the grace is `min`'d with `--wait-time`, and the CLI
    default is 20ms, so default invocations were never polling for longer than that anyway. The
    two fast-path tests above assert `"status":"changed"` explicitly (not just elapsed time), which
    is what would catch this bound being tightened past what a real move can meet.

### Orchestration: `SwayLaunch::build_actions()`/`run()`

`SwayLaunch` no longer always launches a new window — it has a `target: Target` field
(`Target::Exec { command }`, `Target::ConId(i64)`, or `Target::Existing`), and
`resolve_container_id()` turns whichever one is set into a `container_id`:

- `Exec` — today's original behavior: build and run an `ExecAction`.
- `ConId(id)` — the id directly, no IPC call at all.
- `Existing` — `get_tree()` + `matching_container_ids()` (a recursive walk over `Node::nodes`/
  `Node::floating_nodes`, reusing `window_app_id_match`/`window_class_match` — refactored to take
  `&Node` instead of `&WindowEvent`, since `WindowEvent.container` already *is* a `Node`) +
  `resolve_matches()`, which errors on zero or more than one match rather than guessing. The walk
  covers the entire tree returned by `get_tree()`, which includes the `__i3_scratch` scratchpad
  workspace — a hidden scratchpad window matching the criteria is just as eligible as a visible
  one (documented for the user in README.md's "Target an existing window" section).
  `matching_container_ids()`/`find_existing_container_id()` take a third `mark_match: &str`
  alongside `app_id_match`/`class_match` — an external review suggested this (`--mark-match
  <MARK>`, matching a container's own `marks: Vec<String>` via a new `window_mark_match()`, the
  same shape as `window_app_id_match`/`window_class_match`) as a way to retarget a previously-
  `--mark`ed window without tracking its container id, the classic "dropdown terminal" pattern
  (`sway-launch --existing --mark-match dropdown-term --scratchpad`). The three criteria are
  mutually exclusive by construction (`--mark-match` sits in `--app-id`/`--class`'s
  `conflicts_with_all` the same way those two already conflict with each other, and the matching
  `LayoutStep`/`Binding` fields enforce the same three-way exclusivity `to_sway_launch()`/
  `resolve()` already enforced for `app_id`/`class`), but `matching_container_ids()`'s own
  precedence chain (app_id, then class, then mark) still needs a defined order regardless — the
  same `if/else if/else` shape `window_app_id_match`/`window_class_match`'s existing precedence
  already used, just extended by one arm. `SwayLaunch` gained a matching `mark_match: &'a str`
  field alongside `app_id_match`/`class_match`, threaded through the same call sites; `--mark-match`
  only ever applies to `Target::Existing` (a freshly launched `Target::Exec` window has no mark yet
  to match against, since marks are applied *after* launch via `--mark`), so `ExecAction`'s own
  content-matching is untouched.

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
change shape, only *when* each `SwayAction` gets constructed. Each flag now contributes only an
`ActionKind` to a fixed-order list; one `SwayAction::new()` call adds the shared container id,
`--verbose` and timing, so no branch restates them. This is also what makes
`build_actions()` itself unit-testable headlessly for the first time (see its tests in
`launch.rs`, `build_actions_includes_every_flag_in_the_documented_fixed_order` in particular,
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
`new_column_and_new_row_output_guard_still_applies_when_combined_with_output` (the latter covers
both `--new-column`+`--output` and `--new-row`+`--output` in one test, against one created
output).

Before including `NewColumn`/`NewRow` in the plan at all, `build_actions()` calls
`relocates_to_another_output(container_id, direction)`, which checks `get_outputs()` (skipping the
guard entirely when there's only one output) and, if more than one exists, `get_tree()` +
`is_at_the_trailing_workspace_edge()` to see whether `container_id` is at risk of the relocation
described above. This originally checked only "is `container_id` the only window in its
workspace" — live testing found that too narrow: a *non-solo* workspace can escalate too, whenever
`container_id` is already the trailing child of a workspace whose own `layout` already matches the
move's axis (confirmed live: two windows side by side, `move right` on the rightmost relocated it
to a different output, not a same-workspace no-op, even with a sibling to its left). A later
"nested windows are safe" rule turned out wrong too: a window that is the *only* child of a
sub-container sitting directly on the workspace escalates as well (found by a review, reproduced
live with a `layout tabbed` wrapper, a `splitv` wrapper left with one child, and — the widest
reach — every first window under `workspace_layout tabbed`/`stacking`, where the guard used to
never fire at all). So the check no longer approximates Sway's behaviour with a shape rule; it
**replays Sway's own `container_move_in_direction()` walk** (`sway/commands/move.c`) over the tree:
climb from the container to the first ancestor whose parent is laid out along the move's axis
(`tabbed` counting as horizontal and `stacked` as vertical, as Sway's `is_parallel()` does —
reaching the workspace without one means Sway re-orients the workspace, never escalating); the
container itself being the trailing child of a parallel workspace escalates; a container at the end
of its own parallel parent climbs once more; and an ancestor with nothing after it means Sway
promotes the container next to it, *except* for the singleton case above, which Sway (mimicking
i3) sends to the next output. A deeper lone child — two wrappers down — is promoted, not escalated,
per the source (not separately probed live); the unit tests in `tree.rs` pin each branch. A trailing child of a
sub-container *with* a sibling still pops out onto the same output (probed live in both an
axis-mismatched and the axis-matched worst case), so the replay correctly lets it run. Modelling
the source rather than adding a third shape rule is deliberate: both earlier rules were reasonable
generalizations from the cases tried live, and both missed a branch the source spells out.
Floating windows are never flagged, since Sway won't move them out of their floating parent.
When flagged, the action is skipped (logged under `--verbose`) rather than run, trading a silent
cross-monitor relocation for a silent no-op — confirmed by `tests/live_sway.rs`'s
`new_column_does_not_relocate_a_solo_window_to_a_different_output`,
`new_column_does_not_relocate_a_non_solo_window_at_the_trailing_edge`, and (both nested cases —
with a sibling, run; as a lone child of a `tabbed` or `splitv` wrapper, skipped — in one test)
`new_column_does_not_relocate_a_nested_window_to_a_different_output`. A skip is also structurally
reported, not just logged: `build_actions()` returns `Vec<PlannedAction<'a>>` (`enum PlannedAction
{ Run(SwayAction<'a>), Skip(SkippedAction) }`) instead of a bare `Vec<SwayAction<'a>>`, interleaving
a skip in its actual fixed-order position rather than collecting skips into a separate side list —
`SkippedAction { kind, reason }` holds short, stable, machine-readable `&'static str` identifiers
(`ActionKind::flag_name()`'s `"new_column"`/`"new_row"`, `"trailing_workspace_edge"`), not a human-facing message (that's still
what the `--verbose` `diagnostic!` next to each skip is for). `run()` folds each `PlannedAction` into
an `ActionRecord { kind, command, status }` (`ActionStatus::Changed`/`AlreadySatisfied`/`Skipped { reason
}` — see "`--json`'s richer schema" below for the full design of this, added later alongside the
`already_satisfied` distinction), so a `PlannedAction::Skip` never actually calls `.run()` — it
short-circuits straight to an `ActionStatus::Skipped` record. Previously a skip was visible only via
a `--verbose` log line, invisible to a `--json` caller; surfacing it in `RunOutcome`/`--json` closes
that gap without changing the log line at all. `build_actions_for_preview()` (`--dry-run`) filters
down to just the `Run` half — always the *only* half there anyway, since `check_relocation: false`
never produces a `Skip`. Only one skip mechanism exists today, so `SkippedAction.reason` is a plain
`&'static str` rather than an enum; revisit if a second one ever appears.
`step_action_records_json()`'s step-number arithmetic (`step_index + 1`) is unit-tested headlessly
with a synthetic `ActionRecord` (`main.rs`'s
`step_action_records_json_tags_each_entry_with_a_1_based_step_number`) rather than only via a real
skip, which needs a live multi-output Sway session to trigger at all — isolating the off-by-one
risk from the live-only trigger condition. Confirmed live end to end (a real skip surfacing
correctly in real `--json` output) by extending
`new_column_does_not_relocate_a_solo_window_to_a_different_output` itself. Every such test creates
its own output through the `CreatedOutput` guard (see `tests/live_sway.rs` above) rather than
reusing one: the guard's escalation is directional (whether `move right` actually crosses into
another output depends on Sway's own output arrangement and which output is currently focused), so
an output some unrelated test left behind didn't reliably reproduce the scenario a dedicated,
freshly created one does — confirmed by first trying exactly that and observing the guard silently
not firing.

**`new_connection()` builds the socket itself rather than calling `Connection::new()`**, so it can
set a read and write timeout (`IPC_ROUND_TRIP_TIMEOUT`, 10s) on it — `swayipc` exposes no way to
configure its own connection, but it does implement `From<UnixStream>`. Without this, `--timeout`
bounded only the wait for a confirmation *event* and never the IPC round trips around it, so a
compositor that accepted a connection and then stopped answering hung indefinitely: confirmed by
pointing `SWAYSOCK` at a socket that accepts and never replies, where
`--con-id 42 --floating --timeout 2` ran until killed externally at 15s (the first thing it does is
read the tree). Socket discovery is `socket_path()`: `SWAYSOCK`, then `I3SOCK` — deliberately the
reverse of `swayipc`'s own order and the same as `swaymsg`'s. Sway exports both, pointing at the
same socket, to everything it spawns, so overriding `SWAYSOCK` alone is how a user (or
`scripts/run-live-sway-tests`) aims a Sway tool at a different compositor; with `I3SOCK` first that
override was silently ignored and this tool drove the terminal's own session instead. Pinned by
`ipc.rs`'s `preferred_socket_path_*` unit tests and `tests/live_sway.rs`'s
`swaysock_wins_over_a_stale_i3sock`. When neither is set it falls back to `Connection::new()` —
and its `i3 --get-socketpath`, then `sway --get-socketpath` steps; that fallback is unbounded,
accepted because it only applies when the environment names no socket at all. A connection that
can't be made at all is reported by `connect_error_at()` (the path, which variable it came from,
and "is this running inside a Sway session?") or, with neither variable set,
`undiscovered_socket_error()` — both shared by `new_connection()` and `event_loop()`, so a launch
(which subscribes before any other IPC call) words it the same as everything else. Outside a Sway
session the raw errors never mention Sway: `No such file or directory (os error 2)`, or `Invalid
argument (os error 22)` from `swayipc` connecting to the empty path a failed `sway
--get-socketpath` printed. Pinned by `tests/json_output.rs`'s
`a_launch_with_an_unreachable_socket_names_sway_ipc_and_the_socket`/
`a_launch_with_neither_socket_variable_set_says_so`. The bound is deliberately *not* `--timeout` and not derived from it: `--timeout` waits on an
application mapping a window (seconds are reasonable), while this bounds one request/response
exchange (sub-millisecond on a healthy system), so tying them together would make `--timeout 1`
break ordinary tree reads and `--timeout 60` restore a minute-long hang. `ipc_error()` maps the
resulting `WouldBlock`/`TimedOut` into a message naming the condition, since the raw form is
`Resource temporarily unavailable (os error 11)`. `event_loop()` applies the same timeout to the
`subscribe()` handshake only, then clears the read timeout before handing the stream to the reader
thread: a subscription's job is to block until an event arrives, so a read timeout would turn a
normal quiet period into an error, and the reads need no bound because the caller collects from
the reader thread with `recv_timeout()`. The handshake, though, is an ordinary request/response on
the calling thread, and leaving it unbounded made every launch hang forever against a stalled
socket, since subscribing is the first IPC call a launch makes. The timeout is cleared through a
`try_clone()` of the stream, taken before `swayipc` consumes the original: socket options belong to
the socket, not the descriptor, so clearing it on the clone clears it for the subscription too. Both
resolve the socket through the same `socket_path()` (via the private `connect_stream()`), so the
subscription and the requests around it can never reach different compositors. Covered by
`tests/live_sway.rs`'s `a_stalled_sway_socket_fails_instead_of_hanging_forever` (which needs no
compositor, but lives there because it costs ~10s per invocation, and drives the request path,
a plain launch and `--debug-events`) and `ipc.rs`'s `ipc_error` unit tests.

Each Sway IPC call opens its own fresh `Connection` (`new_connection()` in `sway_launch/ipc.rs`) —
there
is no persistent/shared connection across actions. Two exceptions, both places where more than one
read answers a single question:

- A wait-time action's poll cycle: `run_wait_time()` opens a single `Connection` and passes it to
  `poll_baseline()` and every `poll_matches()` iteration (see the "No event exists in Sway IPC for
  it" bullet above), because that's the only place the same read repeats up to 20 times inside
  200ms. `poll_matches()` and the tree/output helpers it calls (`parent_node_layout`, `node_by_id`,
  `position_matches`, `node_and_output_name`, `output_rect`) therefore take `&mut Connection`
  rather than opening their own.
- `relocates_to_another_output()`: its `get_outputs()` and `get_tree()` calls are two halves of one
  check, so they share a connection too. Cheaper than the poll cycle's case (there's no loop to
  amplify it, and the `outputs.len() < 2` early return means the tree fetch only happens on a
  multi-output setup at all), but there's no reason for the two to differ.

Everything else still opens its own.

### Output streams

stdout is reserved for that one result value (bare id or the `--json` object) and nothing else, so
`container_id="$(sway-launch ...)"`-style capture always gets exactly one clean line. Every
diagnostic/debug line behind `if self.verbose()` goes to stderr for this reason, through the
crate-wide `diagnostic!` macro (defined at the top of `main.rs`, ahead of the `mod` declarations so
every module sees it) rather than `eprintln!`. `eprintln!` panics when stderr is a closed pipe, so
`sway-launch -v ... 2>&1 | head -n1` used to exit 101 on the next diagnostic — between two actions,
leaving the window half-arranged. `diagnostic!` drops the write error instead: a diagnostic nobody
can read is no reason to stop the work it describes. Every stderr write in the crate, error
messages included, goes through it; don't reintroduce `eprintln!`. Two exceptions to stdout's
one-value rule:
`SwayLaunch::debug_events()`'s event dump and `--dry-run`'s planned-command listing are each the
command's actual output in that mode, so both stay on stdout — neither is meant to be captured as a
single clean value the way a real run's container id is, so the "exactly one line" property simply
doesn't apply to them.

Every stdout write goes through `main.rs`'s `print_line()`/`print_str()` (and, for
`SwayLaunch::debug_events()`'s own dump, the same `writeln!`-plus-`BrokenPipe` handling inline)
rather than `println!`. Rust ignores `SIGPIPE`, so a `println!` into a closed pipe panics
("failed printing to stdout: Broken pipe") and exits 101 — a poor answer to
`sway-launch --debug-events | head -5`, which is an ordinary way to use the one mode that writes
until killed. `exit_on_write_failure()` treats `ErrorKind::BrokenPipe` as a clean `exit(0)` (the
reader got what it asked for and went away) and any other write failure as a reported `exit(1)`.
Handled this way rather than by restoring the default `SIGPIPE` disposition, which would mean an
`unsafe` libc call and a new dependency for something this narrow. `--completions` is the one
stdout path not routed through these, since `clap_complete::generate()` does its own writing.

That immediate exit is only right where the output *is* the job (`--debug-events`,
`--list-templates`, `--show-template`, `--dry-run`, `--validate`, and a single invocation's id,
which is printed after its work is already done). `run_steps()`'s per-step ids are a side channel
to work still in progress, so they go through `print_line_deferring_failure()` instead: a failed
write is recorded, nothing more is printed, every remaining step still runs, and only then is the
recorded error handed to `exit_on_write_failure()` — exit 0 for `BrokenPipe` once every step has
succeeded, 1 for any other write failure, and a step failure still wins over either. Before this,
`sway-launch --layout four.toml | head -n1` opened three windows, never ran step 4, and exited 0
(with `--rollback-on-error` never firing, since nothing counted as a failure). Covered headlessly
by `tests/layout.rs`'s `layout_keeps_running_its_steps_after_stdout_is_closed` and
`tests/json_output.rs`'s `a_closed_stderr_does_not_abort_a_verbose_run`, and live by
`tests/live_sway.rs`'s `a_layout_finishes_every_step_when_its_output_pipes_are_already_closed`.

Errors always go to stderr regardless of `--json` (`main.rs`'s `fail()`/`fail_with_rollback()`), but
their *shape* on stderr does follow `--json`: a `{"error": "...", "rolled_back": [...]}` object
instead of a plain-text line, mirroring the structured success shapes below rather than leaving a
`--json` caller to also parse plain-text stderr on failure. In plain output the message is prefixed
`error:` (and `rollback()`'s per-kill failure `warning:`) via `severity_label()`, matching clap's
own usage-error labels so the line that stopped a script stands out next to `--verbose` noise; the
label is bold red/yellow only when `stderr_wants_color()` holds (stderr is a terminal and
`NO_COLOR` is unset or empty), written as raw ANSI escapes with `std::io::IsTerminal` rather than
by taking `anstream`/`anstyle` on as direct dependencies for two fixed strings. The JSON object is
never labelled or colored. `rolled_back` is only ever non-empty
when `run_steps()`'s `--rollback-on-error` (see "`--layout`" below) actually killed something
first — every other error path passes an empty `RollbackOutcome`. That type carries the kills that
*failed* separately (`rollback_failed` in `--json`, present only when non-empty; a summary line in
plain output, after the per-failure warnings `rollback()` already emitted): "closed" and "tried to
close and couldn't" are different facts, and collapsing them loses information either way — a
failed kill usually means the window had already closed on its own, but this run can't confirm
that, so reporting it as still-open progress would assert something untrue while dropping it would
hide a window the caller may still need to deal with. `fail_step()` therefore filters *every*
attempted id out of `PartialProgress`, not just the successful ones, leaving the three lists
non-overlapping: `container_ids` open, `rolled_back` closed, `rollback_failed` unknown. Only
reachable with a real compositor (a step must have genuinely launched a window for it to be
rollback-eligible at all), so it's covered by `tests/live_sway.rs`'s
`rollback_on_error_handles_a_launched_window_that_already_closed_itself`. A failing multi-step run's error object also
carries `container_ids`/`containers` for the steps that *did* complete (`main.rs`'s
`PartialProgress`, threaded from `run_steps()` through `fail_step()` into `fail_with_rollback()`):
plain output prints each id as its step finishes, so a mid-layout failure there still leaves the
caller able to clean up, while `--json` collects them for one object at the end and so previously
reported nothing but the error with real windows left open on screen. Anything `--rollback-on-error`
just killed is filtered back out (reporting a closed window as still-open progress would be worse
than reporting nothing), and both fields are omitted entirely for a single invocation, which has no
partial state to report. This is deliberately scoped to *runtime*
failures (file I/O, TOML parsing, a `SwayLaunch`/step actually failing against Sway) — a bad CLI
invocation (missing/conflicting flags, `Args::command().error(...).exit()`) is still reported via
clap's own usage-error formatting regardless of `--json`, the same as `--help` itself isn't JSON
either.

### `--json`'s richer schema

An external review suggested expanding `--json`'s success shapes beyond a bare id/id-list — since
no version has ever shipped (`Cargo.toml` is still `0.1.0`, no `v*` tags exist), there was no
existing consumer to stay compatible with, so this was a free redesign rather than an additive one.

`SwayLaunch::run()` itself now returns `Result<RunOutcome, RunError>` instead of `Result<i64,
String>` — `RunOutcome { container_id: i64, actions: Vec<ActionRecord>, launch_ownership }`
(`launch_ownership` is covered under "`--rollback-on-error`" below). Because `run()` still
stops and returns `Err` at the first action that fails (unchanged), a successful `Ok(RunOutcome)`'s
`actions` is always the *complete* planned list, never a partial one — a failed action never
produces an `ActionRecord` at all; it's `run()`'s `Err` instead, exactly as before this existed.
Plain (non-`--json`) output is unaffected — still just the bare `container_id`.

**Per-action status (`changed`/`already_satisfied`/`unconfirmed`/`skipped`), not just
present-or-absent.** A
later external review pointed out that the first version of this schema (a single invocation's
`--json` output as `{"container_id": N, "actions": [...], "skipped": [...]}`, `actions` a bare
`Vec<String>` of `sway_command_verb()` text) couldn't distinguish an action that actually changed
something from one `already_at_target()` (see above) silently no-oped — both looked identical in
the `"actions"` array, and a skip lived in a separate `"skipped"` list with no way to tell where it
would have fallen relative to the other actions. Fixed by unifying both into one `Vec<ActionRecord>`
(then `ActionRecord { action: String, status: ActionStatus }` — `action` later split into
`kind`/`command`, see below — and `enum ActionStatus { Changed,
AlreadySatisfied, Unconfirmed, Skipped { reason: &'static str } }` — `Unconfirmed` came later, see
below), replacing `RunOutcome.actions: Vec<String>`
and dropping `RunOutcome.skipped` entirely — its contents fold into the same list instead.
`SwayAction::run()`'s own return type changed from `Result<i64, String>` to one carrying an
`ActionOutcome` (today `Result<ActionOutcome, RunError>`) specifically so `run()` can read back distinctions the
action layer already computed internally but previously discarded — first `already_at_target()`'s,
and later the poll/fallback split. `ActionOutcome` (`Changed`/`AlreadySatisfied`/`Unconfirmed`) is
separate from `ActionStatus` because a skip is decided while *planning* and never reaches `run()`
at all. **`Unconfirmed` is what a wait-time action reports when it sent its command and the wait
elapsed without the change ever being observed** — the poll grace period lapsing, a variant with no
matcher (a `ppt` size has no pixel figure to poll for), or the poll connection failing to open.
Deliberately not an error: several of these have legitimate outcomes where the expected state never
arrives (a solo window's resize clamp, a move already at the workspace edge), so failing on them
would break working layouts. But it is a weaker claim than `Changed`, and folding the two together
made a wait-time action's "success" quietly weaker than an event-confirmed one's — an external
review's point, and a fair one. Covered live on both sides by
`height_confirms_via_poll_when_resized_with_a_sibling` (`"changed"`) and
`height_and_width_fall_back_gracefully_when_solo_window_clamps_the_resize` (`"unconfirmed"`),
which run the same command against a window with and without a sibling. `run()` builds each
`ActionRecord` directly from a `PlannedAction` (see above) — `Run` becomes `Changed`/
`AlreadySatisfied`/`Unconfirmed` from the `ActionOutcome` it returned, `Skip` becomes `Skipped { reason }` directly, with no
`.run()` call at all. `main.rs`'s `action_record_json()` renders one record as
`{"kind": ..., "command": ..., "status": "changed"|"already_satisfied"|"unconfirmed"|"skipped"[, "reason": ...]}` —
`"command"` absent and `"reason"` present only for `"skipped"`. A single invocation's `--json`
output is now `{"container_id": N, "actions": [...]}` (`actions_json()`).

**`"kind"` is an action's identity; `"command"` is only what it ran.** A single `"action"` field
used to carry both: `sway_command_verb()` text for an action that ran (`"move right"`,
`"mark \"pinned\""`, `"resize set width 400px"`), and the flag name for a skip (`"new_column"`). So
the same flag answered to two names depending on what happened, the run-side name embedded user
data and Sway quoting, and it changed whenever command rendering did — a consumer had to parse Sway
syntax to ask "did my `--new-column` apply?". `ActionRecord` now carries `kind: &'static str`
(`ActionKind::flag_name()`: the CLI flag in `snake_case`, the same value a skip reports, since a
skip is decided per kind) and `command: Option<String>` (`None` for a skip, which never rendered
one). `--dry-run --json` emits the same `{"kind", "command"}` pair per action, so a preview and a
run compare record for record. Pinned headlessly by `action.rs`'s
`flag_name_is_the_snake_case_cli_flag_and_ignores_the_value` and `tests/json_output.rs`'s
`dry_run_json_names_every_action_by_its_flag_not_its_command`, which sets every action flag.

This was extended to the multi-step case in the same change: `run_steps()` (`--layout`/
`--template`) previously had no `"actions"` field in its own `--json` output at all (only
`"container_ids"`/`"containers"`/`"skipped"`) — `outcome.actions` was fetched per-step but
discarded, an inconsistency found while reworking this schema rather than a separate suggestion.
`step_action_records_json()` (renamed from `step_skipped_actions_json()`, same `"step"`-tagging
shape) now tags every action, not just skips, with its 1-based step number, accumulated into a
top-level `"actions"` array alongside the existing `"container_ids"`/`"containers"` fields.

`run_steps()` (`--layout`/`--template`) separately already tracked a `resolved_ids:
HashMap<String, i64>` of every named step's (`id`, or a template `slot`, which resolves to the same
name — see "`--template`" below) container id, purely to resolve later `target_id` references —
that same map is serialized directly as `--json`'s `"containers"` field, alongside the
`"container_ids"` array (every step's id positionally, named or not). No new bookkeeping was needed
for this specifically because `resolved_ids` already existed for an unrelated reason.

### `--debug-events`

`SwayLaunch::debug_events()` subscribes to all Sway IPC event types and prints every event until
killed. Useful for discovering event shapes when adding a new action.

"All" is a hand-maintained list, `launch.rs`'s `DEBUG_EVENT_SUBSCRIPTIONS`, not something derived
from `swayipc`: its `EventType` is `#[non_exhaustive]`, so no match outside the crate can be
exhaustive and a variant added upstream compiles here silently. `Output` was missing from it for a
while, so output events never appeared. `debug_event_subscriptions_cover_every_sway_ipc_event_type_once`
pins the list against Sway's own documented event names, so recheck both whenever `swayipc` is
bumped.

It's a standalone mode like `--completions`/`--list-templates`: it never acts on a window, so it
`conflicts_with_all`-conflicts with the positional command, `--con-id`/`--existing`, and every
per-window flag, rather than accepting and silently discarding them (`sway-launch --debug-events
foot --floating` used to parse clean, dump events, and never launch `foot`). Unlike those other
standalone modes it still builds a `SwayLaunch` first, so it isn't checked until after that; the
`!args.debug_events` exemption from `main()`'s "Missing COMMAND" check stays too, since a bare
`--debug-events` legitimately has no target. `--timeout`/`--wait-time` remain accepted but unused
here, the same as for the other standalone modes.

**Every standalone mode's per-window conflicts come from one list.** `main.rs`'s `PER_WINDOW_ARGS`
names every flag that selects or acts on a window, and `--debug-events`, `--completions`,
`--layout`, `--template`, `--list-templates`, and `--show-template` each build their
`conflicts_with_all` as `per_window_and(&[...their own extras...])`, rather than six hand-copied
lists where missing one entry silently reintroduces the accept-and-discard bug above. Two tests
keep it honest: `per_window_args_lists_every_flag_a_layout_step_mirrors` fails if a flag with a
`LayoutStep` equivalent is missing from the list (`timeout`/`wait_time` are the deliberate
exceptions, in `MIRRORED_DEFAULTS`), and `per_window_args_conflict_with_every_standalone_mode`
asks clap itself whether each mode conflicts with every entry. A clap `ArgGroup` was tried first and
rejected: clap reports a conflict with a group by listing every member of the group instead of the
flag actually passed, which made `--layout x.toml --floating`'s one-line error a 20-line one.

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
itself just `format!("[con_id={}] {}", self.container_id, self.sway_command_verb())`), not
`sway_command()`, so the
placeholder id is constructed but never rendered anywhere.

`main.rs`'s `DryRunStep { target, actions }` is the shared preview representation for both the
direct-CLI path and `run_steps_dry_run()` (`--layout`/`--template`, dispatched from `run_steps()`
before any real execution begins) — `describe_target()` renders `SwayLaunch.target` as `"launch
<command>"`/`"target existing container"`/`"target existing window (app_id=...)"`, deliberately
never naming a container id even for `Target::ConId` (its id *is* already known, but showing it on
that one target mode while every action line stays id-free would be an inconsistent preview
format). `run_steps_dry_run()` still needs `to_sway_launch()`'s `target_id` lookups to resolve, so
`plan_steps_offline()` (shared with `--validate`, see below) inserts a step's own index into
`resolved_ids` as a synthetic placeholder wherever a step has an `id` — never rendered either, same
reasoning as `container_id: 0`. Plain output is one
continuously-numbered line per target/action across every step (matching the external review's own
illustrative example); `--json` is a single `{"steps": [{"target": ..., "actions": [{"kind": ...,
"command": ...}, ...]}, ...]}` object (`DryRunStep.actions` holds `DryRunAction { kind, command }`,
the same pair a real run's `ActionRecord` reports).

### `--validate`

Parses and validates a `--layout`/`--template` file without launching anything or touching Sway
IPC, exiting 0 (`valid: <source> (N step(s))`, or `{"source": ..., "steps": N, "valid": true}`
under `--json`, `<source>` echoing the `--layout`/`--template` argument as given) or 1 (the same
`step N: <message>`/structured-`--json`-error shape every other runtime failure uses). Requires
`--layout` or `--template` (the manual `Args::command().error(...)` check mirrors
`--rollback-on-error`'s existing one exactly, same reasoning: this needs to fire in `main()`'s
direct-CLI fallthrough path, since `--layout`/`--template` themselves always exit before reaching
it).

Turned out to need less new machinery than `--dry-run`: `LayoutStep::to_sway_launch()` already
*is* the validation — height/width/position formats, target-field consistency, `target_id`
resolution — with no Sway IPC call of its own (only `SwayAction::run()` touches the socket), so
`run_steps_validate()` is just `run_steps()`'s planning with the `.run()` call dropped entirely:
convert every step, propagate the first error, done.

**All three modes plan a step through the same `plan_step()`** (the duplicate-`id` check, then
`to_sway_launch()` against the ids resolved so far); `--dry-run` and `--validate` reach it via
`plan_steps_offline()`, which loops over every step with placeholder ids, and `run_steps()` calls it
per step between real runs. They used to carry three hand-written copies of that loop, which had
already started to drift in how they reported errors — and a check added to one copy but not
another would let `--validate` call a file valid that a real run rejects. `step_error()` is the one
`step N: <message>` formatter, used by `plan_steps_offline()` and `fail_step()` alike. A new
per-step rule belongs in `plan_step()` (or `to_sway_launch()`), never in one mode's loop. A `--template`'s own
`--bindings`/`--apps` resolution (slot count, duplicate slots, binding correctness) already
happens unconditionally before `run_steps()` is ever reached (`run_template()`'s own
`template::resolve()` call) — by the time `--validate` sees `steps`, that part already succeeded,
so there's nothing left for it to repeat.

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

`run_steps()` errors (`no steps found in <source>`, via the shared `steps_source()` helper that
echoes back the `--layout`/`--template` argument as given) when a file resolves to zero steps.
Every field of a `Layout` is optional, so a file with no `[[step]]` blocks parses fine and used to
"succeed" silently — exit 0, no output, nothing launched, indistinguishable from a run that
worked, when the likely causes (steps lost to a bad merge, or commented out while debugging) are
exactly the ones worth surfacing. Checked at the top of `run_steps()`, *before* the
`--dry-run`/`--validate` branches, so all three modes agree — `--validate` in particular exists to
catch a broken layout without a Sway session, and reporting `valid: 0 step(s)` for a file that
can't do anything would defeat that.

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
and weren't this run's to close.

**Eligibility is read off what the run actually observed (`RunOutcome.launch_ownership ==
Some(LaunchOwnership::Launched)`), not off the step's shape.** It used to be `step.command.is_some()`
— true for every `Target::Exec` step — which quietly assumed that resolving a window and launching
it are the same thing. They aren't: `run_wait_matching_exec_event()`'s PID-marker fallback
(see "`Exec`" under the core model above) deliberately accepts a content-matching window it never
confirmed, and that fallback has two branches worth telling apart, which is what `LaunchOwnership`
now does. The marked process being *gone* means nothing marker-confirmed can still arrive and a
matching window appeared regardless — the single-instance browser/editor case, ours by elimination,
`Launched`. The grace period merely elapsing while the marked process is still *running* means the
window came from somewhere else — another `sway-launch`, another launcher, a user opening something
at the wrong moment — and was adopted only because its app_id/class matched: `Adopted`, and not
rolled back. An external review raised rollback ownership as a risk on the grounds that a recorded
container id might later name a different window; that specific premise was checked live and is
false (Sway ids come from a monotonic counter and are never reused — opened 5/6/7, killed all
three, next three were 8/9/10), but the fallback path is a real way to hold an id that was never
this run's, and a kill can't be undone. An adopted window stays in `container_ids`/`containers`, so
a caller that does want it closed still learns its id, and `--verbose` says why it was spared.
Both branches are pinned live, as the same scenario differing only in whether the launched process
is still running: `tests/live_sway.rs`'s `rollback_on_error_leaves_a_window_it_only_adopted_open`
(still running → adopted → survives) and
`rollback_on_error_still_kills_a_window_matched_after_its_launcher_exited` (seen, then exited →
launched → killed). Keep them together — either assertion alone reads as the whole rule and isn't.
A third, `rollback_on_error_leaves_a_window_opened_while_an_env_dropping_wrapper_waits`, covers a
marker that was never seen at all (adopted → survives), the case that exposed "absent" and "gone"
as different facts — see "`Exec`" above.

`ExecAction::run()` returns `(container_id, LaunchOwnership)` to carry this — it's the only action
that creates a window rather than acting on one already resolved, so `SwayAction::run()` has no
ownership to report and returns just its `ActionOutcome`. On any failure — an id collision, a step failing to convert, or a
step's own `.run()` failing — `fail_step()` calls `rollback()` first (if the flag is set), which
best-effort `[con_id] kill`s every tracked id, most-recently-launched first, via the new
`sway_launch::kill_container()` (a thin `pub` wrapper around the existing private
`run_sway_command()`, added specifically so `main.rs` can reach it — every other Sway-IPC-touching
function in the `sway_launch` module stays crate-internal, reachable only from within it). A kill
that
itself fails (e.g. the window already closed on its own) is logged and skipped rather than treated
as fatal — it doesn't stop the rest of the rollback, and the original step failure stays the error
actually reported. `fail_step()` is shared by all three failure branches specifically so an id
collision or conversion error on step *N* still rolls back steps *1..N-1*'s real windows, not just
a failure inside `SwayAction::run()` itself. A step whose *own* window resolved and then failed on
a later action *within that same step* is covered too: `SwayLaunch::run()` stamps every failure
after `resolve_container_id()` with `RunError.container_id`/`launch_ownership`, and `run_steps()`'s
`Err` branch folds that id into `container_ids`/`containers` (so it's reported as partial progress)
when the step *launched* it (any `launch_ownership`), and, when ownership is `Launched`, into the
kill list (pushed last, so it's killed first). A retargeted failing step's id stays out of the
progress lists — a stale `con_id` is itself such a failure, and listing a container that may never
have existed as open would contradict what `container_ids` means — and is reported only via
`container_id`; `rollback_on_error_leaves_a_window_opened_while_an_env_dropping_wrapper_waits`
(its failing step is a nonexistent `con_id`) pins that. This
used to be a documented known gap — `RunError` carried only rejected-window ids, so the failing
step's window was neither rolled back nor even *reported*, and a single invocation's `--json` error
had no id either, despite the window staying on screen; an external review pointed out that the
reporting half needed far less than rollback and the same plumbing closed both. The same
`container_id` is reported for a single invocation (`"container_id"` in the `--json` error, a
`container <id> was the target when this failed` stderr line in plain output — never stdout, which
stays empty on failure), and for the failing layout step alongside `container_ids`, cleared there
when rollback touched it. Headless coverage drives `--con-id` against an unreachable socket
(`tests/json_output.rs`'s `con_id_json_error_reports_the_resolved_container_id` and siblings,
`tests/layout.rs`'s `layout_json_error_output_names_the_failing_step_s_container`); the
launched-then-rolled-back case is `tests/live_sway.rs`'s
`rollback_on_error_kills_the_failing_step_s_own_launched_window`.

**Declined (2026-09-03): a `mutations_not_rolled_back` field in the `--json` error.** An external
review noted that "rollback" can be read as "restore my desktop", while what it does is close
windows this run launched — mutations to retargeted windows (a workspace move, a resize, a mark)
stay applied. That reading is a fair risk, and the same review agreed emulating transactions over
Sway isn't worth it. But the suggested field isn't a clarification, it's a feature: nothing
currently records what a step changed, so populating it means tracking every mutation per step —
the bookkeeping half of transactional rollback, with none of the payoff, and reporting a list of
things deliberately left alone invites the reading that they *should* have been undone. What was
genuinely missing was a test, since the guarantee was documented but unpinned; that's
`rollback_on_error_undoes_launches_not_mutations_to_existing_windows`. Revisit only alongside real
state restoration (the review's own `--restore-on-error` sketch), where recording prior state is
the point rather than an aside.

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
a `target_id` step can reference an earlier `slot` step via the **existing**
`id`/`target_id`/`resolved_ids` mechanism above (since the slot name *is* the id), with no
template-specific code. Two `TemplateStep`s sharing a `slot` name would also trip `run_steps()`'s
"id already used by an earlier step" check, but `resolve()` rejects that first with its own
`template: slot "..." is used more than once` error, which names the authoring mistake rather than
a resolved-step id collision (see `resolve()`'s doc comment).

**Declined (2026-09-03): giving `TemplateStep` its own optional `id`, defaulting to the slot name.**
An external review read the identity coupling above as a schema-evolution risk — a slot is an
application-binding key, a step `id` is an execution-graph reference, and tying them means renaming
a slot silently changes what a `target_id` refers to. The two concepts really are distinct, but the
failure it describes isn't silent: a `target_id` naming a slot that no longer exists is caught by
`to_sway_launch()`'s own lookup and reported as a step error, before anything launches, and
`--validate` catches it without a compositor at all. Against that, a second identity field is a new
TOML key on every template step, another entry in `main.rs`'s mirror-test allowlists, another
README field list, and a second namespace a reader has to hold in mind to answer "what does this
`target_id` point at". Revisit if a template ever needs two steps bound to the same slot, or a
`target_id` that must survive a slot rename — either is a concrete reason the two need to vary
independently, which is exactly what's missing today.

**Template-level workspace/output context (`[layout]` table).** A template file may declare an
optional `[layout]` table (`Template.layout: TemplateLayoutContext`, `workspace`/`output`, both
`Option<String>`, `#[serde(default)]` so the whole table is optional) applied to every resolved
step that doesn't set its own `workspace`/`output` — closes the "works if the workspace/output
happens to already be in the right state" gap README.md's "Recreatable layouts" section
acknowledges, letting a template pin itself to a specific workspace/output instead of always
operating on whatever's currently focused when it runs. `resolve()` applies this as a per-field
fallback (`step.workspace.clone().or_else(|| template.layout.workspace.clone())`, same for
`output`), not a step-level override switch — a step can mix the template's workspace with its own
output, or vice versa. Unlike `[template]`'s table, this one is optional: a template with no
`[layout]` table behaves exactly as before this existed (`TemplateLayoutContext::default()`, both
fields `None`, so `or_else()` never fires). Confirmed live by `tests/live_sway.rs`'s
`template_layout_context_moves_a_step_without_its_own_workspace_to_the_template_s_workspace`; the
fallback/override/mixed-field logic itself is ordinary pure logic in `resolve()`, covered headlessly
by `template.rs`'s `resolve_applies_the_template_layout_context_to_a_step_without_its_own`,
`resolve_lets_a_step_s_own_workspace_and_output_win_over_the_template_layout_context`,
`resolve_mixes_a_step_s_own_field_with_the_template_layout_context_s_other_field`, and
`resolve_leaves_workspace_and_output_unset_without_a_layout_context`, plus two
`tests/template.rs` `--dry-run`-based end-to-end checks
(`template_layout_context_applies_workspace_and_output_to_a_step_without_its_own`,
`template_layout_context_does_not_override_a_step_s_own_workspace_and_output`).

**`LayoutStep`/`TemplateStep` mirror `Args` by design**, and `main.rs`'s
`schemas_mirror_args_field_for_field` test is what keeps them in sync: it reads all three field
lists back at runtime (clap's own `Args::command().get_arguments()` for the CLI, a test-only
`Serialize` derive on the two schema structs for the rest) and fails on any field present in one
but not the others. The intentional differences live in three commented allowlists next to it —
`CLI_ONLY_ARGS` (invocation-mode/global flags with no per-step meaning), `LAYOUT_ONLY_FIELDS`
(`id`/`target_id`, since a single invocation has only one step to name), and
`BINDING_SUPPLIED_FIELDS`/`TEMPLATE_ONLY_FIELDS` (a template step names a `slot` and gets its
target identity from a binding). Adding a flag therefore fails the test until it's either mirrored
or deliberately allowlisted.

That test does *not* check the wiring or the docs, only the field lists — so when adding a new flag
to `Args`, still add the matching field to `layout.rs`'s `LayoutStep` *and* `template.rs`'s
`TemplateStep` in the same change, wire it into `to_sway_launch()`/`resolve()`, and add it to
README.md's "Layout files" and "Templates" field lists. Before this test existed nothing caught any
of it: `--layout`/`--template` mode would silently lack the capability with no signal to anyone
that they'd drifted apart. `TemplateStep`'s action fields
are a plain duplicate of `LayoutStep`'s rather than a shared `#[serde(flatten)]`ed struct — flatten
has a known history of interacting badly with `#[serde(deny_unknown_fields)]` on the outer struct
(unknown fields can silently pass through instead of erroring), which isn't worth risking against
this project's explicit typo-catching regression tests (see `parse_rejects_misspelled_step_field`
in both `layout.rs` and `template.rs`).

**README.md's embedded help block is pinned the same way**, by `main.rs`'s
`readme_help_block_matches_the_rendered_help`: it reads README.md back at runtime (via
`CARGO_MANIFEST_DIR`, so it doesn't depend on the harness's working directory) and asserts the file
still contains `Args::command().render_help()` verbatim, printing the correct replacement block in
the failure message. It's the only place in the repo where documentation is a copy of program
output, and it had already drifted silently once — giving `--debug-events` a multi-paragraph doc
comment switched clap into its long-help layout, so `--help` stopped matching the block entirely
while `-h` still matched it bar one line, caught only by diffing the two by hand during a docs
review. Compared against `render_help()` (short form) rather than `render_long_help()` deliberately:
the compact layout is what belongs in a README. Deterministic because `clap`'s `wrap_help` feature
isn't enabled, so rendering never consults a terminal width.

**`Args`' `///` comments are user-facing help, not maintainer notes.** clap renders every
doc-comment paragraph into `--help`, so `--debug-events`'s and `--completions`' rationale for their
conflict lists used to print verbatim there, naming `debug_events()` and "that list" to users. That
rationale now sits in `//` comments above each field, and `main.rs`'s
`long_help_carries_no_maintainer_notes` fails on any `--help` line containing a backtick or `()`,
the two marks such a note tends to leave. With no field carrying a second paragraph today, `-h`
and `--help` render identically; a field that needs one is fine, as long as it's written for a
user.

### Built-in templates (`--template <name>`, `--list-templates`, `--show-template`)

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

`builtin()` rejects a name containing `/` or `\` outright rather than looking it up. `include_dir`'s
`get_file()` searches nested directories, while `builtin_templates()` (`--list-templates`, via
`Dir::files()`) and `tests/live_sway.rs`'s
`every_shipped_template_resolves_and_launches_successfully` (via a non-recursive `read_dir`) only
ever look at `templates/`'s immediate children — so without this, a template added in a
subdirectory would be runnable as `--template sub/name` while staying invisible to
`--list-templates` and never driven against a real compositor, letting a broken one ship untested.
Nothing is nested today, so this changes no behavior; it pins the flat layout the rest of the code
already assumes rather than leaving the two halves free to disagree later. Covered by
`template.rs`'s `builtin_returns_none_for_a_name_containing_a_path_separator`.

`--list-templates` (`main.rs`'s `print_builtin_templates()`) is a standalone mode — doesn't touch
Sway IPC, so it's checked and short-circuits right after `--completions`, before the `--layout`/
`--template` dispatch — printing every built-in's name, category, one-line description, and slot
info (`template::builtin_templates()`, sourced from the file's own required `[template]` table —
see below), sorted by name, or (`--json`) the same as a `{"templates": [...]}` array. `--json`'s
entry shape is `{"name": ..., "category": ..., "description": ..., "slots": ..., "slot_names":
[...]}`; plain output appends the same info as `(N slot(s): name, name, ...)` after the
description on each line (singular `"slot"` for exactly one, per user decision 2026-08-22 — tried
`--json`-only first, matching where `category` itself was added, but decided the full listing
should carry it too, accepting that a many-slot grid's line runs long as the tradeoff).
`slot_names` is every distinct `slot` name a template's steps declare, in first-appearance order —
the same order `--apps` zips its own comma-separated list against — via a new shared
`template::distinct_slot_names()` helper that `bindings_from_apps()` (see below) now also calls
instead of duplicating the same loop, so the two can never silently disagree on what "slot order"
means. `slots` is simply `slot_names.len()`. That makes `--list-templates --json` the place
anything outside the crate reads slot order from: `tests/live_sway.rs`'s
`every_shipped_template_resolves_and_launches_successfully` and
`scripts/generate-layout-screenshots` both take `slot_names` from it rather than working the order
out from the template text themselves. Covered headlessly by `tests/template.rs`'s
`list_templates_json_output_reports_slots_and_slot_names`,
`list_templates_plain_output_includes_slot_count_and_names`, and
`list_templates_plain_output_uses_singular_slot_for_a_single_slot_template`, and by `template.rs`'s
`builtin_templates_reports_slots_and_slot_names_matching_the_apps_ordering` and
`distinct_slot_names_deduplicates_and_ignores_target_id_steps` unit tests.

**Every template file's `[template]` table** (`Template.template: TemplateMetadata`, `description`
and `category`, both plain `String`s, both required — `#[serde(deny_unknown_fields)]` on
`TemplateMetadata` too, so a typo'd key errors the same way a misspelled step field does) is the
single source of truth `builtin_templates()` reads from — replacing an earlier convention where the
description was scraped from the file's first header-comment line instead. That convention broke
down once a description needed to be machine-readable independent of a file's prose (`--show-template
--json`'s `"contents"` field, category grouping) rather than just eyeballed; a required table is
explicit and `deny_unknown_fields`-checked, where a magic "first comment line" convention was easy
to violate silently (a header that opens with rationale instead of a description, or wraps its
first sentence across two lines) with no error, only a bad `--list-templates` line discovered by
chance. `category` groups a template alongside similarly-shaped ones in README.md's "Templates"
table (`"Grid"`, `"Master/stack"`, `"Sidebar"`, `"Floating"`, `"Multi-workspace/output"`,
`"Retargeting"` today — not a closed enum, so a new category needs no code change, only a new
string and a new README.md table section). Required on every template file, including one a user
writes themselves for their own `--template <file>.toml` use, not just the shipped built-ins — a
deliberate breaking change from the prior convention (no compatibility shim), matching this
project's broader "no version has shipped yet, so there's no real compatibility surface to
preserve" stance for in-progress work on this branch. `builtin_templates()` parses each shipped
file in full via the same `parse()` every other path uses, rather than re-implementing a
lighter-weight scrape — a shipped file that fails to parse, or is missing the table, is a bug in
this repo's own content, not a runtime condition to handle gracefully, so it panics with the
file's name rather than silently dropping it from the `--list-templates` output. `description`
must still be a complete, self-contained sentence (ending in `.`) — `template.rs`'s
`builtin_templates_every_description_is_a_complete_sentence` test guards this, and
`builtin_templates_every_category_is_non_empty` guards `category` similarly. `parse()` alone only
checks TOML shape and the `[template]` table, so
`every_builtin_template_resolves_and_converts_without_a_compositor` also binds each built-in's slots
to a placeholder command and runs `resolve()` plus `to_sway_launch()` on every step, the way
`--validate` does — a bad `width`, `position`, blank `workspace`, or dangling `target_id` in a
built-in fails `cargo test` (and so the release gate, which doesn't run the live suite). Every shipped
`templates/*.toml` file was migrated to this table in the same change that added it, per
this file's "Screenshots" section discipline of never leaving shipped content half-migrated to a
new schema.

`tests/live_sway.rs`'s `builtin_template_name_resolves_and_launches_without_a_toml_extension`
drives the built-in dispatch path against a real compositor (`--template quad-grid`, no path or
extension), alongside the existing `every_shipped_template_resolves_and_launches_successfully`
(which still drives every file by its `.toml` path, unchanged) — proving the embedded copy a bare
name resolves to is the genuine, working template content, not just that the lookup compiles: it
must build the same shape `tests/template-shapes/quad-grid.txt` records for the file on disk.

`--show-template <NAME_OR_PATH>` (`main.rs`'s `print_template_contents()`) is a third standalone
mode alongside `--list-templates`: prints one template's raw TOML and exits, without running it —
useful for inspecting a built-in's exact shape before binding apps to it, or diffing it against a
customized copy. Reuses `resolve_template_contents()` unchanged, so it accepts the exact same
`NAME_OR_PATH` `--template` itself does (a built-in name or a `.toml`-suffixed file path) with no
separate resolution logic to keep in sync. `--json` prints `{"name": ..., "contents": "..."}`
rather than the bare TOML text, `name` echoing back the argument as given (not a canonicalized
path) since it's just an identifying label for the caller, not something meant to be re-parsed.
Like `--list-templates`/`--completions`, it never touches Sway IPC, so it's checked and
short-circuits right after `--list-templates`, before the `--layout`/`--template` dispatch, and
needs no `tests/live_sway.rs` coverage — the same reasoning `--completions` is exempt for above.
Covered headlessly by `tests/template.rs`: `show_template_prints_a_builtin_s_raw_toml`,
`show_template_prints_a_file_s_raw_toml`, `show_template_json_output_is_a_structured_object`,
`show_template_unknown_builtin_name_errors_clearly`, `show_template_conflicts_with_a_command`
(it conflicts with a positional command the same way `--list-templates` does, rather than silently
ignoring one), and `show_template_conflicts_with_template`.

## Example layout scripts

`examples/` splits into two subdirectories by what each file actually is, not just topic; a third
kind of shipped layout file, `--template` files, lives at the repo root instead, in `templates/`
(see below for why):

- `examples/scripts/` — tracked, user-facing example scripts, each a small standalone shell script
  built out of `sway-launch` calls that demonstrates one layout. Basic examples (`dual-terminals`,
  `triple-row`, `column-split`, `quad-terminals`, `workspace-and-position`, `retarget-floating`) use
  only `foot`; advanced examples (`dev-workspace`, `floating-file-manager`, `browser-comparison`,
  `quad-mixed-apps`, `editor-with-floating-terminal`) combine multiple applications (Firefox,
  Chromium, Thunar, VS Code) and exercise more of the CLI surface (`--app-id`/`--class` matching
  across several apps — VS Code by class `Code`, matching README.md, since it runs under XWayland
  by default and has no app_id there — `--floating`, `--mark`, `--width`/`--height`). README.md's "Recreatable layouts"
  section links to and groups all of these; they are full scripts a user runs directly, so they
  follow every Scripts/Shell convention below, including `-h`/`--help`. Keep this set and README's
  list of them in sync when either changes. Every one of the eleven preflights `sway-launch` itself
  (`command -v sway-launch ... || die ...`) before its first call, per the Dependency preflight
  pattern below — it's the one command all of them run, and a missing one would otherwise fail on
  the first line, or on an `exec` whose own failure skips any `die`. That preflight is also why the
  six basic scripts carry `die` like the advanced ones: they used to be a confirmed exception to the
  "every script needs a `die`" rule, on the grounds that `foot` (treated as always present, see
  `tests/live_sway.rs`'s note above) was their only dependency and they had no error condition
  beyond bad CLI usage — an exemption that explicitly lapsed once a basic script grew a real
  failure path. The five advanced scripts additionally check for the app(s) they launch
  (Firefox/Chromium/Thunar/VS Code), none of which is guaranteed present. `tests/live_sway.rs`'s
  `every_basic_example_script_launches_successfully` satisfies the `sway-launch` preflight by
  prepending a directory holding the compiled binary to `PATH`.
- `examples/layouts/` (`quad-terminals.toml`, `retarget-by-id.toml`) — `--layout` files, run via
  `sway-launch --layout <file>` rather than executed directly.

`templates/` (`quad-grid.toml` and a wider library of other app-agnostic shapes — see README.md's
"Templates" section for the full, grouped list) holds `--template` files, run via
`sway-launch --template <file> --apps ...`/`--bindings <file>`. Named for the shape alone, never
an application: spelled-out count words for even splits/grids (`dual-row.toml`,
`triple-column.toml`, `six-grid.toml`, ...) and descriptive compound names for special-purpose
shapes (`master-dual-stack.toml`, `sidebar-left.toml`, `floating-overlay.toml`, ...). Every file
here is also embedded into the binary as a built-in `--template <name>` — see "Built-in templates
(`--template <name>`, `--list-templates`, `--show-template`)" under "`--template`" above for how. It lives at the repo
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
It's Python, not POSIX sh like the rest of `scripts/` — the reasoning being it needs to parse JSON (`swaymsg`'s
tree output, and `sway-launch --list-templates --json` for each template's slot order), and
orchestrate several subprocesses (`sway`,
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
broader change) *and commit the updated PNG(s)* in the *same* piece of work, along with the
template's recorded shape under `tests/template-shapes/` (see `tests/live_sway.rs` in the
Architecture section — a diff to one of those files is the signal that a screenshot is due), the same discipline
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
  contributors. Neither do dependency bumps, unless one changes behavior or adds something a user
  has to install, nor review bookkeeping. The test for an entry is whether someone who didn't make
  the change would want to know about it — not whether it was hard, or how many files it touched.
- How an entry is written:
  - An entry naming files or functions is a commit message in the wrong place — the reader doesn't
    have the code open. Lead with the change itself; the reason goes in the sentence after, where
    it can be skipped.
  - Where a change replaced something that was wrong, **say what was wrong** ("`--floating` no
    longer hangs until `--timeout` on an already-floating window" over "improved `--floating`").
  - End with the issue number in parentheses where the work is tracked, matching the `(#N)` commit
    title suffix.
  - A change needing more than a few sentences gets a second paragraph under the same bullet, not
    several entries. Within a subsection, lead with the largest change.
  - The repo is public, so the strict Content policy applies here like anywhere else, and no shared
    context can be assumed: an entry names what it's talking about the first time it comes up in a
    release.
- A commit that only fixes a not-yet-merged commit is squashed into it (see "History rewriting"),
  so its entry is folded into that commit's entry rather than added as a second one.
- When a stretch of history is little but internal work, say so in one line rather than listing
  it — "roughly sixty commits, most of them from two deep code reviews" tells a reader more than
  sixty entries do.
- `## [Unreleased]` stays in the file even when empty, so there's always somewhere for the next
  entry to go.
- `CHANGELOG.md` is tracked and committed like any other documentation, so it survives a fresh
  clone and can't silently disagree with what shipped. Never add it to `.gitignore` — the opposite
  of `REVIEW.md`, whose whole purpose is to be untracked; don't reason from one to the other.
- `.markdownlint.json` sets `"MD024": { "siblings_only": true }`: every release repeats `### Added`
  and friends, which MD024's default (no duplicate heading anywhere in the file) would reject.
  `siblings_only` permits a repeated heading under a different parent, exactly the changelog shape
  and nothing looser. Don't instead make headings artificially unique (`### Added (2026-09-12)`).
- At release time, see "Merging to main" below: `## [Unreleased]` is renamed to the new version
  heading and a fresh empty `## [Unreleased]` is added above it, as part of the version-bump
  commit.

## Python conventions

`scripts/generate-layout-screenshots` is currently the only Python file in the project, so these
conventions are scoped to it rather than a general `bin/`/`lib/` project layout. Managed with `uv`
via the repo-root `pyproject.toml`/`uv.lock` (`uv run scripts/generate-layout-screenshots ...`
works the same as running it directly, using the same pinned tool versions). `black` (formatter)
and `ruff` (linter) are dev-only dependencies (`[dependency-groups].dev`, never imported at
runtime) — both must pass with no findings before committing. The `dev` group is never installed
by default (`[tool.uv] default-groups = []` in `pyproject.toml`), so every command that needs
`black`/`ruff` — the two below, and both workflows' lint steps — asks for it with
`uv run --group dev`; a plain `uv run <tool>` for either fails rather than silently installing it,
and running the script itself never passes `--group dev`. Turning the default off removes nothing
from an existing environment (`uv run`'s sync is inexact), so run a plain `uv sync` once in a
checkout that already has the dev tools installed to drop them. Adopting `uv` added `/.venv` to
`.gitignore` (confirmed with the user, 2026-08-19, per the Git conventions' "never modify
`.gitignore` without explicit confirmation" rule):

```shell
uv run --group dev black scripts/generate-layout-screenshots
uv run --group dev ruff check scripts/generate-layout-screenshots
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
- Use `cargo fmt` for formatting and `cargo clippy --all-targets --all-features` for linting. Both
  must pass with no errors before committing. `--all-features` is not optional: `tests/live_sway.rs`
  is gated behind `#![cfg(feature = "live-sway-tests")]`, so a plain `cargo clippy` compiles that
  file to nothing and lints none of it — a review found both CI workflows had the same gap, leaving
  the single largest test file unlinted.
- Aim for high test coverage on every new feature or fix; agree the target threshold with the user
  per project and keep coverage at or above it as code is added, rather than letting it slip. It's
  fine for genuinely untestable paths (e.g. something that can't run headless) to stay uncovered —
  note why in the project's issue tracker rather than forcing a brittle test.
  - This project's agreed target: cover every pure/logic function (command-string building, event
    dispatch tables, window-match logic, CLI argument validation/parsing) via `cargo test`.
    Functions that open, read, or write the Sway IPC socket directly are exempt from the
    `cargo llvm-cov` measurement — they need a live compositor, which `cargo test`/`cargo llvm-cov`
    (headless) can't provide.

    **Since the module split (2026-09-03) that exemption is a rule about modules, not a list of
    function names:** `sway_launch/ipc.rs` and `sway_launch/query.rs` are the compositor-bound
    layers and are exempt; everything else is measured. The layering is what makes that true —
    `query.rs` fetches and immediately delegates to a pure predicate in `tree.rs`, so almost
    nothing but the fetch itself sits where the gate can't see it. The gate excludes both files by
    name, along with `main.rs` and the test code itself (`test_support.rs` and every `tests.rs`,
    which is why unit tests live in sibling files rather than inline — an inline test module's
    bodies would count as covered production code and pad the total):
    `cargo llvm-cov --summary-only --ignore-filename-regex '(^|/)(main|ipc|query|test_support|tests)\.rs$'`
    reproduces CI's figure. Measured that way (2026-10-12): `values.rs` 100%, `tree.rs` 99.2%,
    `process_marker.rs` 96.5%, `layout.rs` 100%, `template.rs` 98.6% lines.
    Two modules straddle the line, and both are deliberate: `confirmation.rs` holds the `run_*`
    drivers and the `state_satisfied`/`poll_matches` fetching halves (exempt) *alongside* the pure
    deciding halves — `state_satisfies`, `poll_grace`, `polls`, `matches_window_event` — that stay
    measured, and
    `launch.rs`'s `run`/`resolve_container_id`/`debug_events` and `build_actions()`'s
    `relocates_to_another_output()` call are exempt while the rest of `build_actions()` is not.
    `SwayLaunch::resolve_container_id`'s `Target::ConId` branch never touches the socket and is
    covered headlessly by `tests/json_output.rs` driving the binary with `--con-id`.

    **Adding IPC-touching code means putting it in the right module, not extending an exemption
    list.** Keep the decision pure and let the IPC layer fetch — the shape `expected_position`/
    `position_matches` and `state_satisfies`/`state_satisfied` already use — rather than burying a
    comparison behind a `&mut Connection` where the coverage gate can't see it.

    None of this is *unverifiable*: `tests/live_sway.rs` exercises it for real against a throwaway
    headless Sway compositor via `scripts/run-live-sway-tests`, in CI and locally — a separate test
    tier from `cargo test`'s coverage-measured suite, gated behind the `live-sway-tests` Cargo
    feature so it stays opt-in. No mocking layer has been introduced on the judgment that a
    trait-based abstraction purely to unit-test thin IPC wiring isn't worth the indirection for a
    tool this size; revisit if the IPC-touching logic grows more complex than it is today.
- Measure coverage with `cargo llvm-cov` (requires the `cargo-llvm-cov` subcommand and the
  `llvm-tools-preview` rustup component):
  - `cargo llvm-cov --summary-only --ignore-filename-regex 'main\.rs'` — summary
  - `cargo llvm-cov --html --ignore-filename-regex 'main\.rs'` — line-by-line HTML report
  - Exclude `main.rs` (or an equivalent thin entry-point file) from coverage accounting if it's
    mostly wiring that's better exercised by integration tests than unit tests.
- UI/rendering tests should assert on rendered output content, not styling or presentation
  details, so tests survive cosmetic tweaks.

### Rust workflow

- After making changes, run `cargo fmt` and `cargo clippy --all-targets --all-features` and fix all
  findings before committing.
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
  - A script with subcommands must show its help message (stdout, exit 0) when invoked with no
    subcommand at all, rather than treating a missing subcommand as an error. Reserve the
    error-and-exit-1 behavior for an actually invalid or unrecognized subcommand.
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
- Replacing a file that already exists is the one case where a temporary file belongs beside its
  destination rather than in a temporary directory. Write the new contents to a temporary file in
  the destination's own directory, then `os.replace` (Python) or `mv` (shell) it into place: the
  rename is atomic only within one filesystem, so a scratch file under `/tmp` cannot be renamed
  over a file elsewhere without falling back to a copy — which is exactly the non-atomic write the
  pattern exists to avoid. Remove the temporary file if anything fails before the rename.
  - This matters most for a file that must never be seen half-written or briefly world-readable: a
    credential, a token, a config another process reads on a timer.
- All *other* intermediate files must be created inside a temporary directory and cleaned up on
  exit, including on errors and signals:
  - Shell: use `mktemp -d` and clean up with `trap cleanup EXIT` — and also `trap 'exit N' INT
    TERM` (e.g. `exit 130` for SIGINT), since under POSIX `sh` an `EXIT` trap alone does not fire on
    an unhandled signal, so cleanup silently never runs on an interrupt without it.
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
- When a script ends by running one other command, with nothing left to do after it, `exec` that
  command (`exec some-tool "$@"`) rather than running it as a child. The script's process becomes
  the command, so a single process is left instead of a shell waiting on it: signals (Ctrl-C, a
  `kill`, a service manager stopping it) reach the real program, its exit status is the script's,
  and its PID is the one a supervisor or process list sees. A wrapper or launcher script is the
  usual case.
  - Don't `exec` when anything about the script still has to happen after the command, since
    nothing does once the shell is replaced: an `EXIT` cleanup trap never fires; a step after the
    command (a notification, a status check) never runs; an exit status the script translates
    rather than passes through is lost; so is a message on failure (`some-tool || die 'what
    failed'`) and a status deliberately ignored (`some-tool || true`).
  - `exec` runs an executable, not a shell function.
  - A failed `exec` (the command can't be found or run) ends a non-interactive script on the spot,
    so `exec some-tool || die ...` never reaches the `die`. Check the command with `command -v`
    first when it may be missing.
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
- Use a retry function for retry logic instead of a bespoke inline loop — a `retry` function
  written in the script itself, per the no-snippets rule above.
- Before a script comes to depend on a new external command not already used elsewhere in the
  project, confirm the choice with the user first. This excludes standard POSIX/base-OS utilities
  guaranteed present on the target system (e.g. `sort`, `cut`, `awk`); confirmation is for
  genuinely new tooling (e.g. `jq`, `httpie`, `fzf`), not coreutils.
- Do not use a `.sh` file extension
- Name scripts in lowercase. Use `-` as word separator for scripts run directly by the user (e.g.
  `run-commands`). No separator for scripts called from other scripts (e.g. `runcommands`).

### Established patterns

These patterns show up repeatedly in real scripts and should be followed in new ones too. Since
this project has no `snippets/` directory (see above), every helper named here is written directly
in the script that uses it. The canonical module's color, privilege-escalation, terminal-launcher,
unattended-script, and picker-script patterns are left out: this project's scripts use no color
(see Scripts above), and none of them needs root, a terminal re-exec, an unattended schedule, or a
picker.

**Usage function split**: Separate `print_usage()` (content only, writes to stdout, always custom
per script) from `usage()` (routes to stderr and sets exit code). `usage()` takes an optional exit
code; non-zero redirects `print_usage` to stderr.

**Option parsing**: Use a POSIX `case` loop over `"$@"` with `shift`. Support `--` to end option
processing. Validate that options requiring a value don't receive an empty string or another flag
as their argument.

**Dependency preflight**: Check every external command a script needs before doing any work, so it
fails immediately with a clear message instead of part-way through:

```sh
command -v jq >/dev/null 2>&1 || die "jq is not installed or not in PATH"
```

Put the checks after option parsing (so `--help` works without the tools installed) and before the
first real action. Keep the message wording consistent. Check a related group with a loop rather
than repeating the line:

```sh
for command_name in sway swaymsg foot; do
  command -v "$command_name" >/dev/null 2>&1 ||
    die "$command_name not found on PATH."
done
```

Only preflight what the script actually requires. A tool the script can do without gets a fallback
instead of a `die`. No guard is needed for `git` or `ssh` — treat both as base tools, same as a
POSIX/base-OS utility (`sort`, `cut`, `awk`), always assumed present rather than checked with
`command -v`.

**Error helper**: Use `die` by default for fatal errors. Switch to an `error` variant only when the
script surfaces many distinct error conditions that benefit from an "Error:" prefix to make them
stand out — confirm with the user before switching. Never use both in the same script.

**Cleanup traps**: Use `trap cleanup EXIT` for resource cleanup. This alone is not enough: under
POSIX `sh` (confirmed with `dash`), an `EXIT` trap does not fire when the shell is killed by an
unhandled signal like `SIGINT` (Ctrl-C) or `SIGTERM` — only on a normal `exit` or falling off the
end of the script. Always pair it with `trap 'exit 130' INT TERM` (130 = 128+SIGINT, the
conventional exit code for signal termination) so an interrupt also triggers a normal exit and the
`EXIT` trap still runs. Register a separate `trap 'trap - EXIT; abort' INT TERM` instead if
interrupt handling needs to genuinely differ from normal exit (e.g., to remove partially-created
state differently than the success-path cleanup) — but either way, `INT`/`TERM` must be trapped
explicitly, since without it cleanup silently never happens on an interrupt at all, not only when
the interrupt case needs different handling.

**Temporary directories**:

```sh
work_dir="$(mktemp -d)"
printf 'Working directory: %s\n' "$work_dir"
cleanup() {
  printf 'Cleaning up: %s\n' "$work_dir"
  rm -rf "$work_dir"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
```

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
- Never commit, amend, rebase onto, or push directly to `main`, and never push `dev` to trigger
  anything main-facing on your own initiative. `main` only moves via an explicit
  user-requested merge (see "Merging to main" below).
- Before starting work on a tracked issue or a larger/multi-commit piece of work, ask whether to
  create and switch to a new topic branch for it, rather than committing straight to `dev`.
- When the work is tied to a tracked issue — whether a local file-based checklist (e.g.
  `ISSUES.md`) or an external tracker (Gitea, GitHub, or similar) — name the topic branch with
  that issue's identifier: `issue-<number>-<short-kebab-case-description>` (e.g.
  `issue-123-fix-broken-parser`). This mirrors the `(#N)` suffix used in commit titles, so the
  branch, its commits, and the tracker entry are all traceable to each other, regardless of where
  the issue itself lives.
- Keep `dev` linear on top of `main`: rebase `dev` onto `main` (never merge `main` into
  `dev`), so `dev` stays a fast-forwardable descendant of `main`.

#### History rewriting

- History rewriting — amending, rebasing, force-pushing — is allowed on `dev` (and topic branches
  based on it), but only for commits not yet merged into `main`, and only as long as `dev` stays
  linear on top of `main` per the rule above. This overrides the general "always create new
  commits, never amend/force-push" default, but *only* for `dev`/topic branches — never rewrite
  `main` history.
- Aim to keep `dev`'s commit count low. Whenever a later commit would touch the same change as one
  already on `dev` and not yet merged to `main` — most often a fix or refinement found via
  testing or review of something just committed — rewrite the earlier commit(s) instead of
  stacking a new one on top: `git commit --amend` for the tip commit, or a soft-reset-and-recommit
  for an earlier one (Claude Code's tooling disallows interactive git flags, so `git rebase -i`
  isn't an option). Do not carry multiple commits into a merge to `main` that are really just
  successive fixes or changes to the same not-yet-merged work — combine them into the commit(s)
  they fix before merging. This is about the same change accruing fixes over time, not about
  splitting distinct work: the one-commit-per-concern rule under Commits still applies to
  genuinely separate concerns. Keep each commit correct and self-contained, since it hasn't
  shipped yet; re-run lint and push with `--force-with-lease` afterward per the rule below. Once a
  commit is merged to `main`, this no longer applies — fix it forward with a new commit as
  usual.
- A topic branch gets the same treatment relative to `dev` that `dev` gets relative to `main`:
  rewrite its commits — amend, or soft-reset and recommit — to fold in fixes and refinements found
  while working on it, rather than stacking new fix-up commits on top. Clean it up before merging
  into `dev`, the same way `dev` gets cleaned up before merging into `main`, so what lands on
  `dev` is already tidy rather than a commit plus a trail of its own fixes. This is not optional —
  see "Mandatory pre-merge history check" below, which applies to this merge exactly as much as it
  applies to merging `dev` into `main`.
- Always push `dev` after committing or rewriting its history, so the remote never lags local. The
  same applies to a topic branch: push it after every commit made on it, not only once it's ready
  to merge, so it's never sitting local-only. When history was rewritten (amend/rebase), push with
  `--force-with-lease` (never a bare `--force`), so the push fails safely instead of clobbering
  anything unexpectedly added to the remote branch since the last fetch.

#### Mandatory pre-merge history check

**Before merging `dev` into `main`, or a topic branch into `dev`, the commit history being
merged MUST be checked and rewritten if needed. This is not optional cleanup, not a judgment call,
and not something to skip because the commits "look fine" — it is a required gate, every single
time, with no exceptions. This has been missed before, and a missed check means every messy
in-between commit ships permanently, since the destination branch's history is never rewritten
after the fact.**

The check: walk every commit being merged (`git log --oneline main..dev`, or
`main..<topic-branch>` for a topic branch) and compare each one against the History rewriting
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

#### Merging to main

Merging to `main` happens only on explicit request, following the sequence below.

**`mm` is shorthand for "merge to `main`".** A message consisting of only `mm` (nothing else,
ignoring surrounding whitespace) is a request to merge `dev` into `main` — the same as typing it
out in full. It starts the sequence below at step 1, exactly like any other merge request. It never
approves a merge: it can't stand in for the confirmation in step 4, even when it's sent right after
a merge summary. A merge summary answered with `mm` is a new merge request, and gets a fresh
summary of its own. `mm` inside a longer message is ordinary text, not the shortcut.

`main` doesn't exist yet — no release has shipped (see "`--json`'s richer schema" above). The first
merge creates it by pushing `dev`'s tip to `main` on the remote, per step 5's no-local-checkout
case; every step before that applies unchanged.

The sequence:

1. Perform the mandatory pre-merge history check above. Do not proceed to the next step until
   `dev`'s history is already clean.
2. Recommend a `Cargo.toml` `version` bump based on what's landed on `dev` since the last release
   (semver: patch for fixes/internal work, minor for a backward-compatible feature, major for a
   breaking change) — then ask for confirmation before applying it. Apply it together with
   finalizing `CHANGELOG.md` (see "Changelog" above): rename `## [Unreleased]` to
   `## [X.Y.Z] - YYYY-MM-DD` with the new version and today's date, and add a fresh empty
   `## [Unreleased]` above it. This version-bump-and-changelog commit must be the last commit on
   `dev` before the fast-forward merge to `main` — nothing else lands on `dev` after it.
3. Present a summary of what's about to land — the commit range (`git log --oneline
   main..dev`) and a nutshell of what changed.
4. Get explicit confirmation on that summary. Asking for the merge and confirming its contents are
   two separate steps; don't collapse them just because the user already said "merge."
5. Once confirmed, fast-forward `main` to `dev`: `git merge --ff-only dev` — no merge commit, no
   squash, since `dev`'s history is already linear and clean. If there's no local `main` checkout
   to merge into, push `dev`'s tip directly to `main` on the remote instead.
6. If a true fast-forward isn't possible (something moved `main` independently), stop and ask
   rather than falling back to a merge commit or force-push.
7. Once `main` is updated, create and push an annotated tag matching the new version, `vX.Y.Z`,
   so `.github/workflows/release.yml` (which triggers on a pushed `v*` tag; see the CI section
   above) picks it up and builds/publishes the release archive.

**Steps 3 and 4 have no size exception, and no staleness exception either.** A single-commit
merge, or a merge of work already discussed at length earlier in the same conversation, still gets
its own summary and its own explicit confirmation — this has been skipped before on exactly that
reasoning ("it's small," "we already just went over this"), which collapses the two steps the same
way jumping straight from the user's "merge" to running it does. A summary shown earlier in the
conversation does not count either, no matter how recently — even one turn earlier, even with
nothing but an unrelated message in between. Any turn landing between the summary and the user's
actual merge request breaks the link: treat every occurrence of the user asking to merge as
requiring a summary shown *after* that specific request, with confirmation anchored to that
specific summary, every single time it happens in a conversation, not just the first.

**This step must never be skipped, full stop — not "try harder next time," a hard gate to check
before running the merge command.** Immediately before running `git merge --ff-only` or pushing
directly to `main` for any reason, stop and verify explicitly: *did the user's most recent merge
request get answered with a summary shown after that request, and did they confirm that specific
summary?* If either answer is no, the merge does not happen yet, regardless of confidence,
urgency, commit count, or how many times the contents were already discussed. Treat the user's
message asking to merge as the trigger for step 3, never as permission to skip to step 5.

Remind the user to merge when it seems due: when a large feature/fix on `dev` looks finished, or
`dev` has accumulated a lot of commits ahead of `main`, say so and suggest merging. This is a
reminder only — never merge to `main` automatically or without explicit confirmation, no matter
how done the work looks or how many commits have piled up.

#### Branch cleanup

Once a topic branch has been merged into `dev`, delete it — both the local branch and its `origin`
remote counterpart (`git branch -d <branch>`, then `git push origin --delete <branch>`) — no need
to ask first. A merged branch is fully redundant the moment its commits live on `dev`; its only
purpose was getting them there. `dev` and `main` themselves are never deleted — this applies
only to topic branches.

### Commits

- Every commit Claude Code creates must end with a `Co-Authored-By:` trailer identifying the
  active model, e.g. `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`. Never omit this —
  if a commit is later found to be missing it and hasn't been merged to `main` yet, fix it via
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
- **Commit to `dev`, or to a topic/issue branch, the moment the current piece of work is complete —
  do not ask first, and do not wait to be told to commit.** This is not a judgment call and not
  something to soften into a question ("should I commit this?", "want me to commit?", "ready to
  commit whenever you are"). The instant a change reaches a natural, self-contained stopping point
  (per the one-commit-per-concern rule under Commits), commit it, then move on. "Confirm before
  committing" applies only to the specific, separate actions the Git section calls out by name —
  merging to `main`, force-pushing, and the like — never to an ordinary commit on `dev` or a
  topic/issue branch. If a task produces several distinct commits back to back, make all of them
  without pausing in between to ask; only pause once the whole piece of work is done. A missing
  commit at the end of a task is a bug in following this rule, not a safe default.
- After the commit completes, show a summary of what changed and why — a report of what already
  happened, not a request for permission. If the work produces multiple commits, commit each as its
  own concern per Git conventions above, then show one summary covering the full set once they are
  all done.
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

### Where new instructions go

When asked to add, update, or clarify a convention or instruction, decide where it belongs before
writing it anywhere:

- **Project-specific** — file paths, tool/service choices, behavior, or anything else true only of
  this repo — goes directly in this file and nowhere else.
- **Generic or language-specific** — anything that would read the same in another project (a
  language's style rules, git workflow, a class of script conventions) — has a canonical copy in
  the maintainer's separate shared-conventions repository, outside this one. This repo carries
  those conventions pasted inline into this file rather than `@import`ed (there is no
  `agent-instructions/` directory here), so the inline sections — Rust/Python/Shell conventions,
  Scripts, Git, Workflow, CI, Changelog, Content, Issues — are copies, and must not be edited as
  if this file were their home:
  1. Say explicitly that the change is to a shared convention, not a project-specific one.
  2. Copy the current canonical module into the root of this repo as a scratch working copy (if
     one isn't already there), apply the change to it, and leave it untracked — never add or commit
     it. If no canonical module matches the topic yet, confirm before starting a brand-new one the
     same way.
  3. Tell the user carrying it across is theirs to do. Once *they* have committed it in the
     canonical repository, update the inline copy in this file to match (an ordinary commit here)
     and remove the scratch copy so it can't drift or duplicate.
- A project-specific adaptation of a shared rule (an exception, a narrower scope, a confirmed
  decision such as "the six basic example scripts need no `die`") is project-specific, and is
  written here directly.

**Never name the canonical repository in this one** — not its name, not its path on this machine,
not a commit SHA from it — in any file, commit message, issue, or comment. It's private and this
repo is public, so the name alone leaks something. Call it "the shared conventions" or "the
canonical repository", and its files "the canonical module", as this section already does.

**Never commit in the canonical repository from this project — not the edit, not a fix to it, not
anything.** The scratch copy at this repo's root is the entire deliverable; the user carries it
across themselves. This holds however convenient it looks and however clearly correct the change
is: the canonical repository is checked out on the same machine, so reaching into it is always one
command away, and that is exactly why the rule has to be absolute rather than a judgement call. Do
not offer it as an option either — a user picking it from a list is not the same as the rule not
applying, and offering it is how it gets picked. Two things follow that are easy to get wrong in the
moment:

- **Don't touch the canonical repository's state at all** — no branch checkout, no `git config`,
  no stash. A repo left on a different branch than it was found on is its own small mess, separate
  from the commit. Its own automated sync is the only thing that moves it.
- **Being unable to push is not the signal.** Read access without write access is the normal shape
  of a sandbox credential, so a failed push is a second line of defence catching a rule already
  broken — a local commit in the canonical repo is already the mistake, whether or not it ever
  reaches a remote.

Don't wait to be asked, either: whenever a task brings in a new tool, library, pattern, or idea
that would fit as a general convention (not just a one-off for this task), proactively suggest
documenting it — say what you'd add and where, per the placement rules above, and let the user
decide. Silence is the wrong default just because nobody asked.

### Issue drafts: `new-issues.md`

A `new-issues.md` file in the repo root holds issue drafts the user wants filed on GitHub. Like
`REVIEW.md`, it is a signal file: leave it untracked, and never add it to `.gitignore`.

**When to check.** Look for it at the start of a session, before starting each new task or step,
and after each commit. Work that runs for a long time without any of those moments still checks
between its steps.

**When one is found, pause briefly and ask.** Stop what you're doing at the next safe point: never
in the middle of a file edit or a git operation. Then:

1. Read the file and reword each draft into a proper issue. A draft is raw notes, not text to paste
   as-is. Give each one a short, specific title and a body that states the problem or request,
   where it applies, and what done looks like, following the Issues section below. Keep the draft's
   meaning, and don't invent details it doesn't support.
2. Present a summary: how many drafts there are, and the reworded title plus a one-line gist of
   each.
3. Ask whether to create them as issues: all, some, or none.
4. Wait **one minute** for an answer, and no longer. Ask in plain text, then wait with a 60-second
   sleep (e.g. `sleep 60`), so a reply typed during the wait arrives when it ends. Never ask with a
   prompt that blocks until someone answers, since nothing would then end the wait.

**The pause must never stop the work.** When no answer arrives within the minute, resume the
interrupted work exactly where it left off, with no further prompt and no change to what it was
doing. Never abort, roll back, or skip ongoing work because of this check, and never leave the work
paused indefinitely waiting for an answer. An unanswered question is not a no: create nothing until
the user says yes.

**Ask again at the very next interaction.** Getting the issues filed is the priority, so an
unanswered question never waits for a new session. The next time the user interacts, lead with it:
put the summary and the question at the top of the reply to their next message, before anything
else. If the work finishes first, put them in the final summary too. Keep asking at every
interaction, in this session or a later one, until the user answers. While work is running between
interactions, an unchanged `new-issues.md` doesn't trigger another one-minute pause, but a change to
its contents does.

**When the user says yes**, create the approved issues from the reworded versions, not the raw
drafts, per the Issues section: check for an existing open issue covering the same thing first, and
add the required trailer. Then delete `new-issues.md`, since its content now lives in the tracker.
Report each new issue's number, and list any drafts that were declined or skipped as duplicates, so
none disappears without a mention. Then resume the interrupted work.

### Idea review: `ideas`

**`ideas` is shorthand for "review the open idea issues".** A message
consisting of only `ideas` (ignoring surrounding whitespace) starts the
review below. Inside a longer message it is ordinary text. Idea issues
are the ones the project's issue conventions mark as ideas, such as an
`[IDEA]` title prefix or an idea label.

**Fetch every open idea.** List all open issues, paging through the
tracker until a page comes back empty, since one listing can be cut off
silently. Keep the idea issues, and present them **oldest first** (lowest
issue number first). Say up front how many there are.

**Present them one at a time.** For each idea:

1. **The issue's identifier and title, in bold**, e.g. **#12 [IDEA] Cache
   the API responses**.
2. **A summary of the idea**, written from the issue's body and comments
   and checked against the current code: what it is, where it applies,
   and what done would look like. A simple idea gets two or three
   sentences. A technical or complicated one says so, and gets as long
   a description as it needs to be understood without opening the
   issue. A well-described idea matters more than a short one. If the
   code has already changed what the idea is about (done, superseded,
   or no longer applicable), say so.
3. **A short list of pros, cons and the cost to implement**: a few
   bullets each, with the cost in rough effort (small, medium or large)
   and what it would touch.
4. **A prompt with three choices**, asked with a blocking question, so
   the user answers each idea before the next is shown:
   - **Keep**: leave the issue open and add it to the "to implement"
     list. Nothing is implemented during the review.
   - **Close as won't do**: post a one-line comment giving the reason
     (ask for it when it isn't obvious from the answer), with the
     required trailer, then close the issue.
   - **Needs refinement**: leave it open and set it aside for the
     refinement round at the end.

**Refinement round.** Once every idea has been through the prompt, go
back through the ones marked for refinement, one at a time. Ask what is
unclear or missing, or propose a sharper description, then update the
issue (its title and body, or a comment) with the result. Finish each
one with the same three-way prompt, minus refinement: keep, or close as
won't do.

**Close with a summary**: how many ideas were reviewed, then two
tables covering every reviewed issue:

- **To implement**: identifier, title, and a one-line gist with its
  cost, for every idea kept (refined ones included).
- **Closed as won't do**: identifier, title, and the reason given.

Mention the refined ones too (what changed in each). Never start
implementing anything from the review unless the user asks.

### Naming an issue

**This must always be followed, with no exceptions.** Whenever a reply
mentions an issue by number, write the number and the issue's title
together in bold, followed by a very short summary of what it is about:
one or two sentences. For example: **#12 [IDEA] Cache the API
responses**: the client refetches every page on each run; a small
on-disk cache would make reruns instant.

- This applies everywhere an issue is named: prose, lists, summaries,
  questions, commit reports and tables. A table of issues gets title and
  summary columns, never numbers alone.
- Give the full form at least the first time an issue appears in a
  reply. A later mention in the same reply may repeat just the bold
  number and title.
- Never write a bare number, and never guess a title or summary from
  memory: look it up.

**Cache issue data when lookups would be slow.** When a reply names many
issues, or the tracker is slow to query, fetch the open issues once into
a cache file in a temporary working directory (e.g. a JSON array of
`{number, title, state, summary}`, under the session's scratch directory
or `$TMPDIR`), and look issues up there instead of querying the tracker
for each one. The cache is a convenience, not a record:

- Refresh it at the start of a session and after creating, closing,
  retitling or editing any issue, and refetch any issue missing from it.
- Never commit it, and keep it out of the repository's working tree.

### Issue review: `issues`

**`issues` is shorthand for "review the open issues".** A message
consisting of only `issues` (ignoring surrounding whitespace) starts the
review below. Inside a longer message it is ordinary text.

1. **Fetch every open issue**, paging through the tracker until a page
   comes back empty, and refresh the local issue cache if the project
   keeps one (see Naming an issue above).
2. **Start with a summary list of every open issue**: number and title
   in bold plus a one-line gist each, grouped so it stays readable (e.g.
   by area or by severity), with the total count.
3. **Then take the issues waiting on a decision from the user**, one at
   a time, oldest first. These are:
   - idea issues, as the project's issue conventions mark them;
   - issues only the user can move forward, such as a check that has to
     be done on their own machines;
   - issues that end in an open question to the user, or whose next
     step depends on a choice between options.

   Present each the same way as an idea in the `ideas` review above:
   number and title in bold, a summary checked against the current code
   (longer for a technical one), pros, cons and cost, then a prompt
   answered before the next one is shown. The prompt offers the choices
   that fit the issue: keep or implement later, close as won't do (with
   a one-line reason comment), needs refinement (gone through at the
   end of the batch), or, for a check or question, the answer itself,
   recorded as a comment on the issue.

   **Work in batches of 10.** After every 10 issues (or fewer, for the
   last batch), go through that batch's refinements, show the batch's
   summary tables (as at the end of the `ideas` review), then ask with a
   prompt whether to **start working on the issues kept in this batch**
   or **continue with the next 10**. Starting work ends the review; the
   issues not yet reviewed wait for the next `issues`.
4. **When no issue is waiting on a decision**, or once they have all
   been through the prompt, ask with a prompt whether to plan the open
   issues that are left: an order to work through them, grouping issues
   that touch the same code.

Never start implementing anything from the review unless the user
chooses to, in the batch prompt or otherwise.

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

GitHub Actions is set up: `.github/workflows/check.yml` runs `cargo fmt --check`,
`cargo clippy --all-targets --all-features` (see the Rust conventions above for why the feature flag
is load-bearing),
`cargo build`, `cargo test`, and a `cargo llvm-cov` coverage-regression gate (`--fail-under-lines
68 --fail-under-regions 68 --fail-under-functions 77`) in its `check` job. The gate excludes
`main.rs`, the compositor-bound `ipc.rs`/`query.rs`, and the unit-test files themselves by filename
(see the coverage bullet under Rust conventions above), and its thresholds sit a few points under
the baseline measured on 2026-10-12, recorded in the comment above that step. They read lower than
the 82/80/88 the gate used before that change, but measure more: the old figures counted every
inline test module's bodies as covered production code and averaged in the exempt modules, so an
untested new function in `tree.rs` or `values.rs` could land without moving them. Most of the
remaining gap is the IPC-touching `run_*` drivers in `confirmation.rs` and `launch.rs`. Re-measure
and adjust the thresholds as their own deliberate decision (with the comment updated) rather than
as a side effect of an unrelated change; a separate `lint-scripts-and-docs` job that runs
`shellcheck`/`shfmt` on every shell script under `scripts/`/`examples/scripts/` (discovered by
shebang at run time, not a fixed list, so a newly added script is covered automatically — and the
one Python script in `scripts/` is excluded by the same mechanism rather than by name),
`markdownlint '**/*.md'` (every Markdown file in the repo, same reasoning, so a newly added one
like `CHANGELOG.md` is covered automatically), and `black --check`/`ruff check` on that Python
script; a separate
`live-sway-tests` job that installs `sway`+`foot` and runs `scripts/run-live-sway-tests` (kept as
its own job so a live-Sway hiccup doesn't block the fast unit-test feedback loop); and a `cargo
audit` job via the `actions-rust-lang/audit` action (switched from `rustsec/audit-check`, which has
had no release since September 2024 and still declares the now-deprecated `node20` runtime with
nothing newer to bump to; `actions-rust-lang/audit` is a composite action with no Node runtime of
its own, so it sidesteps the issue rather than just deferring it).

Both workflows declare `GITHUB_TOKEN` permissions explicitly rather than inheriting the repository
default, which on a repo created before GitHub changed that default (or with it set permissively)
hands every job a read/write token it has no use for. `check.yml` declares `contents: read` at
workflow level, with only its `audit` job re-declaring its own (`issues: write`, which the audit
action needs to file advisories). `release.yml` has no workflow-level block; instead every job
declares its own, `contents: read` on `check` and `build` and `contents: write` on the `publish`
job alone, so neither of the others can write. Any new job needing more than read access should
declare it on the job, not widen a workflow-level grant — and a new `release.yml` job must declare
its permissions itself, since there is no workflow-level default there to fall back on.

Every third-party action in both workflows is pinned to a full commit SHA with its tag in a
trailing comment (`uses: actions/checkout@<sha> # v7.0.1`), never a movable tag or branch, so a
retargeted ref can't run new code with a job's token; `.github/dependabot.yml` keeps the pins
current with weekly pull requests. `dtolnay/rust-toolchain` is pinned the same way, which is why
its steps pass `toolchain: stable` explicitly — that was previously implied by the `@stable`
branch name. A new action added to either workflow gets the same SHA-plus-comment form. Checkout
steps in jobs that hold more than a read token, or that build the release, set
`persist-credentials: false`, so the token isn't left on disk where a build script could read it.

`.github/workflows/release.yml` re-runs `check.yml`'s `check` and `lint-scripts-and-docs` checks
combined into one job (not the coverage gate, not `live-sway-tests` or `audit` — a release build
doesn't need a compositor or a coverage measurement, and the security audit runs independently on
every push regardless) against the exact tagged commit, then builds and publishes a release archive
when a `v*` tag is pushed. Its `check` job also fails unless the tag's version (`vX.Y.Z` minus the
`v`) equals `Cargo.toml`'s `version`, so a tag pushed without the version-bump commit can't ship a
binary whose `--version` disagrees with its release name. Building and publishing are separate
jobs: `build` (read-only token) compiles and packages the archive (the binary, `README.md`, and
`LICENSE` — BSD-3-Clause requires a binary redistribution to carry the copyright notice and
conditions, so the project's own release artifact has to include it) and hands it on via
`actions/upload-artifact`, and only `publish` (no checkout, no build — just a download and
`gh release create`) holds `contents: write`, because compiling runs every dependency's build
script and proc-macro, none of which should ever share a job with the write token. Release notes come from `CHANGELOG.md` rather than `gh`'s own
auto-generated notes: an `awk` script extracts the section matching the tag's version (see the
Changelog section above) and fails the release outright if that section is missing or empty — a
release can't ship without the version-bump-and-changelog commit having actually finalized
`CHANGELOG.md` first. Keep both workflow files in sync with this file's Rust/Shell/Python
conventions above whenever the checks change.

## Content

Applies to every file in this repository — not just test fixtures, but source code, comments,
commit messages, and the repo's own documentation (`CLAUDE.md`, `README.md`, any design doc,
etc).

Real content must never be committed, full stop — no confirmation step, no "just this once," and
no exception beyond the single narrow one called out below for non-secret public URLs (plus the
explicitly approved exceptions listed after the rules). Always use placeholders or made-up content
instead otherwise. Real data leaking into the
repository at all is the failure mode to guard against here, not just an unconfirmed one — this
matters more once the repo is public.

- Never write a real person's name anywhere in the repository. Use a generic placeholder instead
  (e.g. "the user", "example-user").
- Never write a real hostname, URL, IP address, command, or any other identifying detail. Use a
  made-up placeholder instead (e.g. `example-host`, `example.com`, or a documentation-range
  address like `203.0.113.10` — never a real one). **Exception:** a real URL to a public website
  is fine to write as-is when nothing about it looks secret or private — no token, session ID, API
  key, or other credential embedded in the path or query string, and no private-looking host (an
  internal hostname, a `.local`/`.internal`-style domain, or a private/loopback IP literal). This
  exception covers a plain link to genuinely public web content (official documentation, a package
  registry page, a public repository, an RFC, etc) — it does not extend to a hostname, IP, or
  command used as a configuration value or infrastructure reference, even one that happens to be
  publicly reachable, nor does it relax any other bullet in this list.
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
this tool is built for), this file's "Issues" section (`https://cli.github.com/`, the `gh` CLI's
own site), and
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
(its "Changelog" section) link to `https://keepachangelog.com/en/1.1.0/` and
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
- List issues: `gh issue list`. **This stops at 30 issues by default**, with no sign that anything
  was cut off. To count or sweep the whole backlog, raise the limit well past the backlog's size
  (`gh issue list --limit 1000`) and check the result count is below it — a listing that comes back
  exactly at its limit has been truncated.
- View a single issue: `gh issue view <number>`
- Create an issue: `gh issue create --title "..." --body "..."`. For a body of any real length
  prefer `--body-file <file>` (or `--body-file -` for stdin), which also keeps a long value out of
  the process's argument list.
- Comment on an issue: `gh issue comment <number> --body "..."` (or `--body-file`).
- Close an issue via `gh issue close <number>`, once it's actually fixed and the fix is committed
  (and pushed, if that's part of the workflow in play) — never with a bare state change; see below.
- Label an issue: `gh issue edit <number> --add-label "..."`
- Unlike some other issue-tracker CLIs, `gh`'s issue subcommands cover the full set of routine
  operations natively (list, view, create, comment, close, label) — there's no need to fall back to
  the API or the web UI for any of these.

### Attribution and closing

- Every issue, comment, or other content posted to the tracker must end with a trailer identifying
  the active model, in the same `Co-Authored-By: <model name> <noreply@anthropic.com>` format used
  in git commit messages (see "Commits" above). Append it as the last line of the body or comment
  text. Never omit it.
- Never close an issue with a bare state change. First post a comment with a short description of
  what changed to resolve it, and the commit(s) that fixed it (short SHA plus subject line, e.g.
  `abc1234 scope: Fix the thing`), then close it — a plain closed state with no comment loses the
  traceability back to what actually resolved it and how. `gh issue close <number> --comment
  "..."` does both in one step.
- Alongside each commit SHA and subject line, also include a link to that commit (e.g.
  `https://<host>/<owner>/<repo>/commit/<full-sha>`), so the closing comment lets a reviewer jump
  straight to the actual diff instead of having to look the commit up themselves.

### Remind about the open issue count

Open issues accumulate silently otherwise — nothing surfaces them again once filed, so they're
easy to forget for months. This project's periodic review timers (kept outside the repository, in
the maintainer's own Claude Code configuration) include an issue-triage timer for this: when it
fires, list the whole backlog (with a limit well past its size, per the listing note above — a
default listing stops at 30 and under-reports the very number this check exists to surface),
spot-check a sample of open issues against the current code (not necessarily every one, for a
large backlog), and report the total open count plus anything found stale to the user — offering
to close the confirmed-stale ones per the closing rule above.

### Filing deep code review findings

When a deep code review (or similar audit prompt) says to file its findings with the project's
issue tracker, file one issue per finding on GitHub instead of writing a local `REVIEW.md` — per
that prompt's own instructions on where findings go. `REVIEW.md` (see "Workflow" above) remains
the fallback for a review prompt that writes a local report.

- Title: `[<severity>] <short description>`, e.g. `[HIGH] Command injection in a quoted value`.
- Body: the same content the review would otherwise put in a finding's entry in a local report —
  file and line number(s), why it's a problem, and the suggested fix — plus the required trailer
  from above.
- Before creating one, check the open issues (the whole backlog, per the listing note above) for
  an existing one covering the same finding, so re-running the review doesn't file a duplicate.
- Create with `gh issue create --title "..." --body-file <file>`, same as any other issue.
- Filing the issue is the full extent of this step — do not apply the fix or open a pull request as
  part of it. The review reports findings before anything gets fixed, the same as the local-report
  fallback would.
