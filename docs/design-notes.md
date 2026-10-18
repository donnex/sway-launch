# Design notes

> [!IMPORTANT]
> **History, not instructions.** This is the long-form Architecture section `CLAUDE.md` carried
> until 2026-10-14 (#102), kept for the reasoning behind the design: measurements, alternatives
> tried, review outcomes and what each fix replaced. It is a snapshot and is not kept up to date.
> Where it disagrees with the code, the code's doc comments or `CLAUDE.md`, those are right and
> this is stale. Function names here may refer to code that has since been renamed or removed.

## Crate and modules

The crate is `main.rs`, two TOML-schema modules, the `sway_launch` module (itself ten files, see
below) plus six integration test files:

- `src/main.rs` — defines the `clap`-derived `Args` struct (CLI flags) and constructs a
  `sway_launch::SwayLaunch` (direct CLI mode) or dispatches to `run_layout()`/`layout.rs`
  (`--layout` mode) or `run_template()`/`template.rs` (`--template` mode) — both funnel into the
  shared `run_steps()` (see "`--template`" below). Argument validation itself (e.g.
  `--height`/`--width` must be `<N>px`/`<N>ppt`) lives in `sway_launch/values.rs` as
  `pub fn parse_size`/`parse_position`/`validate_sway_string_argument`, referenced from
  `main.rs`'s `#[clap(value_parser = ...)]` attributes, so both the CLI parser and `layout.rs`'s
  TOML steps validate the same way without duplicating the format. `validate_sway_string_argument`
  is the third of these, covering `--mark`/`--mark-match`/`--workspace`/`--output` — every value
  interpolated into a Sway command as a quoted string: it rejects a blank value, and both
  characters `quote_sway_string()` escapes (`"` and `\`), since Sway was confirmed live to store
  the escape character literally rather than unescaping it, silently corrupting the value instead
  of round-tripping it (see that function's own doc comment); it rejects every control character
  but tab too (a newline/NUL because Sway rewrites them to `;`, the rest because a name is later
  printed raw by status bars and `swaymsg`, so a shared template could otherwise plant a terminal
  escape there), and `$`, since Sway substitutes its `set $name` config
  variables even inside a quoted argument. `--workspace`/`--output` (and a step's or a template
  `[layout]`'s `workspace`/`output`) go through `validate_workspace_argument`/
  `validate_output_argument` instead, which add one rule: Sway's own keywords (`next`, `prev`,
  `next_on_output`, `prev_on_output`, `current`, `back_and_forth`, and `number`/`to` alone; for
  outputs `left`/`right`/`up`/`down`/`current`), compared ignoring case, are rejected. Probed live
  on Sway 1.11, one fresh compositor per value: Sway acts on every keyword even inside the quotes
  this tool sends, so the window went elsewhere and the name confirmation waited out `--timeout`,
  and most upper-case workspace keywords (`NEXT`, `PREV`, `CURRENT`, `NEXT_ON_OUTPUT`, but not
  `BACK_AND_FORTH`) crash the compositor (`workspace_create: NULL name`). Sway matches workspace and
  output names ignoring case too, so `state_satisfies()` compares them with
  `eq_ignore_ascii_case` (#5). `Output` additionally requires `ContainerState.workspace_visible`
  (the output node's `focus[0]` is the container's workspace): Sway's `move container to output`
  lands on the output's visible workspace, so a window on a hidden workspace of the right output
  isn't there yet, and short-circuiting it as `AlreadySatisfied` left it hidden (#91). An output's make/model/serial identifier, which Sway also accepts, is
  documented as unsupported rather than resolved. `validate_exec_command` covers the launch command (the
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
  schemas apply to free-form identifiers (`id`, `target_id`, `slot`, a
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
  lives in one of ten submodules, layered so that what needs a live compositor is confined to two
  of them:

  | Module | Holds |
  | --- | --- |
  | `values.rs` | `Split`/`Size`/`Position`, `parse_size`/`parse_position`, and the `validate_*` functions |
  | `target.rs` | `Target`/`WindowMatch`/`LaunchFilter`, and `Target::from_fields()`, the one owner of the target-selection rules |
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
  the fetching half goes in `query.rs` and the deciding half still goes in `tree.rs`. Within
  `tree.rs`, the tree is walked in exactly two places: `ancestors_of()` (the root-to-container
  path, for every "where is this container" question — its parent layout, nearest
  workspace/output) and `descendants()` (every node, for finding one by id or every match). A new
  predicate is a lookup on one of those, not another hand-written recursion over `nodes` +
  `floating_nodes`.
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
  target mode that never touches the Sway socket when it has no actions (with actions it's looked
  up first, see "Orchestration"; an unreachable socket then fails that lookup, which is what the
  error-path tests here drive), so this runs headless in CI. Also covers
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
- `tests/target_rules.rs` — one table of target-field combinations (command, `con_id`, `existing`,
  each matcher, blank values, a command Sway would split) driven through all three front ends —
  the CLI with `--dry-run`, a layout step with `--layout --validate`, a template binding with
  `--template --bindings --validate` — requiring the same accept/reject answer from each. The rules
  live once, in `Target::from_fields()`, but each front end reaches it differently (the CLI
  through clap's own conflicts first), so this is what catches a rule that one path enforces and
  another doesn't (#20).
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
  and updates the file. It also fails any template whose `new_column`/`new_row` action reports
  anything but `"changed"`: a relative move that moves nothing leaves the tree identical, so the
  shape comparison can't see it, yet it reports `"unconfirmed"` on one output and leans on the
  trailing-edge guard (`"skipped"`) on several. Nine such dead `new_column` steps once shipped
  (a newly launched window already lands to the right of the focused one, so on a `splith`
  workspace it already *is* the trailing child). `builtin_template_name_resolves_and_launches_without_a_toml_extension`
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
  scoped out — none of Firefox/Chromium/Thunar/VS Code are installed in either live-Sway CI
  job. Several tests
  sleep briefly after their `sway-launch` invocation completes before asserting on tree state, on
  top of `--wait-time`: the last window's geometry can still be settling for a short while after
  the process itself has already exited.

  **Every test starts from the same compositor state, whatever ran before it.** The file runs in
  one compositor with `--test-threads=1`, so libtest runs the tests in name order, and state one
  test leaves behind used to change what later ones exercised — not just their timing. Outputs
  were the worst of it: seven tests called `create_output` and never removed the output, so every
  test sorting after `dual_output_template_*` ran with two or more outputs, enough to make
  the multi-output move guard silently skip the `move right` that
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
  unit-test-only (`WindowMatch::Class`/`LaunchFilter::Class`, `matches_window_event`'s class arm) until one is.
  `--sticky` is covered by `sticky_floating_window_follows_a_workspace_switch`/
  `sticky_on_a_tiled_window_only_takes_effect_once_floated`/
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
  action's timeout (`run_wait_matching_events()`) has no live test: the only reliable way found
  to make Sway withhold a confirming state was the case-insensitive/keyword `--workspace`/`--output`
  move, which #5 fixed, so none is known now.
  `rollback_on_error_handles_a_launched_window_that_already_closed_itself` covers the scenario
  where a step-1-launched window closes on its own before a later step's failure triggers
  rollback — `rollback()`'s own `kill_container()` call against that already-gone container is
  confirmed to log-and-skip rather than hang or panic. Deterministic, not racy: step 2 retargets
  step 1's window with a `NewColumn`/large-`wait_time` action, and `run_wait_time()`'s *second*
  `ensure_container_exists()` check runs after the sleep, so the window is reliably already gone by the
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
  live-Sway jobs, which just run it — it isn't something that self-maintains: nothing forces
  it to keep tracking the application as flags, actions, and shipped examples are added. Whenever a
  change adds or changes a CLI flag/action, an example script, a `--layout` file, or a
  `--template` file, add or update a `tests/live_sway.rs` case for it in the *same* change — driving
  the actual shipped file/flag against a real compositor, not a hand-written stand-in, per the
  precedent above. Treat a gap here exactly like CLAUDE.md drifting from the implementation, a CI
  workflow drifting from the tooling (see "Keeping a workflow up to date" under CI below), or a
  stale screenshot drifting from the template it's supposed to depict (see "Screenshots" below): a
  bug to fix immediately, not a follow-up.

All six integration test files need `CARGO_BIN_EXE_sway-launch` (to invoke the compiled binary as
a subprocess), which is only set for files under `tests/`, not for the bin crate's own unit test
harness — that's why these live here instead of as `#[cfg(test)]` modules in `src/main.rs`.

Helpers more than one of them needs live in `tests/common/mod.rs`, pulled in with `mod common;`:
`TempToml` (a TOML fixture removed on drop), `TestTempDir` (a fresh scratch directory per test,
randomly named, created mode 0700 with a plain `create()` so a pre-planted directory or symlink is
an error rather than reused, and removed on drop; `TempToml` writes into its own) and `sway_launch_without_a_compositor()` (the binary aimed at a socket that cannot
exist, for tests that need an IPC call to fail) and `assert_conflict_error()` (exit 2 plus clap's
`cannot be used with` wording naming both flags). Every flag-conflict test uses that last one rather
than a bare `!output.status.success()`: once a conflict is dropped the combination usually still
fails later for an unrelated reason (a layout parser rejecting a template's fields, no Sway socket),
exiting 1, so a bare failure check keeps passing after the regression it pins. The `--debug-events`
conflict tests also run through `sway_launch_without_a_compositor()`, since without the conflict
that mode subscribes and prints forever inside a Sway session. Add a shared helper there rather
than copying it into a second test file. `tests/live_sway.rs` still carries its own copies of a few `tree.rs`
predicates (`find_node`, `node_is_floating`, the workspace/output ancestor walks, `parent_layout`):
the crate is a binary only, so an integration test can't call them, and sharing them needs a
library/binary split, which hasn't been done.

## Core model: `SwayAction`

Two types, not one. **`ExecAction`** launches the window (`command`, `filter:
Option<LaunchFilter>`, `verbose`, `timeout`) and has no container yet. **`SwayAction`** acts on an
already-resolved container: a struct holding what every such action shares — `container_id`,
`verbose`, and `timing` — plus a `kind: ActionKind`, one variant per remaining CLI flag (`Split`,
`Floating`, `Sticky`, `Fullscreen`, `Focus`, `NewColumn`, `NewRow`, `Workspace`, `Output`, `Mark`,
`Height`, `Width`, `Position`, `Scratchpad`), each holding only what its own flag specifies — plus
`WorkspaceToOutput { workspace, output }`, which no single flag produces: `build_actions()` plans
it in `Output`'s place when `--workspace` and `--output` are both set (see Orchestration).
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
- declare which `WindowChange` event (if any) would confirm it completed
  (`ActionKind::matching_window_change_event()`) — and, by that answer, commit to the matching
  confirmation logic: an event-confirmed kind needs a real `state_satisfies()` arm (without one its
  event arrives, is rejected, and the action times out after a command that worked), a polled one
  needs `polls()`/`poll_matches()` arms (without them it's reported `unconfirmed` on every run).
  Every `ActionKind` match in `confirmation.rs` names each variant rather than ending in `_`, so a
  new kind doesn't compile until both are decided, and `confirmation/tests.rs`'s
  `every_event_confirmed_kind_can_recognise_its_own_end_state` pins the first half

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
`parse_size()`/`parse_position()` (`fn(&str) -> Result<Size|Position, String>`) validate and
convert in one step: `main.rs`'s `#[clap(value_parser = ...)]` hands clap the parser directly, so
`Args.height`/`width` are `Option<Size>` and `position` is `Option<Position>`, and
`layout.rs`'s `to_sway_launch()` calls the same parser once per field, prefixing the error with the
field name. This replaced an earlier pair per format — a regex-based `validate_*` returning the
original `Result<String, String>`, then an infallible `parse_*` that re-split the string and
`.expect()`ed it to work — which defined each format twice and left every caller to remember to
validate before parsing, or panic (#63). The digits are checked by hand (ASCII only, a leading `-`
only for a coordinate, since `str::parse::<i32>()` alone would also accept `+5`), and the `i32`
parse itself is the overflow check, so the bound lives in exactly one place and the `regex`
dependency went away with it.

`Size`'s bound was `u32` at first, and that was the wrong invariant to promise: `swayipc::Rect` is
`i32`, so `poll_matches()` had to cast (`*pixels as i32`), and a value in `i32::MAX+1..=u32::MAX`
validated, reached Sway as a real `resize set` command, and *then* wrapped negative — accepted by
the front door, unrepresentable by the time anything tried to confirm it. An external review caught
the mismatch. Fixed by moving `Size` itself to `i32` (which deletes the cast rather than relocating
the mismatch) and bounding the validator to match; negative values can't arrive regardless, since
`parse_size()` rejects the sign. The lesson generalizes: a validator's bound has to be the
narrowest domain anything downstream uses, not the widest one the parser happens to accept — pinned
by `parse_size_accepts_the_largest_representable_pixel_value`/
`parse_size_rejects_one_past_the_largest_representable_value`.

The validator also enforces a lower bound for both units (`0px`/`0ppt` rejected) and an upper
bound of 100 for `ppt`: confirmed live that Sway leaves a `0px` height unchanged, doesn't produce
a zero width from `0ppt`, and clamps a percentage over 100, each exiting 0 with an `unconfirmed`
status — so a typo was a silent no-op rather than an error, including under `--validate`. Pinned
by `parse_size_rejects_zero_for_both_units`/`parse_size_rejects_ppt_above_100`.

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
  `Floating` event's change type isn't `Scratchpad`'s matching event, so it's
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

  The `ensure_container_exists()` check stays *ahead* of the subscription, though. That ordering used to
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
  1.9 (confirmed live against a headless compositor) never populates a floating container's own `floating` field — it stays `null` even
  though `floating enable` correctly changes the node's `type` to `floating_con` — while Sway 1.11
  populates both, so `node_is_floating()` checks `node_type == FloatingCon` first and falls back to
  the `floating` field. Without this, `already_at_target()` never short-circuited on Sway 1.9,
  reproducing the exact pre-fix hang/error this paragraph describes; confirmed live in both
  directions (broken on 1.9, fixed on both 1.9 and 1.11) during that investigation. `Scratchpad`
  was found to need this the same way, live: re-running `--scratchpad` on an already-scratchpadded
  window fires no event at all. Its own check is `is_scratchpad_workspace()`, which
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
  `in_scratchpad_false_for_a_shown_scratchpad_window` and `tests/live_sway.rs`'s
  `scratchpad_hides_a_window_currently_shown_from_the_scratchpad`. `Mark` was
  checked live too and found *not* to need this — re-applying a mark the container already has
  still fires
  `WindowChange::Mark`. Confirmed by `tests/live_sway.rs`'s
  `workspace_is_a_no_op_when_already_on_the_target_workspace`,
  `output_is_a_no_op_when_already_on_the_target_output`,
  `floating_is_a_no_op_when_already_floating`, `fullscreen_is_a_no_op_when_already_fullscreen`,
  `focus_is_a_no_op_when_already_focused`, and
  `scratchpad_is_a_no_op_when_already_in_the_scratchpad`.

  **A split container is refused by `Workspace`/`Output`/`Focus` before anything is sent** (#93).
  Sway sends window events only for views, so on a marked split container (`--mark-match` and
  `--con-id` both accept one) those three applied their command, waited out `--timeout` for an
  event that never came, and failed, aborting a layout over a move that worked. Confirmed live on
  Sway 1.11; `Floating`, `Fullscreen`, `Scratchpad` and `Mark` were confirmed to work on a container
  and are left alone. `run_wait_matching_events()`'s existence check reads the node through
  `query.rs`'s `existing_container()`, and when `ActionKind::needs_a_window()` holds, the pure
  `tree::is_window()` (a `con`/`floating_con` with no children — Sway destroys an empty split
  container) decides. Checked at action time rather than when resolving `--con-id`, which is
  #107's question. Supporting containers on purpose would need confirmation by polling and a
  rule for Sway dissolving the container mid-move (its id and mark gone); declined for now. Pinned
  by `workspace_refuses_a_marked_split_container_instead_of_timing_out`.

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
  A rejected window whose pid carries the marker is also rechecked rather than given up on: some
  clients set their app_id/class only after mapping (an XWayland app's late `WM_CLASS`, a Wayland
  client's late `set_app_id`), and Sway sends no window event when either changes — confirmed live
  with foot's OSC 176 runtime app-id escape, which fires only a `title` event. Every
  `LATE_PROPERTY_RECHECK_INTERVAL` (50ms, slower than the wait-time poll since it can run for the
  whole `--timeout`), `query.rs`'s `recheck_launched_windows_now()` re-reads those windows through
  one shared connection and hands the decision to the pure `tree.rs` `recheck_launched_windows()`;
  the first to match is returned as `Launched`, a closed one stops being rechecked, and a timeout
  reports each one's latest app_id/class, with a note that it was rechecked. Covered live by
  `launch_matches_a_window_that_sets_its_app_id_after_mapping`, which skips itself (with a note on
  stderr) when foot ignores OSC 176: Ubuntu 24.04's foot 1.16.2, in the `live-sway-1-9` job,
  does, and the window's app_id never changes. The skip is decided from the tree, not a foot
  version, so a window whose app_id did change and still wasn't matched always fails.
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
  `Width`, `Position`, `WorkspaceToOutput`) → `run_wait_time()`: sleeps for `--wait-time` *before* sending the command
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
  `ensure_container_exists()` (a `get_tree()` lookup) and errors clearly if the container is gone,
  naming it ("container id N no longer exists — window may have closed"); pinned by
  `tests/live_sway.rs`'s `wait_time_action_errors_clearly_when_its_container_already_closed`. This
  was long documented as a Sway 1.9 correctness requirement, on the belief that 1.9 answers a
  `[con_id=N]` command for a missing container with success and silently does nothing. It doesn't:
  re-checked live (#158) against Sway 1.9 in an `ubuntu:24.04` container, with the container
  confirmed gone from the tree first, `splith`, `floating enable`, `resize set width`,
  `move position`, `sticky enable`, `move right` and a two-part batch all answered
  `"No matching node."`, exactly as 1.11 does and as `tests/fake-sway/sway-1.9/` records. The
  belief most likely came from a command sent while a `kill` was still being processed, which
  succeeds on any version because the window is still there. So the check is a fast fail with a
  clearer message, not something Sway needs.

  It's called **twice**, on either side of the pre-command sleep, and both calls earn their keep.
  The one after the sleep covers a window that closes *during* the wait: Sway would refuse the
  command anyway, but with a message that doesn't say which container or why, and a
  `--rollback-on-error` run's error is read by whoever has to work out what closed. The one before it is a fast fail, added after an external review pointed out that
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
  - **`Position`** — `position_matches()` compares the window's frame origin with the requested
    `<x>,<y>` or, for `center`, a computed target, **both relative to the window's workspace** and
    both read from one `get_tree()` (the containing workspace node's own `rect` supplies the
    origin and size). That's the space `move position <x> <y>` itself works in: Sway adds the
    workspace origin to the requested coordinates, and `move position center` centers on the
    workspace (`compute_center_position()`, against the window's current outer footprint), not on
    the output. The workspace starts below or beside a bar and inside outer gaps, so on most real
    desktops it isn't at `(0, 0)` — the live suite's `-c /dev/null` compositor is the exception.
    The window's own fields come in two spaces (`node_position()`): `deco_rect` is the
    decoration-inclusive frame origin already relative to the workspace for a floating window,
    while `rect` is absolute, so it has the workspace origin subtracted. `rect` is the fallback
    whenever `deco_rect` is all zero, which Sway reports for any window drawing no title bar: a
    fullscreen window (confirmed live, stable across a multi-second sweep rather than a settle
    race) and every window whose border isn't `normal` (`border pixel <N>`/`border none`, or the
    same through `default_border`). Such a window has no decoration above `rect`, so `rect`'s origin is
    the frame origin. A freshly mapped window reads the same way, transiently: Sway serves
    geometry from pending state but tests the *applied* border, so until the window's first
    layout transaction is applied (the client acking the configure sent on map, or Sway's
    transaction timeout) it reports `border: none`, an all-zero `deco_rect`, and a `rect` that
    includes the title bar. The fallback still yields the frame origin then, so `--position`
    confirms against the pending state later commands act on, and a launch-and-place invocation
    can finish inside the transient. The live tests asserting on `deco_rect` therefore read it
    through `get_settled_node()`, which waits for `border` to stop reading `none` (every window
    they check has the default `normal` border); reading it straight after sway-launch exited
    failed intermittently once the fallback became workspace-relative, most visibly in a test
    placing a window on a negatively offset output — the old absolute comparison never matched on
    a non-zero-origin workspace, so it kept polling until the transaction applied and hid the
    transient. `--position <x>,<y>` itself is workspace-relative too, the same as Sway's
    `move position` without `absolute` (#64): README.md says so, and
    `position_places_negative_coordinates_relative_to_the_workspace` pins a negative coordinate
    against the workspace rect on an offset workspace. An earlier version centered on the output (via a second `get_outputs()`
    call) and compared the absolute `rect` without converting it, so it only confirmed when the
    workspace started at `(0, 0)`: with a bar or outer gaps, a correctly placed window reported
    `unconfirmed` after waiting out the poll grace — `center` always, and explicit coordinates on
    any title-bar-less window. Pinned by `tests/live_sway.rs`'s
    `position_confirms_center_and_coordinates_on_an_offset_workspace`, which sets `gaps outer` to
    move the workspace origin (the same effect as a bar, without needing a bar client). Unlike
    Height/Width, a tiled window's `move position` isn't a silent no-op that falls back
    gracefully — Sway rejects it outright ("Only floating containers can be moved to an absolute
    position"), which `run_sway_command()`'s `?` propagates as an error *before* `poll_matches()`
    is ever reached, confirmed live by `position_errors_clearly_for_a_tiled_window`.
    `position_confirms_via_poll_for_a_floating_window` and
    `position_confirms_via_poll_for_a_fullscreen_window` cover the fast path.
  - **`Sticky`** — a direct read of the container's own `sticky` field on `Node` (via
    `node_by_id()`), unlike `Floating`'s `node_type`/`floating`-field split (see
    `node_is_floating()`'s doc comment) — confirmed live that `sticky` is a plain `bool` with no
    Sway-1.9/1.11 quirk found, and that `sticky enable` sets it directly and immediately,
    regardless of the container's floating state (it succeeds — and the field flips — even against
    a still-*tiled* window). **The flag is not the effect, though:** Sway only carries a
    *floating* sticky window across workspace switches, so on a tiled window the flag is merely
    recorded and the window stays on its own workspace until it's floated. An earlier version of
    the README read the flag test as "works regardless of floating state", which told users the
    `--floating` pairing was optional when it isn't. The poll confirms what `sticky enable`
    actually does (set the flag), so `"changed"` is still accurate for a tiled window — README.md
    states the floating requirement instead. No known no-op case, same as `Split` — re-applying an
    already-set sticky state matches on the very first poll. Confirmed live by
    `tests/live_sway.rs`'s `sticky_floating_window_follows_a_workspace_switch` (the effect),
    `sticky_on_a_tiled_window_only_takes_effect_once_floated` (flag set, window stays put, applies
    once floated), and `sticky_confirms_via_poll_well_under_a_large_wait_time`.
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

## Orchestration: `SwayLaunch::build_actions()`/`run()`

`SwayLaunch` no longer always launches a new window — it has a `target: Target` field
(`target.rs`), and `resolve_container_id()` turns it into a `container_id`:

- `Exec { command, filter: Option<LaunchFilter> }` — today's original behavior: build and run an
  `ExecAction`, accepting a new window that passes `filter` (app_id or class), or any new window
  without one.
- `ConId(id)` — the id directly. When the run has actions, `run()` first checks it with one tree
  read (`query.rs`'s `ensure_con_id_target()`, deciding in `tree.rs`'s `con_id_target_problem()`):
  a missing id fails as `no window with container id N` and a workspace/output/root id as
  `container N is a workspace, not a window` (#107). The per-action "no longer exists — window may
  have closed" check assumed the id had been seen, so it sent a mistyped id's user looking for a
  window that closed. A bare `--con-id N` (or a layout step with no actions) stays unchecked, so it
  still works without a compositor; README.md says so. A split container passes, since
  `--floating` and friends work on one (see #93's refusal for the moves that don't). Pinned live by
  `con_id_with_actions_names_a_missing_id_and_a_non_window_before_acting`.
- `Existing(WindowMatch)` — `get_tree()` + `matching_container_ids()` (every node `descendants()`
  yields that `WindowMatch::matches()` accepts) + `resolve_matches()`, which errors on zero or more
  than one match rather than guessing. The walk covers the entire tree returned by `get_tree()`,
  which includes the `__i3_scratch` scratchpad workspace — a hidden scratchpad window matching the
  criteria is just as eligible as a visible one (documented for the user in README.md's "Target an
  existing window" section). `WindowMatch::Mark` (`--mark-match`) retargets a previously-`--mark`ed
  window without tracking its container id, the classic "dropdown terminal" pattern
  (`sway-launch --existing --mark-match dropdown-term --scratchpad`).

**A `Target` can only be built through `Target::from_fields()`** (#20), and the types make every
combination it rejects unrepresentable: a launch's filter is a `LaunchFilter { AppId, Class }` (a
window that has only just mapped has no marks, so `Mark` can't be one), `Existing` holds exactly
one `WindowMatch { AppId, Class, Mark }`, and `ConId` holds none. `SwayLaunch`/`ExecAction` used
to carry three loose `&str` matchers with `""` meaning "unset", so `ConId(42)` with an app_id, or
`Existing` with no matcher, were valid values that every front end had to rule out by hand, and
`matching_container_ids()`, `find_existing_container_id()` and `describe_target()` each carried an
app_id-then-class-then-mark precedence chain only because two matchers set at once could be
represented. The rules lived in three hand-written copies — clap's conflicts plus manual checks in
`main.rs`, `to_sway_launch()`, and `template::resolve()`'s binding checks — and had already
drifted: the CLI accepted a whitespace-only command a layout step rejected (launching `env` with
nothing to run and timing out), and a binding with `con_id` plus `app_id` slipped past
`resolve()` to be rejected later as a step of the template the user never wrote. Now all three
build a `TargetFields` (a layout step resolves its `target_id` to a container id first) and call
the one constructor, which also runs `validate_exec_command()` on a launch command. Its
`TargetError` is rendered per front end by `message(FieldStyle)`: flag names and a clap usage
error (exit 2) on the CLI, field names in a file, and a binding's error keeps its
`binding for slot "<name>":` prefix and is reported against the bindings file. clap's
`conflicts_with` declarations stay, for their parse-time usage errors; `tests/target_rules.rs`
checks the front ends still agree. A new target kind (#13 run-or-raise, #14 title matching) is a
new variant the compiler makes every `match` handle.

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
runs immediately after `Floating` — sticky only takes effect on a floating window, but the order
between the two isn't load-bearing: Sway records the flag on a tiled window and applies it once
the window is floated (see `SwayAction::poll_matches()`'s `Sticky` arm above), so this ordering is
about grouping related flags together, not about one requiring the other to run first.
`Scratchpad` runs last deliberately — it's the one action that hides the window away, so every
other action (size, position, mark) gets a chance to apply to it first while it's still
visible/tiled, and a `--mark` set earlier in the same invocation is still there to retarget the
window by later (e.g. `swaymsg 'mark dropdown-term scratchpad show'`), the classic "dropdown
terminal" scripting pattern. The final container id is printed to stdout (`main.rs`, as a bare
integer, or as part of a richer object under `--json` — see "`--json`'s richer schema" below) —
this is what makes commands chainable/scriptable (see README examples).

**`--workspace` and `--output` together plan `Workspace` then `WorkspaceToOutput`, not `Output`**
(`launch.rs`'s `output_action()`, #6). Sway's `move container to output` lands on the output's
*visible* workspace, so the old `Workspace` → `Output` pair silently undid the workspace move while
both reported `changed`. `[con_id=N] move workspace to output Y` moves the window's own workspace
instead (probed live on Sway 1.11: it acts on the criteria-matched container's workspace, is a
no-op when already there, and focuses the moved workspace), so the window ends on workspace X on
output Y — and every other window already on X moves along with it, which README.md states. Moving
a workspace fires a workspace event, not a window one, so `WorkspaceToOutput` is polled:
`poll_matches()` reads the tree through `query.rs`'s `container_state_on()` and asks
`state_satisfies()`, which requires both names (ignoring case) but not visibility. The output move
that was planned to run first in the issue's agreed fix was dropped: moving to X creates X on the
focused output when it doesn't exist, and the workspace move then relocates it either way, so the
extra step did nothing. Pinned by `launch.rs`'s
`build_actions_moves_the_workspace_to_the_output_when_both_are_set` and `tests/live_sway.rs`'s
`workspace_and_output_together_put_the_window_on_that_workspace_on_that_output`.

**`NewColumn`/`NewRow` can't be combined with `Workspace`** (#45): a layout step setting
`workspace` with `new_column`/`new_row` is rejected in `to_sway_launch()`, and the CLI's
`--new-column`/`--new-row` carry `conflicts_with = "workspace"`. They run first in the fixed order,
so the relative move acted on the window's *origin* workspace, and the workspace move after it then
dropped the window into the target as an ordinary new sibling. An earlier live investigation
recorded exactly that and called it harmless, since the origin workspace was left undisturbed — but
"the new column never appears where the window ends up" is the bug, and it's what scrambled a
template's `[layout]` workspace into the wrong shape (see the `[layout]` paragraph under
"`--template`"). No shipped template or layout used the combination. The output equivalent
(`new_column` + `output`) is still accepted: #45's decision covered `workspace` only, and
`new_column_and_new_row_output_guard_still_applies_when_combined_with_output` deliberately
exercises it against the multi-output guard.

Immediately before sending each `NewColumn`/`NewRow`, `run()`'s `execute_plan()` asks
`current_move_skip_reason(container_id, direction)` (`query.rs`: one `get_outputs()` + one
`get_tree()` on a shared connection) whether the move is safe *now*, and the pure
`tree::move_skip_reason()` answers with a reason or `None`, following the order of Sway's own
`cmd_move_in_direction()`/`container_move_in_direction()`: a floating window (`node_is_floating()`)
is skipped on any setup, reason `"floating"`, since Sway only nudges it 10 px — never a new column
or row, yet the rect change used to read as a confirmed `"changed"`; with more than one output, a
fullscreen window (`fullscreen_mode != 0`) is skipped, reason `"fullscreen"`, since Sway sends it
straight to the next output whatever its tree position (a layout step retargeting an earlier step's
fullscreen window, or an app that maps fullscreen itself — the fixed action order only protects a
window the same step makes fullscreen); then, still only with more than one output,
`is_at_the_trailing_workspace_edge()`, reason `"trailing_workspace_edge"`, for the relocation
described above. With a single output Sway no-ops a fullscreen or trailing-edge move, so neither is
skipped there. The check used to run once per move while *planning*, in `build_actions()`, so
`--new-row` was judged against a tree `--new-column` was about to change — on a `splitv` workspace
`[A, W]` it skipped a `move down` that `move right`'s re-orientation had made safe (found by a
review; the stale answer was never seen letting a dangerous move through, only blocking a safe
one). Checking per move, against the tree the earlier actions left, is pinned headlessly by
`launch.rs`'s `execute_plan_judges_new_row_only_after_new_column_has_run` (`execute_plan()` takes
the guard and the action runner as closures for exactly this) and live by
`new_row_is_judged_after_new_column_has_run`; the two new reasons by
`new_column_and_new_row_skip_a_floating_window_instead_of_nudging_it` and
`new_column_skips_a_fullscreen_window_on_a_multi_output_setup`. The fullscreen and floating
branches came from reading Sway's source, not from a live reproduction of the escalation itself.
The trailing-edge check originally checked only "is `container_id` the only window in its
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
`is_at_the_trailing_workspace_edge()` itself never flags a floating window (that's
`move_skip_reason()`'s earlier branch), since Sway won't move one out of its floating parent.
When flagged, the action is skipped (logged under `--verbose`) rather than run, trading a silent
cross-monitor relocation for a silent no-op — confirmed by `tests/live_sway.rs`'s
`new_column_does_not_relocate_a_solo_window_to_a_different_output`,
`new_column_does_not_relocate_a_non_solo_window_at_the_trailing_edge`, and (both nested cases —
with a sibling, run; as a lone child of a `tabbed` or `splitv` wrapper, skipped — in one test)
`new_column_does_not_relocate_a_nested_window_to_a_different_output`. A skip is also structurally
reported, not just logged: `execute_plan()` records it as an `ActionRecord { kind, command: None,
status: ActionStatus::Skipped { reason } }` in its actual fixed-order position, never calling
`.run()` for it — `kind` is `ActionKind::flag_name()`'s `"new_column"`/`"new_row"` and `reason` one
of the short, stable, machine-readable `&'static str` identifiers above, not a human-facing message
(that's what the `--verbose` `diagnostic!` next to each skip is for). See "`--json`'s richer schema"
below for the full design of `ActionStatus`. `build_actions()` is therefore pure and infallible, and
`build_actions_for_preview()` (`--dry-run`) is just `build_actions(0)`. Only one skip mechanism
exists today, so the reason is a plain `&'static str` rather than an enum; revisit if a second one
ever appears.
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
`swaysock_wins_over_a_stale_i3sock`. When neither is set it used to fall back to `Connection::new()`
— and its `i3 --get-socketpath`, then `sway --get-socketpath` steps, unbounded; #162 dropped that,
since `sway --get-socketpath` only reads `SWAYSOCK` itself, and instead names any live socket
`running_sway_sockets()` finds in `$XDG_RUNTIME_DIR` in the error. A connection that
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
  200ms. `poll_matches()` and the tree helpers it calls (`parent_node_layout`, `node_by_id`,
  `position_matches`) therefore take `&mut Connection`
  rather than opening their own.
- `current_move_skip_reason()`: its `get_outputs()` and `get_tree()` calls are two halves of one
  check, so they share a connection too. Cheaper than the poll cycle's case (there's no loop to
  amplify it), but there's no reason for the two to differ.

Everything else still opens its own.

## Output streams

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
`debug_events()`'s event dump and `--dry-run`'s planned-command listing are each the
command's actual output in that mode, so both stay on stdout — neither is meant to be captured as a
single clean value the way a real run's container id is, so the "exactly one line" property simply
doesn't apply to them.

Every stdout write goes through `main.rs`'s `print_line()`/`print_str()` (and, for
`debug_events()`'s own dump, the same `writeln!`-plus-`BrokenPipe` handling inline)
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
`--json` caller to also parse plain-text stderr on failure. A plain-text message is passed through
`escape_control_characters(message, true)` (newlines kept, every other control character shown as a
visible escape), because a message can quote file content — a TOML parse error echoes the line it
failed on, and a file can put a terminal escape in a comment precisely so it fails to parse; the
`--verbose` `Sway action:`/`Sway command:` lines are escaped the same way. In plain output the message is prefixed
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
hide a window the caller may still need to deal with. A *successful* kill isn't taken as a close
either: Sway's `kill` only sends the client a close request and replies once it's sent, and a
client is free to ignore it (an unsaved-changes prompt), so `rollback()` waits up to
`ROLLBACK_CLOSE_GRACE` (1s, polling `sway_launch::containers_still_present()`) for each accepted id
to leave the tree, and a window still present when that runs out goes under `rollback_failed`, not
`rolled_back` — classified by the pure `classify_rollback()`, unit-tested in `main.rs`. No live
test pins the still-open branch, since no client available to this project's dev/CI environments
ignores a close request (`foot` closes on the first poll). `fail_step()` therefore filters *every*
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

## `--json`'s richer schema

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
which run the same command against a window with and without a sibling. `execute_plan()` builds
each `ActionRecord` directly: an action that ran becomes `Changed`/`AlreadySatisfied`/`Unconfirmed`
from the `ActionOutcome` it returned, and a guarded move it skipped becomes `Skipped { reason }`
with no `.run()` call at all. `main.rs`'s `action_record_json()` renders one record as
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

## `--debug-events`

`sway_launch::debug_events()` (a free function in `launch.rs`, re-exported from `sway_launch.rs`)
subscribes to all Sway IPC event types and prints every event until killed. Useful for discovering event shapes when adding a new action.

"All" is a hand-maintained list, `launch.rs`'s `DEBUG_EVENT_SUBSCRIPTIONS`, not something derived
from `swayipc`: its `EventType` is `#[non_exhaustive]`, so no match outside the crate can be
exhaustive and a variant added upstream compiles here silently. `Output` was missing from it for a
while, so output events never appeared. `debug_event_subscriptions_cover_every_sway_ipc_event_type_once`
pins the list against Sway's own documented event names, so recheck both whenever `swayipc` is
bumped.

It's a standalone mode like `--completions`/`--list-templates`: it never acts on a window, so it
`conflicts_with_all`-conflicts with the positional command, `--con-id`/`--existing`, and every
per-window flag, rather than accepting and silently discarding them (`sway-launch --debug-events
foot --floating` used to parse clean, dump events, and never launch `foot`). It's dispatched the
same way they are too: `main()` handles it right after `--show-template`, before any target is
resolved or `SwayLaunch` built, so the per-window path ("Missing COMMAND" and the checks after it)
needs no exemption for it. It used to be a `SwayLaunch` method reached only after building one with
a dummy `Target::Exec { command: "" }`, which left the empty command one reordered `if` away from a
real `run()`, and made every new per-window check remember to skip it; the method read none of
`self`'s fields, so it became a free function (#99). Dispatching it that early means it never
reaches `main()`'s manual `--bindings`/`--apps`/`--rollback-on-error` checks, so those three are in
its `conflicts_with_all`, the same as `--completions`'/`--list-templates`'. `--timeout`/
`--wait-time`/`--verbose` remain accepted but unused here, the same as for the other standalone
modes.

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

## `--dry-run`

Prints the planned sequence of Sway commands instead of running them, never touching Sway IPC or
launching anything — the review's suggestion, and the reason Phase 2's `build_actions()`/
`sway_command()` split (see "Orchestration" above and the `sway_command()` bullet under "Core
model" above) was worth doing first: both were designed with this in mind, not retrofitted for it.

`SwayLaunch::build_actions_for_preview()` is just `build_actions(0)`: `build_actions()` never
touches Sway IPC (the `NewColumn`/`NewRow` guard runs per move inside `run()`'s `execute_plan()`,
see "Orchestration" above), so a preview always includes `NewColumn`/`NewRow` when the flag is set —
it has no real `container_id` (nothing has launched) and no tree to judge them against anyway. The
placeholder `container_id: 0` passed to
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
the same pair a real run's `ActionRecord` reports). Plain lines go through
`sway_launch::escape_control_characters()` (in `values.rs`) via `dry_run_plain_lines()`: a TOML
escape can put a carriage return or terminal escape in a `command`, and printed raw it could make
the preview show a different command from the one that runs. `--json` needs nothing, since JSON
already escapes control characters.

## `--validate`

Parses and validates a `--layout`/`--template` file without launching anything or touching Sway
IPC, exiting 0 (`valid: <source> (N step(s))`, or `{"source": ..., "steps": N, "valid": true}`
under `--json`, `<source>` echoing the `--layout`/`--template` argument as given) or 1 (the same
`<source>: step N: <message>`/structured-`--json`-error shape every other runtime failure uses). Requires
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
`<source>: step N: <message>` formatter, used by `plan_steps_offline()` and `fail_step()` alike —
the `--layout`/`--template` argument as given (`steps_source()`) leads, the same `<file>:` prefix
file-read and parse errors carry, so a script applying several files in a row can tell which one
failed. A new
per-step rule belongs in `plan_step()` (or `to_sway_launch()`), never in one mode's loop. A `--template`'s own
`--bindings`/`--apps` resolution (slot count, duplicate slots, binding correctness) already
happens unconditionally before `run_steps()` is ever reached (`run_template()`'s own
`template::resolve()` call) — by the time `--validate` sees `steps`, that part already succeeded,
so there's nothing left for it to repeat.

## `--completions`

Standalone mode handled entirely in `main.rs`, before any of the `SwayLaunch`/`Target` logic:
`clap_complete::generate()` writes the completion script for the given `clap_complete::Shell` to
stdout and exits 0. Doesn't touch Sway IPC at all, so it's checked and short-circuits right after
`Args::parse()`, first of the standalone modes.

## `--layout`

Another standalone mode, short-circuiting after `--completions`/`--list-templates` (before the
command/`--con-id`/`--existing` validation, since a layout file satisfies that requirement on its
own). `main.rs`'s `run_layout()` reads the file, parses it via `layout::parse()`
(`toml::from_str::<layout::Layout>`), then hands `parsed_layout.step` to `run_steps()` (shared with
`--template`, see below), which converts each `[[step]]` in order to a `sway_launch::SwayLaunch` via
`LayoutStep::to_sway_launch()` (reusing `sway_launch::parse_size`/
`parse_position` — the same parsers the CLI's `--height`/`--width`/`--position`
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
as different facts — see "`Exec`" above. A fourth,
`a_hand_off_whose_launcher_exits_after_the_window_maps_is_launched_promptly`, covers the window
arriving *before* the launcher exits (launched, under 1.8s rather than after the 2s grace): it's
the timed marker re-scan, not a later window event, that notices the launcher is gone. A
client/server launcher (`footclient`, `emacsclient -c`) never takes that path, since the client
lives as long as its window — README.md says so, and #114 tracks detecting it.

`ExecAction::run()` returns `(container_id, LaunchOwnership)` to carry this — it's the only action
that creates a window rather than acting on one already resolved, so `SwayAction::run()` has no
ownership to report and returns just its `ActionOutcome`. On any failure — an id collision, a step failing to convert, or a
step's own `.run()` failing — `fail_step()` calls `rollback()` first (if the flag is set), which
best-effort `[con_id] kill`s every tracked id, most-recently-launched first, via the new
`sway_launch::kill_container()` (a thin `pub` wrapper around the same connect-and-`run_command()`
path the private `run_sway_command()` uses, added specifically so `main.rs` can reach it — the only
other Sway-IPC-touching function `main.rs` can reach is `query.rs`'s `containers_still_present()`,
which `rollback()` uses to confirm the kills actually closed their windows; everything else in the
`sway_launch` module stays crate-internal, reachable only from within it). A kill that itself fails
(e.g. the window already closed on its own) is logged and skipped rather than treated as fatal — it
doesn't stop the rest of the rollback, and the original step failure stays the error actually
reported. The one failure that *does* stop it is the compositor not answering: `kill_container()`
returns a `KillError` that tells `Refused` (Sway replied, the kill failed) from `Unreachable` (no
connection, or no reply within `IPC_ROUND_TRIP_TIMEOUT`), and `send_rollback_kills()` puts every
remaining id straight into `rollback_failed` after the first `Unreachable`, skipping the
close-confirmation wait too. Without that, a step that failed on a stalled compositor cost another
10s per launched window before the failure was even reported (#94). The stopping rule is
unit-tested headlessly through `send_rollback_kills()`'s injected `kill`, and live by
`rollback_on_error_stops_trying_once_the_compositor_stops_answering`, which points the binary at a
proxy socket that forwards to the real compositor until three windows have launched, then stops
answering. `fail_step()` is shared by all three failure branches specifically so an id
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

## `--template`

A layer on top of `--layout`, not a replacement: `--template <FILE>` decouples a layout's shape
(what to do) from its application identity (which window), so the same template can be reused
across different applications, or shared/bundled independently of any specific one. `main.rs`'s
`run_template()` reads and parses the template via `template::parse()`
(`toml::from_str::<template::Template>`), builds a `template::Bindings` from either `--bindings`
(reads + `template::parse_bindings()`) or `--apps` (`bindings_from_apps()`: splits the
comma-separated list, zips it 1:1 onto the template's `slot` steps in the order they appear,
erroring on a count mismatch), then calls `template::resolve()` and hands the result to the
same `run_steps()` `--layout` uses — a resolved template is just a `Vec<layout::LayoutStep>`, so
nothing downstream needs to know a template was involved. `resolve()` returns a
`template::ResolveError` rather than a bare string, split by which input holds the mistake:
`Bindings` (a missing, duplicate, unused, or malformed binding) is reported under the bindings
file's path, or `--apps` (as are `bindings_from_apps()`'s own errors), and `Step { index, .. }` (a
template step setting neither/both of `slot`/`target_id`, or a blank one) under the template's,
as `step N:` with the same 1-based number `run_steps()` would use. Before this every one of them
was prefixed with the template's name, so a typo in a user's bindings file for a built-in read as a
fault in the shipped template. `--template` requires exactly one of
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
"id already used by an earlier step" check, but `template::parse()` rejects that first with its own
`template: slot "..." is used more than once` error, which names the authoring mistake rather than
a resolved-step id collision. It lives in `parse()` rather than `resolve()` so that everything
counting or listing slots — `bindings_from_apps()`'s count check, `builtin_templates()`,
`--validate`, `resolve()` — only ever sees a template whose slots are already distinct: while the
check sat in `resolve()`, `--apps`' count check ran first against a de-duplicated slot list and
reported a misleading count for a template that was actually invalid, and the slot helper and its
test still treated a repeated slot as supported.

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

**Template-level workspace/output context (`[layout]` table) and `--on-workspace`: a run builds
on one workspace.** A template file may declare an optional `[layout]` table (`Template.layout:
TemplateLayoutContext`, `workspace`/`output`, both `Option<String>`, `#[serde(default)]`). It is
not copied into the steps: `run_template()` turns it into a `sway_launch::BuildLocation`, and
`run_steps()` applies that once before the first step (`BuildLocation::commands()`: `focus output
"<output>"`, then `workspace "<workspace>"`), so every window opens there and the shape is built in
place. `--on-workspace <WORKSPACE>` (run-level, in `CLI_ONLY_ARGS`, requiring `--layout`/
`--template` via the same manual check as `--rollback-on-error`, and in every standalone mode's
conflict list) supplies the workspace for a layout file and overrides a template's `[layout]`
workspace while keeping its output. The view stays on that workspace afterwards. `--dry-run`
prints those commands first (a `"setup"` list under `--json`), and `--validate` needs nothing extra:
both values are validated where they're declared.

This replaced a per-field fallback in `resolve()` that copied `[layout]`'s workspace/output into
every step that lacked its own (#45). That moved each window after launching it, so the shape was
built on whatever workspace was focused and each window then landed next to the target's focused
container: `quad-grid` with a `[layout]` workspace arrived as one full-height window beside a
column of one window above two, while every action, `move down` included, reported `"changed"`.
The only live test of `[layout]` used a one-slot template and couldn't see it. A step's own
`workspace`/`output` still moves that one window, as before (`workspace-spread` relies on it).

**Two checks back the "one workspace" assumption up** (#105), both only for `--layout`/`--template`
runs. First, a run that launches anything reads the focused workspace once after the switch and,
when it already holds tiled windows (`tree::tiled_window_count()`), warns on stderr and adds a
`"warnings"` list to the `--json` success object: run from a terminal, the Quickstart grid was
built around that terminal. A run that only retargets existing windows skips this, so it still
never touches IPC beyond its own steps. Second, `run_steps()` sets
`SwayLaunch.require_launch_on_focused_workspace`, and a launching step then records the focused
workspace before its `exec` and fails (with the window still reported and rollback-eligible, like
any failure after a launch) if focus has moved by the time its window appears. The check is on
*focus*, not on where the window opened, deliberately: Sway places a window on the workspace its
`exec` ran on wherever focus has gone since (confirmed live — the first version compared the
window's workspace and never fired), so a switch mid-launch leaves that step's window in place and
sends every *later* step to the new workspace. A switch landing in the milliseconds between steps
is not caught. Comparing against the focus at each step's start rather than the run's build
workspace is also deliberate: some steps move focus on purpose (`focus`, a `WorkspaceToOutput`
move, which focuses the moved workspace), and the steps after them must follow.

Pinned by `template/tests.rs`'s
`resolve_leaves_the_template_layout_context_to_the_run_rather_than_each_step` and
`resolve_keeps_a_step_s_own_workspace_and_output_alongside_a_layout_context`, `tests/template.rs`'s
`template_layout_context_switches_to_its_output_and_workspace_before_the_first_step`,
`template_layout_context_leaves_a_step_s_own_workspace_move_in_place` and
`on_workspace_overrides_the_template_layout_workspace_in_the_preview`, and live by
`template_layout_workspace_builds_the_shape_in_place_on_that_workspace` (compared byte for byte with
`tests/template-shapes/quad-grid.txt`, by naming the `[layout]` workspace after the shape test's
own), `a_template_warns_about_an_occupied_workspace_and_on_workspace_builds_cleanly`,
`a_launch_whose_window_opens_on_another_workspace_fails_its_step` and
`template_layout_context_moves_a_step_without_its_own_workspace_to_the_template_s_workspace`.
`template::parse()` validates the table's own values (`validate_workspace_argument()`/
`validate_output_argument()`, errors prefixed `[layout] workspace:`/`[layout] output:`) where
they're declared, covered by `parse_rejects_*_layout_context_*` in `template/tests.rs` and two
`tests/template.rs` `--validate` cases.

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

`BINDING_SUPPLIED_FIELDS` is itself checked against `template::Binding` (which gets the same
test-only `Serialize` derive): every entry but `id` must be a real `Binding` field, and every
`Binding` field but `slot` must be an entry (`RESOLVE_SUPPLIED_FIELDS`/`BINDING_ONLY_FIELDS` name
those two exceptions — `resolve()` fills `id` from the slot name, and `slot` is the lookup key, not
a supplied value). Without this, a new matcher added to `Args`/`LayoutStep` passed the test the
moment it was listed in `BINDING_SUPPLIED_FIELDS` — the obvious fix for the failure — while
`--template` still couldn't use it: a bindings file naming it was rejected by
`deny_unknown_fields`, and `resolve()` never copied it.

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

## Built-in templates (`--template <name>`, `--list-templates`, `--show-template`)

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
`template_unknown_builtin_name_errors_clearly`. The split decides resolution only, not wording:
when a `.toml` read fails with `NotFound` and the value is a bare filename (no directory
component) whose stem is a built-in, `builtin_name_hint()` appends `; there is a built-in template
"<name>", use <flag> <name>` to the OS error — writing a built-in's name with the extension its
repo file carries is an easy mistake, and the bare error didn't say the name itself was right.
`resolve_template_contents()` takes the originating flag (`--template`/`--show-template`) only to
word that hint. Covered by `main.rs`'s `builtin_name_hint_*` unit tests and `tests/template.rs`'s
`template_missing_builtin_name_with_toml_suffix_hints_at_the_bare_name`/
`show_template_missing_builtin_name_with_toml_suffix_hints_at_the_bare_name`.

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
`slot_names` is every `slot` step's name, in step order (already distinct, since `parse()` rejects
a repeated slot — see "`--template`" above) — the same order `--apps` zips its own comma-separated
list against — via a shared `template::slot_names()` helper that `bindings_from_apps()` (see
below) also calls instead of duplicating the same loop, so the two can never silently disagree on what "slot order"
means. `slots` is simply `slot_names.len()`. That makes `--list-templates --json` the place
anything outside the crate reads slot order from: `tests/live_sway.rs`'s
`every_shipped_template_resolves_and_launches_successfully` and
`scripts/generate-layout-screenshots` both take `slot_names` from it rather than working the order
out from the template text themselves. Covered headlessly by `tests/template.rs`'s
`list_templates_json_output_reports_slots_and_slot_names`,
`list_templates_plain_output_includes_slot_count_and_names`, and
`list_templates_plain_output_uses_singular_slot_for_a_single_slot_template`, and by `template.rs`'s
`builtin_templates_reports_slots_and_slot_names_matching_the_apps_ordering` and
`slot_names_lists_slot_steps_in_order_and_ignores_target_id_steps` unit tests.

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
