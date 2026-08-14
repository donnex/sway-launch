# sway-launch

`sway-launch` is a [Sway](https://swaywm.org/) tool for launching applications and running
additional actions against the new window they create. This solves problems like *creating a new
floating Firefox window* or *launching an application and waiting for its window before exiting*.
In turn, this can be used to build scripts that set up a workspace with a basic saved layout.

Requires a running Sway session — `sway-launch` talks to Sway over its IPC socket (the same one
`swaymsg` uses), so it won't do anything useful outside of one.

## Installation

Build from source with Cargo:

```shell
cargo build --release
```

The binary is written to `target/release/sway-launch` — put it somewhere on your `PATH`.

Prebuilt Linux x86_64 binaries for tagged releases are also available from the
[Releases page](https://github.com/donnex/sway-launch/releases).

```shell
Usage: sway-launch [OPTIONS] [COMMAND]

Arguments:
  [COMMAND]  Command to execute

Options:
  -a, --app-id <APP_ID>        app_id match. With --existing, matches an already-open window instead of the newly launched one
  -c, --class <CLASS>          class match. With --existing, matches an already-open window instead of the newly launched one
      --con-id <CON_ID>        Act on an already-open window with this container id, instead of launching a new one
      --existing               Act on an already-open window found via --app-id/--class, instead of launching a new one
  -s, --split <SPLIT>          Change split for new window [possible values: v, h]
  -f, --floating               Make new window floating
      --fullscreen             Make new window fullscreen
  -m, --mark <MARK>            Add mark to new window
  -n, --new-column             Move window to new column (move right)
      --height <HEIGHT>        Set height on new window
      --width <WIDTH>          Set width on new window
  -r, --new-row                Move window to new row (move down)
      --workspace <WORKSPACE>  Move new window to workspace
      --position <POSITION>    Set position on new window. Either "center" or "<x>,<y>" in pixels
  -t, --timeout <TIMEOUT>      Timeout in seconds [default: 5]
  -w, --wait-time <WAIT_TIME>  Wait time in ms. Used for actions that do not have a corresponding Sway IPC event [default: 20]
  -d, --debug-events           Debug events. Output all Sway IPC events until stopped
  -v, --verbose                Verbose output
  -h, --help                   Print help
  -V, --version                Print version
```

The most basic use is to just execute the given command; it then waits for a matching Sway IPC
new-window event before returning the window's unique container id.

```shell
$ sway-launch kitty
271
```

The command must be quoted when passed to `sway-launch`.

```shell
$ sway-launch 'firefox --new-window https://example.com'
272
```

On its own this isn't very useful, but since every `sway-launch` command blocks until its window
is created, multiple commands can be chained together in a script without needing a manual
`sleep` — each command starts only once the previous window exists.

Since the container id of the matching window is returned, it's also possible to combine
`sway-launch` with custom `swaymsg` commands.

```shell
#!/bin/sh
container_id="$(sway-launch kitty)"
swaymsg "[con_id=$container_id] move workspace 1"

container_id="$(sway-launch kitty)"
swaymsg "[con_id=$container_id] floating enable, move position center"

sway-launch 'firefox --new-window https://example.com'
sway-launch 'firefox --new-window https://example.com'
```

It is possible to add additional checks against the new window, to make sure it matches a given
`app_id` or `class`. This is useful when multiple commands run at the same time and you need to
make sure each one matches the correct window.

```shell
sway-launch -a kitty kitty
sway-launch -c Code code
```

`--app-id` and `--class` can't be combined — pick whichever matches the application (native
Wayland apps expose `app_id`; XWayland apps expose `class`).

## Recreatable layouts

Since `sway-launch` blocks on every command, its arguments can be combined into scripts that
recreate a static window setup/layout — for example, always setting up workspace 1 the same way
when Sway starts, or starting VS Code together with three terminals arranged in a certain way via
a launch script.

Not everything will work with the current implementation — it all depends on the layout and the
current workspace state. Most issues should be fixable by capturing the container id and running
some additional `swaymsg` commands.

The `kitty` terminal is used as a stand-in slow-loading application in most of these examples; a
few combine several different applications to show off more advanced layouts.

### Examples

Runnable example scripts live in [`examples/`](examples/) — each one is a small, standalone shell
script built entirely out of `sway-launch` calls; run any of them directly (e.g.
`examples/quad-terminals`) against a live Sway session to see the layout it builds. The advanced
examples expect Firefox, Chromium, Thunar, and VS Code (the `code` command) to be installed and on
`PATH`, in addition to `kitty`.

Basic (all `kitty`):

- [`examples/dual-terminals`](examples/dual-terminals) — two terminals side by side, one row.
- [`examples/triple-row`](examples/triple-row) — three terminals side by side, one row.
- [`examples/column-split`](examples/column-split) — two terminals stacked in one column.
- [`examples/quad-terminals`](examples/quad-terminals) — four terminals as a 2x2 grid, two rows.
- [`examples/workspace-and-position`](examples/workspace-and-position) — a floating terminal moved
  to workspace 2 and centered. Demonstrates `--workspace` and `--position` together.
- [`examples/retarget-floating`](examples/retarget-floating) — a terminal adjusted twice after
  launch, without relaunching it: once via `--con-id` with a captured container id, once via
  `--existing` matching `--app-id`.

Advanced (multiple applications):

- [`examples/dev-workspace`](examples/dev-workspace) — VS Code taking most of the width, with two
  terminals stacked in a column beside it. Demonstrates `--class` matching (`-c Code`) alongside
  `--app-id`, plus `--width` and `--new-column`.
- [`examples/floating-file-manager`](examples/floating-file-manager) — Thunar as a floating,
  fixed-size window with a mark set, ready for a `for_window` rule to reposition it (see the Mark
  section above). Demonstrates combining `--floating`, `--width`/`--height`, and `--mark`.
- [`examples/browser-comparison`](examples/browser-comparison) — Firefox and Chromium side by
  side on the same page, for comparing how each renders it.
- [`examples/quad-mixed-apps`](examples/quad-mixed-apps) — a 2x2 grid like
  `examples/quad-terminals`, but with four different applications (kitty, Firefox, Thunar, VS
  Code) instead of four terminals.
- [`examples/editor-with-floating-terminal`](examples/editor-with-floating-terminal) — VS Code
  full-width, with a small floating terminal on top for quick one-off commands.

More advanced layouts should be possible by focusing earlier windows between launches.

## In depth

It's possible to run additional actions on the new window. Each action waits for its
corresponding Sway IPC event, or for a static `--wait-time` ms if the action doesn't have one.

Multiple actions can be added to `sway-launch` and they'll be run one after another.

These flags exist for convenience — you could just as well get the container id and run manual
`swaymsg` commands against it, set up window rules with a mark, or use other window rules
directly.

### Target an existing window

All the actions above can also run against a window that's already open, instead of always
launching a new one — useful for adjusting a window from a later step in a script without
relaunching it.

Target a specific container id (e.g. one captured from an earlier `sway-launch` call):

```shell
container_id="$(sway-launch -a kitty kitty)"
sway-launch --con-id "$container_id" --floating
```

Or target an already-open window by matching `--app-id`/`--class` against currently open windows,
the same way those flags match a newly launched window:

```shell
sway-launch --existing -a kitty --fullscreen
```

`--existing` requires `--app-id` or `--class`, and errors if that doesn't match exactly one
window — it won't guess which one you meant.

### Floating

Makes the window floating. Useful for applications that share a single `app_id` across all their
windows — Firefox, for example, uses `app_id=firefox`.

```shell
sway-launch --floating 'firefox --new-window https://example.com'
```

### Fullscreen

Makes the window fullscreen.

```shell
sway-launch --fullscreen kitty
```

### Mark

Add a mark to the new window. This is useful when additional rules are set up in Sway — for
example, a floating Firefox window pinned to the left side of the screen with a specific page
open.

```shell
# sway config
for_window [con_mark="firefox-floating-left"] resize set 1100 px 90 ppt, move position 20 20
```

```shell
sway-launch --mark firefox-floating-left 'firefox --new-window https://example.com'
```

### Workspace

Move the new window to a workspace.

```shell
sway-launch --workspace 2 kitty
```

### Height and width

Set the height and width of the new window. This usually works, but it depends on the current
Sway container layout — should work on both tiled and floating windows.

The format used is `100px` or `100ppt` for percent.

```shell
sway-launch --floating --width 1200px --height 80ppt kitty
```

### Position

Set the position of the new window. Only makes sense for a floating window — a tiled window's
position is determined by the layout, not by coordinates. Either `center`, or `<x>,<y>` in pixels
from the top-left corner.

```shell
sway-launch --floating --position center kitty
sway-launch --floating --position 100,200 kitty
```

### Split

Change split on the new window.

```shell
sway-launch --split v kitty
sway-launch --split h kitty
```

### Verbose

Show verbose debug information.

```shell
$ sway-launch --split h -v kitty
Sway action: Exec "kitty" (app_id_match: "") (class_match: "")
Sway command: exec kitty
Event mismatch: Title container id 286 (Event does not match action event matches)
Event match: New container id 437 (New window without app_id or class check)
New window match container id: 437
Sway action: Split (container id: 437) (split: Horizontal)
No matching event types for action. Will run Sway command and wait 20 ms.
Sway command: [con_id=437] splith
437
```

### Wait time

Some actions, like split and move, do not have a corresponding Sway IPC event. For these, a
static sleep time is used instead. Depending on the machine or setup, the wait time may need to
be set higher or lower than the default.

```shell
...
Sway action: Split (container id: 439) (split: Horizontal)
No matching event types for action. Will run Sway command and wait 100 ms.
Sway command: [con_id=439] splith
439
```
