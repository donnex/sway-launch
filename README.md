# sway-launch

> [!NOTE]
> This project was built with AI assistance (Claude). It hasn't necessarily been reviewed by a
> human, and comes with no guarantees of correctness, security, or fitness for any particular
> purpose — read through the code yourself before relying on it.

`sway-launch` launches an application in the [Sway](https://swaywm.org/) window manager, waits for
its window to appear, and then does things to that window: float it, size it, move it to a
workspace or monitor, split beside it, mark it. Each call returns only once the window exists and
every action is confirmed, so calls chain into scripts that build the same window layout every
time, with no `sleep`s.

```shell
sway-launch -a foot --split h foot          # a terminal; the next window opens beside it
sway-launch -a foot --width 30ppt foot      # a second one beside it, 30% of the width
sway-launch --template quad-grid --apps foot,foot,foot,foot --on-workspace 9   # a 2x2 grid
```

- **Waits for the window.** Each call blocks until the launched command's own window is open and
  every action has taken effect, then prints the window's container id.
- **Acts on it.** Floating, sticky, fullscreen, focus, marks, size, position, splits, new
  rows/columns, workspaces, outputs, the scratchpad, plus any other single-window Sway command.
- **Builds whole layouts.** From a shell script, a TOML layout file, or one of about thirty
  built-in templates (grids, sidebars, master/stack) filled with any applications. `--capture`
  writes the layout file for a workspace you arranged by hand.
- **Works on open windows too.** Find a window by app_id, class, title, mark or container id, and
  act on it without launching anything.
- **Made for scripts.** A single binary with no daemon or config. stdout is the container id, or a
  JSON report with `--json`; `--dry-run` and `--validate` work without a running Sway.

Requires Sway 1.9 or newer, running. `sway-launch` talks to it over the same IPC socket `swaymsg`
uses, found the same way (`SWAYSOCK`, then `I3SOCK`); when that socket can't be reached, the error
names any running Sway it finds in `$XDG_RUNTIME_DIR`, such as after a Sway restart left a tmux
session with a stale `SWAYSOCK`. Every change is tested against Sway 1.9 and
the newest Sway release. On an older Sway, or on a compositor that isn't Sway (i3, through
`I3SOCK`), it still runs but prints a warning first, since results may be wrong there; `--verbose`
prints the version it found.

## Contents

