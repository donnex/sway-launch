# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--mark-match <MARK>` (and the matching `--layout`/`--template` step/binding field) for
  `--existing`: matches an already-open window carrying this mark, instead of `--app-id`/`--class`
  — useful for retargeting a previously-`--mark`ed window without tracking its container id, the
  classic "dropdown terminal" pattern (`sway-launch --existing --mark-match dropdown-term
  --scratchpad`). Mutually exclusive with `--app-id`/`--class`.
- `--scratchpad` flag (and the matching `--layout`/`--template` step field) to move a window to
  Sway's scratchpad, runs last after every other action — pairs well with `--mark`/`--floating`/
  `--width`/`--height`/`--position` for the classic "dropdown terminal" pattern.
- `--rollback-on-error`: if a later `--layout`/`--template` step fails, kill every window this
  invocation itself launched by an earlier, already-completed step, instead of leaving them open.
  Requires `--layout` or `--template`.
- `master-dual-stack-left`/`master-triple-stack-left` built-in templates — `master-dual-stack`/
  `master-triple-stack` mirrored to the other side (stack on the left, main window on the right).
- `--sticky` flag (and the matching `--layout`/`--template` step field) to make a window show on
  every workspace instead of just the one it was launched on. Works regardless of floating state,
  pairs well with `--floating`.
- `--dry-run`: prints the planned sequence of Sway commands instead of running them, numbered
  continuously across every step — never touches Sway IPC or launches anything. Works with a direct
  command or `--layout`/`--template`; `--json` prints a structured `{"steps": [...]}` object.
- `--validate`: parses and validates a `--layout`/`--template` file (formats, target-field
  consistency, `target_id` references, and for `--template`, `--bindings`/`--apps` resolution)
  without launching anything or touching Sway IPC. Requires `--layout` or `--template`.
- `--show-template <NAME_OR_PATH>`: prints a `--template`'s raw TOML and exits, without running it
  — the same built-in-name-or-file-path resolution `--template` itself uses. `--json` prints
  `{"name": ..., "contents": "..."}`.
- A `--template` file may now declare an optional `[layout]` table (`workspace`/`output`) applied
  to every step that doesn't set its own — pins the whole template to a specific workspace/output
  instead of always operating on whatever's currently focused when it runs. A step's own
  `workspace`/`output` still wins when set, applied per field.
- `six-grid-vertical`/`eight-grid-vertical` built-in templates — `six-grid`/`eight-grid` rotated
  (two columns instead of three/four, three/four rows instead of two).
- `master-dual-stack-top`/`master-triple-stack-top` built-in templates — `master-dual-stack-left`/
  `master-triple-stack-left` rotated (stack on top, main window below).
- `dual-sidebars-wide` built-in template — `dual-sidebars` with a wider main window (20%/60%/20%
  instead of 50%/25%/25%).
- `master-dual-stack-wide` built-in template — `master-dual-stack` with a wider stack (60%/40%
  instead of ~65%/35%), better suited to browser/documentation/chat-style pairing than the
  narrower IDE-oriented ratio.
- Launching an application and blocking until its window actually appears, via Sway's IPC event
  stream, then printing the matching window's container id — the core of the tool, and what makes
  `sway-launch` calls chainable in a shell script with no manual `sleep`s.
- Follow-up actions applied to that window in one invocation: `--split`, `--floating`,
  `--fullscreen`, `--focus`, `--mark`, `--new-column`, `--new-row`, `--workspace`, `--output`,
  `--height`, `--width`, `--position`.
- `--app-id`/`--class` matching, so a call in a multi-window script matches the right window rather
  than whichever one happened to appear first.
- `--con-id`/`--existing`, applying any of the actions above to an already-open window instead of
  launching a new one — `--existing` errors rather than guessing when its criteria match zero or
  more than one window.
- `--layout <FILE>`: run a whole layout from one declarative TOML file and one invocation, each
  `[[step]]` the equivalent of one call's flags, stopping at the first error. A step's `id` names it
  so a later step's `target_id` can retarget that specific window — the one way to single out one of
  several windows sharing an `app_id`.
- `--template <FILE|NAME>` with `--apps`/`--bindings`: the same shape decoupled from the
  applications it applies to, so one template can be reused across completely different programs.
- A built-in template library compiled into the binary — grid, sidebar, master/stack, floating,
  multi-workspace/output and retargeting shapes, applied with `--template <name> --apps ...` and no
  files to write. `--list-templates` prints every name with its category, description and slots.
- `--json` for structured output instead of a bare container id.
- `--completions <SHELL>` for bash, zsh, fish, elvish and PowerShell.
- `--timeout`, `--wait-time`, `--verbose`, and `--debug-events` (a raw dump of Sway's event stream,
  for diagnosing what the compositor actually sends).
