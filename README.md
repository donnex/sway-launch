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

Prebuilt Linux x86_64 binaries for tagged releases are also available from the repository's
Releases page.

```shell
Usage: sway-launch [OPTIONS] [COMMAND]

Arguments:
  [COMMAND]  Command to execute

Options:
  -a, --app-id <APP_ID>        app_id match
  -c, --class <CLASS>          class match
  -s, --split <SPLIT>          Change split for new window [possible values: v, h]
  -f, --floating               Make new window floating
  -m, --mark <MARK>            Add mark to new window
  -n, --new-column             Move window to new column (move right)
      --height <HEIGHT>        Set height on new window
      --width <WIDTH>          Set width on new window
  -r, --new-row                Move window to new row (move down)
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

## Recreatable layouts

Since `sway-launch` blocks on every command, its arguments can be combined into scripts that
recreate a static window setup/layout — for example, always setting up workspace 1 the same way
when Sway starts, or starting VS Code together with three terminals arranged in a certain way via
a launch script.

Not everything will work with the current implementation — it all depends on the layout and the
current workspace state. Most issues should be fixable by capturing the container id and running
some additional `swaymsg` commands.

The `kitty` terminal will be used in these examples. It could as well be Firefox or any other
slow-loading window/application.

### Examples

Quad terminals with four equally sized terminal windows in two rows.

```shell
#!/bin/sh
sway-launch -a kitty --split h kitty
sway-launch -a kitty --split v kitty

sway-launch --new-row -a kitty --split h kitty
sway-launch -a kitty kitty
```

More advanced layouts should be possible by focusing earlier windows between launches.

## In depth

It's possible to run additional actions on the new window. Each action waits for its
corresponding Sway IPC event, or for a static `--wait-time` ms if the action doesn't have one.

Multiple actions can be added to `sway-launch` and they'll be run one after another.

These flags exist for convenience — you could just as well get the container id and run manual
`swaymsg` commands against it, set up window rules with a mark, or use other window rules
directly.

### Floating

Makes the window floating. Useful for applications that share a single `app_id` across all their
windows — Firefox, for example, uses `app_id=firefox`.

```shell
sway-launch --floating 'firefox --new-window https://example.com'
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

### Height and width

Set the height and width of the new window. This usually works, but it depends on the current
Sway container layout — should work on both tiled and floating windows.

The format used is `100px` or `100ppt` for percent.

```shell
sway-launch --floating --width 1200px --height 80ppt kitty
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
