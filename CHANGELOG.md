# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- A layout or template file can no longer run an arbitrary Sway command, `exec` included, through
  `sway_commands`. An entry that left a quote open (`'title_format "a'`) passed validation, and
  since a step's entries are sent to Sway as one command, the open quote changed how Sway split the
  entries after it, so text quoted inside a later entry ran as a separate command outside the list
  of allowed window commands. An entry (or `--sway-command`) ending inside an unclosed quote or
  after an unpaired backslash is now refused, as is a launch command ending that way, and every
  command is checked once more as a whole before it is sent, rather than only after Sway had
  already run it. (#123)
- `--capture` no longer writes a window's app_id into the layout file as a shell command. A client
  sets its own app_id, and foot takes one from an escape sequence that anything printed in the
  terminal can send, so a captured `foot && rm ...` ran as shell code the next time the layout was
  replayed. An app_id or class that isn't a plain program name is now written as a single quoted
  word (`command = ["..."]`), with a note at the top of the file. (#125)
- `--dry-run` no longer prints Unicode bidirectional formatting characters (right-to-left
  overrides, embeddings, isolates and marks) as they are. In a terminal that implements bidi, a
  layout file's command carrying them could be drawn reordered, so that the preview showed
  something other than what the step runs. They are now shown as visible escapes (`\u{202e}`), as
  control characters already were, in `--dry-run`, in `--list-windows`' table, in `--capture`'s
  comments and in error messages quoting a file. (#182)
- `--verbose` no longer prints a window's app_id or class to the terminal unescaped. Both are set
  by the window's client, so a window whose app_id carried a terminal escape sequence could act on
  the terminal through the "Event mismatch" line; control characters in them are now shown as
  visible escapes (`\u{1b}`), as they already were in a title. (#139)

### Added

- `sway-launch` now warns when the compositor it talks to is older than Sway 1.9, the oldest
  supported version, or isn't Sway at all (i3, reached through `I3SOCK`). Such a run used to go
  ahead silently and fail however the difference surfaced, typically as a timeout, which pointed
  at `--timeout` or `--app-id` rather than at the compositor; Debian 12 and Ubuntu 22.04 both ship
  Sway 1.7. The run still goes ahead after the warning, and `--verbose` now prints the version it
  found. (#147)

### Fixed

- A `--concurrent` run now says when a launch may still open its window on the run's hidden
  staging workspace, where the README promised no window was ever left. A launch that timed out or
  was interrupted before its window appeared has nothing to bring back, and an application slower
  than the run opens its window there afterwards, out of sight. The run now names each such step when it ends, with a warning on stderr and under
  `--json`'s new `unresolved_launches` field (`step`, `staging_workspace`), on failure and on a
  `--continue-on-timeout` run alike; under `--rollback-on-error` such a launch still watches 2 more
  seconds for its window and closes it. (#126)
- A `--layout` or `--template` run no longer opens half a layout before reporting a mistake in a
  later step. A typo such as `height = "50pct"` in step 3 used to be found only after steps 1 and 2
  had opened their windows; the whole file is now checked before the first window opens, as
  `--validate` and `--concurrent` already did. A step reusing the `id` of an earlier step whose
  window timed out is now refused under `--continue-on-timeout` too, rather than accepted with the
  later `target_id` skipped. (#179)
- A launch that times out because a single-instance application (VS Code, KeePassXC, most
  Electron apps) was already open no longer tells you to check that the command is on `PATH`. The
  command ran, raised its existing window and exited; the error now says it ran and exited without
  opening a new window, and points at `--existing` for acting on the open one. A command that never
  started still gets the `PATH` hint. (#175)
- A window reporting an empty app_id or class is now described the same way everywhere: as having
  none. `--list-windows` and a launch's timeout message showed it as `app_id ""` (and `--json` as
  an empty string) while `--capture` already treated it as missing; all of them now show no app_id
  (`-` in the table, `null` in `--json`). (#174)
- A `--continue-on-timeout` run with a timed-out step no longer exits 0 when stdout is closed early.
  Piped into a reader that stops after the first line (`sway-launch --layout ... | head -n1`
  under `set -o pipefail`), the run reported success even though its layout was incomplete; it now exits 1 as it does with stdout open, with or without `--json`. (#165)
- A `SWAYSOCK` left stale by a Sway restart, as a tmux or screen session keeps it, no longer
  ends in "is this running inside a Sway session?". When the socket it names isn't there, the error
  now says the variable is out of date and names the running Sway's socket to export instead. With
  neither `SWAYSOCK` nor `I3SOCK` set (a systemd unit, cron or ssh), the error names a running Sway
  rather than saying none could be found, and the stray `sway socket not detected.` line before it
  is gone, so a `--json` error is now always the whole of stderr. (#162)
- A launch that timed out while some other program opened a window no longer blames
  `--app-id`/`--class`. Any window the filter rejected used to become the whole explanation, so a
  mistyped command was reported as `1 other new window did (...) — check --app-id/--class` when a
  chat app or browser pop-up happened to open meanwhile. The error now leads with what happened to
  the launched command (it exited, or is still running) and lists the other window after it as
  context; a rejected window the command itself opened is still reported as before. (#161)
- `--settle` no longer mistakes a window that closed while its size was being held, or a moment when
  the window's state couldn't be read, for the application changing its size. A closed window had
  its geometry sent again, and the run then failed with Sway's `No matching node.` under the name
  of an unrelated action (`floating enable: resize set width 700px: ...`); it now fails at once
  saying the window closed. An unreadable state was retried as a resize three times; the hold now
  ends there, with the held size and position reported `unconfirmed`. An error that belongs to no
  single action, such as the window closing during `--wait-time`, is now prefixed with every
  action it stopped rather than with the first one, which may never have been sent. (#159)
- A mistake in a template binding's `cwd` or `env` (a missing directory, a bad variable name, a `$`
  in a value) is now reported against the bindings file, like every other binding mistake. It was
  blamed on the template's step instead, which with a built-in template such as `dual-column` read
  as a fault in the shipped file. (#157)
- A launch command with `\'` inside single quotes (`foot -T 'a\' #x'`) is now refused before
  anything runs. Sway reads it as an escaped quote, but the shell that runs the command ends the
  quote there, so the rest could become a comment that cut off the command: nothing launched, and
  the run waited out `--timeout` and then blamed `PATH`. Put the backslash outside the quotes, or
  use double quotes. (#151)
- A launch that times out no longer says its command exited, and to check `PATH`, while the
  command is in fact still running. A program started with a cleared environment (`env -i`, or a
  launcher that does the same) or one whose environment can't be read (a setgid program, or one
  that hides it, as some password managers do) was reported as exited, pointing away from the real
  fix, a longer `--timeout`. It is now reported as still running. (#152)
- `--completions` no longer crashes when its output can't be written. Piped into a reader that
  quits early it now exits quietly, and writing to a full disk reports `failed writing to stdout`
  and exits 1, like every other mode, instead of panicking with exit status 101. (#138)
- Closing the terminal a layout or template run was started from, or losing the ssh session it ran
  in, now ends the run the way Ctrl-C does: `--rollback-on-error` closes what it launched,
  `--json` reports its partial progress, and with `--concurrent` the windows are brought back from
  the hidden staging workspace instead of being left there with focus. The hangup signal (SIGHUP)
  used to kill the run on the spot; it now exits 129. (#128)
- An `existing = true` step ahead of a step launching the same application now works the same with
  `--concurrent` (or `concurrent = true`) as without it. The later step's window, already open on
  the staging workspace, counted as a match: a plain `existing` step failed with "2 windows match",
  and one with `wait = true` took that window at once instead of waiting for the one being
  autostarted, marking and arranging the wrong window with exit status 0. A step's `existing`
  search now leaves out every window a later step launched ahead of its turn, and a `con_id` naming
  one fails the run. (#190)
- `--capture` now rebuilds a workspace holding a fullscreen window in the right shape. Sway keeps
  focus on a fullscreen window when another opens beside it, so the captured file, which made the
  window fullscreen as it launched, opened every later window behind it in reverse order, and a
  later focus step took it out of fullscreen again, all with exit status 0. Sway also reports a
  fullscreen window as filling its whole split, so an even split was written with a 100% width
  step. The file now makes the window fullscreen in a final step of its own and sizes it from its
  siblings, and the README explains how to place fullscreen in a layout file of your own. (#189)
- `--capture` now refuses a workspace holding a split container floated as a whole (several
  windows floating together), as it already refused a tabbed one. It used to keep only the first
  window in it and rebuild that one tiled, so the file silently described a different workspace.
  A floating container holding a single window now keeps its floating state, size and position.
  (#153)
- A `--layout` or `--template` run now works alongside a daemon that renames workspaces as windows
  open on them (workspace-icon helpers that turn `1` into `1: <icons>`). A step could fail with a
  "focus moved" error although focus never left the workspace, and a
  `--concurrent` run built the rest of the layout on a second, new workspace under the old name
  while reporting success. The build workspace is now followed through a rename. (#178)
- The run lock that stops a layout or template being built twice on one workspace at the same time
  now recognises the workspace however it's named. `--on-workspace Dev` and `dev`,
  `--on-workspace-number 3` and a run started on `3: web`, and a workspace before and after an icon
  daemon renamed it each took separate locks, so the second run went ahead and interleaved its
  windows with the first's. A numbered workspace is now known by its number and any other by its
  name ignoring case. With only `--on-output`, the lock now names the workspace that output shows
  rather than the one focused before the run, and two different layouts piped in on stdin no longer
  block each other. (#129)
- `--rollback-on-error` now closes a launched window that timed out only because it opened under a
  different app_id, class or title than the step names, the commonest way a launch times out. The
  run already knew the window was its own, and reported it as `launched_by_this_run`, but left it
  open while reporting the rest as rolled back; with `--concurrent` it stayed out of sight on the
  hidden staging workspace. It's now closed with the others, after being brought back from the
  staging workspace under `--concurrent`. (#168)
- A `--layout` or `--template` run now warns when your own Sway config put a step's window
  somewhere other than where the layout builds it: an `assign` or `for_window` rule that sent it to
  another workspace or the scratchpad, or floated it. The run reported success with nothing said
  while the shape it built was split across workspaces. The warning names the step and where the
  window went, on stderr and in `--json`'s `warnings`; the Troubleshooting section covers these
  rules, `no_focus` included. (#160)
- `--existing --title` with no `--app-id`, `--class` or `--mark-match` no longer resolves to a
  workspace, an output or the scratchpad. The pattern was tested against every name in the tree,
  so a title matching a workspace's name returned that workspace's id with exit 0 (and an action on
  it then failed), counted it as a second match beside a real window, or ended `--wait` at once.
  It now considers exactly the windows `--list-windows` shows. (#144)
- `--on-workspace`, `--on-workspace-number` and a template's `[layout] workspace` no longer build
  the layout on the previous workspace when Sway's `workspace_auto_back_and_forth` is on and the
  named workspace already has focus. Sway's back-and-forth switched away from it, and the run then
  built the whole layout there and reported success. The workspace is now always focused as named.
  (#142)
- `--rollback-on-error` no longer closes a window you opened yourself while a launch was failing.
  When the launched command exited early (a mistyped flag, a crash) and a matching window opened
  meanwhile, that window was counted as one the run had launched, so a later step's failure closed
  it. A window whose process started after the launch is now only adopted, which rollback leaves
  open; a single-instance application handing the request to a copy that was already running is
  still recognised as launched. (#124)
- Interrupting a layout or template run with Ctrl-C or SIGTERM while a step's application is still
  starting no longer leaves that application's window open under `--rollback-on-error`. Rollback
  closed the earlier steps' windows and the run exited, and then the slow application's window
  appeared and stayed. The run now waits up to 2 seconds for it, as it already did after a launch
  timeout, and closes it with the rest. (#127)
- A run combining an action Sway reports with an event (`--floating`, `--fullscreen`, `--focus`,
  `--mark`) and one it doesn't (`--width`, `--height`, `--position`, `--split`, `--sticky`,
  `--sway-command`) no longer fails with "no matching window event" when `--wait-time` is raised to
  near `--timeout` or past it. The `--timeout` clock started before the `--wait-time` pause rather
  than when the command was sent, so the window took every action and the run still reported a
  timeout; `--wait-time 5000` failed README's own quick-start example this way. (#143)
- `--validate` and `--dry-run` no longer fail a layout or bindings file whose `cwd` doesn't exist on
  the machine checking it. A file checked in CI or kept in a dotfiles repository names a directory
  that exists only where it runs, so it could never pass `--validate` there; both now check only
  the format of `cwd`, and a real run still fails before launching anything when the directory is
  missing. (#146)
- `--help`, `-h` and the man page now link to the guide on the project's repository page instead
  of sending readers to `README.md`, which an installed copy usually doesn't have: neither `cargo
  install` nor the release archive's install steps keep it. (#177)
- `--debug-events --json` now names a bar config change `barconfig_update`, the name Sway and
  `swaymsg -t subscribe` use, rather than `bar_config_update`, so a script filtering on Sway's name
  sees those events. The README also no longer claims each line is Sway's own payload: it is the
  event as the `swayipc` library reads it, which leaves out fields it doesn't know and fills in
  `null` for ones Sway didn't send. (#191)

## [0.1.0] - 2026-10-18

The first release.

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
  accepting one that then launched nothing and timed out. A launch without `--app-id`/`--class`
  that ends up taking a window this way says so under `--verbose`, since without a filter any new
  window can qualify. (#2, #3, #4, #18, #20)
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
- The follow-up actions after the window is found — `--split`, `--floating`, `--sticky`,
  `--fullscreen`, `--focus`, `--height`, `--width`, `--position` and `--mark` — reach Sway as one
  command, so the window takes its whole arrangement at once instead of floating, then resizing,
  then jumping into place. This makes a typical call about twice as fast, and an application no
  longer sees half-applied geometry it could react to. Each action is still confirmed and reported
  on its own, and a part Sway refuses fails naming that action. Moves (`--new-column`,
  `--new-row`, `--workspace`, `--output`) still run first, each on its own, and `--scratchpad`
  last. `--dry-run` shows the actions sent together on one line. (#32)
- `--settle <ms>` (and `settle` in a layout step, template step or binding) keeps holding a pixel
  `--height`/`--width` and `--position` for that long after they're confirmed, applying them again
  if the application changes them. Browsers set their own size just after opening, so a Firefox or
  Chromium window used to end up a different size from one run to the next while `sway-launch`
  reported success. A binding's `settle` replaces the step's. (#17)
- `--capture [WORKSPACE]` prints a `--layout` file that rebuilds a workspace arranged by hand:
  its splits, child order and sizes, marks, and floating windows' size and position, so a layout no
  longer has to be worked out step by step. Each window's command is a guess from its app_id or
  class, marked as such in the file. Every built-in template rebuilds its exact shape from its own
  capture; tabbed and stacked containers aren't supported yet. (#31)
- `--concurrent` starts every application in a `--layout`/`--template` run at once, then arranges
  the windows in step order, so a layout takes about as long as its slowest application instead of
  all of them together: a four-window grid of applications taking about a second each to start
  drops from about 5 seconds to under 2. Every built-in template builds the same shape as without
  it. The applications start on a temporary workspace of their own, so focus jumps there briefly;
  a failure moves whatever already opened to the build workspace, and two steps ending up with the
  same window (as a single-instance application can cause) fail the run. A layout file or a
  bindings file can turn it on itself with a top-level `concurrent = true`, since the file that
  names the applications is the one that knows whether they can start together. (#73, #121)
- A launched program that never sees the launch marker is now recognised through its process
  ancestry, as long as it descends from the launch, instead of waiting out the 2-second grace and
  being treated as adopted (which kept `--rollback-on-error` from closing it). This covers wrappers
  that clear or unset the environment (`env -u`, `env -i`) and programs whose environment can't be
  read. A launch command can no longer contain a shell comment (an unquoted `#` starting a word),
  since the command now runs inside a `( ... )` group that keeps the launch's shell alive for this
  check. (#96)
- `--sway-command '<command>'` (repeatable, and `sway_commands = [...]` in a layout or template
  step) sends a Sway command of your own to the window, for properties no flag covers, such as
  `border none` or `opacity 0.9`, which used to need a separate `swaymsg` call with the captured id
  and couldn't be written in a layout file at all. It's sent with the other actions and reported
  `unconfirmed`. Only commands that act on one window are accepted, since Sway runs session-wide
  ones like `exec`, `bindsym` or `exit` even when they're aimed at a window, and a shared template
  shouldn't be able to. (#90)
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
- `--title <REGEX>` (and `title` in a layout step or binding) matches a window by its title as
  well, an unanchored regular expression like Sway's own `title=` criteria. It picks one of
  several windows of the same app (`--existing -a foot --title '^logs$'`), and a launch now waits
  past a splash or welcome window for the one whose title matches, including a window that
  changes its title after appearing. It works with `--list-windows` too. (#14)
- `--existing --wait` (and `wait = true` in a layout step or binding) waits up to `--timeout` for
  a matching window to open, instead of failing at once when none is open yet. It arranges windows
  something else starts, such as an `exec` line in the Sway config, an autostart entry or a
  launcher, with no `sleep` in front: `exec sway-launch --existing --wait -a firefox --workspace
  2`. A window that sets its app_id/class only after appearing is still found. Several matching
  windows are still an error, and a window found this way is never closed by
  `--rollback-on-error`. (#108)
- `--workspace-number <N>`, and `workspace_number` in a layout or template step, move a window to a
  workspace by number the way Sway's own `workspace number N` keybindings do, so `2` finds a
  workspace named `2: web`. `--workspace 2` used to be the only option there, and it silently
  created a second workspace literally named `2` next to `2: web` while reporting success.
  `--on-workspace-number <N>` and a template's `[layout]` `workspace_number` build a whole layout on
  the numbered workspace the same way. (#43)
- `--list-windows`: a table of every open window, scratchpad included, with the container id,
  app_id, class, workspace, marks and title a matcher or `--con-id` needs, so finding them no
  longer means reading `swaymsg -t get_tree`. `--json` prints the same as `{"windows": [...]}`,
  and `-a`/`-c`/`--mark-match` narrow the list to exactly the windows `--existing` would choose
  between. The errors for an `--existing` that matches no window or several now point to it. (#42)
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
  several windows sharing an `app_id`. A file with no steps is an error. A step's or binding's
  `command` can be a list of words, `["foot", "-T", "a, b"]`, which `sway-launch` quotes so each
  word reaches the program exactly, commas, semicolons and quotes included. (#77)
- `--cwd <DIR>` and `--env KEY=VALUE` (and `cwd`/`env` in a layout step or binding) for a launched
  command, which otherwise starts in Sway's own directory and environment rather than yours.
  (#34, #113) `--layout -` reads the
  file from stdin, as do `--template -` and `--bindings -` (one per run), so a generated layout can
  be piped in. (#50)
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
- Interrupting a `--layout`/`--template` run with Ctrl-C or SIGTERM no longer leaves every window
  it launched open with nothing reported: the run now fails as the step it was on, so
  `--rollback-on-error` closes its windows and `--json` reports partial progress, then exits with
  status 130 or 143. A second signal exits at once. With `--rollback-on-error`, a launch that
  timed out also waits up to 2 more seconds for its window and closes it if it opens, instead of
  leaving it to appear after the run has failed. (#15)
- `--continue-on-timeout`: a `--layout`/`--template` step whose window doesn't appear in time no
  longer has to end the run. With the flag, it's reported (a warning on stderr, `timed_out_steps`
  under `--json`) and the next step runs; a step that targets the missing window is skipped and
  reported too. The run still exits non-zero. A binding can also set its own slot's `timeout`
  and `wait_time`, so one slow-starting application can be given longer without every slot
  waiting that long. (#19)
- Template parameters: a template's `[params]` table names sizes a step uses as `"{name}"`, and
  `--param NAME=VALUE` (or a `[params]` table in a bindings file) changes them for one run, so
  `--template master-dual-stack --param stack-width=40ppt` gives a wider stack without copying
  the template. Every built-in with a size has params, listed with their defaults by
  `--list-templates`. A wrong name or a value the field can't take is an error before anything
  runs. (#92)
- A bindings file can place each slot's window, with the same `workspace`, `workspace_number` and
  `output` keys a step has, replacing the template's own. The built-in `dual-output`, which ships
  with placeholder output names, now works as a built-in: give each slot your own outputs in the
  bindings instead of copying the template out and editing it. `--on-output <OUTPUT>` builds a
  `--layout` or `--template` run on an output of your choice, overriding a template's `[layout]`
  output the way `--on-workspace` overrides its workspace. (#30)
- Layouts and templates are built on one workspace, and say when that can't work. They assume an
  empty workspace that keeps focus: run from a terminal, the Quickstart grid used to be built around
  that terminal, and switching workspace while an application started split the layout across two
  workspaces, both with nothing reported. Now a run warns when its workspace already holds tiled
  windows (on stderr, and as `"warnings"` under `--json`), a step fails if focus leaves its
  workspace while its window is opening, and `--on-workspace <WORKSPACE>` builds the run on a
  workspace of your choice. A step can no longer combine `workspace` with `new_column`/`new_row`
  (nor the CLI `--workspace` with `--new-column`/`--new-row`): the relative move only ever applied
  on the workspace the window opened on. (#45, #105)
- A layout or template run can't overlap with itself: a second run of the same file or built-in
  template on the same workspace stops at once instead of interleaving its windows with the first
  run's, as a double-pressed key used to. `--no-lock` runs anyway. Building from source now needs
  Rust 1.89, for the file lock this uses. (#55)
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
  Sway sends, for diagnosing what the compositor actually does. With `--json` each event is one
  line of JSON (Sway's own payload plus an `event` type), ready for `jq`. (#47, #85)
- Example scripts and `--layout` files to copy from, plus a README section on finding an
  application's `app_id` or `class` (`swaymsg -t get_tree`, or `--debug-events` while it opens),
  which can differ between XWayland and native Wayland, distro packages and Flatpaks. (#11, #29,
  #40, #46)
- Supported Sway versions stated up front: 1.9 and newer. Every change is tested against both Sway
  1.9 and the newest Sway release. (#98)
- A declared minimum Rust version for building from source: 1.89. An older compiler — common with
  distribution-packaged Rust — gets a clear message naming `sway-launch` and the version it needs,
  instead of a dependency error about `clap`. The README's Installation section says so up front,
  and shows `cargo install --path . --locked` for building and installing in one step. (#55, #67)
- `--help` is grouped into sections (launching, matching, acting on the window, size and position,
  moving it, layouts and templates, run options, inspecting, general), with a usage line for each
  way to run the tool and a few examples at the end. `-h` gives a one-line summary per flag and
  `--help` the full detail; the man page follows the same sections. (#119)
- Prebuilt Linux x86_64 release binaries, statically linked against musl, so they run on any
  distribution whatever its glibc version. The archive also includes `LICENSE`, a `sway-launch(1)`
  man page generated from the same definition as `--help`, and ready-made bash, zsh and fish
  completions. (#26, #39, #109)