- Window correlation that survives concurrent invocations: the launched command's environment is
  tagged with a per-invocation marker, so two `sway-launch` processes started at the same time no
  longer risk matching each other's windows and silently returning the wrong container id. Falls
  back to a bounded heuristic only for a single-instance application that forwards to an
  already-running process, where there is no spawned process to correlate against.
- Confirmation by polling Sway's tree for the actions that have no IPC event to wait on
  (`--split`, `--new-column`, `--new-row`, `--height`, `--width`, `--position`), so they return as
  soon as the change is observable instead of always sleeping the full `--wait-time` twice. Where
  no confirmation is possible (a solo window's resize is silently clamped by Sway, a move at the
  edge of a workspace is a no-op) they fall back to the original sleep rather than hanging.
- Short-circuiting for actions whose target state is already satisfied — re-applying `--floating`,
  `--fullscreen`, `--focus`, `--workspace` or `--output` to a window already in that state returns
  immediately instead of waiting for an event Sway never fires for a no-op.

### Changed

- `--position <x>,<y>` (and the matching `--layout`/`--template` step field) now accepts negative
  coordinates — a real, valid position on a multi-monitor setup where an output sits left of or
  above the primary one.
- `--json` now also applies to error output: a failure prints `{"error": "...", "rolled_back": [...]}`
  instead of a plain-text message, so a `--json` caller doesn't need to also parse plain stderr on
  failure.
- `--rollback-on-error` now reports a kill it couldn't complete, instead of leaving that window
  indistinguishable from one it never touched. Under `--json` the id appears in a new
  `rollback_failed` array (present only when non-empty); in plain output, a summary line follows
  the existing per-failure warnings. Such an id is also no longer reported in `container_ids` as
  though it were still open — the three lists now mean exactly "still open", "closed by this run",
  and "couldn't be closed, worth checking", with no overlap. The usual cause is a window that had
  already closed on its own before rollback reached it.
- A failing `--layout`/`--template` run's `--json` error now also reports `container_ids`/
  `containers` for the steps that already completed. Plain output prints each id as its step
  finishes, so a failure there always left the caller able to clean up; `--json` collected them for
  a single object at the end, so a mid-layout failure reported nothing but the error while real
  windows stayed open with no way to identify them. Ids closed by `--rollback-on-error` are
  excluded, and both fields are omitted for a single invocation, which has no partial progress.
