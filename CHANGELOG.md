# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
- `--sticky` flag (and the matching `--layout`/`--template` step field) to make a floating window
  show on every workspace instead of just the one it was launched on. Sticky only takes effect
  while the window is floating, so pair it with `--floating`: on a tiled window Sway just records
  the flag, and it applies if the window is floated later (#66).
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

### Changed

- `--json` now identifies each action by a stable `"kind"` — the flag that produced it, such as
  `"new_column"` or `"mark"` — with the Sway command it sent under a separate `"command"` field.
  Previously a single `"action"` field held the Sway command text when the action ran but the flag
  name when it was skipped, so the same flag appeared under two names, and matching on it meant
  parsing Sway syntax that included your own values. `--dry-run --json` reports the same
  `kind`/`command` pair per action, so a preview and a run can be compared directly. (#28)
- `--mark`'s help text and the README now say what it actually does: it **replaces** the window's
  existing marks rather than adding one, and takes the mark away from any other window that held
  it. The old wording ("Add mark to new window") suggested adding, which could quietly break a later
  `--mark-match` on a mark the window no longer carries. (#8)

- `--new-column`/`--new-row` now stop looking for confirmation 25ms after sending their command,
  rather than up to 200ms. These two confirm by noticing the window's geometry changed at all —
  the only thing Sway offers for a relative move — so a longer window mostly meant more chances to
  mistake another program's change to the same window for this one's. A real move is observable
  almost immediately, so this doesn't change what a normal run reports; a move that somehow takes
  longer now reports `unconfirmed` instead of `changed`, and still runs exactly as before. Only
  affects invocations with `--wait-time` above 25ms (the default is 20).
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
- A wait-time action (`--split`, `--sticky`, `--new-column`, `--new-row`, `--height`, `--width`,
  `--position`) that sends its command but never observes the change now reports
  `"status": "unconfirmed"` under `--json`, instead of being indistinguishable from one that was
  actually confirmed. It is still a success, not an error — several of these have legitimate
  outcomes where the expected state never arrives (a solo window's resize is silently clamped, a
  move at the edge of a workspace is a no-op), and a percentage `--height`/`--width` has no pixel
  figure to check at all — but "we waited long enough" and "we saw it happen" are different claims,
  and a script chaining actions on the first one can now tell.
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
  step/binding fields) now also reject a value containing `$`. Sway replaces `$name` with a
  variable from its own config even inside a quoted value, so `--workspace '$ws1'` moved the window
  to whatever the config set `$ws1` to and then failed with a timeout, waiting for a workspace
  literally named `$ws1` (and stopped a `--layout` there). `--validate` reported such a value as
  valid. (#53)
- `--mark`, `--mark-match`, `--workspace`, `--output` (and the matching `--layout`/`--template`
  step/binding fields) now reject every control character except tab, not only a newline. A
  template contains no commands and is meant to be shared, but a TOML escape in one could put a
  terminal escape sequence into a workspace or mark name, which status bars and `swaymsg` then
  print raw. A carriage return, previously stored as given, is rejected too. (#54)
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
- A failed run's error message on stderr now starts with `error:`, and a window that
  `--rollback-on-error` couldn't close is reported with `warning:`, matching the labels the
  argument parser already used. The message used to be printed bare, so in a script's output or
  next to `--verbose` diagnostics the line explaining why the run stopped looked like any other.
  The label is colored when stderr is a terminal, unless `NO_COLOR` is set; `--json` error objects
  are unchanged. (#38)

### Fixed

- `--position` now reports `"changed"` for a correctly placed window on a desktop with a bar, outer
  gaps or a second output, instead of `"unconfirmed"`. It checked where the window landed against
  the output rather than the workspace, so it only confirmed when the workspace started at the
  output's top-left corner: `--position center` reported `"unconfirmed"` on most real setups, and so
  did explicit coordinates on a window without a title bar (`border pixel` or `border none`, a common
  setting) or a fullscreen one. Each such action also waited out the 200 ms confirmation window
  first. Where windows end up is unchanged. (#65)
- `--dry-run` now shows control characters in a command, mark or name as visible escapes (`\r`,
  `\u{1b}`, ...) instead of printing them raw. A layout file could otherwise use a terminal escape
  sequence to make the preview show a different command from the one it would run — the opposite
  of what `--dry-run` is for. Error messages are escaped the same way, so a TOML parse error
  quoting a line from a template or layout file can no longer send a live escape sequence (to
  retitle the window or clear the screen) to the terminal; `--verbose`'s `Sway command:` lines
  likewise. `--json` output was already escaped. (#54)
- The VS Code example scripts (`dev-workspace`, `editor-with-floating-terminal`,
  `quad-mixed-apps`) now match VS Code by class `Code`, the way the README does, instead of app_id
  `code`. VS Code runs under XWayland by default, where it has no app_id, so they used to open the
  editor and then stop with a timeout. The README now also explains how to find an application's
  `app_id` or `class` (`swaymsg -t get_tree`, or `--debug-events` while it opens), and that the
  value can differ between XWayland and native Wayland, distro packages and Flatpaks. (#40)
- The `dev-workspace` example script now actually gives VS Code most of the width. It set
  `--width 65ppt` on VS Code while it was still the only window on its workspace, which Sway
  ignores, so the editor and the terminal column always came out at 50% each. The width now goes on
  the first terminal (`35ppt`), and the `--new-column` that had nothing to move past is gone. (#46)
- `sway-launch --help` no longer prints notes meant for the program's maintainers under
  `--debug-events` and `--completions`, and its option descriptions now match what the options do:
  `--mark-match` says it requires `--existing`, the per-window actions say "the window" rather than
  "new window" (they apply equally to a window picked with `--con-id`/`--existing`), and
  `--timeout` says it applies to the launch and to each event-confirmed action rather than to the
  whole run. A bad `--width` value no longer gets an error asking for `<HEIGHT>px|ppt`. (#37)
- Running `sway-launch` where it can't reach Sway — outside a Sway session (cron, a systemd unit,
  ssh), or with `SWAYSOCK` pointing at the wrong place — now says so: the error names Sway IPC, the
  socket path and the variable it came from, or that neither `SWAYSOCK` nor `I3SOCK` is set. A plain
  launch used to print only a bare OS error such as `No such file or directory (os error 2)` or
  `Invalid argument (os error 22)`. (#35)
- The `examples/scripts/retarget-floating` example no longer fails when another foot terminal is
  open, which is the usual case when running it from foot: its `--existing` step matched every
  window with app_id `foot` and stopped with "windows match". The terminal it launches now gets its
  own app_id, so the step matches only that window. (#11)
- A window that launched fine and then failed on a later action (say `--position` on a tiled
  window) is no longer lost: the error now names its container id, as `container_id` in the
  `--json` error object and as a `container <id> was the target when this failed` line on stderr in
  plain output. Previously the id was dropped, so a script had no way to find the window it needed
  to clean up. In a `--layout`/`--template` run, the failing step's own launched window is also
  listed in `container_ids`, and `--rollback-on-error` now closes it along with the earlier steps'
  windows instead of leaving it open. (#7)
- On a multi-monitor setup, `--new-column`/`--new-row` no longer throw a window onto another
  monitor when it's the only window inside a container at the edge of its workspace. Sway moves
  such a window to the next output, and the guard that skips a move like that only recognized
  windows sitting directly on the workspace. This hit a lone window you'd run `layout tabbed` on, a
  window left alone in a split after its neighbour closed, and — with `workspace_layout tabbed` or
  `stacking` in your Sway config — every first window on a workspace. The move is now skipped and
  reported as `"skipped"` in `--json`, the same as the other cases. (#70)
- A `--layout`/`--template` run whose output pipe closes early now finishes every step, instead of
  stopping at the next container id it couldn't print and still exiting 0. Taking the first id
  with `sway-launch --layout four.toml | head -n1` used to open three windows and silently skip
  the fourth. Likewise, `--verbose` with a closed stderr (`-v ... 2>&1 | head -n1`) no longer
  crashes with exit 101 between two actions, leaving the window half-arranged. (#52)
- A timed-out action now says which action it was and, for a launch, why nothing matched, instead
  of a bare `5 sec timeout reached` for every failure. A wrong `--app-id`/`--class` and a typo'd
  command used to look identical: now a window that opened but didn't match the filter is named
  (and listed under `unmatched_windows` in the `--json` error, so a script can close it), a command
  that exited without opening a window is called out with a hint to check `PATH`, and `--verbose`
  logs each rejected window as it appears. Every action's error also leads with what it was —
  `launch "foot": ...`, `move container to output "left": ...` — so an invocation with several
  actions, or a layout's `step 3: ...`, says which one failed. (#33)
- `--scratchpad` now hides a scratchpad window that is currently shown, instead of reporting it
  `already_satisfied` and leaving it on screen. A window brought out with `scratchpad show` still
  counts as a scratchpad window to Sway 1.11, which made the README's dropdown-terminal example
  (`sway-launch --existing --mark-match dropdown-term --scratchpad`) do nothing in exactly the case
  it exists for, while exiting 0. Sway 1.9 was unaffected. (#69)
- A launch command containing an unquoted `,` or `;` is now rejected up front, instead of being
  split by Sway into separate commands. Sway reads the command before the shell does, so
  `'foot -T a,b'` used to open a window and then fail on `b`, leaving that window open and
  unreported (and out of `--rollback-on-error`'s reach), and `'foot; mark x'` ran `mark x` against
  whatever window happened to be focused. Quote or backslash-escape the separator, or move the
  command into a script. A newline, carriage return or NUL in a command is rejected the same way,
  even inside quotes, since Sway turns each into a `;` before reading quotes; that includes
  `--layout`/`--bindings` files, where `--validate` now catches it. A NUL in a `mark`, `workspace`,
  `output` or `mark_match` value is rejected too, since Sway stores it as `;`. (#4)
- Launching a window no longer claims someone else's window just because its own process hadn't
  started yet. When no window carrying the private per-launch marker had appeared, `sway-launch`
  treated "no process carries the marker" as "the launched process already exited", and took the
  first matching window from anywhere as its own — including in the moments before its own
  process existed, behind a launcher that clears the environment (`systemd-run`, `env -u`), and on
  a system where `/proc` can't be read. Follow-up actions then went to the wrong window, and
  `--rollback-on-error` could close it. Most visibly, two `--layout`/`--template` runs started
  together could both report the same container id. A window without the marker is now only
  treated as this run's own when the launched process was seen running and has since exited;
  otherwise it's used only after the short grace period, and never closed by
  `--rollback-on-error`. (#2)
- A launch command that starts with a shell builtin or keyword — `'cd ~/project && foot'`,
  `'exec foot'`, a `( ... )` group — now runs the way `swaymsg exec` runs it. Before, nothing
  launched and `sway-launch` failed with a `--timeout` error that pointed nowhere near the
  command, because the private per-launch marker was added through `env`, which can only start a
  program. (#3)
- `sway-launch` now finds the Sway socket the way `swaymsg` does, preferring `SWAYSOCK` over
  `I3SOCK`. Sway sets both, so pointing a tool at a different compositor (a nested or headless
  Sway, another session) by overriding `SWAYSOCK` alone used to be silently ignored, and
  `sway-launch` kept acting on the session the terminal belonged to. (#1)
- An action that waits out `--wait-time` (`--split`, `--sticky`, `--new-column`, `--new-row`,
  `--height`, `--width`, `--position`) now reports a target window that has already closed straight
  away, instead of sleeping the full wait first and only then saying so. With a large `--wait-time`
  that looked like a hang. A window that closes *during* the wait is still caught as before.
- `--height`/`--width` (and the matching `--layout`/`--template` step fields) now reject a pixel
  value above 2147483647 instead of accepting it. Such a value passed validation and was sent to
  Sway, but wrapped negative internally before the resize could be confirmed, so it could only ever
  report as unconfirmed. Values at or below that limit are unaffected.
- `--rollback-on-error` no longer closes a window it can't prove this run launched. When no window
  carrying the private per-launch marker appears, `sway-launch` falls back to matching on
  `app_id`/`class` alone — and if that fallback fires while the launched process is still running,
  the window it matched came from somewhere else (another launcher, another `sway-launch`, or
  something opened at the wrong moment). Rolling back used to kill it anyway. It's now left open
  and reported as an open container id instead, with `--verbose` saying so. A window matched after
  the launched process exited — the single-instance browser/editor case — is still treated as this
  run's own and still rolled back.
- An action that waits on a Sway IPC event (`--floating`, `--fullscreen`, `--focus`, `--workspace`,
  `--output`, `--scratchpad`) no longer times out when another client puts the window into the
  requested state a moment before it sends its own command. The check for "already there, nothing
  to do" ran before subscribing to the event stream, leaving a short window in which a competing
  `sway-launch`, keybinding, or `swaymsg` could make the command a no-op — and a no-op fires no
  event, so the action waited out the whole `--timeout` and then errored, having asked for a state
  that was in effect the entire time. It now subscribes first, so a change landing on either side of
  the check is seen.
- A compositor that accepts an IPC connection and then stops answering no longer hangs
  `sway-launch` indefinitely. `--timeout` bounded only the wait for a confirmation event, never the
  IPC round trips around it, so every tree read and command dispatch was an unbounded blocking
  socket read — `--con-id N --floating --timeout 2` against a wedged socket ran until killed. A
  single request/response is now bounded at 10 seconds and fails with a message saying so.
  `--timeout` is unchanged and still means "how long to wait for the confirmation event"; the two
  are deliberately separate, so a short `--timeout` doesn't break ordinary tree reads on a slow
  machine.

  Launching a command — the default invocation, and every `command` step of a `--layout` or
  `--template` — was still exposed until later, along with `--debug-events`: both start by
  subscribing to Sway's event stream, and that handshake had no bound, so `sway-launch --timeout 2
  foot` against a wedged socket never returned. The subscription handshake now has the same
  10-second bound. (#51)
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
- `--debug-events` combined with `--json` now errors, instead of printing the ordinary plain-text
  event dump and discarding the flag — the same fix `--completions` already had, for the same
  reason: a raw event stream has no JSON shape to take.
- `--completions` combined with `--json` now errors, instead of printing the ordinary shell
  completion script and discarding the flag. A completion script is shell source, so there is no
  JSON shape for it to take — unlike `--list-templates`/`--show-template`, which both have one and
  are unaffected.
- `--timeout 0` (or a step's `timeout = 0`) and absurdly large timeouts are now rejected up front.
  Both used to send the command first and only then report a timeout, so a launched window stayed
  open with no container id printed and nothing for `--rollback-on-error` to clean up. (#27)
- The example scripts now stop with a clear message when `sway-launch` isn't on `PATH`, instead of
  failing on their first call with the shell's own "not found" error. (#29)
- `--debug-events` now prints output events too. It never subscribed to them, so a session left
  running while a monitor was added, removed or reconfigured showed nothing about it, despite
  promising every event Sway sends. (#47)
- `--height`/`--width` (and a layout or template step's `height`/`width`) now reject `0` and
  percentages over 100 — `ppt` must be 1-100, `px` at least 1. Sway ignores a zero size and clamps
  a percentage over 100, so a typo such as `600ppt` for `60ppt` used to exit 0 with the window
  unchanged or oversized; it is now an error, which `--validate` also catches. (#48)
