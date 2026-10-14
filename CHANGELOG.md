# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Launching an application and blocking until its window actually appears, via Sway's IPC event
  stream, then printing the matching window's container id — the core of the tool, and what makes
  `sway-launch` calls chainable in a shell script with no manual `sleep`s.

  Concurrent invocations don't mix up their windows: the launched command's environment carries a
  private per-launch marker, so two `sway-launch` processes started at the same time each get their
  own window. A window without the marker — a single-instance application handing the request to
  an already-running process — is accepted as this run's own only once the launched process was
  seen running and has exited, and otherwise only after a short grace period. The command runs the
  way `swaymsg exec` runs it, so one starting with a shell builtin or keyword (`'cd ~/project &&
  foot'`, a `( ... )` group) works. A command containing an unquoted `,` or `;` is rejected up
  front, since Sway would split it into separate commands before the shell sees it; so is a
  newline, carriage return or NUL anywhere in it, which Sway turns into a `;` even inside quotes.
  The same target rules apply on the command line, in a layout step and in a template binding: a
  blank or whitespace-only command is rejected up front everywhere, instead of the command line
  accepting one that then launched nothing and timed out. (#2, #3, #4, #20)
- Follow-up actions applied to that window in one invocation: `--split`, `--floating`, `--sticky`,
  `--fullscreen`, `--focus`, `--mark`, `--new-column`, `--new-row`, `--workspace`, `--output`,
  `--height`, `--width`, `--position` and `--scratchpad`.
  - `--mark` replaces the window's existing marks rather than adding one, and takes the mark away
    from any other window that held it. (#8)
  - `--sticky` shows a floating window on every workspace. It only takes effect while the window is
    floating, so pair it with `--floating`; on a tiled window Sway records the flag and applies it
    if the window is floated later. (#66)
  - `--scratchpad` runs after every other action, so it pairs with `--mark`/`--floating`/`--width`/
    `--height`/`--position` for the classic "dropdown terminal" pattern, and it hides a scratchpad
    window that is currently shown. (#69)
  - `--height`/`--width` take `<n>px` (1 to 2147483647) or `<n>ppt` (1 to 100); a zero size or a
    percentage over 100, which Sway would ignore or clamp, is an error. (#48)
  - `--position` takes `center` or `<x>,<y>`, relative to the window's workspace as Sway's own
    `move position` is, so a bar or outer gaps shift the origin; negative values are accepted. An
    earlier description called the coordinates global across every output, which they never were:
    combine `--output` with `--position` to place a window on another monitor. (#64)
  - `--workspace` and `--output` match names the way Sway does, ignoring case, so `--workspace web`
    joins an existing `Web` instead of failing at `--timeout` after a move that worked. Sway's
    keywords (`next`, `prev`, `current`, `back_and_forth`, ...; `left`/`right`/`up`/`down` for an
    output) are rejected as names: Sway acts on them even quoted, and an upper-case one such as
    `NEXT` crashes Sway 1.11. (#5)
  - `--output` puts the window on the output's visible workspace, as Sway's own command does. A
    window on a hidden workspace of that output used to be reported `already_satisfied` and left
    out of view. (#91)
  - `--workspace` and `--output` together put the window on that workspace on that output, by moving
    the workspace to the output. The output move used to land on whatever workspace the output was
    showing, silently dropping `--workspace` while both reported `changed`. (#6)
  - `--workspace`, `--output` and `--focus` refuse a split container (say, one marked by a
    `focus parent; mark` binding and found with `--mark-match`) with an error saying so, instead of
    moving it and then failing at `--timeout`: Sway confirms those moves only for a window. (#93)
  - On a multi-monitor setup, a `--new-column`/`--new-row` that Sway would carry onto another
    output — a window at the edge of its workspace, including the only window inside a container
    there, or a window that's already fullscreen — is skipped rather than run. On a floating
    window both are skipped on any setup, since Sway would only nudge it 10 px. Each move is
    checked right before it's sent, so `--new-column --new-row` judges the row move against the
    layout the column move left. (#70, #110)
- Every action confirms that the state it asked for is actually in effect before returning, rather
  than only waiting. An action Sway fires an event for (`--floating`, `--fullscreen`, `--focus`,
  `--workspace`, `--output`, `--mark`, `--scratchpad`) waits for that event and then checks the
  window really is in the requested state, so another client driving the same window can't make it
  report success for a change it didn't get. Re-applying a state the window already has returns
  immediately instead of waiting for an event Sway never fires for a no-op, including when another
  client gets there a moment first. The actions Sway fires no event for (`--split`, `--sticky`,
  `--new-column`, `--new-row`, `--height`, `--width`, `--position`) poll the window's state and
  return as soon as the change is visible. Where it never becomes visible — a solo window's resize
  is silently clamped by Sway, a move at the edge of a workspace is a no-op — they wait out
  `--wait-time` and report the action as unconfirmed rather than hang or fail. An action whose
  target window has already closed fails straight away, naming the container. (#65)
- `--app-id`/`--class` matching, so a call in a multi-window script matches the right window rather
  than whichever one happened to appear first. The value (like `--mark-match`'s) must match exactly,
  case included — unlike Sway's own regex criteria, `-a firefox` doesn't match
  `org.mozilla.firefox` — and the help text and README now say so. A window the launched command
  opened is still matched when the app sets its app_id or class only after the window appears (as
  some XWayland apps do with their class), something Sway sends no event for. (#86, #87)
- `--con-id`/`--existing`, applying any of the actions above to an already-open window instead of
  launching a new one. `--existing` errors rather than guessing when its criteria match zero or
  more than one window, and with `--mark-match <MARK>` it can find a window by a mark set earlier
  (`sway-launch --existing --mark-match dropdown-term --scratchpad`) instead of by `app_id`/`class`.
  With actions to apply, a `--con-id` is looked up before the first one, so a mistyped id fails
  as `no window with container id N`, and a workspace's or output's id as `container N is a
  workspace, not a window`, rather than as a window that "may have closed". A bare `--con-id N`
  with no actions is still printed back unchecked. (#107)
- Input checks that catch values Sway would quietly misread. `--app-id`, `--class` and
  `--mark-match` reject a blank value, and `--mark-match` requires `--existing`. `--mark`,
  `--mark-match`, `--workspace` and `--output` also reject a double quote, a backslash, a `$` and
  every control character except tab: Sway stores the first two with their escape character
  intact, replaces `$name` with a variable from its own config even inside quotes, and stores or
  prints control characters in ways that break a later `--mark-match` or a status bar. Spaces and
  Sway's own `,`/`;` separators are fine. (#53, #54)
- `--timeout` (seconds to wait for the window, and for each event-confirmed action) and
  `--wait-time` (milliseconds to wait before each action Sway fires no event for); a zero or unrepresentable `--timeout` is
  rejected before anything is sent. A single request to Sway gives up after 10 seconds, so a
  compositor that stops answering can't hang a run. The Sway socket is found the way `swaymsg`
  finds it, preferring `SWAYSOCK` over `I3SOCK`, and when it can't be reached the error says so,
  naming the socket path and the variable it came from. (#1, #27, #35, #51)
- `--layout <FILE>`: run a whole layout from one declarative TOML file and one invocation, each
  `[[step]]` the equivalent of one call's flags, stopping at the first error. A step's `id` names it
  so a later step's `target_id` can retarget that specific window — the one way to single out one of
  several windows sharing an `app_id`. A file with no steps is an error.
- `--template <FILE|NAME>` with `--apps`/`--bindings`: the same shape decoupled from the
  applications it applies to, so one template can be reused across completely different programs.
  A template file declares a `[template]` table with a `description` and `category`, and may
  declare a `[layout]` table (`workspace`/`output`) to build on: the run switches there before the
  first step, so the whole shape is built in place. (Copying it into every step as a move after
  launch, as it first worked, built the shape on the old workspace and scrambled it on arrival:
  `quad-grid` came out as one tall window beside three.) Mistakes in a template or
  bindings file — a slot used twice, a binding with both `app_id` and `class`, a blank `id`,
  `slot` or `command` — are reported against the field that's wrong. Each slot belongs to one step
  (a later step acts on the same window with `target_id`), and a template that repeats one is
  rejected as soon as it's read, so `--apps` never reports a misleading application count for
  it. (#45, #80, #100)
- A built-in template library compiled into the binary — grid, sidebar, master/stack, floating,
  multi-workspace/output and retargeting shapes, applied with `--template <name> --apps ...` and no
  files to write. `--list-templates` prints every name with its category, description and slots,
  in the order `--apps` fills them, and `--show-template <NAME_OR_PATH>` prints one template's
  TOML without running it. A value ending in `.toml` is always read as a file, so writing a
  built-in's name with that extension fails — but the error names the built-in and the bare name
  to use instead. (#88)
- Layouts and templates are built on one workspace, and say when that can't work. They assume an
  empty workspace that keeps focus: run from a terminal, the Quickstart grid used to be built around
  that terminal, and switching workspace while an application started split the layout across two
  workspaces, both with nothing reported. Now a run warns when its workspace already holds tiled
  windows (on stderr, and as `"warnings"` under `--json`), a step fails if focus leaves its
  workspace while its window is opening, and `--on-workspace <WORKSPACE>` builds the run on a
  workspace of your choice. A step can no longer combine `workspace` with `new_column`/`new_row`
  (nor the CLI `--workspace` with `--new-column`/`--new-row`): the relative move only ever applied
  on the workspace the window opened on. (#45, #105)
- `--rollback-on-error`: if a `--layout`/`--template` step fails, close every window this run
  launched — by an earlier step, or by the failing step before it failed — instead of leaving them
  open. A window this run only adopted by matching, without proof it launched it, is left open.
  Rollback waits up to a second for each window to actually go, since an application can decline
  to close, and reports one still open with a warning. If the compositor has stopped answering,
  rollback gives up after the first close request goes unanswered rather than waiting out another
  ten-second timeout for every window. (#7, #71, #94)
- `--json` for structured output instead of a bare container id: the container id plus every
  planned action, each with a stable `kind` (the flag that produced it), the Sway `command` it
  sent, and a `status` of `changed`, `already_satisfied`, `unconfirmed` or `skipped` (with a
  `reason`). A `--layout`/`--template` run also maps each named step to its container id. Errors
  are JSON too: the message, which windows were rolled back or couldn't be, the windows earlier
  steps left open, the failing step's own container id, and any window that opened but didn't
  match the filter. (#7, #28, #33)
- Error messages that say what went wrong: each starts with `error:` (a rollback problem with
  `warning:`), colored on a terminal unless `NO_COLOR` is set, and leads with the action that
  failed. A launch that times out says whether a window opened but didn't match the filter (and
  which), or whether the command exited without opening one. Control characters from a layout or
  template file are shown as visible escapes, never sent to the terminal raw. (#33, #38, #54)
- Output that behaves in a pipeline: stdout carries only the result, closing it early (`| head`)
  ends the run cleanly, and a `--layout`/`--template` run still finishes every step when its output
  pipe closes. (#52)
- `--dry-run`: prints the planned sequence of Sway commands instead of running them, numbered
  continuously across every step — never touches Sway IPC or launches anything. Works with a direct
  command or `--layout`/`--template`; `--json` prints the same `kind`/`command` pair per action a
  real run reports, so a preview and a run can be compared directly.
- `--validate`: parses and validates a `--layout`/`--template` file (formats, target-field
  consistency, `target_id` references, and for `--template`, `--bindings`/`--apps` resolution)
  without launching anything or touching Sway IPC, and names the file it checked. An error names
  the file that holds the mistake, and the step number for a mistake in a step
  (`layout.toml: step 2: height: ...`): a bad binding is reported under the `--bindings` file (or
  `--apps`), not under the template it was applied to — with a built-in template, that used to
  read as a fault in the shipped template. A real run reports its errors the same way. (#106)
- `--completions <SHELL>` for bash, zsh, fish, elvish and PowerShell.
- `--verbose`, and `--debug-events`: a raw dump of every window, workspace, output and other event
  Sway sends, for diagnosing what the compositor actually does. (#47)
- Example scripts and `--layout` files to copy from, plus a README section on finding an
  application's `app_id` or `class` (`swaymsg -t get_tree`, or `--debug-events` while it opens),
  which can differ between XWayland and native Wayland, distro packages and Flatpaks. (#11, #29,
  #40, #46)
- Supported Sway versions stated up front: 1.9 and newer. Every change is tested against both Sway
  1.9 and the newest Sway release. (#98)
- A declared minimum Rust version for building from source: 1.85. An older compiler — common with
  distribution-packaged Rust — gets a clear message naming `sway-launch` and the version it needs,
  instead of a dependency error about `clap`. The README's Installation section says so up front,
  and shows `cargo install --path . --locked` for building and installing in one step. (#67)
- Prebuilt Linux x86_64 release binaries, statically linked against musl, so they run on any
  distribution whatever its glibc version. (#26)