- `--json`'s success output is richer: a single invocation now also reports `"actions"` (every
  planned action, in order, each as `{"action": ..., "status": "changed"|"already_satisfied"|
  "skipped"[, "reason": ...]}` — `"already_satisfied"` covers an action that no-oped because the
  window was already in the target state, e.g. re-applying `--floating` to an already-floating
  window; `"skipped"` covers a `--new-column`/`--new-row` action the multi-output relocation guard
  chose not to run at all, with a machine-readable `"reason"`); `--layout`/`--template` now also
  reports `"containers"` (a map from each named step's `id`/`slot` to its container id) alongside
  its own per-step-tagged `"actions"`. No compatibility shim — no version has shipped yet, so this
  is a free redesign rather than an additive one.
- Every `--template` file now requires a `[template]` table with `description` and `category`
  fields, replacing the old convention of scraping the description from the file's first header
  comment line. `--list-templates`/`--show-template --json` now also report each template's
  `category`. This is a breaking change for a hand-authored `--template <file>.toml` without the
  table — add one (see README.md's "Templates" section) to keep it working. Every built-in template
  under `templates/` has been migrated to the new format.
- `floating-overlay` template's overlay window is now explicitly centered (`position = "center"`),
  so it lands somewhere deterministic every run instead of wherever Sway's own default floating
  placement happened to put it.
- A `--bindings` binding setting both `app_id` and `class` now errors immediately, naming the
  offending slot, instead of only being caught later by a more generic step-level error. A binding
  setting `existing = true` with none of `app_id`/`class`/`mark_match` is now caught the same way,
  rather than surfacing as an error about a layout step the user never wrote.
- Two `--template` steps sharing the same `slot` name now error immediately with a message naming
  the slot (`template: slot "..." is used more than once`), instead of a generic, implementation-detail-flavored
  "id already used by an earlier step" message.
- `--list-templates` now also reports each template's slot count and names, in the order `--apps`
  zips its own comma-separated list against — appended to each line as `(N slot(s): name, name,
  ...)` in plain output, or as separate `slots`/`slot_names` fields under `--json`, so a script can
  size or pre-fill `--apps`/a `--bindings` file without parsing the template's TOML itself.
- `--layout`/`--template` step fields that name an identifier (`id`, `target_id`, `slot`), a
  binding's `command`, and a template's `[template]` `description`/`category` now all reject an
  empty or whitespace-only value immediately, naming the offending field — previously an empty
  binding `command` only surfaced as a confusing, one-layer-removed error on the resulting step, and
  a blank `id`/`target_id`/`slot`/`description`/`category` was silently accepted.
- `--validate` now names the `--layout`/`--template` argument it validated: `valid: <source> (N
  step(s))`, or `{"source": ..., "steps": N, "valid": true}` under `--json` — previously just
  `valid: N step(s)`/`{"steps": N, "valid": true}` with no indication of which file was checked.
- A `--layout` step combining `target_id` with `app_id`/`class`/`mark_match` now errors, instead of
  silently discarding the match criteria. `target_id` resolves to an exact container the same way
  `con_id` does, which the step already rejected combining with a matcher for exactly this reason.
- `--mark-match`/`mark_match` now requires `--existing`/`existing = true` — previously it silently
  had no effect when combined with a launch command, since a freshly launched window has no marks
  yet to match against.
- `--app-id`/`--class` (and the matching `--layout` step and `--template` binding fields) now
  reject a blank value, completing the same rule already applied to the fields below. A blank
  matcher can only ever match nothing, and an empty `--app-id` was previously indistinguishable
  from an absent one — so `sway-launch --existing --app-id ''` reported `--existing requires
  --app-id, --class, or --mark-match` at a caller who had just passed `--app-id`. Unlike the
  fields below, a double quote or backslash stays allowed here: these are compared against a
  window's own `app_id`/`class` rather than sent to Sway, so there's no quoting round trip for
  them to break.
- `--mark`, `--mark-match`, `--workspace`, `--output` (and the matching `--layout`/`--template`
  step/binding fields) now also reject a value containing a newline. Sway rewrites it to `;` when
  storing the value, so a mark set as `a⏎b` came back as `a;b` and could never be found again by
  `--mark-match` — the same round-trip failure as the double quote and backslash below, reached by
  a different mechanism. Tabs, carriage returns, and Sway's own `,`/`;` separators are unaffected:
  all were confirmed to round-trip byte-for-byte, so only the character that actually breaks is
  rejected.
- `--mark`, `--mark-match`, `--workspace`, `--output` (and the matching `--layout`/`--template`
  step/binding fields) now reject a value that is blank, or that contains a double quote or a
  backslash. Sway stores those two characters with the escape character intact rather than
  unescaping them, so a mark set as `dropdown"term` was silently stored as `dropdown\"term` and
  could never be found again by `--mark-match`; a blank `mark` silently did nothing at all. Values
  containing spaces or Sway's own `,`/`;` command separators are unaffected — those are quoted and
  stored literally, as before. One consequence: a mark containing a backslash that was set by some
  other tool (`swaymsg mark 'a\b'`) can no longer be targeted by `--mark-match`.

### Fixed

- An action that waits on a Sway IPC event (`--floating`, `--fullscreen`, `--focus`,
  `--workspace`, `--output`, `--mark`, `--scratchpad`) now confirms that the state it asked for is
  actually in effect, instead of treating the arrival of the matching event as proof on its own. An
  event only says Sway emitted that event type for that container — if something else is driving
  the same window (another `sway-launch`, a keybinding, a `swaymsg` in the same script), two
  invocations sending `--workspace 2` and `--workspace 3` at once would each see their own
  container's `Move` event and both report success, while the window is on exactly one of them.
  The event is now the signal to look, and the tree is what confirms.
- An action that waits on a Sway IPC event (`--floating`, `--fullscreen`, `--focus`, `--workspace`,
  `--output`, `--mark`, `--scratchpad`) now fails immediately, naming the container, when its target
  window has already closed. Previously only the poll-based actions checked this, so on Sway 1.9 —
  which treats a `[con_id=N]` criteria matching nothing as success — a closed window meant blocking
  for the whole `--timeout` and then reporting `5 sec timeout reached`, which points at the wrong
  cause. Most visible on a `--layout`/`--template` step retargeting an earlier step's window that
  exited on its own.
- Piping `sway-launch`'s output into a command that stops reading early (`--debug-events | head -5`,
  say) now ends the run cleanly instead of panicking with `failed printing to stdout: Broken pipe`
  and exiting 101.
- A `--layout`/`--template` file that resolves to no steps at all now errors (`no steps found in
  <file>`), under `--dry-run` and `--validate` too, instead of exiting 0 having done nothing —
  which was indistinguishable from a run that worked.
- `--debug-events` combined with a command or any per-window flag now errors, instead of dumping
  events while silently discarding them — `sway-launch --debug-events foot` never launched `foot`.
  A bare `--debug-events` is unaffected.
- `--dry-run` and `--validate` combined now error immediately as conflicting flags, instead of
  `--dry-run` silently winning and `--validate` being ignored with no indication anything was
  skipped.
- `--completions` combined with `--json` now errors, instead of printing the ordinary shell
  completion script and discarding the flag. A completion script is shell source, so there is no
  JSON shape for it to take — unlike `--list-templates`/`--show-template`, which both have one and
  are unaffected.
