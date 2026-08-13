# sway-launch

`sway-launch` is a [Sway](https://swaywm.org/) tool for launching applications with windows and run addition actions against the new window. This can solve some problems like *Create a new floating Firefox window* or *Launch application and wait for the window before exiting*. This in turn can be used to create scripts that setups a workspace with some basic saved layouts.

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

The most basic use just execute the command given and it then waits for a matching Sway IPC new window event before returning the unique container id.

```shell
$ sway-launch kitty
271
```

The command must be quoted when passed to `sway-launch`.

```shell
$ sway-launch 'firefox --new-window https://example.com'
272
```

While this is not very useful on it's own the fact that all commands are blocking (waits for the window to be created before exiting) multiple `sway-launch` commands can be combined in scripts. This makes sure the previous window is created before the next command runs and no custom `sleep` needs to be run. The next command will run as soon as the previous window was created.
Since the container id of the matching window is returned it's also possible to combine `sway-launch` with custom `swaymsg` commands.

```shell
#!/bin/sh
container_id="$(sway-launch kitty)"
swaymsg "[con_id=$container_id] move workspace 1"

container_id="$(sway-launch kitty)"
swaymsg "[con_id=$container_id] floating enable, move position center"

sway-launch 'firefox --new-window https://example.com'
sway-launch 'firefox --new-window https://example.com'
```

It is possible to add addition checks against the new window. This makes sure the new window matches `app_id` or `class`. Could be useful when multiple commands are run at the same time in order to make sure the correct window are matched.

```shell
sway-launch -a kitty kitty
sway-launch -c Code code
```

## Recreatable layouts

Since `sway-launch` runs everything blocking it's possible to combine the different arguments to created scripts that recreate a static window setup/layout. For example always setup workspace 1 the same way when Sway starts or start VS Code together with tree terminals arranged in a cetain way with a launch script.

Not everything will work with the current implementation. It's all dependend on the layout and current workspace layout. Most of the issues should be fixable by catching the container id and run some additional `swaymsg` commands.

The `kitty` terminal will be used in these examples. It could as well be Firefox or any other slow loading window/application.

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

It's possible to run additional actions on the new window. Currently all these actions will wait for the corresponding Sway IPC event or a static `--wait-time` ms for actions without an event.

Multiple actions can be added to `sway-launch` and they'll be run one after another.

These exist for convinience. You could as well just get the container id and run manual `swaymsg` commands against the container id, setup windows rules with mark or other window rules.

### Floating

Makes the windows floating. Useful for applications with a single shared `app_id`. Firefox for example uses `app_id=firefox`. With `--floating` the window will be set to floating.

```shell
sway-launch --floating 'firefox --new-window https://example.com'
```

### Mark

Add a mark to the new window. This is useful when additional rules are setup in Sway. For example a left floating Firefox window with devdocs.io opened.

```shell
# sway config
for_window [con_mark="firefox-floating-left"] resize set 1100 px 90 ppt, move position 20 20
```

```shell
sway-launch --mark firefox-floating-left 'firefox --new-window https://example.com'
```

### Height and width

Set the height and widht on the new window. This usually works but it's dependend on the current Sway container layout. Should work on both tiled and floating windows.

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

Some actions like split and move does not have a corresponding Sway IPC event. For these actions a static sleep time will be used. For some machines or setups the wait time must be set higher or lower than the default.

```shell
...
Sway action: Split (container id: 439) (split: Horizontal)
No matching event types for action. Will run Sway command and wait 100 ms.
Sway command: [con_id=439] splith
439
```