- [Installation](#installation)
- [Quick start](#quick-start)
- [Guide](#guide)
  - [Launching a command](#launching-a-command)
  - [Matching the right window](#matching-the-right-window)
  - [Acting on an open window](#acting-on-an-open-window)
  - [Scripts](#scripts)
  - [Layout files](#layout-files)
  - [Templates](#templates)
  - [Capturing a workspace](#capturing-a-workspace)
  - [Running layouts](#running-layouts)
- [Actions reference](#actions-reference)
  - [Floating](#floating) · [Sticky](#sticky) · [Fullscreen](#fullscreen) · [Focus](#focus) ·
    [Mark](#mark) · [Scratchpad](#scratchpad) · [Sway command](#sway-command)
  - [Height and width](#height-and-width) · [Position](#position) · [Settle](#settle)
  - [Split](#split) · [New column](#new-column) · [New row](#new-row) ·
    [Workspace](#workspace) · [Output](#output)
- [Output and diagnostics](#output-and-diagnostics)
  - [Exit status and output streams](#exit-status-and-output-streams)
  - [JSON output](#json-output) · [Verbose](#verbose) · [Dry run](#dry-run) ·
    [Validate](#validate) · [Debug events](#debug-events)
- [Timing: timeout and wait time](#timing-timeout-and-wait-time)
- [How a launch finds its window](#how-a-launch-finds-its-window)
- [Troubleshooting](#troubleshooting)
- [Command-line reference](#command-line-reference)

## Installation

**Prebuilt binary.** Download the archive for the latest release from the
[Releases page](https://github.com/donnex/sway-launch/releases), extract it, and put `sway-launch`
somewhere on your `PATH`. It's statically linked, so it runs on any x86_64 Linux distribution
whatever its glibc version. The archive also holds the man page and shell completions:

```shell
install -Dm755 sway-launch ~/.local/bin/sway-launch
install -Dm644 sway-launch.1 ~/.local/share/man/man1/sway-launch.1
install -Dm644 completions/sway-launch.bash ~/.local/share/bash-completion/completions/sway-launch
install -Dm644 completions/_sway-launch ~/.local/share/zsh/site-functions/_sway-launch  # on $fpath
install -Dm644 completions/sway-launch.fish ~/.config/fish/completions/sway-launch.fish
```

**From source.** This needs Rust 1.89 or newer. Distribution packages are often older (Ubuntu 24.04
ships 1.75, Debian 12 ships 1.63), so install a current toolchain with rustup if yours is behind.
Then build and install to `~/.cargo/bin` in one step:

```shell
cargo install --path . --locked
```

or build only, with the binary written to `target/release/sway-launch`:

```shell
cargo build --release --locked
```

A source build makes the man page with `cargo run --features generate-man -- --generate-man >
sway-launch.1`, and any build prints completions for bash, zsh, fish, elvish or PowerShell:

```shell
sway-launch --completions bash > ~/.local/share/bash-completion/completions/sway-launch
# System-wide: the redirect needs root too, so pipe through sudo tee rather than sudo sway-launch >
sway-launch --completions bash | sudo tee /usr/share/bash-completion/completions/sway-launch
```

## Quick start

Run these from a terminal in your Sway session. The examples use `foot`, Sway's default terminal;
any application works.

**1. Launch a window.** `sway-launch` starts the command, waits until its window is open, and
prints the window's container id:

```console
$ sway-launch foot
271
```

**2. Do something with it.** Actions are flags, applied to the new window before `sway-launch`
returns:

```shell
sway-launch --floating --width 800px --height 500px --position center --settle 500 foot
```

`--settle 500` holds the size and position for half a second, putting them back if the application
changes them as it starts (see [Settle](#settle)).

**3. Say which window you mean.** If an application might already be running, or several windows
may open at once, tell `sway-launch` which one belongs to the launch with `-a` (a Wayland app_id)
or `-c` (an X11 class). `--list-windows` shows the values your open windows have:

```shell
sway-launch -a foot foot
sway-launch -c Code code
sway-launch --list-windows
```

**4. Chain calls into a layout.** Each call returns only once its window is in place, so the next
one can build on it. `--split h` makes the next window open beside this one, `--split v` below:

```shell
sway-launch -a foot --split h foot    # left
sway-launch -a foot --split v foot    # right, top
sway-launch -a foot foot              # right, bottom
```

**5. Act on a window that's already open.** `--existing` finds one instead of launching:

```shell
sway-launch --existing -a firefox --workspace 2 --focus
```

**6. Use a built-in template.** A template is a ready-made shape you fill with any applications.
`--on-workspace` builds it on an empty workspace rather than around your terminal:

```shell
sway-launch --list-templates
sway-launch --template quad-grid --apps foot,foot,foot,foot --on-workspace 9
```

**7. Write your own layout file.** The script from step 4 as a file, one `[[step]]` per call:

```toml
# my-layout.toml
[[step]]
command = "foot"
app_id = "foot"
split = "h"

[[step]]
command = "foot"
app_id = "foot"
split = "v"

[[step]]
command = "foot"
app_id = "foot"
```

```shell
sway-launch --layout my-layout.toml --validate     # check it without running anything
sway-launch --layout my-layout.toml --dry-run      # print the Sway commands it would send
sway-launch --layout my-layout.toml --on-workspace 3
```

**8. Capture a workspace you arranged by hand.** `--capture` writes the layout file that rebuilds
it:

```shell
sway-launch --capture > workspace.toml
```

From here, the [Guide](#guide) covers each of these in depth, the
[Actions reference](#actions-reference) every action, and `sway-launch --help` every flag.

## Guide

### Launching a command

Quote a command that has arguments: `sway-launch 'firefox --new-window https://example.com'`. It
runs as a shell command line, exactly as `swaymsg exec` would run it, so shell syntax works too:
`'cd ~/project && foot'`.

Sway's own command parser reads the command before the shell does, which has three consequences:

- An unquoted `,` or `;` would end the `exec` and start a separate Sway command, so `sway-launch`
  rejects one up front. Wrap the part containing it in quotes (`'foot -T "a,b"'`), escape it with a
  backslash, or move the command into a script. This rules out an unquoted shell `;` too:
  `'sh -c "if true; then foot; fi"'` works, a bare `if true; then foot; fi` doesn't.
- A newline (or carriage return) is rejected anywhere in the command, even inside quotes, since
  Sway turns it into a `;` before it looks at quotes at all. Put a multi-line command in a script.
- Sway substitutes its own `set $name` variables (`$term`, `$mod`, ...) in the command before
  running it.

`sway-launch` also runs the command inside a `( ... )` group, which keeps the shell Sway starts
alive above the program so the window can be recognised (see
[How a launch finds its window](#how-a-launch-finds-its-window)). A shell comment (an unquoted `#`
starting a word) would cut that group short, so it's rejected too. The same rules apply to a
`command` in a layout or bindings file, where a NUL character is rejected along with a newline.

**The command runs in Sway's working directory and environment, not yours.** `sway-launch` hands
the command to Sway's `exec`, so the program is started by the compositor and inherits *Sway's*
working directory (usually your home directory) and environment, not those of the shell or script
that ran `sway-launch`:

- A relative path resolves against Sway's directory, not the current one, including in a layout
  or bindings file's `command`, where it isn't relative to the file either. `cd ~/project &&
  sway-launch 'code .'` opens Sway's directory.
- A variable you export before running `sway-launch` (an activated virtualenv's `PATH`,
  `GDK_BACKEND`, anything set only in a shell rc file Sway never read) doesn't reach the program.
- The program's own stdout and stderr go to Sway's log, not your terminal. A command that works in
  your shell but can't start this way usually shows up as a timeout, with the program's own error
  in Sway's log.

`--cwd <DIR>` starts the command in a directory (`~` is your home directory; a relative path is
relative to where you run `sway-launch`, and it must exist when the command launches; `--validate`
and `--dry-run` check only its format), and `--env KEY=VALUE` (repeatable)
sets a variable for it. A layout step or binding takes the same as `cwd = "~/project"` and
`env = { GDK_BACKEND = "x11" }`. Both apply only to a launched command, and a value can't contain a
`$`, which Sway would replace with one of its config variables.

The variables and the directory don't go on the launch's command line, which every local user can
read (`ps`, `/proc/<pid>/cmdline`) for as long as the program runs. `sway-launch` writes them to a
file only you can read, under `$XDG_RUNTIME_DIR/sway-launch/`, which the launch reads and then
deletes; `sway-launch` deletes it too if the launch fails or times out first. Without
`XDG_RUNTIME_DIR` they go on the command line instead, with a warning, so don't pass a secret
through `--env` in that case.

```shell
sway-launch --cwd ~/projects/website --env GDK_BACKEND=x11 -c Code 'code .'
```

### Matching the right window

**Pass `-a`/`-c` whenever the application might already be running**, even when the call launches
only one window. A browser or editor that's already open usually hands the request to the running
copy, and the window it opens can't be traced back to this launch, so `sway-launch` falls back to
the first new window that appears within a short grace period. Without a filter that can be any
window (a notification, another application finishing its own start), and every action then lands
on it. With `-a`/`-c` only the right application's window can match. The same goes for
`app_id`/`class` in a layout step or a template binding. `--apps` has no way to set either, so
fill a template with `--bindings` when one of its applications might already be running.
`--verbose` points it out when an unfiltered launch was matched that way.

`--app-id` and `--class` can't be combined: a native Wayland application has an `app_id` and no
`class`; one running under XWayland has a `class` and no `app_id`. Both compare the whole value
exactly, case included, so `-a firefox` matches only `firefox`, not `firefox-esr`,
`org.mozilla.firefox` or `Firefox`. That's unlike Sway's own `[app_id="..."]` criteria, which are
regular expressions, so a value copied out of a Sway config may need adjusting.

#### Finding a window's app_id or class

The value an application reports isn't always its command name. Open the application, then run
`sway-launch --list-windows`, which prints one row per open window, the scratchpad included:

```console
$ sway-launch --list-windows
ID  APP_ID   CLASS  WORKSPACE     FLAGS     MARKS     TITLE
4   foot     -      1             -         -         ~/projects
7   firefox  -      2             focused   -         Sway — Mozilla Firefox
9   -        Code   2             floating  editor    main.rs - sway-launch - Visual Studio Code
12  foot     -      (scratchpad)  floating  dropdown  ~
```

A missing value prints as `-`, and a title longer than 50 characters is shortened. `--json` prints
`{"windows": [...]}`, one object per row with the full title and each window's `output`,
`scratchpad`, `floating`, `focused` and `split_container` fields (a missing value is `null`).
`-a`/`-c`/`--mark-match`/`--title` narrow the list to the windows `--existing` would choose between,
so `sway-launch --list-windows -a foot` shows what `sway-launch --existing -a foot` would act on. A
split container carrying a mark is listed too, with the flag `split`, since `--mark-match` can
select one. `swaymsg -t get_tree` shows the same values (`"app_id"`, and `"class"` under
`"window_properties"`).

The same application can report different values depending on how it was built, packaged or
started:

- VS Code runs under XWayland by default, with class `Code`; started as a native Wayland client
  (`code --ozone-platform=wayland`) its app_id is `code` instead.
- Chromium under XWayland has class `Chromium`; as a native Wayland client its app_id is `chromium`.
- Firefox is `firefox`, but Debian's Firefox ESR is `firefox-esr`.
- A Flatpak application usually uses its reverse-DNS id (`org.mozilla.firefox`).

With the wrong value the window still opens, but `sway-launch` never sees a match and stops with a
timeout that names the window it saw instead (see [Troubleshooting](#troubleshooting)).

Some applications, typically XWayland ones, set their app_id or class only after their window has
appeared, and Sway sends no event when it changes. A window the launched command itself opened is
therefore not given up on when it first fails to match: `sway-launch` keeps rechecking it until it
matches or `--timeout` runs out.

#### Matching by title

`--title <REGEX>` (`title` in a layout step or binding) also requires the window's title to match a
regular expression. Like Sway's own `title=` criteria it's unanchored, so `--title project-a`
matches `main.rs - project-a - Visual Studio Code`; use `^` and `$` to anchor it. It narrows
`--app-id`/`--class` (or `--mark-match` with `--existing`), or works on its own:

```shell
# Several terminals share app_id foot; pick the one titled "logs"
sway-launch --existing -a foot --title '^logs$' --fullscreen

# An editor that shows a splash window first: wait for the project window instead
sway-launch -c Code --title 'project-a' --width 60ppt 'code ~/projects/project-a'
```

For a launch, a window that opens with a different title doesn't end the wait, which is how a
splash or welcome window is skipped. A window this run opened is checked again when its title
changes, so one that starts as `Loading…` and later shows its project's title still matches. An
older window that happens to change its title meanwhile never counts. On a timeout, the windows
that were turned away are listed with their titles.

The pattern syntax is the usual one (`.`, `*`, `[...]`, `(a|b)`, `(?i)` for case-insensitive);
`\w`, `\d` and `(?i)` work on ASCII letters only. An invalid pattern is rejected before anything
runs.

### Acting on an open window

Every action also works on a window that's already open, for adjusting a window from a later step
in a script without relaunching it.

By container id, for example one an earlier call printed:

```shell
container_id="$(sway-launch -a foot foot)"
sway-launch --con-id "$container_id" --floating
```

With actions to apply, the id is looked up first, and a wrong one fails before anything is sent:
`no window with container id N` for an id nothing has, or `container N is a workspace, not a
window` (likewise for an output) for one copied from the wrong line of `swaymsg -t get_tree`. A
bare `--con-id N` with no actions isn't looked up: it just prints the id back, without needing a
Sway session.

By app_id, class or title, with `--existing`:

```shell
sway-launch --existing -a foot --fullscreen
```

Or by a mark an earlier `--mark` (or `swaymsg mark`) set, the classic "dropdown terminal" pattern
of a script finding its own window again without tracking a container id:

```shell
sway-launch --existing --mark-match dropdown-term --scratchpad
```

`--existing` needs one of `--app-id`, `--class` or `--mark-match` (pick one), or `--title`, which
can also narrow any of them. The match is exact and case-sensitive, and must find exactly one
window: several matches are an error, since `sway-launch` won't guess which one you meant. The same
flags with `--list-windows` show the windows it would choose between. The search includes Sway's
scratchpad, so if a visible and a hidden window share an app_id or mark, use `--con-id` instead.

**Waiting for a window something else starts.** With `--wait`, `--existing` waits up to `--timeout`
for a matching window to open instead of failing when there isn't one yet. That covers windows
`sway-launch` doesn't start itself: an `exec` line in the Sway config, an autostart entry or systemd
user unit, an app restoring its session, a launcher such as `fuzzel`:

```shell
# In the Sway config: start Firefox, and put its window on workspace 2 once it opens
exec firefox
exec sway-launch --existing --wait --timeout 30 -a firefox --workspace 2
```

A window already open is used at once. Several matching windows are still an error, at once, since
waiting can't tell them apart. In a layout step or a binding, write `wait = true` next to
`existing = true`. A window found this way wasn't launched by the run, so `--rollback-on-error`
never closes it.

A mark (or a `--con-id`) can also name a split container holding several windows, which is what a
`focus parent; mark <name>` binding marks. `--floating`, `--fullscreen`, `--scratchpad` and `--mark`
work on one, but `--workspace`, `--output` and `--focus` are refused with an error: Sway reports
those moves only for a single window, so `sway-launch` couldn't confirm them. Target one of the
windows inside, or use `swaymsg` directly for the container.

### Scripts

Since every call blocks until its window is in place, a shell script of `sway-launch` calls builds
the same layout every time: workspace 1 set up the same way when Sway starts, or an editor with
three terminals arranged around it from a launch script. The printed container id also lets you
combine `sway-launch` with your own `swaymsg` commands:

```shell
#!/bin/sh
container_id="$(sway-launch -a foot foot)"
swaymsg "[con_id=$container_id] move workspace 1"
```

**Run calls one after another rather than backgrounding them with `&`.** Two `sway-launch`
processes launching ordinary applications at the same time don't take each other's windows, since
each recognises the window its own command opened. But that can't work for a single-instance
application that's already running (see [Matching the right window](#matching-the-right-window)),
so two concurrent calls both starting one can still collide. Chaining calls sequentially is always
reliable. A layout file's `--concurrent` (see [Running layouts](#running-layouts)) launches in
parallel safely.

Not every layout can be built from the window's own actions: it depends on the layout and on the
workspace's current state. Most gaps can be filled with the container id and a few `swaymsg`
commands, or by focusing an earlier window between launches.

Runnable example scripts live in [`examples/scripts/`](examples/scripts/), each a small standalone
script built out of `sway-launch` calls; run one directly to see the layout it builds. The advanced
ones also need Firefox, Chromium, Thunar and VS Code (`code`) on `PATH`, and match each by the value
it reports on a typical setup (VS Code by class `Code`, the others by app_id); if one times out on
your system, check its real value with `--list-windows` and edit the script to match.

Basic (all `foot`):

- [`dual-terminals`](examples/scripts/dual-terminals): two terminals side by side, one row.
- [`triple-row`](examples/scripts/triple-row): three terminals side by side, one row.
- [`column-split`](examples/scripts/column-split): two terminals stacked in one column.
- [`quad-terminals`](examples/scripts/quad-terminals): four terminals as a 2x2 grid, two rows.
- [`workspace-and-position`](examples/scripts/workspace-and-position): a floating terminal moved
  to workspace 2 and centered.
- [`retarget-floating`](examples/scripts/retarget-floating): a terminal adjusted twice after
  launch, once by `--con-id` with its container id and once by `--existing` matching `--app-id`.

Advanced (several applications):

- [`dev-workspace`](examples/scripts/dev-workspace): VS Code taking most of the width, with two
  terminals stacked beside it; `--class`/`--app-id`, `--split` and `--width`.
- [`floating-file-manager`](examples/scripts/floating-file-manager): Thunar as a floating,
  fixed-size window with a mark, ready for a `for_window` rule (see [Mark](#mark)).
- [`browser-comparison`](examples/scripts/browser-comparison): Firefox and Chromium side by side on
  the same page.
- [`quad-mixed-apps`](examples/scripts/quad-mixed-apps): a 2x2 grid of foot, Firefox, Thunar and VS
  Code.
- [`editor-with-floating-terminal`](examples/scripts/editor-with-floating-terminal): VS Code
  full-width, with a small floating terminal on top.

### Layout files

`--layout <FILE>` runs a whole layout from one TOML file and one invocation. Each `[[step]]` is the
equivalent of one call's flags, run in order, stopping at the first error. The whole file is
checked before the first step starts, so a mistake in a later step opens no windows:

```toml
[[step]]
command = "foot"
app_id = "foot"
split = "h"

[[step]]
command = "foot"
app_id = "foot"
```

```shell
sway-launch --layout layout.toml
```

> [!WARNING]
> A layout file's `command` fields are executed. Running someone else's layout file is exactly as
> dangerous as running their shell script, so read it first. The same applies to a `--bindings`
> file (see [Templates](#templates)). Template files themselves are safe: they contain no commands
> at all, which is what makes them shareable on their own.

A step's keys mirror the CLI flags of the same name: `cwd`, `env`, `app_id`, `class`,
`mark_match`, `title`, `con_id`, `existing`, `wait`, `split`, `floating`, `sticky`, `fullscreen`,
`focus`, `mark`, `new_column`, `new_row`, `workspace`, `workspace_number`, `output`, `height`,
`width`, `position`, `settle`, `scratchpad`, `timeout`, `wait_time`, and `sway_commands` (a list,
one `--sway-command` per entry). Each is validated the same way as its flag, and a step without its
own `timeout`/`wait_time` takes the command line's `--timeout`/`--wait-time`. A step's `command` is
checked against the same Sway-parsing rules as the command line's (see
[Launching a command](#launching-a-command)), so `--validate` catches an unquoted `,`/`;` or an
embedded newline before anything launches. Steps launch next to the focused window, which a
fullscreen window keeps, so put `fullscreen = true` on a `target_id` step after the last launch
(see [Fullscreen](#fullscreen)).

Exactly one of `command`, `con_id`, `existing = true` or `target_id` is required per step, and only
one of `app_id`, `class` and `mark_match`, as on the command line. Two keys exist only in files:

- `id` names a step, so a later step can act on its window again through `target_id`. That's
  useful when several steps share an `app_id`, where `existing = true` would be ambiguous. See
  [`examples/layouts/retarget-by-id.toml`](examples/layouts/retarget-by-id.toml).
- `target_id` targets an earlier step's window by that name, instead of
  `command`/`con_id`/`existing`. It's an error if the named `id` doesn't exist or was used by more
  than one step. Like `con_id`, it can't be combined with `app_id`/`class`/`mark_match`, since it
  already names an exact window.

`command` can also be a list of words, the safest form for a generated file:

```toml
[[step]]
command = ["foot", "-T", "notes, todo; misc"]
app_id = "foot"
```

Each word reaches the program exactly as written: `sway-launch` quotes it for Sway and the shell, so
commas, semicolons, quotes, spaces and backslashes inside a word need no escaping. The first word is
the program, with no shell features (`&&`, `~`, `*`). A `$` is refused, since Sway replaces `$name`
with one of its config variables before the shell runs, even inside quotes; so is a newline. Use the
string form, or a script, for anything that needs the shell. A binding's `command` takes the same
two forms.

`-` in place of a file name reads it from stdin, so a script that generates a layout can pipe it
in. The same works for `--template` and `--bindings`, one of them per run; errors and `--validate`
then name the source `<stdin>`:

```shell
generate-my-layout | sway-launch --layout -
generate-my-bindings | sway-launch --template quad-grid --bindings -
```

Besides its steps, a layout file takes one top-level key, `concurrent = true`, which runs it as
[`--concurrent`](#running-layouts) does.

The per-window flags (`--split`, `--floating`, ...) can't be combined with `--layout`, since it
would be unclear which step they applied to. `--timeout`, `--wait-time`, `--verbose` and `--json`
still apply, `--json` printing one object at the end (see [JSON output](#json-output)). Examples
are in [`examples/layouts/`](examples/layouts/): [`quad-terminals.toml`](examples/layouts/quad-terminals.toml)
is the `quad-terminals` script as a file.

### Templates

A layout file names an application in every step. A template separates the two: its steps describe
the shape, and each `slot` names a window that you fill when you run it, from a list of commands
(`--apps`) or a bindings file (`--bindings`). The same template can build the same shape around
any applications.

```toml
[template]
description = "An editor beside a terminal."
category = "Grid"

[[step]]
slot = "editor"
split = "h"

[[step]]
slot = "terminal"
```

Since a template names no commands, it can't run anything on its own: it's safe to share, and the
commands stay in the `--apps` list or bindings file you supply.

Fill it from a list of commands, in the order the template's slots appear:

```shell
sway-launch --template template.toml --apps code,foot
```

`--apps` splits on plain commas, with no escaping, and trims each entry, so a command that itself
contains a comma needs `--bindings` instead. It also launches every slot **with no `app_id`/`class`
filter**, so when an application may already be running (an editor or browser that hands the
request to its open copy), a slot can take whichever new window appears first; see
[Matching the right window](#matching-the-right-window). Use `--bindings` with `app_id`/`class`
for such a slot. A bindings file gives each slot its own entry, including the filter, or matching
an already-open window instead of launching one:

```toml
[[binding]]
slot = "editor"
command = "code"
class = "Code"

[[binding]]
slot = "terminal"
existing = true
app_id = "foot"
```

```shell
sway-launch --template template.toml --bindings bindings.toml
```

A binding takes the keys that choose a window, as a layout step does (`command`, `con_id`,
`existing`, `wait`, `app_id`, `class`, `mark_match`, `title`, with exactly one of
`command`/`con_id`/`existing = true`), plus the launch's `cwd` and `env`. It can also replace the
step's `workspace`/`workspace_number`/`output` (where the window goes), `timeout`/`wait_time` (so
the slot a slow application fills can wait longer than the rest) and `settle` (so the slot a browser
fills can hold its size, see [Settle](#settle)). Besides its bindings and `[params]`, a bindings
file takes a top-level `concurrent = true`, which runs the template as
[`--concurrent`](#running-layouts) does.

A template step takes the same action keys a layout step has. Its window comes from `slot`, or from
`target_id`, which acts again on an earlier slot's window (a slot's name is its step's `id`). Each
slot name may appear on only one step. Every template file needs a `[template]` table with a
one-sentence `description` and a `category` (`Grid`, `Master/stack`, `Sidebar`, `Floating`,
`Multi-workspace/output`, `Retargeting`, or a new one).

**Where it's built.** A template's shape is built wherever focus is when it runs, and assumes that
workspace is empty. Run from a terminal, the terminal is on that workspace, so the shape is built
around it: `sway-launch` warns when the workspace already holds tiled windows (on stderr, and as a
`"warnings"` list under `--json`) but still runs. Your own Sway config applies to the windows a
template opens like to any other: an `assign` or `for_window` rule can send one to another
workspace or float it, and a `no_focus` rule leaves focus where it was, so the next step opens
beside the wrong window. `sway-launch` warns, naming the step, when a step's window didn't open
tiled on the build workspace. To build somewhere specific, pass
`--on-workspace <WORKSPACE>` (or `--on-workspace-number <N>`, see [Workspace](#workspace)) and
`--on-output <OUTPUT>`, which work for `--layout` files too, or declare it in the template:

```toml
[layout]
workspace = "3"
output = "HDMI-A-1"
```

The run switches to that workspace before the first step, focusing the output first so a new
workspace is created there, and builds the whole shape in place. A workspace that already exists
on another output, or that your Sway config assigns to one (`workspace 3 output DP-1`), is switched
to where it is, so with both set the run checks where it landed: an empty one is moved to the
requested output (`move workspace to output`) before anything launches, and one that already holds
windows fails the run, naming both outputs, rather than building on the wrong monitor. The command-line options win over
the table. A step's own `workspace`/`output` still moves that one window afterwards. A binding's
`workspace`/`workspace_number`/`output` replaces the step's, so a built-in that places windows on
fixed workspaces or outputs works on your setup without copying the file:

```toml
# sway-launch --template dual-output --bindings outputs.toml
[[binding]]
slot = "first"
command = "foot"
output = "eDP-1"

[[binding]]
slot = "second"
command = "firefox"
output = "HDMI-A-2"
```

A binding can't give a `workspace` to a slot whose step uses `new_column`/`new_row`, for the same
reason a step can't (see [New column](#new-column)); use `--on-workspace` to build the whole
template elsewhere instead.

**Params.** A template can leave its sizes to the caller with a `[params]` table: each entry names a
value and gives its default, and a step's `height`, `width` or `position` refers to one by writing
its name in braces as the whole value:

```toml
[params]
stack-width = "35ppt"

[[step]]
slot = "stack-top"
width = "{stack-width}"
```

`--param NAME=VALUE` (repeatable) overrides a default for one run, and so does a `[params]` table
in a bindings file, with `--param` winning over both:

```shell
sway-launch --template master-dual-stack --apps foot,foot,foot --param stack-width=40ppt
```

A value is a plain substitution: there are no expressions, and a param can't stand for part of a
value. An unknown name, or a value a field using it can't take, is an error before anything runs.

#### Built-in templates

Every file in [`templates/`](templates/) is built into the binary. `--template <name>`, with no
`.toml`, uses the built-in of that name, so nothing needs downloading; a value ending in `.toml` is
always a file on disk, so the two never collide. `--list-templates` prints each built-in's name,
category, description, slots in `--apps` order, and params with their defaults; `--json` gives the
same as `slots`, `slot_names` and `params`, so a script can fill `--apps` without reading the
TOML:

```console
$ sway-launch --list-templates
...
quad-grid                   Grid                    An equally sized 2x2 grid, two rows of two, with no application baked in. (4 slots: top-left, top-right, bottom-left, bottom-right)
...
```

`--show-template <NAME|FILE>` prints a template's TOML without running it (`--json` gives
`{"name": ..., "contents": ...}`), and each file's header comment has a ready-to-run `--apps`
example. Every shape with a size has params.

Each screenshot is a real `--template` run on a live Sway, with every slot labelled by name
(generated by `scripts/generate-layout-screenshots`).

| Preview | Category | Template | Shape | Slots |
| --- | --- | --- | --- | --- |
| [<img src="screenshots/dual-row.png" width="120" alt="dual-row template layout">](screenshots/dual-row.png) | Grid | [`dual-row`](templates/dual-row.toml) | Two windows side by side, one row | 2 |
| [<img src="screenshots/dual-column.png" width="120" alt="dual-column template layout">](screenshots/dual-column.png) | Grid | [`dual-column`](templates/dual-column.toml) | Two windows stacked, one column | 2 |
| [<img src="screenshots/triple-row.png" width="120" alt="triple-row template layout">](screenshots/triple-row.png) | Grid | [`triple-row`](templates/triple-row.toml) | Three windows side by side, one row | 3 |
| [<img src="screenshots/triple-column.png" width="120" alt="triple-column template layout">](screenshots/triple-column.png) | Grid | [`triple-column`](templates/triple-column.toml) | Three windows stacked, one column | 3 |
| [<img src="screenshots/quad-grid.png" width="120" alt="quad-grid template layout">](screenshots/quad-grid.png) | Grid | [`quad-grid`](templates/quad-grid.toml) | Equal 2×2 grid | 4 |
| [<img src="screenshots/six-grid.png" width="120" alt="six-grid template layout">](screenshots/six-grid.png) | Grid | [`six-grid`](templates/six-grid.toml) | Equal grid, two rows of three | 6 |
| [<img src="screenshots/eight-grid.png" width="120" alt="eight-grid template layout">](screenshots/eight-grid.png) | Grid | [`eight-grid`](templates/eight-grid.toml) | Equal grid, two rows of four | 8 |
| [<img src="screenshots/nine-grid.png" width="120" alt="nine-grid template layout">](screenshots/nine-grid.png) | Grid | [`nine-grid`](templates/nine-grid.toml) | Equal 3×3 grid | 9 |
| [<img src="screenshots/six-grid-vertical.png" width="120" alt="six-grid-vertical template layout">](screenshots/six-grid-vertical.png) | Grid | [`six-grid-vertical`](templates/six-grid-vertical.toml) | Equal grid, three rows of two — six-grid.toml rotated | 6 |
| [<img src="screenshots/eight-grid-vertical.png" width="120" alt="eight-grid-vertical template layout">](screenshots/eight-grid-vertical.png) | Grid | [`eight-grid-vertical`](templates/eight-grid-vertical.toml) | Equal grid, four rows of two — eight-grid.toml rotated | 8 |
| [<img src="screenshots/master-dual-stack.png" width="120" alt="master-dual-stack template layout">](screenshots/master-dual-stack.png) | Master/stack | [`master-dual-stack`](templates/master-dual-stack.toml) | One main window, a 2-window stack beside it | 3 |
| [<img src="screenshots/master-triple-stack.png" width="120" alt="master-triple-stack template layout">](screenshots/master-triple-stack.png) | Master/stack | [`master-triple-stack`](templates/master-triple-stack.toml) | One main window, a 3-window stack beside it | 4 |
| [<img src="screenshots/master-dual-stack-left.png" width="120" alt="master-dual-stack-left template layout">](screenshots/master-dual-stack-left.png) | Master/stack | [`master-dual-stack-left`](templates/master-dual-stack-left.toml) | A 2-window stack, one main window beside it — master-dual-stack.toml mirrored | 3 |
| [<img src="screenshots/master-triple-stack-left.png" width="120" alt="master-triple-stack-left template layout">](screenshots/master-triple-stack-left.png) | Master/stack | [`master-triple-stack-left`](templates/master-triple-stack-left.toml) | A 3-window stack, one main window beside it — master-triple-stack.toml mirrored | 4 |
| [<img src="screenshots/dual-sidebars.png" width="120" alt="dual-sidebars template layout">](screenshots/dual-sidebars.png) | Master/stack | [`dual-sidebars`](templates/dual-sidebars.toml) | One main window, a single narrow sidebar flanking each side | 3 |
| [<img src="screenshots/dual-stack-sidebars.png" width="120" alt="dual-stack-sidebars template layout">](screenshots/dual-stack-sidebars.png) | Master/stack | [`dual-stack-sidebars`](templates/dual-stack-sidebars.toml) | One main window, a 2-window stack flanking each side | 5 |
| [<img src="screenshots/master-dual-stack-bottom.png" width="120" alt="master-dual-stack-bottom template layout">](screenshots/master-dual-stack-bottom.png) | Master/stack | [`master-dual-stack-bottom`](templates/master-dual-stack-bottom.toml) | One main window on top, a 2-window stack split below it | 3 |
| [<img src="screenshots/master-triple-stack-bottom.png" width="120" alt="master-triple-stack-bottom template layout">](screenshots/master-triple-stack-bottom.png) | Master/stack | [`master-triple-stack-bottom`](templates/master-triple-stack-bottom.toml) | One main window on top, a 3-window stack split below it | 4 |
| [<img src="screenshots/master-dual-stack-top.png" width="120" alt="master-dual-stack-top template layout">](screenshots/master-dual-stack-top.png) | Master/stack | [`master-dual-stack-top`](templates/master-dual-stack-top.toml) | A 2-window stack on top, one main window below it — master-dual-stack-left.toml rotated | 3 |
| [<img src="screenshots/master-triple-stack-top.png" width="120" alt="master-triple-stack-top template layout">](screenshots/master-triple-stack-top.png) | Master/stack | [`master-triple-stack-top`](templates/master-triple-stack-top.toml) | A 3-window stack on top, one main window below it — master-triple-stack-left.toml rotated | 4 |
| [<img src="screenshots/sidebar-left.png" width="120" alt="sidebar-left template layout">](screenshots/sidebar-left.png) | Sidebar | [`sidebar-left`](templates/sidebar-left.toml) | Narrow sidebar on the left, wide main window on the right | 2 |
| [<img src="screenshots/sidebar-right.png" width="120" alt="sidebar-right template layout">](screenshots/sidebar-right.png) | Sidebar | [`sidebar-right`](templates/sidebar-right.toml) | Wide main window on the left, narrow sidebar on the right | 2 |
| [<img src="screenshots/sidebar-top.png" width="120" alt="sidebar-top template layout">](screenshots/sidebar-top.png) | Sidebar | [`sidebar-top`](templates/sidebar-top.toml) | Narrow bar across the top, wide main window below | 2 |
| [<img src="screenshots/sidebar-bottom.png" width="120" alt="sidebar-bottom template layout">](screenshots/sidebar-bottom.png) | Sidebar | [`sidebar-bottom`](templates/sidebar-bottom.toml) | Wide main window on top, narrow bar across the bottom | 2 |
| [<img src="screenshots/sidebar-left-dual-stack.png" width="120" alt="sidebar-left-dual-stack template layout">](screenshots/sidebar-left-dual-stack.png) | Sidebar | [`sidebar-left-dual-stack`](templates/sidebar-left-dual-stack.toml) | Sidebar on the left split into two windows (75%/25% height), wide main window on the right | 3 |
| [<img src="screenshots/sidebar-right-dual-stack.png" width="120" alt="sidebar-right-dual-stack template layout">](screenshots/sidebar-right-dual-stack.png) | Sidebar | [`sidebar-right-dual-stack`](templates/sidebar-right-dual-stack.toml) | Wide main window on the left, sidebar on the right split into two windows (75%/25% height) | 3 |
| [<img src="screenshots/floating-overlay.png" width="120" alt="floating-overlay template layout">](screenshots/floating-overlay.png) | Floating | [`floating-overlay`](templates/floating-overlay.toml) | A tiled main window, with a small floating window on top | 2 |
| [<img src="screenshots/floating-centered.png" width="120" alt="floating-centered template layout">](screenshots/floating-centered.png) | Floating | [`floating-centered`](templates/floating-centered.toml) | A single floating window, centered | 1 |
| — | Multi-workspace/output | [`workspace-spread`](templates/workspace-spread.toml) | Each window moved to its own named workspace | 3 |
| — | Multi-workspace/output | [`dual-output`](templates/dual-output.toml) | Each window moved to a different output (monitor) | 2 |
| [<img src="screenshots/retarget-by-slot.png" width="120" alt="retarget-by-slot template layout">](screenshots/retarget-by-slot.png) | Retargeting | [`retarget-by-slot`](templates/retarget-by-slot.toml) | Two windows side by side, then the first one retargeted by slot name | 2 |

`workspace-spread` and `dual-output` have no screenshot, since a single-output screenshot can't
show either.

### Capturing a workspace

Arrange a workspace by hand once, then let `sway-launch` write the layout file that rebuilds it:

```shell
sway-launch --capture > my-workspace.toml        # the focused workspace
sway-launch --capture 3 > three.toml             # workspace "3"
sway-launch --capture-number 3 > three.toml      # workspace "3: web", or whichever has number 3
sway-launch --layout my-workspace.toml --on-workspace 4
```

`--capture` takes a workspace's exact name. For workspaces named `<number>: <label>` and switched
to with `workspace number N`, use `--capture-number N`, which finds the workspace the same way;
`--capture 3` on such a workspace fails and suggests it. When two workspaces share the number, it
lists their names, so name one with `--capture` instead.

The file opens each window where it has to open to rebuild the same splits, then sets the sizes,
and finishes with the floating windows' size and position. Marks, sticky and fullscreen are kept
too, fullscreen on a step of its own at the very end (see [Fullscreen](#fullscreen) for why). It can't know how a window was started, so each step's `command` is a guess from the window's
app_id (or its class, lowercased): check and edit the commands before running the file. A client
sets its own app_id, so one that isn't a plain program name (letters, digits, `.`, `_`, `+`, `-`) is
written in the list form, `command = ["..."]`, which runs it as a single word rather than as shell
code, and the file notes it at the top. Each step
carries a comment with the window's title, to help tell them apart. Every built-in template
rebuilds its exact shape from its own capture.

Two things can't be captured exactly. Tabbed and stacked containers aren't supported, nor is a
split container floated as a whole (several windows floating together), and `--capture` refuses a
workspace holding one. And some uneven sizes come out approximate, since Sway
shares each resize with a window's siblings, and Sway 1.12 shares it differently from older
versions: two windows, three whose outer two are the same size, and even splits are exact on every
version; any other three are exact on Sway 1.9 to 1.11; four or more uneven ones are approximate.
The file says so in a note at the top when it applies.

### Running layouts

These apply to `--layout` and `--template` runs.

**Faster: `--concurrent`.** A layout normally starts each application once the previous step's
window is in place, so it takes as long as all of its applications' startups added together.
`--concurrent` starts them all at once on a workspace of their own (`sway-launch-staging-<pid>`),
then moves each window to the build workspace in step order and applies its step's actions there,
so it lands exactly where it would have opened. A four-window grid of applications taking about a
second each to start drops from about 4 seconds to just over 1. When the run fails, every window
already open is brought to the build workspace first. A launch that timed out or was interrupted
before its window appeared can't be: its application may still open that window, and Sway then
puts it on the staging workspace, out of sight. So the run names each such step when it ends, with
a warning on stderr and under `--json`'s `unresolved_launches` (`step`, `staging_workspace`, see
[JSON output](#json-output)). Under `--rollback-on-error` the launch first watches up to 2 more
seconds for its window and closes it, so only an application slower than that is left. An `existing` step
sees the same windows it would in a sequential run: a later step's window, already open on the
staging workspace, is never one it chooses between or waits for. Focus visibly jumps to the
staging workspace while the applications start. A single-instance application can't be told apart
this way: when two steps' launches end up with the same window, the run fails, so run such layouts
without `--concurrent`. A layout file or a bindings file can ask for it itself with a top-level
`concurrent = true`, so its callers needn't remember the flag:

```toml
concurrent = true

[[step]]
command = "foot"
app_id = "foot"
split = "h"
```

A template file can't: it names no applications, so it can't know whether they're safe to start
together. That's the bindings file's to say.

**Slow applications: `--continue-on-timeout`.** By default a step whose window doesn't appear within
its `timeout` stops the run, so one slow application at the start leaves every later one unopened.
A binding's `timeout` gives just that slot longer. `--continue-on-timeout` instead reports the step
and carries on; a later step that targets the missing window (`target_id`) is skipped too. The run
still exits non-zero, so a calling script knows the layout is incomplete. Each such step gets a
warning on stderr, and `--json` lists them under `timed_out_steps` (`step`, `error`, and any
`unmatched_windows` and `late_container_id`) and `skipped_steps` (`step`, `target_id`). Plain output prints a container id
only for each step that has one, so match steps to windows with `--json`'s `containers` when steps
may be missing. Only a missing window counts: a step that fails any other way still stops the run.

A slow application is usually still starting when its step gives up, and its window would then
open inside the layout, beside whatever a later step just placed. Until the run ends,
`--continue-on-timeout` watches for it instead: such a window is moved to the scratchpad
(`swaymsg scratchpad show` brings it back), with a warning on stderr, and reported as its
`timed_out_steps` entry's `late_container_id`. `--rollback-on-error` closes it with the rest when a
later step fails. A window that opens after the run has ended is no longer watched for and opens
wherever focus is.

**Clean up on failure: `--rollback-on-error`.** A failed run leaves whatever earlier steps launched
open, and the failing step's own window too when it launched one and then failed on an action.
`--rollback-on-error` closes them instead:

```shell
sway-launch --layout layout.toml --rollback-on-error
```

Only windows this run launched itself are closed, most recent first. A window a step retargeted
(`con_id`, `existing`, `target_id`) existed before the run and is left alone, and so is a window the
run matched but couldn't prove it launched (see
[How a launch finds its window](#how-a-launch-finds-its-window)); both are still reported as open
container ids. This undoes launches, not actions: a move, resize or mark an earlier step applied to
a retargeted window stays applied.

Closing a window asks its application to close, and the application decides: an editor with unsaved
changes can keep its window open. So rollback waits up to a second for each window to actually go,
and only counts the ones that did. A window still open after that, or whose close request failed
(usually because it had already closed), is reported with a warning and skipped, and the original
step failure stays the error that's reported. If the compositor stops answering, the remaining
windows are reported as unconfirmed at once rather than each waiting out the same timeout. With
`--json`, the error object lists what was closed and what couldn't be confirmed closed:
`"rolled_back": [123, 456], "rollback_failed": [789]` (see [JSON output](#json-output)). A launch
that times out or is interrupted keeps watching up to 2 more seconds for its window under
`--rollback-on-error`, and rolls that one back too rather than leaving it to appear after the run
has failed. A launch that timed out because its window opened under another app_id, class or title
than the step names also closes that window, when its process proves this run launched it (the
`unmatched_windows` entry's `launched_by_this_run`); with `--concurrent`, it's first brought back
from the staging workspace.

**Interrupting a run.** Ctrl-C (SIGINT), SIGTERM (a `timeout(1)` wrapper, a stopped service) or
SIGHUP (the terminal it was started from closing, an ssh session dropping) ends a run within a
moment as a failure of the step it was on, so `--rollback-on-error` closes what the
run launched and `--json` reports its partial progress. If the step's application is still
starting, `--rollback-on-error` first waits up to 2 seconds for its window, so that one is closed
too. The run exits 130, 143 or 129. A second signal exits at once, skipping the cleanup.

**Focus moving away.** A run notes the workspace it builds on once it has switched there. A step
that finds focus on another workspace (a keybinding, a notification click, another script), as it
starts or while its application is still starting, fails, naming both workspaces: every later step
would otherwise open on the other workspace and split the layout in two.

**One run at a time.** A run takes a lock on its file (or built-in template name) and the workspace
it builds on, so a second run of the same layout on the same workspace (a key pressed twice, a login
script still running when you start it by hand) stops at once with `another run of this layout on
workspace "3" is still in progress`, instead of interleaving its windows with the first run's.
A workspace is the same however it's named: `--on-workspace Dev` and `dev` are one workspace, and
a numbered workspace is known by its number, so `--on-workspace-number 3`, a run started on `3: web`
and that workspace after an icon daemon renamed it all take one lock (two workspaces sharing a
number share it too). With only `--on-output`, the workspace is the one that output shows. A layout
piped in on stdin is known by its contents.

**Layouts started together take turns.** New windows open wherever focus is, and there is one focus
for the whole Sway session, so two layouts can't both be built at once, even on different
workspaces. A run that launches windows also takes a lock for the Sway session, so runs started
together (several `exec sway-launch --template ... --on-workspace N` lines in your Sway config)
build one after another, each on its own workspace. A run waits up to its `--timeout` for the
others, then fails with `another layout is running in this Sway session`. A run that only retargets
open windows doesn't wait. The locks live under `$XDG_RUNTIME_DIR/sway-launch/` and are released when
the run ends, even if it crashes. `--no-lock` skips both, so the run neither refuses a repeat nor
waits.

## Actions reference

Actions apply to the window a call launched or found. They run in a fixed order: the moves
(`--new-column`, `--new-row`, `--workspace`, `--output`) first, one at a time, since each changes
where the window is before the next is judged; then `--split`, `--floating`, `--sticky`,
`--fullscreen`, `--focus`, `--height`, `--width`, `--position`, `--mark` and each `--sway-command`,
sent to Sway together as one command (`[con_id=N] floating enable, resize set width 700px, ...`), so
the window takes them all at once instead of visibly floating, then resizing, then jumping into
place; and `--scratchpad` last. Each action is still confirmed and reported on its own, and one
whose state already holds is left out of the command and reported `already_satisfied`.

An action is confirmed by the Sway event it causes together with the window's new state, or, for
actions Sway sends no event for, by checking the window's state shortly after the command (see
[Timing](#timing-timeout-and-wait-time)). These flags are a convenience: you could equally take the
container id and run `swaymsg` commands, or set up window rules with a mark.

### Floating

Makes the window floating.

```shell
sway-launch --floating 'firefox --new-window https://example.com'
```

### Sticky

Makes the window sticky: a floating sticky window shows on the current output across every
workspace, instead of just the one it was launched on. Sticky only takes effect while the window is
floating, so pair it with `--floating`. On a tiled window Sway just records the flag, and it applies
if the window is floated later.

```shell
sway-launch --floating --sticky foot
```

### Fullscreen

Makes the window fullscreen.

```shell
sway-launch --fullscreen foot
```

Sway keeps focus on a fullscreen window when another window opens on its workspace, and places the
new window right after it, so windows launched after it open behind it, unfocused and in reverse
order. Focusing another window on the workspace also takes it out of fullscreen. In a layout file,
make a window fullscreen last, on a step of its own with `target_id`, once every other window is
open and sized.

### Focus

Focuses the window, for when a later step would otherwise leave a different window focused, such
as a layout that ends by launching a background application.

```shell
sway-launch --focus foot
```

### Mark

Marks the window, for Sway rules or keybindings that act on it by mark, or for finding it again
with `--existing --mark-match`:

```shell
# sway config
for_window [con_mark="firefox-floating-left"] resize set 1100 px 90 ppt, move position 20 20
```

```shell
sway-launch --mark firefox-floating-left 'firefox --new-window https://example.com'
```

`--mark` uses Sway's plain `mark` command, so it **replaces** rather than adds, in two ways:

- the new mark becomes the window's **only** mark, and any marks it already carried are removed;
- a mark is unique across windows, so if **another window** already holds it, that window loses it.

So `sway-launch --existing --mark-match dropdown-term --mark pinned` leaves the window marked
`pinned` and no longer `dropdown-term`, and a later `--mark-match dropdown-term` finds nothing. To
add a mark instead, use `swaymsg '[con_id=<id>] mark --add <name>'`.

A mark may contain spaces, tabs and Sway's own command separators (`,`, `;`), which are quoted and
stored literally. A few characters are rejected up front, because Sway doesn't store them as given
and the mark could never be found again:

- a **double quote** or a **backslash**, stored with the escape character still attached;
- a **newline**, rewritten to `;` when stored;
- a **dollar sign**, since Sway replaces `$name` with the matching `set $name ...` variable from its
  config, even inside quotes.

Every other control character is rejected too, even though Sway would store it: a name is printed
later by status bars and `swaymsg`, and a shared template shouldn't be able to plant a terminal
escape sequence there. The same rules apply to `--mark-match`, `--workspace` and `--output`.

### Scratchpad

Moves the window to Sway's scratchpad, hidden until shown again (`swaymsg scratchpad show`, or a
keybinding matching a mark). It runs after every other action, so one call can build a
fully-configured window and then hide it away, the classic dropdown terminal:

```shell
sway-launch --floating --width 500px --height 400px --mark dropdown-term --scratchpad foot
```

A window already in the scratchpad is left there and reported `already_satisfied`.

### Sway command

Sends a Sway command of your own to the window, for a property no flag sets: borders, opacity, a
title format, a tabbed parent. Each `--sway-command` is sent as `[con_id=N] <command>` along with
the other actions, after `--mark` and before `--scratchpad`, in the order given. It's an escape
hatch: `sway-launch` can't tell what an arbitrary command was meant to do, so its effect is reported
`unconfirmed` under [`--json`](#json-output), and only Sway refusing it fails the run.

```shell
sway-launch --floating --width 900px --height 500px --sway-command 'border none' \
  --sway-command 'opacity 0.9' foot
```

A layout or template step takes a list: `sway_commands = ["border none", "opacity 0.9"]`.

Sway runs any command given window criteria, including ones that act on the whole session
(`exec`, `reload`, `bindsym`, even `exit`), and a layout or template file may come from someone
else. So a command must start with one that acts on one window: `allow_tearing`, `border`,
`floating`, `focus`, `fullscreen`, `inhibit_idle`, `layout`, `mark`, `max_render_time`, `move`,
`opacity`, `resize`, `scratchpad`, `shortcuts_inhibitor`, `split`/`splith`/`splitv`/`splitt`,
`sticky`, `title_format`, `unmark` or `urgent`. Forms of these that reach past the window are
refused too: `focus` with any argument (`focus left`, `focus output <name>`, `focus parent`), since
it moves focus somewhere else and the next window would open there (a bare `focus` is fine), and
`move workspace ...`, which can move the window's whole workspace with every window on it (use
`move container to workspace <name>`, or `--workspace`). `swap` isn't allowed, since it moves another
window into this one's place.

`inhibit_idle` and `shortcuts_inhibitor` work as `--sway-command` only, and are refused in a layout
or template file: the first keeps the screen from locking while the window is open, the second lets
the application grab every key, Sway's own bindings included, and neither is a setting a file from
someone else should be able to change for you.

Each entry is one command: an unquoted `,`
or `;`, a newline, and a leading `[` (the criteria are `sway-launch`'s to add) are refused, and so
is a `$`, which Sway would replace with a variable from your own config. So is an entry that ends
inside an unclosed quote or after an unpaired `\`: a step's entries reach Sway as one command, so
the quote would carry on into the next entry.

### Height and width

Sets the window's height and width, tiled or floating: `<n>px`, or `<n>ppt` for a percentage. A
pixel value must be at least 1 and a percentage 1-100; `0` and percentages over 100 are rejected,
since Sway would ignore or clamp them.

```shell
sway-launch --floating --width 1200px --height 80ppt foot
```

A tiled window that's alone on its workspace can't be resized, so size the second window of a
split rather than the first. A pixel size is confirmed against the window's new size; a `ppt` one
has no pixel figure to check against, so it's reported `unconfirmed` after the
[wait time](#timing-timeout-and-wait-time).

`changed` means Sway applied the size, not that the application kept it. The application draws the
window and has the last word on its size, a moment after Sway set it: some terminals, `foot`
included, round a floating window to whole character cells (an 800px-wide `foot` can end up
796px), and some applications put their own size back, which [`--settle`](#settle) is for.

### Position

Moves a floating window to `center` or to `<x>,<y>` in pixels. A tiled window's position comes from
the layout, and Sway refuses the command for one, so pair this with `--floating`. Coordinates are
relative to the window's workspace, as Sway's own `move position` is: the workspace starts below or
beside a bar and inside any outer gaps, so `0,0` is its top-left corner, not the output's. `<x>` and
`<y>` may be negative. To place a window on another monitor, combine `--output` with `--position`.

`center` centers the window's whole frame, borders and title bar included. With a pixel `--width` or
`--height` in the same call, it's centered on that size, as Sway does, even if the application then
rounds or changes its size (see [Height and width](#height-and-width)).

```shell
sway-launch --floating --position center foot
sway-launch --floating --position 100,200 foot
sway-launch --floating --output HDMI-A-1 --position 100,200 foot
```

### Settle

Some applications, browsers especially, set their own window size in the first fraction of a second
after opening, often just after `sway-launch` applied and confirmed the requested one. Without help,
such a window ends up a different size from one run to the next while every action reports
`changed`. `--settle <MS>` keeps holding the pixel `--height`/`--width` and `--position` for that
many milliseconds after they're confirmed, applying them again if the application undoes them:

```shell
sway-launch -a firefox --floating --width 1100px --height 800px --position 20,20 --settle 500 \
  'firefox --new-window'
```

Each re-apply starts the hold over; after three, the application is left to have its way and what
it changed is reported `unconfirmed` rather than failing the run. If the window's state can't be
read during the hold, the hold ends there, with the held actions reported `unconfirmed`; if the
window closes during it, the run fails saying so. A `ppt` size can't be held, so
`--settle` needs a pixel size or a position. It takes 1 to 60000 ms, and a run with it takes at
least that long, so use it only for applications that need it. In a template, a binding's `settle`
replaces the step's, since the binding knows which slot is a browser.

### Split

Sets the split for what opens next to the window: `h` makes the next window open beside it, `v`
below it.

```shell
sway-launch -a foot --split h foot
sway-launch -a foot foot
```

### New column

Moves the window right, into a new column beside its current one (`move right`):

```shell
sway-launch -a foot foot
sway-launch -a foot --new-column foot
```

A move is skipped, not sent, when it wouldn't make a new column, and reported `skipped` with a
`reason` under [`--json`](#json-output) (and a line under `--verbose`):

- `floating`: Sway only nudges a floating window 10 px for `move right`.
- `trailing_workspace_edge` (with several outputs): Sway would move the window onto the next output
  instead. That happens when the window is already the rightmost child of a horizontal (or tabbed)
  workspace, or the only window inside a container at that edge, which includes a lone window
  you've made tabbed and every first window when your config sets `workspace_layout tabbed` or
  `stacking`.
- `fullscreen` (with several outputs): Sway sends a fullscreen window's `move right` straight to
  the next output.

With a single output, only `floating` applies. The check runs right before the move is sent, so
`--new-column --new-row` judges the row move after the column move has happened.

`--new-column` and `--new-row` can't be combined with `--workspace`/`--workspace-number` (in a layout
step either): the move would apply on the workspace the window opened on, and the workspace move
would then drop it into the target as an ordinary new window. Build on the target workspace instead,
with `--on-workspace` or a template's `[layout]` table.

### New row

Moves the window down, into a new row beneath its current one (`move down`):

```shell
sway-launch -a foot foot
sway-launch -a foot --new-row foot
```

It's skipped in the same cases as [New column](#new-column), along the vertical axis (where a
stacked layout counts as vertical, the way tabbed counts as horizontal).

### Workspace

Moves the window to a workspace:

```shell
sway-launch --workspace 2 foot
```

The name is matched the way Sway matches it, ignoring case, so `--workspace web` joins an existing
`Web`. Sway's own keywords (`next`, `prev`, `next_on_output`, `prev_on_output`, `current`,
`back_and_forth`, in any case, and `number` and `to` on their own) are rejected, because Sway acts
on them even when quoted.

If your workspaces are named `<number>: <label>` (`1: term`, `2: web`) and your keybindings use
`workspace number N`, use `--workspace-number` (`workspace_number` in a file). It finds the
workspace the way Sway does, by the number its name starts with, so `--workspace-number 2` joins
`2: web`, where `--workspace 2` would create a separate workspace named `2`. With no workspace
numbered 2 yet, Sway creates one named `2`.

```shell
sway-launch --workspace-number 2 firefox
```

For the keyword forms, use the container id with `swaymsg`:

```shell
container_id="$(sway-launch foot)"
swaymsg "[con_id=$container_id] move container to workspace next"
```

### Output

Moves the window to an output (monitor):

```shell
sway-launch --output HDMI-A-1 foot
```

As with Sway's own `move container to output`, the window lands on the workspace that output is
showing, which brings into view a window that was on a hidden workspace of the same output. With
`--workspace` too, the window goes to that workspace and the workspace is moved to the output, along
with any other window on it, since a workspace moves as a whole.

The value is the output's connector name as `swaymsg -t get_outputs` shows it (`HDMI-A-1`, `DP-2`),
matched ignoring case. Sway also accepts an output's make, model and serial, but `sway-launch`
confirms the move by connector name, so that form isn't supported. Sway's directional keywords
(`left`, `right`, `up`, `down`, `current`) are rejected; use `swaymsg` with the container id for
those.

## Output and diagnostics

### Exit status and output streams

stdout carries exactly one thing: the container id on success (one line per step for a layout or
template), or the `--json` object. Everything else goes to stderr, so
`container_id="$(sway-launch ...)"` always gets a clean value, even with `--verbose`. The modes
whose output is the point (`--list-windows`, `--capture`, `--list-templates`, `--show-template`,
`--dry-run`, `--validate`, `--debug-events`, `--completions`) print it to stdout.

The exit status is 0 on success, 1 when the run fails, 2 for invalid arguments, and 130, 143 or 129
when a layout or template run is interrupted by SIGINT, SIGTERM or SIGHUP. A failure's message on stderr starts
with `error:` (and a rollback problem with `warning:`), colored only when stderr is a terminal and
`NO_COLOR` is unset.

### JSON output

`--json` prints the result as one JSON object. `actions` lists every planned action in order, each
as `{"kind": ..., "command": ..., "status": ...}`:

- `kind` names the action by its flag, in `snake_case` (`new_column`, `mark`, `height`, ...),
  whether it ran or was skipped. Match on this field: it never carries your values or Sway syntax.
- `command` is the Sway command the action sent, the text `--dry-run` prints for it (`move right`,
  `mark "pinned"`), on its own even when it was sent along with other actions. It's absent for a
  skipped action.
- `status` is one of:
  - `changed`: the action ran and changed the window. For a size or position, this is what Sway
    applied, which the application can still change (see [Height and width](#height-and-width)).
  - `already_satisfied`: the window was already in that state (floating, on that workspace, ...),
    so nothing was sent.
  - `unconfirmed`: the command was sent but the change wasn't observed in time. Only actions
    confirmed by checking the window's state can report this (see
    [Timing](#timing-timeout-and-wait-time)), plus `--sway-command` and anything `--settle` gave up
    on. It isn't an error, since some of these have legitimate outcomes where the state never
    changes (a lone tiled window can't be resized, a `ppt` size has nothing to compare against),
    but if you chain further actions assuming this one took effect, this is the field that tells
    you it may not have.
  - `skipped`: the action was never sent, with a `reason` (see [New column](#new-column)).

```console
$ sway-launch --json --floating --mark pinned foot
{"actions":[{"command":"floating enable","kind":"floating","status":"changed"},{"command":"mark \"pinned\"","kind":"mark","status":"changed"}],"container_id":437}
```

For a layout or template, `container_ids` lists every step's container id in order, `containers`
maps each *named* step (one with an `id`, or a template `slot`) to its id, and `actions` covers
every step, each action tagged with its 1-based `step`. A run that built on a workspace already
holding tiled windows, or one with a window your Sway config placed elsewhere (see
[Templates](#templates)), adds a `warnings` list:

```console
$ sway-launch --template quad-grid --apps foot,foot,foot,foot --json
{"actions":[{"command":"splith","kind":"split","status":"changed","step":1},{"command":"splitv","kind":"split","status":"changed","step":2},{"command":"move down","kind":"new_row","status":"changed","step":3},{"command":"splith","kind":"split","status":"changed","step":3}],"container_ids":[437,438,439,440],"containers":{"bottom-left":439,"bottom-right":440,"top-left":437,"top-right":438}}
```

**Errors.** A failure prints a JSON object to stderr instead of the plain message. Read it as the
**last line** of stderr.

```console
$ sway-launch --json --con-id 999999 --floating
{"container_id":999999,"error":"no window with container id 999999","rolled_back":[]}
```

- `container_id` is the window the run had resolved (launched, or found by `--con-id`/`--existing`)
  when a later action failed, so a window that launched and then failed on, say, `--position` can
  still be found and closed. It's absent when nothing was resolved yet (a launch that timed out),
  and it may itself be the window that closed. Plain output says the same with a `container <id>
  was the target when this failed` line.
- `unmatched_windows` lists windows a timed-out launch turned away because they didn't match its
  filter, present only when there are any. `launched_by_this_run` says whether the window's process
  was started by this run; when it was, the filter is what's wrong, and the window is yours to
  close:

  ```console
  $ sway-launch --json --timeout 2 -a notfoot foot
  {"error":"launch \"foot\": no window with app_id \"notfoot\" appeared within 2 sec; ...","rolled_back":[],"unmatched_windows":[{"app_id":"foot","class":null,"container_id":12,"launched_by_this_run":true,"title":"foot"}]}
  ```

- For a layout or template, `container_ids`/`containers` report the steps that completed, plus the
  failing step's own window if it launched one, so a caller can find the windows left open without
  walking Sway's tree. They're absent when there's no progress to report.
- `rolled_back` lists the windows `--rollback-on-error` closed, and `rollback_failed` (present only
  when non-empty) the ones it asked to close but couldn't confirm closed. The lists never overlap:
  `container_ids` are still open, `rolled_back` are gone, `rollback_failed` are worth checking.
- `unresolved_launches` (present only when non-empty) lists the `--concurrent` steps whose launch
  timed out or was interrupted before any window appeared, each as `step` and `staging_workspace`:
  the application may still open its window there, out of sight. A `--continue-on-timeout` run
  that succeeds otherwise reports it in its result object too.

```console
$ sway-launch --layout layout.toml --json
{"container_id":439,"container_ids":[437,438,439],"containers":{"editor":437},"error":"layout.toml: step 3: floating enable: no matching window event within 5 sec","rolled_back":[]}
```

### Verbose

`--verbose` (`-v`) explains on stderr what the run is doing: the commands it sends, how the window
was recognised, and how each action was confirmed.

```console
$ sway-launch --split h -v foot
Sway action: Exec "foot"
Sway command: exec export SWAY_LAUNCH_PID_MARKER=<token> && ( foot )
Event match: New container id 437 (PID-marker-confirmed)
Target container id: 437
Sway action: Split (container id: 437) (split: Horizontal)
Some actions have no matching event type. Will wait 20 ms, then run the Sway command.
Sway command: [con_id=437] splith
Confirmed via poll: splith (container id: 437)
437
```

### Dry run

`--dry-run` prints the Sway commands a run would send, numbered across every step, without
launching anything or touching Sway, so it works with no Sway session at all. It works with a
command or a layout or template:

```console
$ sway-launch --template master-triple-stack --apps foot,foot,foot,foot --dry-run
1. launch foot
2. splith
3. launch foot
4. splitv, resize set width 30ppt
5. launch foot
6. launch foot
```

Actions a real run sends together share a line, though a real run leaves out any whose state
already holds. A run that switches workspace first (`--on-workspace`, a `[layout]` table) lists those
commands first. `--json` prints `{"steps": [{"target": ..., "actions": [{"kind": ..., "command": ...}]}]}`,
with a `"setup"` list of those commands ahead of `"steps"` when there are any. Control characters show as visible escapes (`\u{1b}`, `\t`), so a file can't use
a terminal escape sequence to make the preview show something other than what it would run.

The preview is planned, not predicted: `--new-column`/`--new-row` always show, even where a real run
would skip one (that needs the live window), and a `target_id` resolves against a placeholder.

### Validate

`--validate` checks a `--layout` or `--template` file (and, for a template, its `--bindings`/`--apps`)
without launching anything or touching Sway: every value's format, each step's target keys, and
`target_id` references. Useful in CI or a dotfiles repository. It checks the file, not this machine:
a `cwd` that doesn't exist here passes, and a real run is what fails on a missing directory.

```console
$ sway-launch --layout layout.toml --validate
valid: layout.toml (3 step(s))
$ sway-launch --layout broken.toml --validate
error: broken.toml: step 2: height: Must be <N>px (N >= 1) or <N>ppt (N 1-100), e.g. 300px or 20ppt (ppt = percent)
```

An error names the file holding the mistake: a bad binding is reported against the bindings file
(or `--apps`), not the template. `--json` prints `{"source": "...", "steps": N, "valid": true}`, or
the usual error object.

### Debug events

`--debug-events` prints every Sway IPC event until stopped with Ctrl-C, for seeing what Sway
actually sends when something doesn't behave as expected. With `--json`, each event is one line:
Sway's event as the `swayipc` library models it, plus an `event` field naming its type by Sway's own
name (`window`, `barconfig_update`, ...). A field `swayipc` doesn't know is left out, and one Sway
didn't send reads `null`; `swaymsg -t subscribe -m '["window"]'` shows the raw events instead. For
example, the app_id of every window that opens:

```shell
sway-launch --debug-events --json | jq -r 'select(.event == "window" and .change == "new") | .container.app_id'
```

## Timing: timeout and wait time

`--timeout <SECONDS>` (default 5, at least 1) bounds how long a run waits for the launched window,
and for each action's confirmation. An action's wait counts from when its command is sent, so the
`--wait-time` below never shortens it. A layout step's or binding's `timeout` replaces it for that
step. Each single request to Sway is bounded separately, at 10 seconds, so a compositor that stops
answering fails with a clear message rather than hanging. The default is sized for a terminal:
give a browser, an Electron app such as VS Code, or anything else slow to open its first window a
longer `--timeout` (the example scripts use `--timeout 30` for these), especially at login.

`--wait-time <MS>` (default 20) applies to the actions Sway sends no event for: `--split`,
`--sticky`, `--new-column`, `--new-row`, `--height`, `--width`, `--position`, `--workspace` together
with `--output`, and `--sway-command`. The wait runs once before their command, to let other IPC
clients finish their own, and afterwards `sway-launch` checks the window's state until the change
shows, for up to `--wait-time` and at most 200 ms (25 ms for `--new-column`/`--new-row`). When the
change never shows, the action waits `--wait-time` once more and is reported `unconfirmed`: the run
still succeeds, since several of these are legitimate no-ops, so a script that wants to treat
`unconfirmed` as fatal checks the `--json` field. On a heavily loaded machine, raise `--wait-time`.

## How a launch finds its window

`sway-launch` doesn't start the program itself, so it can't simply know its process id. Instead it
runs the command as `exec export SWAY_LAUNCH_PID_MARKER=<token> && ( <command> )`, and a new window
is this launch's once its process carries that token in its environment (read from `/proc`). That's
deterministic and lets launches run side by side.

A program that never sees the marker is still recognised when it descends from the launch: a wrapper
that clears the environment (`env -i`, a script that unsets it), or a program whose environment
can't be read (a setuid program, or one that marks itself non-dumpable). `sway-launch` walks up from
the window's process to the shell Sway started for this launch, which it knows by its command line.
A program that has left that line of descent (started through `systemd-run`, `setsid -f`, or a daemon
such as an app launcher) can't be recognised this way.

When no window is recognised, the first new window that matches the filters is taken as a
fallback: at once if the launched process was seen running and has exited (the single-instance case:
the request went to an already-running copy), and otherwise after a grace period of about 2 seconds.
`--rollback-on-error` closes a fallback window only in the first case, and only when the window's
process was already running before the launch, as an already-running copy is. A window taken after
the grace period, or one from a process started after the launch (the launched command failed and
exited while you opened a window of your own), is treated as adopted and never closed. This is why
`-a`/`-c` matter for an application that may already be running.

A client/server launcher always takes the slow path: `footclient` (with `foot --server` running),
`emacsclient -c`, `code --wait`. The window belongs to the already-running server, so it never
carries the marker, and the client stays alive until the window closes, so it never exits either.
Each such launch waits out the grace period and then uses the first window that matches. On a system
without `/proc` (a BSD running Sway, an unusual container), every launch goes through the same
fallback; nothing breaks, but sequential chaining matters more there.

## Troubleshooting

**The run times out waiting for the window.** The error says what it saw:

```console
$ sway-launch --timeout 2 -a notfoot foot
error: launch "foot": no window with app_id "notfoot" appeared within 2 sec; 1 other new window did (con_id 12, app_id "foot", opened by this command) — check --app-id/--class (a window opened by this command was rechecked until the timeout, in case the app sets its app_id/class after mapping)
$ sway-launch --timeout 2 footx
error: launch "footx": no window appeared within 2 sec: the launched command exited without opening a window — check that it is on PATH in Sway's environment
```

- A window opened under a different app_id or class: check the real value with
  `sway-launch --list-windows` (see [Finding a window's app_id or class](#finding-a-windows-app_id-or-class)).
  The rejected window stays open, and `--json` lists it under `unmatched_windows`.
- The command exited without a window: it probably isn't on `PATH` in Sway's environment, or
  failed; its own error is in Sway's log (see [Launching a command](#launching-a-command)).
- The command ran and exited without a new window: a single-instance application (VS Code,
  KeePassXC, most Electron apps) that was already running raises its open window instead of
  opening another. Act on that window with `--existing` (for example `sway-launch --existing -c
  Code --floating`).
- A window some other program opened during the wait (not traced to this command) is listed after
  the explanation as `meanwhile 1 other new window opened, not traced to this command (...)`,
  rather than replacing it. It only points at `--app-id`/`--class` when that window is really
  this application's, handed off to another process (a client/server launcher).
- The application is just slow to start: raise `--timeout`, or a binding's `timeout` for its slot.
- A `warning: Sway 1.7.0 is older than 1.9` (or `... is not Sway`) line came first: the
  compositor is one `sway-launch` doesn't support, and the timeout may be that difference
  showing. Debian 12 and Ubuntu 22.04 ship Sway 1.7.

**An action lands on the wrong window.** The application was probably already running and the
launch fell back to the first new window; pass `-a`/`-c` (see
[Matching the right window](#matching-the-right-window)).

**`--width`/`--height` did nothing.** A tiled window alone on its workspace can't be resized; size
the second window of a split instead. A browser that changes its own size right after opening needs
[`--settle`](#settle).

**`--new-column`/`--new-row` was skipped.** See the reasons under [New column](#new-column); `--json`
and `--verbose` say which one applied.

**A template was built around the terminal you ran it from.** Pass `--on-workspace` to build it on
an empty workspace (see [Templates](#templates)).

**A layout's shape comes out wrong, or a warning says a window opened elsewhere.** Your Sway config's
`assign`, `for_window` and `no_focus` rules apply to the windows a layout opens. A warning like
`step 2: container 14 opened on workspace "8", not on workspace "4"` (or `opened floating`) names
the step whose window a rule moved or floated; a `no_focus` rule gives no warning, but shows as the
next step's window opening beside the wrong one. Narrow the rule, or give the step's application an
app_id the rule doesn't match.

**A layout run says another run is still in progress.** The same layout is already being built on
that workspace; wait for it, or pass `--no-lock`.

**A layout run says another layout is running in this Sway session.** It waited its `--timeout` for
another layout run to finish, and that one was still going (a long `--timeout` of its own, or an
`--existing --wait` step). Raise `--timeout`, chain the runs (`sway-launch --template a ... &&
sway-launch --template b ...`), or pass `--no-lock` to run alongside it, at the risk of the two
building into each other's workspace.

## Command-line reference

`sway-launch -h` prints the summary below, and `sway-launch --help` (or `man sway-launch`) the full
description of each flag.

```shell
Launch an application, wait for its Sway window, then run follow-up actions against it

Usage: sway-launch [OPTIONS] <COMMAND>
       sway-launch [OPTIONS] --existing <-a APP_ID|-c CLASS|--mark-match MARK|--title REGEX>
       sway-launch [OPTIONS] --con-id <ID>
       sway-launch --layout <FILE> [OPTIONS]
       sway-launch --template <NAME|FILE> <--apps APPS|--bindings FILE> [OPTIONS]
       sway-launch <--list-windows|--capture [WORKSPACE]|--list-templates|--debug-events>

Launch a window:
  [COMMAND]              The command to launch, as one string: quote it if it has arguments
      --cwd <DIR>        Start the command in this directory
      --env <KEY=VALUE>  Set a variable for the command (repeatable)

Match the window:
  -a, --app-id <APP_ID>    Match the window by app_id (exact, case-sensitive)
  -c, --class <CLASS>      Match the window by X11 class (exact, case-sensitive)
      --title <REGEX>      Match the window by title (a regular expression)
      --mark-match <MARK>  Match an open window by one of its marks (exact, case-sensitive)

Use an open window instead:
      --existing     Act on an open window found by the match flags, launching nothing
      --wait         With --existing, wait for the window to open
      --con-id <ID>  Act on the open window with this container id

Act on the window:
  -s, --split <SPLIT>           Split the window, so the next one opens beside (h) or below (v) it [possible values: v, h]
  -f, --floating                Make the window floating
      --sticky                  Make the window sticky (a floating window shows on every workspace)
      --fullscreen              Make the window fullscreen
      --focus                   Focus the window
  -m, --mark <MARK>             Mark the window, replacing its marks
      --scratchpad              Move the window to the scratchpad, after every other action
      --sway-command <COMMAND>  Send a Sway command to the window, for what no flag sets (repeatable)

Size and position:
      --height <SIZE>        Set the window's height: <n>px or <n>ppt (percent)
      --width <SIZE>         Set the window's width: <n>px or <n>ppt (percent)
      --position <POSITION>  Move a floating window to "center" or "<x>,<y>"
      --settle <MS>          Hold the px size and position for MS ms, reapplying them if changed

Move the window:
  -n, --new-column             Move the window into a new column (move right)
  -r, --new-row                Move the window into a new row (move down)
      --workspace <WORKSPACE>  Move the window to this workspace
      --workspace-number <N>   Move the window to the workspace with this number
      --output <OUTPUT>        Move the window to this output (monitor)

Layouts and templates:
      --layout <FILE>             Build the windows a TOML layout file describes (- reads stdin)
      --template <NAME|FILE>      Build a template's shape: a built-in name or a .toml file
      --apps <APPS>               Commands for the template's slots, comma-separated, in slot order
      --bindings <FILE>           Bindings file filling the template's slots (- reads stdin)
      --param <NAME=VALUE>        Set one of the template's params (repeatable)
      --on-workspace <WORKSPACE>  Build the run on this workspace
      --on-workspace-number <N>   Build the run on the workspace with this number
      --on-output <OUTPUT>        Build the run on this output (monitor)
      --concurrent                Start every step's application at once, then arrange them in order
      --continue-on-timeout       Carry on past a step whose window doesn't appear in time
      --rollback-on-error         Close the windows this run launched if a step fails
      --no-lock                   Run even if another layout is already being built

Run and output:
  -t, --timeout <SECONDS>  Seconds to wait for the window, and for each action to be confirmed [default: 5]
  -w, --wait-time <MS>     Milliseconds to wait before checking actions Sway sends no event for [default: 20]
      --dry-run            Print the Sway commands a run would send, without running them
      --validate           Check a --layout/--template run's files without running it
      --json               Print the result as a JSON object instead of a bare container id
  -v, --verbose            Explain what the run is doing on stderr

Inspect:
      --list-windows               List every open window and exit
      --capture [<WORKSPACE>]      Print a layout file that rebuilds a workspace, and exit
      --capture-number <N>         Like --capture, for the workspace with this number
      --list-templates             List the built-in templates and exit
      --show-template <NAME|FILE>  Print a template's TOML and exit
  -d, --debug-events               Print every Sway event until interrupted

General:
      --completions <SHELL>  Print a shell completion script and exit [possible values: bash, elvish, fish, powershell, zsh]
  -h, --help                 Print help (see more with --help)
  -V, --version              Print the version

Examples:
  # Launch foot and print its container id
  sway-launch foot
  # Launch htop in foot as a floating window, half the screen wide
  sway-launch -a foot -f --width 50ppt 'foot -e htop'
  # Move the open Firefox window to workspace 2
  sway-launch --existing -a firefox --workspace 2
  # Fill a 2x2 grid with four terminals
  sway-launch --template quad-grid --apps foot,foot,foot,foot
  # List the built-in layout templates
  sway-launch --list-templates

The full guide, with layout files and templates, is at https://github.com/donnex/sway-launch#readme
```
