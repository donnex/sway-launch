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

- All pre-commit checks: `scripts/run-checks` — `cargo fmt --check`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo build`, `cargo test` (all `--locked`; plus the man page
  test, which needs `--features generate-man`), `shellcheck`/`shfmt`
  on every shell script under `scripts/`/`examples/scripts/` (found by shebang), `markdownlint` on
  every tracked Markdown file (tracked only, so an untracked signal file such as `REVIEW.md` isn't
  linted; stage a new file to include it), `scripts/check-changelog`, and `black`/`ruff` on the
  Python script. Stops at the first failure. Both workflows run this same script (see CI below),
  so a new check is added there once (#62). Not included: the coverage gate, the live-Sway tests
  and the audit.
- Changelog shape: `scripts/check-changelog` fails unless `CHANGELOG.md`'s first second-level
  heading is `## [Unreleased]` and it has exactly one non-empty `## [X.Y.Z] - YYYY-MM-DD` section for
  `Cargo.toml`'s `version`, which holds on every commit from a version-bump commit on; so a bump
  that loses `[Unreleased]` or never finalizes its section fails on `dev`, before any tag is
  pushed. `--release-notes` prints that section, which is how `release.yml` gets its release notes,
  so the extraction has one copy (#186).
- Man page: `cargo run --features generate-man -- --generate-man > sway-launch.1`. The optional
  `generate-man` feature (and its `clap_mangen` dependency) exists only for this: `main()` checks
  for `--generate-man` before parsing `Args`, so it never appears in `--help` or in the shipped
  binary, which is built without the feature. The release job builds it separately (#109).
- Build: `cargo build`
- Run: `cargo run -- [OPTIONS] [COMMAND]` (e.g. `cargo run -- -a foot foot`)
- Format: `cargo fmt`
- Lint: `cargo clippy`
- Test: `cargo test` — runs the unit tests in `src/sway_launch/`'s modules, `src/main.rs` and `src/run.rs` (covering all
  pure/logic functions; see the Testing bullet under Rust conventions) plus the integration tests
  in `tests/` (driving the compiled binary directly, including against the fake Sway IPC server in
  `tests/fake_sway.rs` — see Architecture below). This never touches a real Sway compositor, so it
  runs headlessly in CI.
  - Run a single test: `cargo test <test_name>`
  - Run with debug output: `cargo test -- --nocapture`
- Live-Sway test: `scripts/run-live-sway-tests` — starts a throwaway headless Sway compositor, runs
  `tests/live_sway.rs` (gated behind the `live-sway-tests` Cargo feature) against it, then tears it
  down. On a failure it first says whether Sway is still running and prints the tail of Sway's own
  log, which the teardown deletes (#184). This is what checks the IPC-touching code against real Sway behavior rather than a
  scripted fake's, plus anything else worth confirming live rather than assuming it. Requires `sway`, `swaymsg`, and `foot` on `PATH`, and prints
  `sway --version` first. **Supported Sway: 1.9 and newer** (README.md says the same), so CI runs it
  in two jobs of `.github/workflows/check.yml`, both separate from the main `check` job so a
  live-Sway hiccup doesn't block the fast unit-test feedback loop: `live-sway-1-9`, pinned to
  `ubuntu-24.04` (whose archive ships Sway 1.9 and foot 1.16.2) so the oldest version stays tested
  on purpose rather than until a runner image update, and `live-sway-latest`, in a rolling
  `archlinux` container (Sway, foot and Rust from Arch's own packages), so a new Sway release is
  tested as soon as Arch ships it; the printed version tells a Sway change from a code regression
  in its log (#98). Code that exists for one version names the version ("Sway 1.9"), never "what
  CI installs". Running the scripts in
  `examples/scripts/` against a live Sway session is still useful for eyeballing real layouts, but
  is no longer the only way these code paths get exercised.
- Tool versions: CI pins `markdownlint-cli@0.49.1` (`npm install -g markdownlint-cli@0.49.1`) and
  `cargo-llvm-cov@0.9.1` (`cargo install cargo-llvm-cov --version 0.9.1 --locked`); use the same
  versions locally, since each linter's findings and the coverage gate's counts depend on the
  version, so a pass under a different one guarantees nothing in CI. Both are bumped deliberately
  in `.github/workflows/` and here together (a `cargo-llvm-cov` bump re-measures the coverage
  baseline in the same change), never left to float. `shellcheck`/`shfmt` come from the CI runner
  image's `apt` instead, unpinned: the `check` job prints both versions so a new
  finding on an untouched script can be traced to a tool upgrade from the log. Rust itself floats
  on purpose (see Rust conventions). `scripts/run-live-sway-tests` runs `cargo test --locked`, the
  same as the `check` job, so both test against the dependency set `Cargo.lock` records.

See the CI section below for how GitHub Actions runs these same checks.

## Architecture

This section is the map and the rules. The reasoning behind a rule lives in the doc comment of the
function it names; read that before changing the rule. The design history (measurements,
alternatives tried, review outcomes) is in `docs/design-notes.md`, which is a snapshot and not an
instruction set: where it disagrees with this file or the code, it's stale. The same goes for
`docs/plan-poll-based-wait-time-actions.md`, the plan the polled actions were built from.

### Crate map

- `src/main.rs` — the `clap` `Args` struct, `main()`'s dispatch (standalone modes first, then
  `--layout`/`--template` through `run_layout()`/`run_template()` into `run.rs`'s shared
  `run_steps()`, then a single direct-CLI `SwayLaunch`), and everything about output: printing,
  `--json`, errors (`fail_with_rollback()` renders what a rollback did). Excluded from the coverage
  gate, so it holds no run logic.
- `src/run.rs` — the `--layout`/`--template` run (#134): `run_steps()`'s per-step loop
  (`--continue-on-timeout`, `target_id` skipping), `plan_step()`/`plan_steps_offline()`,
  `--concurrent`'s staging (`stage_launches()`/`arrange_staged()`/`release_staged()`), the run
  lock (`acquire_run_lock()`) and `--rollback-on-error` (`rollback()`). A run's progress is one
  `RunProgress` (#135): every window goes in through `record()`/`record_failed()`, so `--json`'s
  `container_ids`/`containers` and the rollback list can't disagree, and every failure through
  its `fail()`, which runs `release_staged()` before `fail_step()`. A child of `main.rs`, so it
  uses the printing helpers and `Args` directly; measured by the coverage gate like any module.
- `src/layout.rs` — `--layout`'s TOML schema (`Layout`/`LayoutStep`) and
  `LayoutStep::to_sway_launch()`, which turns one step into a `SwayLaunch`.
- `src/template.rs` — `--template`'s schemas (`Template`/`TemplateStep`, `Bindings`/`Binding`),
  `resolve()` (template + bindings → ordinary `LayoutStep`s), and the built-in templates.
- `src/sway_launch.rs` — a façade: module declarations, the public surface the three files above
  use, and the poll-timing constants. Everything else is in `src/sway_launch/`:

  | Module | Holds |
  | --- | --- |
  | `values.rs` | `Split`/`Size`/`Position`, `parse_size()`/`parse_position()`, every `validate_*` function, `escape_control_characters()` |
  | `target.rs` | `Target`/`WindowMatch`/`LaunchFilter`, and `Target::from_fields()`, the one owner of the target-selection rules |
  | `ipc.rs` | connections, socket discovery, command dispatch, the event subscription, `kill_container()` |
  | `tree.rs` | every pure `&Node` predicate: matching, geometry, `ContainerState::from_tree()`, the move guard, and how a window is named (`window_app_id()`/`window_class()`, empty read as none, and the `WindowIdentity` every window report holds) |
  | `query.rs` | fetch-a-tree-then-delegate wrappers, and nothing else |
  | `process_marker.rs` | the PID marker and its `/proc` reads |
  | `action.rs` | `SwayAction`/`ActionKind`/`ExecAction` themselves: fields, `Display`, `sway_command_verb()`, `flag_name()` |
  | `confirmation.rs` | how an action's effect is established: event matching, state checks, polling, the `run_*` drivers |
  | `outcome.rs` | `RunOutcome`/`RunError`/`UnmatchedWindow`/`ActionRecord`/`ActionStatus`/`LaunchOwnership` |
  | `interrupt.rs` | SIGINT/SIGTERM/SIGHUP as a flag (`install()`, `check()`), and the interruptible `sleep()`/`recv()` every long wait uses |
  | `capture.rs` | `--capture` (#31): `capture()`, a workspace `Node` to the `--layout` file rebuilding it, pure |
  | `launch.rs` | `SwayLaunch`, `ActionRequest`, `BuildLocation`, `build_actions()`/`execute_plan()`/`run()`, `debug_events()` |

**Layering rules:**

- Only `ipc.rs` opens connections, sends commands and subscribes to events. Its callers are
  `query.rs` (every tree and output read), `confirmation.rs`'s `run_*` drivers and
  `hold_geometry()`, `launch.rs`'s `BuildLocation::apply()`, `Staging` (through `send()`) and
  `debug_events()`, and `run.rs`'s rollback through `kill_container()`; a new caller is added to
  this list in the same change. A new predicate goes in `tree.rs`; if it can't answer
  from a `&Node`, the fetching half goes in `query.rs` and the deciding half still goes in
  `tree.rs`, where a unit test can reach every case without scripting a compositor.
- `tree.rs` walks the tree in exactly two places, `ancestors_of()` and `descendants()`. A new
  predicate is a lookup on one of those, never another recursion over `nodes`/`floating_nodes`.
- `impl SwayAction`/`impl ExecAction` are split on purpose: what an action *is* in `action.rs`, how
  its effect is confirmed in `confirmation.rs`.
- Unit tests live in a sibling `tests.rs` (`#[cfg(test)] mod tests;`), never inline, so the
  coverage gate can exclude test code by filename. `test_support.rs` holds fixtures shared between
  modules' tests. The same applies to `layout.rs`/`template.rs`/`run.rs`, and to `main.rs`, whose tests are
  `src/main/tests.rs` (a crate root's `mod tests;` would mean `src/tests.rs`, hence its `#[path]`).

### Input validation

Every value a user supplies is validated once, by a function both the CLI (`#[clap(value_parser =
...)]`) and the TOML schemas call, so the two can't disagree. Each function's doc comment explains
what it rejects and why:

- `parse_size()`/`parse_position()` — parse straight into the typed `Size`/`Position`, bounded to
  what everything downstream can represent (`Size` is `i32` because Sway's rects are). Coordinates
  are workspace-relative, as Sway's `move position` is (#64).
- `validate_sway_string_argument()` — every value interpolated into a quoted Sway argument
  (`--mark`, `--mark-match`): no `"`, `\`, `$` or control character but tab, since Sway mangles
  each of them. `quote_sway_string()` still escapes, as defense in depth.
- `validate_workspace_argument()`/`validate_output_argument()` — the same, plus Sway's own keywords
  (`WORKSPACE_KEYWORDS`/`OUTPUT_KEYWORDS`, compared ignoring case), which Sway acts on even when
  quoted. Names are compared ignoring case everywhere, as Sway does (#5).
- `parse_workspace_number()`/`validate_workspace_number()` — `--workspace-number`/
  `--on-workspace-number`/`workspace_number`: `0..=i32::MAX`, since Sway's `num` is an `int` and
  -1 is what every unnumbered workspace has. Either form becomes a `WorkspaceTarget` (`Name` or
  `Number`), the one type `ActionKind::Workspace`/`WorkspaceToOutput` and `BuildLocation` carry:
  `sway_argument()` renders it (a number unquoted after `number`, or Sway reads it as a name) and
  `WorkspaceTarget::matches()` decides it against a workspace's name and `num` (#43).
- `workspace_choice()`/`check_relative_move()` — the two cross-field placement rules, for every
  file schema (a layout step, a binding, a template's `[layout]` table): a workspace by name or by
  number, not both, turned into the `WorkspaceTarget` (`WorkspaceTarget::from_pair()`, which the
  CLI uses too), and no workspace move alongside `new_column`/`new_row` (#45). The CLI keeps clap's
  `conflicts_with` for them; `tests/placement_rules.rs` checks every front end gives the same
  answer.
- `validate_exec_command()` — the launch command is sent raw, so it rejects what Sway's own parser
  would split on (an unquoted `,`/`;`, any newline/CR/NUL), and a shell comment, an unclosed
  quote or a `\'` inside single quotes (where Sway's quoting and sh's disagree, #151), any of which
  would cut the `( ... )` it runs in short. `run_single_sway_command()` backs it
  up twice: the pre-send `sway_command_split_problem()` check (below), and treating a
  multi-outcome reply as an error.
- `render_argv()` — a step's or binding's `command` given as a list of words (#77), turned into one
  quoted string by `layout::deserialize_command()` at parse time, so nothing past parsing knows
  which form a file used. Each word is single-quoted, with `\` written outside the quotes because
  Sway reads a backslash inside single quotes as an escape; `$` is refused because Sway substitutes
  its variables even there (both confirmed live). Files only: on the CLI, `--` keeps meaning "the
  command string", by decision.
- `launch_setup()` — `--cwd`/`--env` and a step's or binding's `cwd`/`env` (#113): the directory is
  resolved (`~`, relative to the caller's directory, must exist; `DirectoryCheck::FormatOnly` under
  `--validate`/`--dry-run` skips only the existence check, #146), then both are rendered as a
  `cd '…' && export NAME='…'` prefix with `render_argv()`'s quoting, carried as
  `SwayLaunch.launch_setup` and put between the PID marker and the command by
  `ExecAction::sway_command()`. Only for a launch; `--dry-run` shows the prefix. A binding's
  `cwd`/`env` is checked by the template's `resolve()` too, with the run's `DirectoryCheck`, so its
  mistakes are the bindings file's, not the template step's (#157).
- `validate_sway_command()` — `--sway-command`/`sway_commands` (#90), checked for files by
  `layout::deserialize_sway_commands()` at parse time: the first word must be in
  `SWAY_WINDOW_COMMANDS`, since Sway runs any command behind `[con_id=N]`, `exec`/`bindsym`/`exit`
  included (confirmed live), and a shared file may only change its own window. No unquoted `,`/`;`,
  control character but tab, `$`, or leading `[`, so one entry stays one command, and no unclosed
  quote or trailing unpaired `\` (`open_quoting()`, also applied by `validate_exec_command()`),
  which would carry on into the next entry once a batch joins them (#123). Behind both,
  `run_single_sway_command()` and `run_sway_commands()` refuse, before sending, a command
  `sway_command_split_problem()` says Sway would split into other than the parts it was built from.
- `validate_non_blank_argument()`/`require_non_blank()` — free-form identifiers.
- `validate_timeout_argument()`/`validate_timeout_seconds()` — `1..=MAX_TIMEOUT_SECONDS`; the
  deadline is computed (`deadline_after()`) before any command is sent.
- `validate_settle_argument()`/`settle_duration()` — `--settle`/`settle`: `1..=60000` ms, and
  only alongside a pixel height/width or a position, the geometry `hold_geometry()` can check. A
  binding's `settle` is range-checked in `validate_overrides()`, which can't yet tell `px` from a
  `{param}`.

### Targets

A `Target` is built only through `Target::from_fields()` (#20), which every front end calls with a
`TargetFields` and renders its `TargetError` in its own style (`FieldStyle`). The types make every
rejected combination unrepresentable: `Exec { command, filter: Option<LaunchFilter>, title }`,
`ConId(i64)`, or `Existing { criteria: WindowCriteria, wait }`, whose criteria hold at most one
`WindowMatch` plus an optional title and at least one of the two. `tests/target_rules.rs` checks
the CLI, a layout step and a template binding give the same answer for every combination.

A title (`--title`/`title`, #14) is a `TitlePattern`: an unanchored `regex-lite` regex (not
`regex`: same syntax, without about a megabyte of Unicode tables), compiled by
`parse_title_pattern()` when the flag or file is parsed (clap's value parser, serde for the
files), so `Target` only borrows it and stays `Copy`.

`SwayLaunch::resolve_container_id()` turns the target into a container id: `Exec` launches,
`Existing` resolves exactly one match from the whole tree (scratchpad included) or errors, `ConId`
uses the id. `Existing { wait: true }` (`--wait`, #108) goes through
`run_wait_for_existing_window()` instead: subscribe, then re-read the tree on every window event
and every `LATE_PROPERTY_RECHECK_INTERVAL` (a late app_id sends no event) until
`tree::resolve_awaited_matches()` finds one, errors on several, or `--timeout` runs out. A `ConId` run with actions is then looked up once (`ensure_con_id_target()`, deciding
in `con_id_target_problem()`); a bare `--con-id N` is never looked up, so it works without a
compositor (#107).

### Actions and how each is confirmed

`ExecAction` launches a window; `SwayAction` acts on a resolved container and holds a
`kind: ActionKind`, one variant per flag, plus `WorkspaceToOutput`, which no single flag produces.
`sway_command_verb()` is each action's one serialization point (`batch_command()` puts
`[con_id=N]` in front); `flag_name()` is its stable identity in `--json`. Every `SwayAction` is
confirmed by one engine, `run_batch()` (#133), whether it is sent in a batch or alone; within it,
`ActionKind::is_event_confirmed()` decides which of two ways:

- **Event-confirmed** (`Floating`, `Fullscreen`, `Focus`, `Workspace`, `Output`, `Mark`,
  `Scratchpad`) — wait until an event for the container arrives **and** `state_satisfies()`
  confirms the requested state on a fresh tree read (an event alone proves something happened,
  not that our state applied). Before sending, a kind `is_skipped_when_already_in_effect()`
  (every one but `Mark`) whose state already holds is left out of the command, read only once the
  subscription is open; both doc comments explain why. A split container is refused up front for
  the kinds Sway only reports for views (`ActionKind::needs_a_window()`, #93).
- **Polled** (`Split`, `Sticky`, `NewColumn`, `NewRow`, `Height`, `Width`, `Position`,
  `WorkspaceToOutput`, and `SwayCommand`, which `polls()` never confirms) — the container's
  existence is checked before *and* after the `--wait-time` sleep (fast failure, and an error naming
  the closed container; Sway itself refuses a missing `[con_id=N]` on every supported version,
  #158), `poll_baseline()` is taken,
  then the command is sent and polled for. `polls()` decides whether a kind can be confirmed at
  all; `poll_shows()` judges one from a tree the caller read, and every tick reads one tree on a
  shared connection for every action still polling (#187); the grace is `poll_grace()`
  (`WAIT_TIME_POLL_GRACE`, or the shorter `MOVE_POLL_GRACE` for the `poll_baseline()`-based
  `NewColumn`/`NewRow` check). A kind that can't be confirmed reports `Unconfirmed`, never an
  error.

- **Exec** — `run_wait_matching_exec_event()`. The command runs as `exec export
  SWAY_LAUNCH_PID_MARKER=<token> && ( <command> )` and a `New` window is ours once
  `launch_evidence()` finds the marker in its process's environment, or an ancestor that is this
  launch's (#96): marked in its environment, or Sway's own `sh -c` for the launch, known by its
  command line (`launch_shell_token()`). A window `is_sibling_launchs_window()` says belongs to
  another launch of the same `ConcurrentLaunches` group (`--concurrent`'s staging, each launch
  recorded by `register_launch()`) is never a fallback candidate. The `( ... )` keeps that shell alive under bash, which
  otherwise execs the last command of an `&&` list (confirmed live); `validate_exec_command()`
  refuses a `#` comment, which would swallow the `)`. A content-matching window without it is held as a
  fallback and taken once the marker was seen and then gone (`MarkerTracker`: "gone" means seen,
  then gone, never merely absent) → `LaunchOwnership::Launched` if its process started before every
  marked one (`predates_launch()`, from `process_start_ticks()`; a failed command is gone too,
  #124), else `Adopted`, or after `PID_MARKER_FALLBACK_GRACE` → `Adopted`. Windows the filter or title rejected are rechecked for a
  late app_id/class/title (`recheck_launched_windows()`) and reported as `UnmatchedWindow`s on
  timeout, with `exec_timeout_message()` explaining what was seen (whether the command still runs is
  `launched_command_running()`'s call: the marker, or failing that the launch's own shell, #152;
  a rejected window explains the timeout only when it is `launched_by_this_run`, anyone else's is
  listed after the command's state, #161). With a title, a `Title` event
  re-runs `matches_window()` too, but only for a window whose `New` arrived during this run
  (`opened_this_run`): an older window renaming itself is never this launch's.
- **Batched** (#32) — consecutive kinds `ActionKind::joins_batch()` accepts (`Split`,
  `Floating`, `Sticky`, `Fullscreen`, `Focus`, `Mark`, `Height`, `Width`, `Position`,
  `SwayCommand`) go to `run_batch()` together as one `[con_id=N] a, b, c` command
  (`batch_command()`), every other kind alone, and both are confirmed per action by the rules
  above in one loop. Existence check, subscribe, one state read for the already-there check,
  `--wait-time` once if anything polled is sent, send, then `batch_failure()` maps Sway's per-part
  replies back to actions (Sway runs every part even after one fails, and stops only at a parse
  error; confirmed live). Each event wakes one state read for every action whose event arrived;
  polled actions are polled in the same loop. Moves and `Scratchpad` stay out of a batch: each
  changes what the next action is judged against. With `--settle`
  (#17, `settle_duration()`), `hold_geometry()` then holds the confirmed pixel sizes and position,
  sending them again together when the client undoes them (at most `MAX_GEOMETRY_REAPPLIES`
  times, then `Unconfirmed`). It reads the tree itself once per tick (`tree_on()`) and tells apart
  what the confirmation poll folds into "doesn't match", a failed read and a closed window: an
  unreadable tree ends the hold `Unconfirmed`, and a closed window fails without re-sending. An
  error belonging to no one action is prefixed with every verb it stopped (`batch_verbs()`).

**Every `ActionKind` match in `confirmation.rs` names each variant** (no `_` arm), so a new kind
doesn't compile until both its confirmation path and its state/poll arms are decided.
`every_event_confirmed_kind_can_recognise_its_own_end_state` pins the event half.

### Orchestration

`build_actions()` plans actions from the `SwayLaunch`'s `ActionRequest` (every action flag plus
`settle`, in a struct of its own so a launch can drop them all with its `Default`, #180) in a
fixed order without touching IPC (pinned by
`build_actions_includes_every_flag_in_the_documented_fixed_order`): `NewColumn` → `NewRow` →
`Workspace` → `Output` (or `WorkspaceToOutput`, see `output_action()`) → `Split` → `Floating` →
`Sticky` → `Fullscreen` → `Focus` → `Height` → `Width` → `Position` → `Mark` → each
`SwayCommand` in the order given → `Scratchpad`.
`Scratchpad` is last so everything else applies while the window is visible. `NewColumn`/`NewRow`
can't be combined with `Workspace` (#45).

`execute_plan()` runs the plan and records an `ActionRecord` per action, handing consecutive
batching actions over together (`plan_groups()`) and each to `run_batch()`. Immediately before each
`NewColumn`/`NewRow` it asks `current_move_skip_reason()`, which delegates to
`tree::move_skip_reason()`: a floating window, or (with several outputs) a fullscreen window or one
Sway would escalate to the next output (`is_at_the_trailing_workspace_edge()`, a replay of Sway's
own `container_move_in_direction()`), is recorded as `Skipped { reason }` instead of being sent.
`reason` is a `SkipReason` (in `tree.rs`), whose `as_str()` is the published `--json` value and
`explanation()` the `--verbose` line, each an exhaustive match, so a new reason needs both (#183).

Every IPC call opens its own connection through `new_connection()`, which bounds each round trip
by `IPC_ROUND_TRIP_TIMEOUT` (independent of `--timeout`) and finds the socket with `socket_path()`:
`SWAYSOCK`, then `I3SOCK`, as `swaymsg` does, with no other fallback (`swayipc`'s
`Connection::new()` is never used). Only once a connection fails does `running_sway_sockets()`
look for live `sway-ipc.<uid>.<pid>.sock` sockets in `$XDG_RUNTIME_DIR`, for the error to name
(#162). `event_loop()` bounds the subscribe handshake the same
way. A poll cycle and the move guard each share one connection across their reads.

Before a run's first request to the compositor (a direct-CLI run unless
`SwayLaunch::needs_compositor()` says it's a bare `--con-id`; a `--layout`/`--template` run once,
before `BuildLocation::apply()` or its first step that needs one), `check_compositor_version()`
sends one `GET_VERSION` (#147): `ipc::compositor_version()` reads the reply itself, since
`swayipc`'s `Version` drops `variant`, bounded by the shorter `VERSION_REQUEST_TIMEOUT`, and the
pure `version_report()` turns it into a `--verbose` line plus a stderr warning (every mode,
`--json` included) for a Sway older than `OLDEST_SUPPORTED_SWAY` or a compositor that isn't Sway.
Never an error: an unreachable compositor skips the check and is left for the run to report.

### Output contract

- stdout carries exactly one value: the container id, or the `--json` object. The exceptions are
  modes whose output *is* the job (`--debug-events`, `--dry-run`, `--validate`, `--list-templates`,
  `--list-windows`, `--show-template`, `--completions`, `--capture`).
- Every stdout write goes through `print_line()`/`print_str()`, so a closed pipe is a clean exit
  (`exit_on_write_failure()`), never a panic. `run_steps()`'s per-step ids use
  `print_line_deferring_failure()`, so a closed stdout never stops a layout halfway.
- Every stderr write goes through the `diagnostic!` macro, never `eprintln!`, which panics on a
  closed stderr.
- Errors go to stderr through `fail()`/`fail_with_rollback()`: labelled plain text
  (`severity_label()`, colored only when `stderr_wants_color()`), or a JSON object under `--json`.
  Text that can quote a user's file is passed through `escape_control_characters()`, which also
  escapes the bidi formatting characters a terminal would reorder the line around (#182).
- `--json` success is `RunOutcome` rendered by `actions_json()`/`action_record_json()`: each action
  has `kind` (`flag_name()`), `command` (absent when skipped) and a `status` of `changed`,
  `already_satisfied`, `unconfirmed` or `skipped` (with `reason`). A failed run's error object
  carries `container_ids`/`containers` for steps that completed, `container_id` for the failing
  step's container, `unmatched_windows`, and the three non-overlapping rollback lists
  (`PartialProgress`, `RollbackOutcome`, `classify_rollback()`).

### Standalone modes

`--completions`, `--list-templates`, `--show-template`, `--list-windows`, `--capture` and
`--debug-events` are dispatched first in `main()`, before any target is resolved. `--capture`'s
`capture()` builds each split level by level, since splitting a container's only child changes its
layout instead of nesting one: every child's first window opens beside its siblings first, then each
nested child's is split and focused. Sizes come next, through each child's first window, in the
order its module doc explains, then fullscreen, as a retarget step of its own (#189: Sway keeps
focus on a fullscreen window, so later launches open behind it, and `share_of()` ignores the
`1.0` percent it reports) (`every_shipped_template_round_trips_through_capture` holds it to
every template's shape). A command guessed from an app_id or class that isn't a plain program name
(`is_plain_program_name()`) is written in the list form, since the client sets it (#125). It's the
one recursion over a tree outside `tree.rs`. `--layout` and `--template` build their
`conflicts_with_all` with `per_window_and()` from `PER_WINDOW_ARGS`, and every other standalone
mode with `standalone_mode_conflicts()`, which adds `RUN_ARGS` (the flags only a
`--layout`/`--template` run uses), so neither kind of flag is ever silently discarded (#136).
`--list-windows` alone keeps `LIST_WINDOWS_FILTERS` (`-a`/`-c`/`--mark-match`/`--title`, via
`standalone_mode_conflicts_but_filters()`): they narrow its listing
(`tree::window_listing()`) to what `--existing` would choose between, so its rows and
`matching_container_ids()` must keep agreeing on what a match is: both filter on
`is_listable_candidate()` (a window, or a marked split container) before the criteria (#144).
`per_window_args_conflict_with_every_standalone_mode` and
`per_window_args_lists_every_flag_a_layout_step_mirrors` keep the list complete, and the
`run_args_*` tests do the same for `RUN_ARGS`. clap's `requires` is not used for
`--bindings`/`--apps`/`--rollback-on-error`/`--on-workspace`/`--validate`: `main()` checks those by
hand (`layout_or_template_only_flags()`), because the derive macro doesn't enforce `requires`
reliably. A new entry in
`DEBUG_EVENT_SUBSCRIPTIONS` is checked against Sway's documented event list by
`debug_event_subscriptions_cover_every_sway_ipc_event_type_once`; recheck it when bumping `swayipc`.
`--debug-events --json` names each event by its `EventType`'s serialization, Sway's own name
(`barconfig_update`), through `debug_event_type()`, never by case-converting the `Event` variant
(#191); a variant added there needs an arm too.

### `--layout`, `--template`, `--dry-run`, `--validate`

- All three modes plan each step through `plan_step()` (duplicate `id` check, then
  `to_sway_launch()`), `--dry-run`/`--validate` via `plan_steps_offline()` with placeholder ids. A
  real run calls `plan_steps_offline()` too, before the run lock and anything else, so a mistake in
  a later step fails it before the first window opens (#179); its loop then plans each step again
  against the real ids. A new per-step rule goes in `plan_step()` or `to_sway_launch()`, never in one mode's loop, or
  `--validate` can accept what a real run rejects. Errors are formatted once, by `step_error()`.
- A step's `id` names its container; a later `target_id` resolves to it through `resolved_ids`. A
  template's `slot` becomes the step's `id`, so templates use the same mechanism.
- `run_steps()` applies the run's `BuildLocation` (a template's `[layout]` table, or
  `--on-workspace`/`--on-output`, each overriding its half of it) once before the first step, so the shape is built in place. Every workspace switch goes through `focus_command()`, whose `--no-auto-back-and-forth` keeps Sway's `workspace_auto_back_and_forth` from switching to the previous workspace instead (#142). It warns when that
  workspace already holds tiled windows, and sets `require_launch_on_focused_workspace`, which
  fails a launch if focus moved while it waited (`focus_moved_during_launch()`, #45/#105), and
  `warn_on_misplaced_launch` for a launching step: before its actions, `SwayLaunch::run()` reads
  the tree once and `tree::launch_placement_problems()` says whether a config rule (`assign`,
  `for_window`) put the window off the focused workspace or floated it, unless the step moves or
  floats it itself. Each problem is a `RunOutcome::warnings` entry, printed and added to `--json`'s
  `"warnings"` by `run_steps()` (#160). `launch_only()` clears it, so a staged window is judged
  after `arrange_staged()` has moved it.
- `run_steps()` takes a run lock (`acquire_run_lock()`, #55) before `BuildLocation::apply()`: an
  exclusive `File::try_lock()` on `$XDG_RUNTIME_DIR/sway-launch/<hash>.lock`, keyed on the
  canonical file or template name (stdin by its contents, `run_lock_source()`) plus the build
  workspace's `WorkspaceTarget::run_lock_key()`: its number when its name starts with one (Sway's
  `num`, which a rename keeps, #178), else its name lowercased, so every spelling of one workspace
  takes one lock (#129). With no `--on-workspace`, only a launching run reads it from the tree
  (`unnamed_build_workspace()`: the focused one, or the one `--on-output` shows), so a
  retarget-only run still makes no IPC call of its own. Held until the process exits.
  `--no-lock` skips it; no runtime directory skips it with a `--verbose` note. It is why
  `rust-version` is 1.89.
- `--concurrent` (#73), or a top-level `concurrent = true` in a layout or bindings file (#121,
  OR-ed with the flag into `run_steps()`'s `concurrent` argument; a template file can't set it, since
  it names no applications), splits `run_steps()` in two: `stage_launches()` switches to a per-process staging workspace (`Staging`, in
  `launch.rs`) and runs every `command` step's launch at once on a thread each, actions stripped by
  `launch_only()` (its `ActionRequest` replaced by the default, pinned by
  `launch_only_keeps_the_launch_and_drops_every_action`); `reject_shared_windows()` fails a later step whose window an earlier one already
  has. Then each step runs in order as before, a staged one through `arrange_staged()`: the window
  is moved to the build workspace and focused (where a new window would open; every template's
  shape matches, `every_shipped_template_builds_the_same_shape_with_concurrent`), then the step
  runs against `Target::ConId` with the launch's ownership carried over. Every failure path goes
  through `RunProgress::fail()`, which calls `release_staged()` first, so no window is left on the staging workspace and each counts as
  progress and for rollback. While staged windows wait, a step's `SwayLaunch.excluded_containers`
  holds them (`staged_window_owners()`), so `Existing` resolution (`tree::existing_candidates()`)
  sees what a sequential run would; a `ConId` naming one fails before acting (#190).
- A run tracks a workspace by its node id (`tree::WorkspaceIdentity`, read through
  `query::workspace_now()` with a `WorkspaceLookup`), never by a name read earlier: workspace-icon
  daemons rename a workspace whenever a window opens on it, keeping its id (#178). So
  `focus_moved_during_launch()` compares ids, and `Staging` keeps the build workspace's id and
  sends each `bring()`/`leave()` the name it has right then, re-reading which workspace that is
  after each, since Sway destroys an empty build workspace when focus leaves for the staging one.
  The run lock's key still uses the name (#129).
- `--continue-on-timeout` (#19) carries `run_steps()` past a step whose `RunError` has
  `window_timed_out` (set only where no window was resolved: a launch's timeout and
  `--existing --wait`'s), recording it in `timed_out_steps`; a later `target_id` naming its id is
  skipped (`skipped_steps`). Any other failure still goes through `fail_step()`. The result object
  still goes to stdout, then the run exits 1, a closed stdout included
  (`exit_on_write_failure_with_status()`, #165). A binding's `timeout`/`wait_time` override the step's
  like its placement does.
- `run_steps()` installs the signal handlers (`interrupt::install()`: SIGINT, SIGTERM and SIGHUP,
  #15/#128) before its first step and checks them before each one; every wait that can run long (`run_batch()`,
  the exec and `--existing --wait` loops, `--wait-time`'s sleeps) goes through
  `interrupt::recv()`/`interrupt::sleep()`, so a signal fails the run through `fail_step()` within
  50ms, and `fail_with_rollback()` exits `128 + signal`. A direct-CLI run installs nothing and
  dies to a signal as before. Under `--rollback-on-error` (and not `--continue-on-timeout`), a
  launch timeout or interrupt watches `LATE_WINDOW_GRACE` more for a marked window
  (`late_marked_window()`, which doesn't use `interrupt::recv()`, since the first signal is still
  set and a second exits on its own) and hands it to rollback as the failed step's `Launched`
  container (#127). A timeout also hands over every still-open window the filter rejected whose
  process carries the marker (`launched_by_this_run`): the first as `container_id` when no late
  window came, the rest in `RunError.also_launched`; `failed_launch_windows()` lists them for
  `run_steps()` and `release_staged()`, and under `--concurrent` each is brought off the staging
  workspace first, the failure `take_staging_failure()` picked included (#168).
- `--rollback-on-error` closes only windows whose `launch_ownership` is `Launched` — never
  retargeted or `Adopted` ones — most recent first (`rollback()`, `send_rollback_kills()`), waits
  `ROLLBACK_CLOSE_GRACE` once, in total, for all of them to actually close, and stops at the first unanswered kill. It
  undoes launches, not mutations to existing windows.
- A template's `[params]` (#92) names values a step's `height`/`width`/`position` takes by
  writing `"{name}"` as the whole value: plain substitution, no expressions. `parse()` checks every
  reference, every default against each field using it (`Template::check_param()`) and that each
  param is used; `resolve()` applies a bindings file's `[params]`, then `--param`, each checked the
  same way and reported against its own source (`ResolveError::Bindings`/`Param`). A ratio-only
  variant of a built-in is a param, not another file.
- `template::parse()` rejects a repeated slot; `resolve()` reports a `ResolveError` against the
  file that holds the mistake (bindings or template). A template file requires a `[template]` table
  (`description` as a full sentence, `category`).
- Built-in templates are every file in `templates/`, embedded with `include_dir` as
  `BUILTIN_TEMPLATES`. `resolve_template_contents()` treats a `.toml` suffix as a path and anything
  else as a built-in name, so the two never shadow each other; `builtin()` rejects a path separator,
  keeping `templates/` flat.

### Keep these in sync

Each of these is a second copy of something, and drift is a bug to fix in the same change, not a
follow-up:

- **`Args` ↔ `LayoutStep` ↔ `TemplateStep` ↔ `Binding`.** `schemas_mirror_args_field_for_field`
  checks the field lists (exceptions in `CLI_ONLY_ARGS`, `LAYOUT_ONLY_FIELDS`,
  `BINDING_SUPPLIED_FIELDS`, `TEMPLATE_ONLY_FIELDS`, `RESOLVE_SUPPLIED_FIELDS`,
  `BINDING_ONLY_FIELDS`, and `BINDING_OVERRIDE_FIELDS`: a binding's placement replaces the step's, #30). It doesn't check wiring: a new flag also needs `to_sway_launch()`,
  `resolve()` and README.md's "Layout files"/"Templates" field lists. The schemas duplicate fields
  rather than `#[serde(flatten)]`, which weakens `deny_unknown_fields`.
- **README.md's help block** is `Args::command().render_help()` verbatim
  (`readme_help_block_matches_the_rendered_help`). `Args`' `///` comments are user-facing help;
  maintainer notes go in `//` comments (`long_help_carries_no_maintainer_notes`). Every field
  sets `help_heading` to one of the `HEADING_*` sections (#119); its first doc line is the `-h`
  summary (short, no full stop) and any further paragraphs are `--help`'s detail, kept as
  written by `verbatim_doc_comment`, so wrap them by hand to fit 80 columns. `USAGE` and
  `EXAMPLES` are the usage and examples blocks, and `render_man_page()` puts `EXAMPLES` under
  the man page's own EXAMPLES heading.
- **Templates**: README.md's "Templates" table, `screenshots/`, `tests/template-shapes/` and each
  file's header comment, pinned by the tests described under "Screenshots" below.
- **`tests/live_sway.rs`**: see "Tests" below.
- **`CHANGELOG.md`**: see "Changelog" below.
- **This file**: a function named here in backticks with `()` must exist in `src/`, `tests/` or
  `scripts/` (`claude_md_names_only_functions_that_exist`). Rename or remove one, and this file
  changes in the same commit. The same goes the other way: a comment in `src/` or `tests/`
  naming a function (any `name()`, in backticks or not), or a CLAUDE.md section by its heading,
  must name one that exists
  (`source_comments_name_only_functions_that_exist`,
  `source_comments_name_only_claude_md_sections_that_exist`).

### Tests

- **Unit tests** (`cargo test`) cover every pure function, in each module's sibling `tests.rs`.
- **Headless integration tests** in `tests/` drive the compiled binary
  (`CARGO_BIN_EXE_sway-launch`, only set for files under `tests/`): `completions.rs`,
  `json_output.rs` (stdout/stderr/`--json` shapes, via `--con-id` or an unreachable socket),
  `layout.rs` and `template.rs` (end to end, `--dry-run`, `--validate`, every shipped example and
  built-in), `target_rules.rs` (see Targets), `placement_rules.rs` (see Input validation).
  Shared helpers live in `tests/common/mod.rs` (temp files, `sway_launch()`,
  `assert_conflict_error()`); add new ones there. Every one of them runs
  the binary through `sway_launch()`, which points it at a socket that can't exist and a private
  `XDG_RUNTIME_DIR`, so no test depends on the code under test happening to make no IPC call, and
  no run lock lands in the developer's own runtime directory (#137);
  `integration_tests_run_the_binary_only_through_the_shared_helpers` fails on a direct
  `CARGO_BIN_EXE_sway-launch` use anywhere under `tests/` but `common/` and `live_sway.rs`. A
  flag-conflict test uses `assert_conflict_error()`, never a bare failure check, which keeps
  passing after the conflict is dropped.
- **A Unix socket's directory goes under `/tmp`, never `TMPDIR`** (#130): a socket path is
  limited to about 108 bytes, which a long `TMPDIR` (a sandbox's or CI container's per-session
  directory) uses up. A test binding one takes its directory from `TestTempDir::for_sockets()`;
  the scripts that start Sway create its `XDG_RUNTIME_DIR` with `mktemp -d -p /tmp` (or
  `dir="/tmp"` in Python).
- **Fake-Sway tests** (`tests/fake_sway.rs`, part of `cargo test`) drive the binary against
  `tests/common/fake_sway.rs`, a fake Sway IPC server on the far side of `SWAYSOCK` (#103): no
  production code knows it exists. It replays a recorded tree, outputs and missing-container reply
  per supported version (`tests/fake-sway/sway-1.9/`, `sway-1.11/`, written by
  `scripts/capture-fake-sway-fixtures` against a real compositor; 1.9 was recorded in an
  `ubuntu:24.04` container), and each test's handler decides how a command is answered: the tree
  changes and window events it causes, now or later (`after()`), or none. A launch's `exec` runs
  for real through `sh -c`, so the PID-marker logic reads a real `/proc`; `when_marked()` acts once
  the marked process exists. This is the tier for event orderings a live compositor can't be made
  to produce on demand (an event without its state, a late app_id, a hand-off to an unmarked
  process) and for version-specific tree shapes; whether real Sway behaves as a handler scripts it
  is still the live tier's to show. Re-record the fixtures when a new Sway version changes what
  they hold, or when the oldest supported version moves.
- **Live tests** (`tests/live_sway.rs`, `live-sway-tests` feature) run only through
  `scripts/run-live-sway-tests`, against a throwaway headless compositor; `live_socket()` panics
  otherwise, so they can never touch a real session. They exercise everything IPC-touching, and
  every shipped template (compared with its recorded shape in `tests/template-shapes/`; rewrite
  with `UPDATE_TEMPLATE_SHAPES=1`), layout and basic example script. Every test calls
  `fresh_workspace()` first, adds outputs only through `CreatedOutput` and closes windows through
  `KillOnDrop`, so each starts from the same state. Assert a specific path through `--json`'s
  per-action status, not elapsed time alone. `--class` has no live test (no X11 client available).
- **This file's coverage must stay complete.** A change that adds or changes a flag, an action, an
  example script, a `--layout` file or a `--template` file adds or updates its `tests/live_sway.rs`
  case in the same change, driving the shipped file or flag itself, and runs it.

### Declined proposals

Decided against, with the reasoning in `docs/design-notes.md` or the issue. Don't re-propose one
without a reason that wasn't considered then.

- A `mutations_not_rolled_back` field in `--json` errors: needs per-step mutation tracking;
  revisit only with real state restoration (2026-09-03 review).
- A template step `id` separate from its `slot` (2026-09-03 review).
- Capping `--wait-time` (2026-09-03 review); the early existence check removed the wasted wait.
- Accepting a post-command state read as confirmation: Sway applies before replying, so it would
  replace event confirmation outright (2026-09-03 review).
- A structural (parent/index) predicate for `NewColumn`/`NewRow`, or always reporting them
  `Unconfirmed`: both measured worse than the bounded `rect` check (2026-09-03 review).
- A mocking layer or trait over Sway IPC: the live tier covers it, and the socket-level fake
  (`tests/fake_sway.rs`, #103) covers the orderings it can't, with no production change.
- A clap `ArgGroup` for the standalone-mode conflicts: clap's error lists every group member.
- `#[serde(flatten)]` for the shared schema fields.
- Restoring the default `SIGPIPE` disposition: needs `unsafe` and a dependency.
- `anstream`/`anstyle` for two colored labels.
- A single `state.rs` instead of `tree.rs`/`query.rs` (module split, 2026-09-03).
- Supporting a split container for `--workspace`/`--output`/`--focus` (#93).
- Resolving an output's make/model/serial identifier (#5).
- Moving the window to the output before `WorkspaceToOutput` (#6): it did nothing.
- Making `--position` coordinates global with `move absolute position` (#64).
- Looking up a bare `--con-id` with no actions (#107): it would need a compositor for no benefit.

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
  by default and has no app_id there — `--floating`, `--mark`, `--width`/`--height`). README.md's "Scripts"
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
here is also embedded into the binary as a built-in `--template <name>` — see "`--layout`, `--template`, `--dry-run`,
`--validate`" under Architecture above for how. It lives at the repo
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
real and captured. Between the two passes the script waits (bounded, 5s) until Sway has actually
removed every first-pass window, not just until `kill` returns: Sway replies to `kill` before the
windows are gone, and a leftover still mapped on the same workspace when the second pass starts
would have the new windows placed relative to it, capturing a shape the template doesn't produce. `figlet`'s own `-c` (center) flag is deliberately not used: it pads to figlet's
own default 80-column canvas rather than the pane's real width, which reintroduced the same
wrapping bug this two-pass measurement exists to avoid; labels are left-aligned instead.

Each binding's `command` is the path of a small per-slot launcher script the tool writes into the
compositor's runtime directory, not an inline `foot ... sh -c '...'` string. A binding's command
goes through Sway's command parser and then `sh -c`, and slot names are free-form (the schema
only requires them non-blank), so pasting a slot or `--font` into that string let a space split
it into separate words (`slot = "main editor"` silently measured and labeled nothing) and a
quote, `;`, `$` or backtick run as shell code. The launcher `shlex`-quotes both values and hands
them to `figlet` as positional arguments; the only thing left in the Sway-parsed string is the
launcher's own path, which the tool refuses if it contains anything
outside `[A-Za-z0-9_./-]`. The bindings TOML escapes control characters too and is parsed back
with `tomllib` before use, so a slot name TOML can't carry raw fails there rather than inside
`sway-launch` against a temporary file the user never saw. A slot containing a control character
still launches, but Sway reports no window title for it, so that one slot falls back to the
default label size.

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

The hand-copied parts of this are pinned by tests in `src/template/tests.rs`, the same way
`readme_help_block_matches_the_rendered_help` pins the help block:
`readme_templates_table_matches_every_builtin_template` requires exactly one README row per
built-in, with the category and slot count `--list-templates` reports;
`every_builtin_template_has_a_screenshot_unless_the_screenshot_script_excludes_it` requires
`screenshots/<name>.png` and a matching preview cell for every template outside the screenshot
script's `EXCLUDED_TEMPLATES` (read back from the script itself, so it stays the one list — keep it
a single-line set literal), a `—` cell for those inside it, and no stray PNGs; and
`builtin_templates_header_comment_repeats_its_description` requires each file's header comment to
restate its `[template]` `description`. None of them can tell whether a screenshot still depicts
its template's current shape — regenerating after a shape change is still on whoever changes it.

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
works the same as running it directly, using the same pinned tool versions). That Python project
is named `sway-launch-maintainer-tools` at version `0`, deliberately not `sway-launch` at the
crate's version: it exists only to pin these tools, is never built or released, and isn't bumped
at release time — `Cargo.toml`'s `version` is the only one a release tracks, so a reader never
has to guess which of two identical `sway-launch 0.1.0`s is real. `black` (formatter)
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
    The IPC-touching code is measured too: `tests/fake_sway.rs` runs the instrumented binary
    against a fake compositor (see Tests under Architecture), so the gate counts the `run_*`
    drivers in `confirmation.rs`, `SwayLaunch::run()`, and `ipc.rs`/`query.rs` like any other
    module (#103). Until then those were exempt by module name, since only the live tier could
    reach them. The gate excludes only `main.rs` and the test code itself (`test_support.rs` and
    every `tests.rs`, which is why unit tests live in sibling files rather than inline — an inline
    test module's bodies would count as covered production code and pad the total):
    `cargo llvm-cov --summary-only --ignore-filename-regex '(^|/)(main|test_support|tests)\.rs$'`
    reproduces CI's figure. The measured baseline lives in one place, the comment above the gate
    in `.github/workflows/check.yml`, which changes with the thresholds; this file doesn't restate
    it. What's left uncovered is mostly IPC-error branches (a `get_tree()` that fails mid-run) and
    `--debug-events`' event loop.

    **Keep the decision pure and let the IPC layer fetch** — the shape
    `state_satisfies`/`container_state` and `poll_shows`/`run_batch()`'s one read per tick already
    use — rather than burying a comparison behind a `&mut Connection`. A fake-Sway test can reach it there, but only by
    scripting a whole compositor exchange for each case a unit test would cover in one line.

    `tests/live_sway.rs` is still what shows the IPC code agrees with real Sway, against a
    throwaway headless compositor via `scripts/run-live-sway-tests`, in CI and locally — a separate
    test tier from `cargo test`'s coverage-measured suite, gated behind the `live-sway-tests` Cargo
    feature so it stays opt-in. No mocking layer has been introduced on the judgment that a
    trait-based abstraction purely to unit-test thin IPC wiring isn't worth the indirection for a
    tool this size; the fake sits on the other side of the socket instead.
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

`main` was created by the 0.1.0 merge and is the repository's default branch, so visitors see the
released code; Dependabot targets `dev` on its own (`target-branch` in `.github/dependabot.yml`).

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
  - Project-specific adaptation (#16, decided 2026-10-13): when one concern spans files in a few
    unrelated places that share no useful directory (only the repo root), list them
    comma-separated instead, most central first (`src, tests, *.md:`), with `*.md` standing for
    the root Markdown files. Keep the list short enough that the title stays under 80 characters;
    past that, use the shortest scope that still says where the change is, and name the files in
    the body's bullets. The same change to the shared conventions is pending; once it lands there
    and is synced into the rule above, this sub-bullet goes.
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

**Never name the canonical repository in this one** — not its name, not its path, not a commit
SHA from it — in any file, commit message, issue, or comment. It's private and this
repo is public, so the name alone leaks something. Call it "the shared conventions" or "the
canonical repository", and its files "the canonical module", as this section already does.

**Never commit in the canonical repository from this project — not the edit, not a fix to it, not
anything.** The scratch copy at this repo's root is the entire deliverable; the user carries it
across themselves. This holds however convenient it looks and however clearly correct the change
is: the rule is absolute rather than a judgement call. Do
not offer it as an option either — a user picking it from a list is not the same as the rule not
applying, and offering it is how it gets picked. Two things follow that are easy to get wrong in the
moment:

- **Don't touch the canonical repository's state at all** — no branch checkout, no `git config`,
  no stash. A repo left on a different branch than it was found on is its own small mess, separate
  from the commit.
- **Being unable to push is not the signal.** A failed push is a second line of defence catching a
  rule already broken — a local commit in the canonical repo is already the mistake, whether or not it ever
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
4. **A prompt with four choices**, asked with a blocking question, so
   the user answers each idea before the next is shown:
   - **Keep**: leave the issue open and add it to the "to implement"
     list. Nothing is implemented during the review.
   - **Keep open for later**: leave the issue open and untouched, but
     not on the "to implement" list: it stays in the backlog for a later
     look, such as a research report to work through some other time.
     Nothing is posted to the issue.
   - **Close as won't do**: post a one-line comment giving the reason
     (ask for it when it isn't obvious from the answer), with the
     required trailer, then close the issue.
   - **Needs refinement**: leave it open and set it aside for the
     refinement round at the end.

**Refinement round.** Once every idea has been through the prompt, go
back through the ones marked for refinement, one at a time. Ask what is
unclear or missing, or propose a sharper description, then update the
issue (its title and body, or a comment) with the result. Finish each
one with the same prompt, minus refinement: keep, keep open for later,
or close as won't do.

**Close with a summary**: how many ideas were reviewed, then three
tables covering every reviewed issue:

- **To implement**: identifier, title, and a one-line gist with its
  cost, for every idea kept (refined ones included).
- **Kept open for later**: identifier, title, and a one-line gist.
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

### Deciding on an issue

**This must always be followed, with no exceptions: every question that
asks the user to decide something about an issue comes after a summary
of that issue, shown first in plain text.** The summary is never
optional, never skipped because the issue seems familiar or was
discussed earlier, and never left for the prompt to carry. This holds everywhere such a question is asked, not only in the `ideas`
and `issues` reviews: while planning, mid-task, after a review, or when an
open question on an issue comes up in passing. A decision prompt naming
only an issue's number and title asks the user to decide from memory.

The summary, written out before the prompt appears:

- **The issue's identifier and title, in bold.**
- **What it is about**, checked against the current code: the problem or
  request, where it applies, and what is still open. Size it to the
  issue, neither too long nor too short: two or three sentences for a
  simple one; for a technical issue, or one that needs more to be
  understood, as long an explanation as it takes to decide without
  opening the issue.
- **The choice being asked**, with what each option would mean.
- **Pros, cons and cost**: a few bullets each, always, even when the
  choice looks small. The cost is the rough effort (small, medium or
  large) and what it would touch; for a choice between several options,
  give each option its own pros, cons and cost.

When one prompt asks about several issues at once, summarize each of them
before it. A prompt's own option labels and descriptions don't count as
the summary: they are too short, and some interfaces hide them until an
option is selected.

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
   that fit the issue: keep (onto the "to implement" list), keep open
   for later (left open and off that list), close as won't do (with
   a one-line reason comment), needs refinement (gone through at the
   end of the batch), or, for a check or question, the answer itself,
   recorded as a comment on the issue.

   **Work in batches of 10.** After every 10 issues (or fewer, for the
   last batch), go through that batch's refinements, show the batch's
   summary tables (as at the end of the `ideas` review), then ask with a
   prompt whether to **start working on the issues this batch put on
   the "to implement" list**
   or **continue with the next 10**. Starting work ends the review; the
   issues not yet reviewed wait for the next `issues`.
4. **When no issue is waiting on a decision**, or once they have all
   been through the prompt, ask with a prompt whether to plan the open
   issues that are left: an order to work through them, grouping issues
   that touch the same code.

Never start implementing anything from the review unless the user
chooses to, in the batch prompt or otherwise.

### Change summary: `changes`

**`changes` is shorthand for "summarize what changed".** A message
consisting of only `changes`, optionally followed by a period such as
`changes since yesterday` or `changes 2026-03-01..2026-03-03`, starts the
summary below. Inside a longer message it is ordinary text. With no
period given, cover everything on the working branch not yet on `main`.

1. **Collect every change in the period**: the commit log of the working
   branch (and of `main`, if anything was merged in the period), the
   issues closed or filed, and any pull requests opened in other
   repositories the work depends on, with their state.
2. **Group the changes by kind**, each entry one line with its short
   commit hash, a plain description written from the user's side, and its
   issue numbers:
   - **New features**: something the user can do that they couldn't
     before.
   - **Changed behaviour**: something that works differently now.
   - **Bug fixes**: something that was wrong and now isn't.
   - **Tests and CI**, then **Docs and housekeeping**.

   Group within a kind by area when it runs long. Fold a commit that only
   fixes another commit in the same period into that commit's entry.
   Every commit in the period appears somewhere: brief, but complete.
3. **Then the state of things**: what still has to happen before the
   changes are live (merge and deploy order, pull requests elsewhere),
   and what is still open, such as issues waiting on a decision or ideas
   filed.
4. **End with what is worth verifying**: the checks that could only be
   done on the real systems, grouped by area, with anything already
   verified marked as done. Draw on the checks recorded in the issues
   rather than inventing new ones.

Publish it as a shareable page when the environment can host one (an
artifact or similar), otherwise write it as a Markdown file in a
temporary working directory, outside the repository. Reply with the link
or path and a few lines on the headline changes. Keep the page's name
stable for a period that is asked for again, so a later run updates it
rather than starting another.

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
live-Sway jobs actually *run*, not just their YAML: those jobs are only as good as
`tests/live_sway.rs`'s coverage, so an application feature, flag, or shipped example with no
live-Sway case is exactly as much a bug as a missing lint step — see the coverage note under
`tests/live_sway.rs` in the Architecture section and the matching Rust workflow bullet above.

GitHub Actions is set up: `.github/workflows/check.yml`'s `check` job installs the linters
(`shellcheck`/`shfmt` from the image with their versions printed, `markdownlint-cli` and `uv`
pinned), runs `scripts/run-checks` (see Commands above: fmt, clippy with `--all-features` — see the
Rust conventions above for why the feature flag is load-bearing — build, test, shellcheck/shfmt on
every shell script discovered by shebang at run time, so a newly added script is covered
automatically and the one Python script in `scripts/` is excluded by the same mechanism rather than
by name, markdownlint on every tracked Markdown file, black/ruff), then a `cargo llvm-cov` (pinned — see "Tool versions" under Commands) coverage-regression gate (`--fail-under-lines
88 --fail-under-regions 86 --fail-under-functions 87`). The checks used to be spelled out step by
step in both workflows, and had drifted between them; the shared script replaced both copies, and
the separate `lint-scripts-and-docs` job folded into `check`. The gate excludes
`main.rs` and the unit-test files themselves by filename (see the coverage bullet under Rust
conventions above), and its thresholds sit a few points under the measured baseline recorded in the comment above that
step (the one place it's written down). Until #103 it also excluded the compositor-bound
`ipc.rs`/`query.rs`, and the `run_*` drivers were most of the gap; the fake-Sway suite reaches
both. Re-measure
and adjust the thresholds as their own deliberate decision (with the comment updated) rather than
as a side effect of an unrelated change. Alongside it: two live-Sway jobs, `live-sway-1-9`
(`ubuntu-24.04`, apt's `sway`+`foot`) and `live-sway-latest` (an `archlinux` container), each
running `scripts/run-live-sway-tests` (see Commands above for why there are two); and a `cargo
audit` job via the `actions-rust-lang/audit` action (switched from `rustsec/audit-check`, which has
had no release since September 2024 and still declares the now-deprecated `node20` runtime with
nothing newer to bump to; `actions-rust-lang/audit` is a composite action with no Node runtime of
its own, so it sidesteps the issue rather than just deferring it).

**`Cargo.toml` declares `rust-version`** (currently `1.89`, for `std::fs::File::try_lock()`, which
the `--layout`/`--template` run lock uses (#55); before that the floor was 1.85, what the locked
dependency set itself needs), so a too-old compiler reports
the requirement against `sway-launch` rather than failing inside a dependency; README.md's
"Installation" section states the same figure. This is a floor for *building* the crate, separate
from the Rust conventions' "target the latest stable" rule, which is about the development
toolchain. `check.yml`'s `msrv` job reads the value back out of `Cargo.toml` and runs `cargo build
--locked` (binary only) on exactly that toolchain, so a dependency bump or a newer std API that
raises the real floor fails CI instead of silently drifting past the declared one. It builds
rather than tests on purpose: the integration tests use `std::io::pipe` (stable since 1.87), and
the declared floor promises nothing about running them. Raise `rust-version` and README.md's
figure together, in the change that needs it.

Both workflows declare `GITHUB_TOKEN` permissions explicitly rather than inheriting the repository
default, which on a repo created before GitHub changed that default (or with it set permissively)
hands every job a read/write token it has no use for. `check.yml` declares `contents: read` at
workflow level, with only its `audit` job re-declaring its own (`issues: write`, which the audit
action needs to file advisories). `release.yml` does the same: `contents: read` at workflow level,
with only the `publish` job re-declaring `contents: write`, so neither `check` nor `build` can
write (both also restate `contents: read` on the job, which is redundant but harmless). Any new
job needing more than read access should declare it on the job, not widen the workflow-level
grant; a new job that declares nothing starts read-only in both files.

Every third-party action in both workflows is pinned to a full commit SHA with its tag in a
trailing comment (`uses: actions/checkout@<sha> # v7.0.1`), never a movable tag or branch, so a
retargeted ref can't run new code with a job's token; `.github/dependabot.yml` keeps the pins
current with weekly pull requests, opened against `dev` (`target-branch`) rather than the default
branch. `dtolnay/rust-toolchain` is pinned the same way, which is why
its steps pass `toolchain: stable` explicitly — that was previously implied by the `@stable`
branch name. A new action added to either workflow gets the same SHA-plus-comment form. Checkout
steps in jobs that hold more than a read token, or that build the release, set
`persist-credentials: false`, so the token isn't left on disk where a build script could read it.

`.github/workflows/release.yml`'s `check` job installs the same tools and runs the same
`scripts/run-checks` (not the coverage gate, not the live-Sway jobs or `audit` — a release build
doesn't need a compositor or a coverage measurement, and the security audit runs independently on
every push regardless) against the exact tagged commit, then builds and publishes a release archive
when a `v*` tag is pushed. Its `check` job also fails unless the tag's version (`vX.Y.Z` minus the
`v`) equals `Cargo.toml`'s `version`, so a tag pushed without the version-bump commit can't ship a
binary whose `--version` disagrees with its release name. Building and publishing are separate
jobs: `build` (read-only token) compiles for `x86_64-unknown-linux-musl`, so the published binary
is statically linked and runs whatever the user's glibc version (no `musl-tools` needed: the crate
has no C dependencies and Rust's musl target ships its own C runtime), and packages the archive (the
binary, `README.md`, `LICENSE` — BSD-3-Clause requires a binary redistribution to carry the
copyright notice and conditions — the man page and bash/zsh/fish completions) and hands it on via
`actions/upload-artifact`, and only `publish` (no checkout, no build — just a download and
`gh release create`) holds `contents: write`, because compiling runs every dependency's build
script and proc-macro, none of which should ever share a job with the write token. Release notes come from `CHANGELOG.md` rather than `gh`'s own
auto-generated notes: `scripts/check-changelog --release-notes` extracts the section matching
`Cargo.toml`'s version (which the `check` job has matched against the tag; see Commands above) and
fails the release outright if that section is missing or empty. `scripts/run-checks` runs the same
script on every push, so a version-bump commit that didn't finalize `CHANGELOG.md` fails CI on
`dev`, before a tag exists; the release-time failure is only a last line of defence. Keep `scripts/run-checks` in sync with this file's Rust/Shell/Python
conventions above whenever the checks change, and both workflow files with any tool it newly
needs installed.

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
`Cargo.toml:10` (`https://doc.rust-lang.org/cargo/reference/manifest.html`, `cargo init`'s
boilerplate "see more keys" comment pointing at Cargo's own manifest reference docs) are real
URLs, but each is a necessary reference to the specific upstream project/tool the surrounding text
is about, not a leak of unrelated real-world data — the user explicitly signed off on these after
being asked (2026-08-15). Don't flag these specific ones in a future content-policy review, and
don't use this as precedent for adding other real URLs without the same explicit confirmation.

**Approved exception:** `Cargo.toml:8`'s `repository = "https://github.com/donnex/sway-launch"` is
a real URL, but — like `README.md`'s Releases-page link above — it's self-referential (the repo
describing its own location), not a leak of unrelated real-world data, and the user explicitly
signed off on it after being asked (2026-08-19). Don't flag it in a future content-policy review.

**Approved exception:** the `-h`/`--help` and man page link to `https://github.com/donnex/sway-launch#readme`
(built from `Cargo.toml`'s `repository`), which README.md's help block repeats verbatim. It's
self-referential for the same reason, and the user explicitly signed off on it after being asked
(2026-10-18, #177). Don't flag it in a future content-policy review.

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
