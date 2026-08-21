use clap::ValueEnum;
use regex::Regex;
use std::sync::mpsc;
use std::{fmt, thread, time};
use swayipc::{
    Connection, Event, EventStream, EventType, Node, NodeLayout, NodeType, WindowChange,
    WindowEvent,
};

/// Environment variable name `run_wait_matching_exec_event()` tags a
/// launched command's environment with, to correlate a matching `New`
/// window event back to the specific process this invocation spawned. See
/// that function's doc comment for the full mechanism.
const PID_MARKER_VAR: &str = "SWAY_LAUNCH_PID_MARKER";

/// How long `run_wait_matching_exec_event()` waits for a PID-marker-
/// confirmed match after seeing a content-matching-but-unconfirmed one,
/// before giving up and using that fallback candidate — independent of the
/// overall `--timeout` (though still capped by it, via `deadline.min(...)`,
/// for a short `--timeout`), so a genuinely ambiguous case adds a bounded
/// delay rather than the full timeout. Live testing under concurrent load
/// showed a shorter cap (500ms) occasionally forces a fallback before the
/// real PID-marker-confirmed match — which is still coming, just slightly
/// delayed by system load — arrives, causing exactly the wrong-container-id
/// collision this mechanism exists to prevent; 2s comfortably clears that
/// without meaningfully slowing the genuinely-ambiguous (single-instance
/// application) case, which resolves via `any_process_has_env_var()`
/// well before this cap in practice.
const PID_MARKER_FALLBACK_GRACE: time::Duration = time::Duration::from_millis(2000);

/// The upper bound `run_poll_then_fallback()` polls `get_tree()` for a
/// wait-time action's own confirmation before giving up and falling back to
/// the original blind sleep-the-rest-of-`--wait-time` behavior — capped at
/// the actual `--wait-time` in play (`run_poll_then_fallback()` computes
/// `WAIT_TIME_POLL_GRACE.min(wait_time)`), not used directly. Several of
/// these actions have legitimate no-op outcomes (e.g. resizing a solo
/// window, per docs/plan-poll-based-wait-time-actions.md) where the
/// expected tree state never arrives at all, so — like
/// `PID_MARKER_FALLBACK_GRACE` — this must stay well short of `--timeout`
/// rather than growing into a second hang. The `.min(wait_time)` cap exists
/// because this constant alone isn't: it's 10x the CLI's own 20ms
/// `--wait-time` default, and an earlier version of this code used it
/// unconditionally, so any fallback case at default settings cost ~220ms
/// instead of the ~40ms (`2 * wait_time`) it cost before this feature
/// existed — confirmed live before the cap was added. 200ms comfortably
/// covers every matcher this project has shipped (`Split`, `Height`,
/// `Width`, `Position`, `NewColumn`, `NewRow`), all confirmed live to
/// converge in a handful of milliseconds when they converge at all.
const WAIT_TIME_POLL_GRACE: time::Duration = time::Duration::from_millis(200);

/// How often `run_wait_time()`'s poll loop re-queries `get_tree()` while
/// inside `WAIT_TIME_POLL_GRACE`. Cheap enough on a local Unix socket to run
/// this often without meaningfully loading the compositor, while still
/// avoiding a zero-sleep busy loop.
const WAIT_TIME_POLL_INTERVAL: time::Duration = time::Duration::from_millis(10);

#[derive(Copy, Clone, PartialEq, ValueEnum, serde::Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    V,
    H,
}

impl fmt::Display for Split {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Split::V => write!(f, "Vertical"),
            Split::H => write!(f, "Horizontal"),
        }
    }
}

/// A validated `--height`/`--width` value, parsed once via `parse_size()`
/// rather than carried as a string and re-parsed on every use (previously,
/// `SwayAction::poll_matches()`'s `Height`/`Width` arms each called
/// `parse_pixel_value()` on the raw string on every poll iteration).
/// `Display` renders the same `<n>px`/`<n>ppt` text `sway_command()` needs
/// to interpolate into the Sway command, and that `SwayAction::Display`
/// needs for its human-readable `Sway action: ...` line — this makes
/// `sway_command()` a pure serialization step for these variants, not a
/// second place the format is defined.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Size {
    Pixels(u32),
    Percent(u32),
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Size::Pixels(pixels) => write!(f, "{}px", pixels),
            Size::Percent(percent) => write!(f, "{}ppt", percent),
        }
    }
}

/// Parses a value already validated by `validate_size_argument()` — never
/// called on unvalidated input. `validate_size_argument()` itself confirms
/// the digits fit in a `u32`, not just that they're digits, specifically so
/// the `.expect()`s here are trusting an already-checked invariant rather
/// than gambling on one.
pub fn parse_size(value: &str) -> Size {
    if let Some(pixels) = value.strip_suffix("px") {
        Size::Pixels(
            pixels
                .parse()
                .expect("validate_size_argument guarantees this parses as u32"),
        )
    } else {
        let percent = value
            .strip_suffix("ppt")
            .expect("validate_size_argument guarantees a px or ppt suffix");
        Size::Percent(
            percent
                .parse()
                .expect("validate_size_argument guarantees this parses as u32"),
        )
    }
}

/// A validated `--position` value, parsed once via `parse_position()`
/// rather than carried as a string — same reasoning as `Size` above.
/// `Display` renders `center` or `<x>,<y>`, the same text a `--position`
/// CLI argument or TOML field would itself use.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Position {
    Center,
    Coordinates { x: i32, y: i32 },
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Position::Center => write!(f, "center"),
            Position::Coordinates { x, y } => write!(f, "{},{}", x, y),
        }
    }
}

/// Parses a value already validated by `validate_position_argument()` —
/// never called on unvalidated input. `validate_position_argument()` itself
/// confirms `<x>`/`<y>` each fit in an `i32`, so the `.expect()`s here are
/// trusting an already-checked invariant rather than gambling on one.
pub fn parse_position(value: &str) -> Position {
    if value == "center" {
        return Position::Center;
    }
    let (x, y) = value
        .split_once(',')
        .expect("validate_position_argument guarantees a comma-separated pair");
    Position::Coordinates {
        x: x.parse()
            .expect("validate_position_argument guarantees this parses as i32"),
        y: y.parse()
            .expect("validate_position_argument guarantees this parses as i32"),
    }
}

enum WindowEventMatch {
    WindowAppId,
    WindowClass,
    NewWindowMatchWithoutCheck,
    WindowContainerIdMatch,
}

impl fmt::Display for WindowEventMatch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowEventMatch::WindowAppId => {
                write!(f, "Window app_id match")
            }
            WindowEventMatch::WindowClass => {
                write!(f, "Window class match")
            }
            WindowEventMatch::NewWindowMatchWithoutCheck => {
                write!(f, "New window without app_id or class check")
            }
            WindowEventMatch::WindowContainerIdMatch => {
                write!(f, "Window container id match")
            }
        }
    }
}

enum WindowEventMatchError {
    EventChangeTypeMismatch,
    WindowAppIdMismatch,
    WindowClassMismatch,
    NoMatchingEvent,
}

impl fmt::Display for WindowEventMatchError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowEventMatchError::EventChangeTypeMismatch => {
                write!(f, "Event does not match action event matches")
            }
            WindowEventMatchError::WindowAppIdMismatch => {
                write!(f, "app_id mismatch")
            }
            WindowEventMatchError::WindowClassMismatch => {
                write!(f, "class mismatch")
            }
            WindowEventMatchError::NoMatchingEvent => {
                write!(f, "No matching event")
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum SwayAction<'a> {
    Exec {
        command: &'a str,
        app_id_match: &'a str,
        class_match: &'a str,
        verbose: bool,
        timeout: time::Duration,
    },
    Split {
        container_id: i64,
        split: Split,
        verbose: bool,
        wait_time: time::Duration,
    },
    Floating {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    Sticky {
        container_id: i64,
        verbose: bool,
        wait_time: time::Duration,
    },
    Fullscreen {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    Focus {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    NewColumn {
        container_id: i64,
        verbose: bool,
        wait_time: time::Duration,
    },
    NewRow {
        container_id: i64,
        verbose: bool,
        wait_time: time::Duration,
    },
    Workspace {
        container_id: i64,
        workspace: &'a str,
        verbose: bool,
        timeout: time::Duration,
    },
    Output {
        container_id: i64,
        output: &'a str,
        verbose: bool,
        timeout: time::Duration,
    },
    Mark {
        container_id: i64,
        mark: &'a str,
        verbose: bool,
        timeout: time::Duration,
    },
    Height {
        container_id: i64,
        height: Size,
        verbose: bool,
        wait_time: time::Duration,
    },
    Width {
        container_id: i64,
        width: Size,
        verbose: bool,
        wait_time: time::Duration,
    },
    Position {
        container_id: i64,
        position: Position,
        verbose: bool,
        wait_time: time::Duration,
    },
    Scratchpad {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
}

impl fmt::Display for SwayAction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SwayAction::Exec {
                command,
                app_id_match,
                class_match,
                ..
            } => {
                write!(
                    f,
                    "Exec \"{}\" (app_id_match: \"{}\") (class_match: \"{}\")",
                    command, app_id_match, class_match
                )
            }
            SwayAction::Split {
                container_id,
                split,
                ..
            } => {
                write!(
                    f,
                    "Split (container id: {}) (split: {})",
                    container_id, split
                )
            }
            SwayAction::Floating { container_id, .. } => {
                write!(f, "Floating (container_id: {})", container_id)
            }
            SwayAction::Sticky { container_id, .. } => {
                write!(f, "Sticky (container_id: {})", container_id)
            }
            SwayAction::Fullscreen { container_id, .. } => {
                write!(f, "Fullscreen (container_id: {})", container_id)
            }
            SwayAction::Focus { container_id, .. } => {
                write!(f, "Focus (container_id: {})", container_id)
            }
            SwayAction::NewColumn { container_id, .. } => {
                write!(f, "New column (container_id: {})", container_id)
            }
            SwayAction::NewRow { container_id, .. } => {
                write!(f, "New row (container_id: {})", container_id)
            }
            SwayAction::Workspace {
                container_id,
                workspace,
                ..
            } => {
                write!(
                    f,
                    "Workspace (container id: {}) (workspace: {})",
                    container_id, workspace
                )
            }
            SwayAction::Output {
                container_id,
                output,
                ..
            } => {
                write!(
                    f,
                    "Output (container id: {}) (output: {})",
                    container_id, output
                )
            }
            SwayAction::Mark {
                container_id, mark, ..
            } => {
                write!(f, "Mark (container id: {}) (mark: {})", container_id, mark)
            }
            SwayAction::Height {
                container_id,
                height,
                ..
            } => {
                write!(
                    f,
                    "Height (container id: {}) (height: {})",
                    container_id, height
                )
            }
            SwayAction::Width {
                container_id,
                width,
                ..
            } => {
                write!(
                    f,
                    "Width (container id: {}) (width: {})",
                    container_id, width
                )
            }
            SwayAction::Position {
                container_id,
                position,
                ..
            } => {
                write!(
                    f,
                    "Position (container id: {}) (position: {})",
                    container_id, position
                )
            }
            SwayAction::Scratchpad { container_id, .. } => {
                write!(f, "Scratchpad (container_id: {})", container_id)
            }
        }
    }
}

impl SwayAction<'_> {
    /// The full `swaymsg` command string, `[con_id=N] <verb>` for every
    /// variant except `Exec` (which has no target container yet — it *is*
    /// the command that creates one). Just wraps `sway_command_verb()` with
    /// the `[con_id=N]` prefix; kept as a separate method (rather than
    /// inlining that wrap at every `run_sway_command()` call site) so
    /// there's exactly one place that does it.
    fn sway_command(&self) -> String {
        match self {
            SwayAction::Exec { command, .. } => format!("exec {}", command),
            other => {
                let container_id = other
                    .container_id()
                    .expect("only Exec lacks a container_id, and it's handled in the arm above");
                format!("[con_id={}] {}", container_id, other.sway_command_verb())
            }
        }
    }

    /// The `swaymsg` command *without* the `[con_id=N]` target prefix —
    /// `sway_command()`'s own building block, and also what `--dry-run`
    /// prints for a planned action whose container id isn't resolved yet
    /// (a not-yet-launched `Exec` target has no id to show). Never called
    /// for `Exec` itself, which has no verb-only form (`exec <command>` has
    /// no target-container concept at all) — `sway_command()` handles that
    /// variant directly instead of delegating here.
    pub fn sway_command_verb(&self) -> String {
        match self {
            SwayAction::Exec { .. } => {
                unreachable!("Exec has no verb-only form; sway_command() handles it directly")
            }
            SwayAction::Floating { .. } => "floating enable".to_string(),
            SwayAction::Sticky { .. } => "sticky enable".to_string(),
            SwayAction::Fullscreen { .. } => "fullscreen enable".to_string(),
            SwayAction::Focus { .. } => "focus".to_string(),
            SwayAction::Split { split, .. } => match split {
                Split::V => "splitv".to_string(),
                Split::H => "splith".to_string(),
            },
            SwayAction::Mark { mark, .. } => format!("mark {}", quote_sway_string(mark)),
            SwayAction::NewColumn { .. } => "move right".to_string(),
            SwayAction::NewRow { .. } => "move down".to_string(),
            SwayAction::Workspace { workspace, .. } => {
                format!("move workspace {}", quote_sway_string(workspace))
            }
            SwayAction::Output { output, .. } => {
                format!("move container to output {}", quote_sway_string(output))
            }
            SwayAction::Height { height, .. } => format!("resize set height {}", height),
            SwayAction::Width { width, .. } => format!("resize set width {}", width),
            SwayAction::Position { position, .. } => {
                let position_command = match position {
                    self::Position::Center => "center".to_string(),
                    self::Position::Coordinates { x, y } => format!("{} {}", x, y),
                };
                format!("move position {}", position_command)
            }
            SwayAction::Scratchpad { .. } => "move scratchpad".to_string(),
        }
    }

    fn verbose(self) -> bool {
        match self {
            SwayAction::Exec { verbose, .. }
            | SwayAction::Split { verbose, .. }
            | SwayAction::Floating { verbose, .. }
            | SwayAction::Sticky { verbose, .. }
            | SwayAction::Fullscreen { verbose, .. }
            | SwayAction::Focus { verbose, .. }
            | SwayAction::NewColumn { verbose, .. }
            | SwayAction::NewRow { verbose, .. }
            | SwayAction::Workspace { verbose, .. }
            | SwayAction::Output { verbose, .. }
            | SwayAction::Mark { verbose, .. }
            | SwayAction::Height { verbose, .. }
            | SwayAction::Width { verbose, .. }
            | SwayAction::Position { verbose, .. }
            | SwayAction::Scratchpad { verbose, .. } => verbose,
        }
    }

    /// The `--timeout` for an event-confirmed action, or the `--wait-time`
    /// for a time-based one — whichever field this variant actually has.
    fn duration(self) -> time::Duration {
        match self {
            SwayAction::Exec { timeout, .. }
            | SwayAction::Floating { timeout, .. }
            | SwayAction::Fullscreen { timeout, .. }
            | SwayAction::Focus { timeout, .. }
            | SwayAction::Workspace { timeout, .. }
            | SwayAction::Output { timeout, .. }
            | SwayAction::Mark { timeout, .. }
            | SwayAction::Scratchpad { timeout, .. } => timeout,
            SwayAction::Split { wait_time, .. }
            | SwayAction::Sticky { wait_time, .. }
            | SwayAction::NewColumn { wait_time, .. }
            | SwayAction::NewRow { wait_time, .. }
            | SwayAction::Height { wait_time, .. }
            | SwayAction::Width { wait_time, .. }
            | SwayAction::Position { wait_time, .. } => wait_time,
        }
    }

    fn container_id(self) -> Option<i64> {
        match self {
            SwayAction::Split { container_id, .. }
            | SwayAction::Floating { container_id, .. }
            | SwayAction::Sticky { container_id, .. }
            | SwayAction::Fullscreen { container_id, .. }
            | SwayAction::Focus { container_id, .. }
            | SwayAction::NewColumn { container_id, .. }
            | SwayAction::NewRow { container_id, .. }
            | SwayAction::Workspace { container_id, .. }
            | SwayAction::Output { container_id, .. }
            | SwayAction::Mark { container_id, .. }
            | SwayAction::Height { container_id, .. }
            | SwayAction::Width { container_id, .. }
            | SwayAction::Position { container_id, .. }
            | SwayAction::Scratchpad { container_id, .. } => Some(container_id),
            SwayAction::Exec { .. } => None,
        }
    }

    fn matching_window_change_events(&self) -> Option<Vec<WindowChange>> {
        match self {
            // Only `New` identifies a window as the one just launched.
            // `Move` ("the view has been reparented in the tree") fires for
            // any window whenever Sway's tree is restructured, including
            // pre-existing windows unrelated to this exec — accepting it
            // here let sway-launch return the wrong container id.
            SwayAction::Exec { .. } => Some(vec![WindowChange::New]),
            SwayAction::Floating { .. } => Some(vec![WindowChange::Floating]),
            SwayAction::Fullscreen { .. } => Some(vec![WindowChange::FullscreenMode]),
            SwayAction::Focus { .. } => Some(vec![WindowChange::Focus]),
            // Unlike NewColumn/NewRow's "move right"/"move down" below,
            // "move workspace"/"move container to output" reliably fire
            // WindowChange::Move for an actual move (tests/live_sway.rs's
            // workspace_moves_window_to_named_workspace/
            // output_moves_window_to_named_output) — but live testing also
            // showed they *do* have an "already there" no-op case after
            // all (moving to the workspace/output the window is already
            // on doesn't reparent anything, so no Move event fires either):
            // `already_at_target()` below checks for and short-circuits
            // that case before this event wait ever starts, rather than
            // hanging until --timeout for a move that was never going to
            // happen.
            SwayAction::Workspace { .. } | SwayAction::Output { .. } => {
                Some(vec![WindowChange::Move])
            }
            SwayAction::Mark { .. } => Some(vec![WindowChange::Mark]),
            // NewColumn/NewRow ("move right"/"move down") were event-based
            // via WindowChange::Move too, until live-Sway testing showed
            // "move right" doesn't fire it when the window is already at
            // the tree's rightmost position (the ordinary two-window case)
            // — it silently no-ops instead, hanging until --timeout.
            // "move down" happened to work in that same test, but relies on
            // the identical unverified assumption for a different tree
            // shape, so both moved to the same wait-time pattern as
            // Split/Height/Width/Position rather than leaving one exposed.
            //
            // On a multi-monitor setup, "move right"/"move down" on a
            // window with no sibling to move past within its own workspace
            // don't always no-op the way they do with a single output —
            // Sway's own move-direction semantics can instead escalate and
            // relocate the whole workspace to the next output in that
            // direction. `SwayLaunch::run()` guards against this (see
            // `relocates_to_another_output()`) by skipping NewColumn/
            // NewRow rather than silently moving the window to a
            // different monitor when that's the situation.
            // No WindowChange variant exists for sticky at all — confirmed
            // live by subscribing to window events while toggling
            // `sticky enable`/`sticky disable` on a live container: zero
            // events fired either way, unlike Floating/Fullscreen/Focus
            // above, which each have a dedicated WindowChange variant.
            SwayAction::Sticky { .. } => None,
            SwayAction::Split { .. }
            | SwayAction::NewColumn { .. }
            | SwayAction::NewRow { .. }
            | SwayAction::Height { .. }
            | SwayAction::Width { .. }
            | SwayAction::Position { .. } => None,
            // "move scratchpad" reparents the container into the special
            // __i3_scratch workspace, which — like Workspace/Output above —
            // fires WindowChange::Move. Confirmed live: for a previously
            // *tiled* window, Sway also auto-floats it first, firing a
            // WindowChange::Floating event before the Move — matches_window_event()
            // simply doesn't match that one (its change type isn't in this
            // list) and the loop keeps waiting for the Move that follows.
            // Re-scratchpadding an already-scratchpadded window is a no-op
            // that fires no event at all (confirmed live), so
            // already_at_target() below checks for and short-circuits that
            // case the same way it does for Workspace/Output/Floating/
            // Fullscreen/Focus.
            SwayAction::Scratchpad { .. } => Some(vec![WindowChange::Move]),
        }
    }

    fn run(&self) -> Result<i64, String> {
        if self.verbose() {
            eprintln!("Sway action: {}", self);
        }

        if let Some(container_id) = self.already_at_target()? {
            if self.verbose() {
                eprintln!(
                    "Already at target, nothing to move (container id: {})",
                    container_id
                );
            }
            return Ok(container_id);
        }

        match self {
            SwayAction::Exec { .. } => self.run_wait_matching_exec_event(),
            _ => match self.matching_window_change_events() {
                Some(_) => self.run_wait_matching_events(),
                None => self.run_wait_time(),
            },
        }
    }

    /// Checks whether the container is already where/how a given action
    /// would put it — Sway doesn't fire the event each of these actions
    /// waits on when the command turns out to be a no-op, so
    /// `run_wait_matching_events()` would otherwise hang until `--timeout`
    /// for an event that was never coming. Returns `Some(container_id)` to
    /// short-circuit `run()` with success, or `None` to proceed normally.
    ///
    /// `Workspace`/`Output` were the first two found needing this (checked
    /// via `current_workspace()`/`current_output()`). `Floating`/
    /// `Fullscreen`/`Focus` were found to have the identical failure mode —
    /// confirmed live: re-running `--floating`/`--fullscreen`/`--focus` on
    /// a window already in that state hangs 5s and then errors out, rather
    /// than completing promptly — so they're checked the same way, via
    /// `find_container_node()`'s own `floating`/`fullscreen_mode`/`focused`
    /// fields. `Mark` was checked live too and found *not* to need this:
    /// re-applying a mark the container already has still fires
    /// `WindowChange::Mark`, unlike these three. `Scratchpad` was found to
    /// need it as well — re-running `[con_id] move scratchpad` on a window
    /// already in the scratchpad fires no event at all, confirmed live —
    /// checked via `container_is_in_scratchpad()` (ancestor workspace name,
    /// not `floating`, since a scratchpad window also reports as floating
    /// for the unrelated reason `matching_window_change_events()`'s doc
    /// comment covers). Every other action falls through to `None`
    /// unconditionally, which this never touches.
    fn already_at_target(&self) -> Result<Option<i64>, String> {
        match self {
            SwayAction::Workspace {
                container_id,
                workspace,
                ..
            } => match self::current_workspace(*container_id)? {
                Some(current) if current == *workspace => Ok(Some(*container_id)),
                _ => Ok(None),
            },
            SwayAction::Output {
                container_id,
                output,
                ..
            } => match self::current_output(*container_id)? {
                Some(current) if current == *output => Ok(Some(*container_id)),
                _ => Ok(None),
            },
            SwayAction::Floating { container_id, .. } => {
                match self::find_container_node(*container_id)? {
                    Some(node) if self::node_is_floating(&node) => Ok(Some(*container_id)),
                    _ => Ok(None),
                }
            }
            SwayAction::Fullscreen { container_id, .. } => {
                match self::find_container_node(*container_id)? {
                    Some(node) if node.fullscreen_mode.is_some_and(|mode| mode != 0) => {
                        Ok(Some(*container_id))
                    }
                    _ => Ok(None),
                }
            }
            SwayAction::Focus { container_id, .. } => {
                match self::find_container_node(*container_id)? {
                    Some(node) if node.focused => Ok(Some(*container_id)),
                    _ => Ok(None),
                }
            }
            SwayAction::Scratchpad { container_id, .. } => {
                if self::container_is_in_scratchpad(*container_id)? {
                    Ok(Some(*container_id))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Whether this variant has a poll-based way to confirm its own command
    /// took effect (see docs/plan-poll-based-wait-time-actions.md) — `Some`
    /// with the current match state, or `None` if this variant has no poll
    /// matcher yet (only `Split` does today; `run_wait_time()` falls back to
    /// its original blind-sleep behavior for every other `None` variant).
    /// Errors reading the tree (transient IPC hiccup, container gone) are
    /// folded into "not confirmed yet" rather than propagated — this is only
    /// ever used to try to return *faster* than the unconditional sleep
    /// already does, never to turn success into failure.
    /// `baseline` is only used by `NewColumn`/`NewRow` (see
    /// `poll_baseline()`'s doc comment) — every other variant ignores it and
    /// checks against a fixed target derived from its own fields instead.
    fn poll_matches(&self, container_id: i64, baseline: Option<swayipc::Rect>) -> Option<bool> {
        match self {
            SwayAction::Split { split, .. } => {
                let expected = match split {
                    Split::V => NodeLayout::SplitV,
                    Split::H => NodeLayout::SplitH,
                };
                Some(self::parent_node_layout(container_id) == Some(expected))
            }
            // A `ppt` (percent) `Size` has no pixel figure to poll for
            // without also resolving the reference dimension it's a
            // percentage of, so those opt out of polling entirely (`None`)
            // rather than resolving to `Some(false)` that could never
            // become true. `Size::Pixels` values do have a formula
            // (`width_matches`/`height_matches`), so they opt in
            // unconditionally — a transient tree-read failure inside
            // `node_by_id()` folds into `Some(false)` ("not confirmed
            // yet"), not `None`, so a one-off hiccup doesn't skip the whole
            // poll grace period the way returning `None` here would.
            SwayAction::Height {
                height: Size::Pixels(pixels),
                ..
            } => Some(
                self::node_by_id(container_id)
                    .is_some_and(|node| self::height_matches(&node, *pixels as i32)),
            ),
            SwayAction::Height {
                height: Size::Percent(_),
                ..
            } => None,
            SwayAction::Width {
                width: Size::Pixels(pixels),
                ..
            } => Some(
                self::node_by_id(container_id)
                    .is_some_and(|node| self::width_matches(&node, *pixels as i32)),
            ),
            SwayAction::Width {
                width: Size::Percent(_),
                ..
            } => None,
            SwayAction::Position { position, .. } => {
                Some(self::position_matches(container_id, position))
            }
            // Unlike Floating's `floating`/node-type split (see
            // node_is_floating()'s doc comment), `sticky` is a plain `bool`
            // on `Node` with no version-dependent quirk found — confirmed
            // live that `sticky enable` sets it directly and immediately,
            // even on a still-tiled container (see the Sticky doc comment
            // on `matching_window_change_events()`'s arm for why this is a
            // wait-time action at all).
            SwayAction::Sticky { .. } => {
                Some(self::node_by_id(container_id).is_some_and(|node| node.sticky))
            }
            // No fixed target exists for "move right"/"move down" — a
            // successful move can land the window almost anywhere in the
            // tree. Instead, `poll_baseline()` snapshots the window's own
            // `rect` just before the command runs, and this compares the
            // current `rect` against it: any real move changes it (a
            // sibling swap shifts x/y, a row/column change shifts
            // width/height too), confirmed live across both an ordinary
            // sibling swap and a row-changing move that also rewrites the
            // surrounding split structure. Critically, this does *not*
            // compare parent/sibling structure directly — live testing
            // (see docs/plan-poll-based-wait-time-actions.md) found that
            // the documented "already at the edge" no-op still incidentally
            // restructures the tree around *other* siblings (Sway wraps
            // one in a new split container) even though the target window's
            // own `rect` never changes, which made an earlier
            // parent/sibling-list comparison here false-positive on
            // exactly the no-op case this whole mechanism exists to fall
            // back gracefully on.
            SwayAction::NewColumn { .. } | SwayAction::NewRow { .. } => {
                let baseline = baseline?;
                Some(self::node_by_id(container_id).is_some_and(|node| node.rect != baseline))
            }
            _ => None,
        }
    }

    /// Snapshots whatever state a wait-time action's `poll_matches()` needs
    /// to compare against *after* the command runs — captured *before* it
    /// runs. Only `NewColumn`/`NewRow` need one (their own current `rect`);
    /// every other action checks against a fixed target derived from its
    /// own fields, with no prior snapshot needed. Returns `None` if the
    /// tree can't be read, which `poll_matches()` treats as "no matcher
    /// applies" for these two variants — without a baseline there's nothing
    /// to compare against, so falling back to the original unconditional
    /// sleep is the only sound choice.
    fn poll_baseline(&self, container_id: i64) -> Option<swayipc::Rect> {
        match self {
            SwayAction::NewColumn { .. } | SwayAction::NewRow { .. } => {
                self::node_by_id(container_id).map(|node| node.rect)
            }
            _ => None,
        }
    }

    /// Polls for up to `WAIT_TIME_POLL_GRACE` for `poll_matches()` to confirm
    /// the command `run_wait_time()` just sent took effect, returning as
    /// soon as it does (the fast path). Several wait-time actions have
    /// legitimate no-op outcomes where confirmation never arrives (see
    /// docs/plan-poll-based-wait-time-actions.md) — telling that apart from
    /// "hasn't happened yet" is impossible from tree state alone, so once
    /// the grace period elapses this falls back to today's original
    /// behavior: assume success and sleep out the rest of `wait_time`,
    /// mirroring `run_wait_matching_exec_event()`'s
    /// `PID_MARKER_FALLBACK_GRACE` fallback.
    fn run_poll_then_fallback(
        &self,
        container_id: i64,
        wait_time: time::Duration,
        baseline: Option<swayipc::Rect>,
    ) -> i64 {
        // Capped at `wait_time`: `WAIT_TIME_POLL_GRACE` (200ms) is only a
        // sound upper bound when `--wait-time` is at least that — at the
        // CLI's own 20ms default, polling for the full 200ms before
        // falling back would make the fallback path (a legitimate, common
        // case — a solo-window resize clamp, a tiled NewColumn/NewRow
        // already at the edge) take ~220ms total instead of the ~40ms it
        // cost before this feature existed, a regression this constant's
        // own original design was supposed to rule out. Confirmed live
        // this fix restores that bound: with the cap, the fallback path
        // costs `wait_time` (pre-sleep) + at most `wait_time` (grace, now
        // capped, plus whatever's left of the sleep) — back to roughly
        // `2 * wait_time`, same worst case as before polling existed.
        let grace = WAIT_TIME_POLL_GRACE.min(wait_time);
        let poll_started = time::Instant::now();
        loop {
            if self.poll_matches(container_id, baseline).unwrap_or(false) {
                if self.verbose() {
                    eprintln!("Confirmed via poll (container id: {})", container_id);
                }
                return container_id;
            }
            if poll_started.elapsed() >= grace {
                break;
            }
            thread::sleep(WAIT_TIME_POLL_INTERVAL);
        }

        if self.verbose() {
            eprintln!(
                "Poll grace period elapsed without confirmation, falling back to wait-time \
                 (container id: {})",
                container_id
            );
        }
        thread::sleep(wait_time.saturating_sub(poll_started.elapsed()));
        container_id
    }

    fn run_wait_time(&self) -> Result<i64, String> {
        let wait_time = self.duration();

        if self.verbose() {
            eprintln!(
                "No matching event types for action. Will run Sway command and wait {} ms.",
                wait_time.as_millis()
            );
        }

        // Wait before and after running the Sway command: before, to let
        // other running IPC clients finish their own commands; after, to
        // let this command finish before the next action runs.
        thread::sleep(wait_time);

        let container_id = self
            .container_id()
            .expect("run_wait_time() is only ever called for variants other than Exec");

        // On Sway 1.9 (still what `apt` installs on Ubuntu 24.04/CI — see
        // node_is_floating()'s doc comment for the same version split), a
        // wait-time action's [con_id=N] criteria matching zero containers is
        // "success" as far as Sway's concerned (there's simply nothing to
        // apply the command to) — unlike an event-confirmed action, which
        // would visibly hang until --timeout instead. Without this check, a
        // container that closed between an earlier action resolving it and
        // this one running would silently no-op rather than error on 1.9.
        // Sway 1.11 already errors clearly ("No matching node.") on its own,
        // confirmed live, making this check redundant there — it's kept for
        // 1.9 compatibility, not because it's still needed on every version.
        if !self::container_exists(container_id)? {
            return Err(format!(
                "container id {} no longer exists — window may have closed",
                container_id
            ));
        }

        // Captured before the command runs, not after — see
        // poll_baseline()'s doc comment for why NewColumn/NewRow need a
        // "before" snapshot while every other poll-matched action doesn't.
        let baseline = self.poll_baseline(container_id);

        let sway_command = self.sway_command();
        if self.verbose() {
            eprintln!("Sway command: {}", sway_command);
        }

        run_sway_command(&sway_command)?;

        if self.poll_matches(container_id, baseline).is_some() {
            return Ok(self.run_poll_then_fallback(container_id, wait_time, baseline));
        }

        thread::sleep(wait_time);

        Ok(container_id)
    }

    /// `Exec`-only variant of `run_wait_matching_events()`: matching purely
    /// on event content (app_id/class, or nothing at all with no filter) is
    /// ambiguous when more than one qualifying `New` window can appear
    /// around the same time — a concurrently-running second `sway-launch`
    /// process, or any other coincidentally-timed window — since Sway
    /// broadcasts window events to every IPC connection. To disambiguate,
    /// the launched command's environment is tagged with a random,
    /// per-invocation marker (`env <PID_MARKER_VAR>=<token> <command>`,
    /// prepended without otherwise touching the user's command), and a
    /// content-matching event is only accepted outright once
    /// `/proc/<event pid>/environ` confirms that marker.
    ///
    /// A content-matching event whose marker doesn't confirm isn't rejected
    /// outright, though: some applications (browsers, editors) are
    /// single-instance and forward a second invocation's request to an
    /// already-running process before exiting, so the window that
    /// eventually appears is legitimately the right one, owned by a PID
    /// that was never given our marker. The first such event is kept as a
    /// fallback candidate, used once either `any_process_has_env_var()`
    /// shows the marked process (or a marked descendant) is no longer
    /// running — nothing marker-confirmed is coming — or
    /// `PID_MARKER_FALLBACK_GRACE` elapses, whichever comes first, bounding
    /// how long a genuinely ambiguous case can add to the wait.
    fn run_wait_matching_exec_event(&self) -> Result<i64, String> {
        let SwayAction::Exec {
            command,
            verbose,
            timeout,
            ..
        } = *self
        else {
            unreachable!("run_wait_matching_exec_event is only called for SwayAction::Exec");
        };

        let event_loop = self::event_loop(&[EventType::Window])?;

        let token = self::generate_pid_marker_token();
        let sway_command = format!("exec env {}={} {}", PID_MARKER_VAR, token, command);
        if verbose {
            eprintln!("Sway command: {}", sway_command);
        }
        run_sway_command(&sway_command)?;

        let (event_sender, event_receiver) = mpsc::channel();
        thread::spawn(move || {
            for event in event_loop {
                if event_sender.send(event).is_err() {
                    break;
                }
            }
        });

        let deadline = match time::Instant::now().checked_add(timeout) {
            Some(deadline) => deadline,
            None => return Err(format!("{} sec timeout reached", timeout.as_secs())),
        };

        // The first content-matching-but-unconfirmed event seen, and when —
        // used both to cap how long the grace period below can run and as
        // the value returned once it's used.
        let mut fallback: Option<(i64, time::Instant)> = None;

        // Once any_process_has_env_var() observes the marked process (or a
        // marked descendant) is gone, it can't come back for this token —
        // cached so a burst of further content-matching events before the
        // fallback is actually used doesn't re-scan all of /proc on each one.
        let mut marked_process_confirmed_gone = false;

        loop {
            let effective_deadline = match fallback {
                Some((_, first_seen)) => first_seen
                    .checked_add(PID_MARKER_FALLBACK_GRACE)
                    .map_or(deadline, |capped| deadline.min(capped)),
                None => deadline,
            };
            let remaining = effective_deadline.saturating_duration_since(time::Instant::now());
            if remaining.is_zero() {
                if let Some((container_id, _)) = fallback {
                    if verbose {
                        eprintln!(
                            "No PID-marker-confirmed match arrived; using earlier \
                             content-matched container id {}",
                            container_id
                        );
                    }
                    return Ok(container_id);
                }
                return Err(format!("{} sec timeout reached", timeout.as_secs()));
            }

            let event = match event_receiver.recv_timeout(remaining) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Event stream closed unexpectedly".to_string());
                }
            };

            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            let window = match event {
                Event::Window(window) => window,
                _ => continue,
            };

            if self.matches_window_event(&window).is_err() {
                continue;
            }

            let pid_confirmed = window
                .container
                .pid
                .is_some_and(|pid| self::process_has_env_var(pid, PID_MARKER_VAR, &token));

            if pid_confirmed {
                if verbose {
                    eprintln!(
                        "Event match: {:?} container id {} (PID-marker-confirmed)",
                        window.change, window.container.id
                    );
                }
                return Ok(window.container.id);
            }

            if fallback.is_none() {
                if verbose {
                    eprintln!(
                        "Event content-matches but PID marker unconfirmed (container id {}) \
                         — keeping as a fallback candidate",
                        window.container.id
                    );
                }
                fallback = Some((window.container.id, time::Instant::now()));
            }

            marked_process_confirmed_gone = marked_process_confirmed_gone
                || !self::any_process_has_env_var(PID_MARKER_VAR, &token);
            if marked_process_confirmed_gone {
                let (container_id, _) = fallback.expect("just set above if it wasn't already");
                if verbose {
                    eprintln!(
                        "Marked process no longer running; using fallback container id {}",
                        container_id
                    );
                }
                return Ok(container_id);
            }
        }
    }

    fn run_wait_matching_events(&self) -> Result<i64, String> {
        let event_loop = self::event_loop(&[EventType::Window])?;

        let sway_command = self.sway_command();
        if self.verbose() {
            eprintln!("Sway command: {}", sway_command);
        }
        run_sway_command(&sway_command)?;

        // Read events on a separate thread and forward them through a
        // channel, so recv_timeout() below enforces a real deadline even if
        // the event stream itself never produces another event (a blocking
        // iterator has no way to time out on its own). The thread may
        // outlive this function if it's still blocked on the socket when we
        // return — harmless, since sway-launch is a short-lived process and
        // the thread dies with it.
        let (event_sender, event_receiver) = mpsc::channel();
        thread::spawn(move || {
            for event in event_loop {
                if event_sender.send(event).is_err() {
                    break;
                }
            }
        });

        // checked_add rather than a plain `+`: an unrepresentable deadline
        // (a pathological --timeout) is treated as already expired instead
        // of panicking.
        let deadline = match time::Instant::now().checked_add(self.duration()) {
            Some(deadline) => deadline,
            None => return Err(format!("{} sec timeout reached", self.duration().as_secs())),
        };
        loop {
            let remaining = deadline.saturating_duration_since(time::Instant::now());
            if remaining.is_zero() {
                return Err(format!("{} sec timeout reached", self.duration().as_secs()));
            }

            let event = match event_receiver.recv_timeout(remaining) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!("{} sec timeout reached", self.duration().as_secs()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Event stream closed unexpectedly".to_string());
                }
            };

            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            let window = match event {
                Event::Window(window) => window,
                _ => continue,
            };

            match self.matches_window_event(&window) {
                Ok(result) => {
                    if self.verbose() {
                        eprintln!(
                            "Event match: {:?} container id {} ({})",
                            window.change, window.container.id, result
                        );
                    }

                    return Ok(window.container.id);
                }
                Err(error_result) => {
                    if self.verbose() {
                        eprintln!(
                            "Event mismatch: {:?} container id {} ({})",
                            window.change, window.container.id, error_result
                        );
                    }
                }
            }
        }
    }

    fn matches_window_event(
        &self,
        window: &WindowEvent,
    ) -> Result<WindowEventMatch, WindowEventMatchError> {
        let matching_window_change_events = self.matching_window_change_events().expect(
            "matches_window_event() is only ever called for variants with a matching event type",
        );

        if !matching_window_change_events.contains(&window.change) {
            return Err(WindowEventMatchError::EventChangeTypeMismatch);
        }

        match self {
            SwayAction::Exec {
                app_id_match,
                class_match,
                ..
            } => {
                if !app_id_match.is_empty() {
                    match window_app_id_match(&window.container, app_id_match) {
                        true => return Ok(WindowEventMatch::WindowAppId),
                        false => return Err(WindowEventMatchError::WindowAppIdMismatch),
                    }
                }

                if !class_match.is_empty() {
                    match window_class_match(&window.container, class_match) {
                        true => return Ok(WindowEventMatch::WindowClass),
                        false => return Err(WindowEventMatchError::WindowClassMismatch),
                    }
                }

                return Ok(WindowEventMatch::NewWindowMatchWithoutCheck);
            }
            _ => {
                if self
                    .container_id()
                    .expect("only Exec lacks a container_id, and it's handled in the arm above")
                    == window.container.id
                {
                    return Ok(WindowEventMatch::WindowContainerIdMatch);
                }
            }
        }

        Err(WindowEventMatchError::NoMatchingEvent)
    }
}

fn new_connection() -> Result<Connection, String> {
    match Connection::new() {
        Ok(connection) => Ok(connection),
        Err(error) => Err(error.to_string()),
    }
}

fn event_loop(subscriptions: &[EventType]) -> Result<EventStream, String> {
    match self::new_connection()?.subscribe(subscriptions) {
        Ok(event_iterator) => Ok(event_iterator),
        Err(error) => Err(error.to_string()),
    }
}

fn run_sway_command(command: &str) -> Result<(), String> {
    let outcomes = match self::new_connection()?.run_command(command) {
        Ok(outcomes) => outcomes,
        Err(error) => return Err(error.to_string()),
    };

    first_outcome_error(outcomes, command)
}

/// Kills `container_id` via `[con_id] kill`. Used by `main.rs`'s
/// `--rollback-on-error`: best-effort cleanup of a window this invocation
/// itself launched earlier in the same `--layout`/`--template` run, once a
/// later step fails.
pub fn kill_container(container_id: i64) -> Result<(), String> {
    run_sway_command(&format!("[con_id={}] kill", container_id))
}

/// Sway splits a command string into multiple sub-commands on unquoted
/// `,`/`;`, so `run_command()` can return more than one outcome for a single
/// call. Report the first failure found among all of them, rather than only
/// the first outcome — an early success must not hide a later failure.
fn first_outcome_error<E: fmt::Display>(
    outcomes: Vec<Result<(), E>>,
    command: &str,
) -> Result<(), String> {
    // Every SwayAction::sway_command() builds a non-empty string, and
    // swayipc always returns at least one outcome for one, so this branch
    // isn't known to be reachable in practice — it's defensive against a
    // theoretical empty reply rather than a case this crate can construct
    // a test for without mocking swayipc.
    if outcomes.is_empty() {
        return Err(format!("{} command failed", command));
    }

    for outcome in outcomes {
        if let Err(error) = outcome {
            return Err(error.to_string());
        }
    }

    Ok(())
}

/// A random-enough per-invocation token for `PID_MARKER_VAR`: this process's
/// own pid (unique across concurrently-running `sway-launch` invocations,
/// which is all that actually matters here) plus a nanosecond timestamp (so
/// a single process running several `Exec` actions in sequence — e.g. a
/// multi-step `--layout` — doesn't reuse the same token for each).
fn generate_pid_marker_token() -> String {
    let nanos = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", std::process::id(), nanos)
}

/// Whether `/proc/<pid>/environ` contains exactly `<var_name>=<expected_value>`
/// as one of its NUL-separated entries. Returns `false` for any I/O error
/// (pid already gone, no permission, `/proc` unavailable) rather than
/// erroring — this is a best-effort correlation signal for
/// `run_wait_matching_exec_event()`, never a hard requirement.
fn process_has_env_var(pid: i32, var_name: &str, expected_value: &str) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{}/environ", pid)) else {
        return false;
    };
    let needle = format!("{}={}", var_name, expected_value);
    environ
        .split(|&byte| byte == 0)
        .any(|entry| entry == needle.as_bytes())
}

/// Whether any currently-running process still carries `<var_name>=<expected_value>`
/// in its environment. Used by `run_wait_matching_exec_event()` to tell
/// whether the command it spawned (or a descendant that inherited the
/// marker) might still be about to create the matching window, versus
/// having already exited — e.g. a single-instance application that forwards
/// a request to an already-running instance and exits immediately, with no
/// further marker-confirmed match ever coming.
fn any_process_has_env_var(var_name: &str, expected_value: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .any(|pid| self::process_has_env_var(pid, var_name, expected_value))
}

/// Quotes a value for safe interpolation into a Sway IPC command string.
/// Sway's command parser splits on `,`/`;` and whitespace outside quotes, so
/// an unquoted value containing one of those could inject additional
/// commands; wrapping it in escaped double quotes forces it to be read back
/// as a single literal argument.
fn quote_sway_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Validates a `--height`/`--width` value, or a `LayoutStep`'s `height`/
/// `width` field — both take the same `\d+(px|ppt)` format. Also confirms
/// the digits actually fit in a `u32`, not just that the regex's unbounded
/// `\d+` matched — `parse_size()` trusts a value that passed this check to
/// parse infallibly, so a value that matches the shape but overflows (e.g.
/// 11+ digits) must be rejected here, not discovered as a panic later.
pub fn validate_size_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^(\d+)(?:px|ppt)$").unwrap();
    match re.captures(value) {
        Some(captures) if captures[1].parse::<u32>().is_ok() => Ok(value.to_string()),
        _ => Err("Must be in format <HEIGHT>px|ppt. E.g. 300px/20ppt. ppt = percent".to_string()),
    }
}

/// Validates a `--position` value, or a `LayoutStep`'s `position` field —
/// both take the same `center`/`<x>,<y>` format. `<x>`/`<y>` each allow a
/// leading `-`: Sway's coordinate space is global across every output, and
/// an output positioned left of or above the primary one legitimately has a
/// negative origin (confirmed live: `compute_center_position()`, used for
/// `--position center`, already accounts for output origin and can itself
/// land on a negative coordinate on such a layout) — rejecting a
/// user-supplied negative coordinate here would make the tool unable to
/// target a position its own `center` computation can already produce.
/// Also confirms `<x>`/`<y>` each actually fit in an `i32`, not just that
/// the regex's unbounded `\d+` matched — `parse_position()` trusts a value
/// that passed this check to parse infallibly, so a value that matches the
/// shape but overflows must be rejected here, not discovered as a panic
/// later (same reasoning as `validate_size_argument`'s `u32` check).
pub fn validate_position_argument(value: &str) -> Result<String, String> {
    if value == "center" {
        return Ok(value.to_string());
    }
    let re = Regex::new(r"^(-?\d+),(-?\d+)$").unwrap();
    match re.captures(value) {
        Some(captures)
            if captures[1].parse::<i32>().is_ok() && captures[2].parse::<i32>().is_ok() =>
        {
            Ok(value.to_string())
        }
        _ => Err(
            "Must be \"center\" or \"<X>,<Y>\" in pixels (X/Y may be negative). E.g. \
             center/100,200/-100,200"
                .to_string(),
        ),
    }
}

fn window_app_id_match(node: &Node, app_id_match: &str) -> bool {
    let node_app_id = match node.app_id.as_ref().ok_or(()) {
        Ok(app_id) => app_id,
        Err(_) => return false,
    };

    matches!(node_app_id, _ if node_app_id == app_id_match)
}

fn window_class_match(node: &Node, class_match: &str) -> bool {
    let window_properties = match node.window_properties.as_ref().ok_or(()) {
        Ok(window_properties) => window_properties,
        Err(_) => return false,
    };

    let node_class = match window_properties.class.as_ref().ok_or(()) {
        Ok(class) => class,
        Err(_) => return false,
    };

    matches!(node_class, _ if node_class == class_match)
}

/// Recursively collects the container ids of every node in `tree` (tiling
/// and floating children at every level) whose app_id/class matches, used to
/// target an already-open window instead of launching a new one. app_id
/// takes priority over class when both are set, mirroring
/// `matches_window_event`'s Exec-matching precedence.
fn matching_container_ids(tree: &Node, app_id_match: &str, class_match: &str) -> Vec<i64> {
    let matches = if !app_id_match.is_empty() {
        window_app_id_match(tree, app_id_match)
    } else if !class_match.is_empty() {
        window_class_match(tree, class_match)
    } else {
        false
    };

    let mut ids = if matches { vec![tree.id] } else { vec![] };

    for child in tree.nodes.iter().chain(tree.floating_nodes.iter()) {
        ids.extend(matching_container_ids(child, app_id_match, class_match));
    }

    ids
}

/// Finds exactly one already-open window matching `app_id_match`/
/// `class_match` via `get_tree()`, for `Target::Existing`. Errors — rather
/// than silently picking one — if zero or more than one window matches,
/// since guessing which of several matches the caller meant would be a
/// worse default than asking them to retarget with `--con-id`.
fn find_existing_container_id(app_id_match: &str, class_match: &str) -> Result<i64, String> {
    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };

    let criteria = if !app_id_match.is_empty() {
        format!("app_id \"{}\"", app_id_match)
    } else {
        format!("class \"{}\"", class_match)
    };

    resolve_matches(
        matching_container_ids(&tree, app_id_match, class_match),
        &criteria,
    )
}

/// Turns the container ids `matching_container_ids()` found into a single
/// target, erroring — rather than silently picking one — on zero or more
/// than one match.
fn resolve_matches(matches: Vec<i64>, criteria: &str) -> Result<i64, String> {
    match matches.len() {
        0 => Err(format!("No existing window matches {}", criteria)),
        1 => Ok(matches[0]),
        _ => {
            let ids = matches
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "{} windows match {}: {} — retarget with --con-id",
                matches.len(),
                criteria,
                ids
            ))
        }
    }
}

/// The name of the workspace currently containing `con_id`, or `None` if
/// `con_id` isn't found in the tree at all. Used by
/// `SwayAction::already_at_target()` to detect a no-op `--workspace` move
/// before waiting on an event Sway won't fire for it.
fn current_workspace(con_id: i64) -> Result<Option<String>, String> {
    self::containing_node_name(con_id, NodeType::Workspace)
}

/// The name of the output currently containing `con_id`, or `None` if
/// `con_id` isn't found in the tree at all. Used by
/// `SwayAction::already_at_target()` to detect a no-op `--output` move
/// before waiting on an event Sway won't fire for it.
fn current_output(con_id: i64) -> Result<Option<String>, String> {
    self::containing_node_name(con_id, NodeType::Output)
}

fn containing_node_name(con_id: i64, kind: NodeType) -> Result<Option<String>, String> {
    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };

    Ok(self::find_containing_name(&tree, con_id, kind, None))
}

/// `container_id`'s own tree node, or `None` if it isn't found. Unlike
/// `node_by_id()` (used by the poll-then-fallback machinery, where a
/// transient IPC failure is deliberately swallowed into "not confirmed
/// yet"), this propagates a genuine connection/`get_tree()` failure as an
/// error — matching `containing_node_name()`'s contract, since both are
/// used by `SwayAction::already_at_target()` to check state *before*
/// deciding whether to act, where silently treating a real IPC failure as
/// "not already there" would let a later step fail with a confusing
/// timeout instead of surfacing the actual problem immediately.
fn find_container_node(container_id: i64) -> Result<Option<Node>, String> {
    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };
    Ok(self::find_node(&tree, container_id).cloned())
}

/// Whether `node` is currently floating. Sway 1.9 (still what `apt` installs
/// on Ubuntu 24.04, confirmed live against a headless compositor during a
/// CI-failure investigation) never populates a floating container's own
/// `floating` field — it stays `null` even though the container's `type` is
/// correctly `floating_con` — while Sway 1.11 populates both. Checking
/// `node_type` alone therefore covers both versions; the `floating` field is
/// checked too only so a version that reverses this (populates `floating`
/// but not `node_type`, unconfirmed but not ruled out) still works.
fn node_is_floating(node: &Node) -> bool {
    node.node_type == NodeType::FloatingCon
        || matches!(
            node.floating,
            Some(swayipc::Floating::UserOn) | Some(swayipc::Floating::AutoOn)
        )
}

/// Whether `node` is currently in the scratchpad, per its own
/// `scratchpad_state` field. `Some(ScratchpadState::None)` means an ordinary
/// window (present but "no scratchpad state", not the same as the field
/// being absent/`None` outright — both fold into "not in the scratchpad"
/// here) and `Some(ScratchpadState::Fresh)`/`Some(ScratchpadState::Changed)`
/// means one that's actually been moved there, confirmed live. Deliberately
/// not `node_is_floating()`: a window Sway auto-floats as part of moving it
/// to the scratchpad (see `SwayAction::matching_window_change_events()`'s
/// doc comment on `Scratchpad`) would otherwise be misreported as already in
/// the scratchpad the moment it's floating, before it actually is.
///
/// A CI-failure investigation found this field alone isn't reliable enough
/// to gate `already_at_target()` on, though: Sway 1.9 (still what `apt`
/// installs on Ubuntu 24.04/CI) leaves `scratchpad_state` at
/// `Some(ScratchpadState::None)` even for a container genuinely in the
/// scratchpad — the same kind of version-dependent gap `node_is_floating()`'s
/// doc comment documents for the `floating` field, just not caught locally
/// beforehand since this project's own dev/CI environments so far have only
/// ever run Sway 1.11, where the field *is* populated correctly. Kept as a
/// secondary, OR'd check in `container_is_in_scratchpad()` below, alongside
/// the version-independent ancestor-workspace-name check that function uses
/// as its primary signal.
fn node_is_in_scratchpad(node: &Node) -> bool {
    !matches!(
        node.scratchpad_state,
        None | Some(swayipc::ScratchpadState::None)
    )
}

/// Whether `container_id` is currently in Sway's scratchpad, given an
/// already-fetched `tree`. Checks the container's ancestor workspace name
/// first: the scratchpad is always the fixed internal workspace Sway names
/// `__i3_scratch`, confirmed live to be populated reliably on both Sway 1.9
/// and 1.11 — unlike `node_is_in_scratchpad()`'s own `scratchpad_state`
/// field check (see that function's doc comment), which is kept here only
/// as a secondary, redundant signal.
fn tree_shows_container_in_scratchpad(tree: &Node, container_id: i64) -> bool {
    let in_scratchpad_workspace =
        self::find_containing_name(tree, container_id, NodeType::Workspace, None).as_deref()
            == Some("__i3_scratch");
    let node_flagged = self::find_node(tree, container_id).is_some_and(self::node_is_in_scratchpad);
    in_scratchpad_workspace || node_flagged
}

/// `tree_shows_container_in_scratchpad()`, fetching the tree itself. Used by
/// `SwayAction::already_at_target()`'s `Scratchpad` arm.
fn container_is_in_scratchpad(container_id: i64) -> Result<bool, String> {
    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };
    Ok(self::tree_shows_container_in_scratchpad(
        &tree,
        container_id,
    ))
}

/// Recursively walks `node` tracking the name of the nearest ancestor whose
/// `node_type` matches `kind`, returning that name once `con_id` is found —
/// e.g. with `kind: NodeType::Workspace`, the name of the workspace
/// containing `con_id`.
fn find_containing_name(
    node: &Node,
    con_id: i64,
    kind: NodeType,
    current: Option<&str>,
) -> Option<String> {
    let current = if node.node_type == kind {
        node.name.as_deref()
    } else {
        current
    };
    if node.id == con_id {
        return current.map(String::from);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| self::find_containing_name(child, con_id, kind, current))
}

/// The direction `NewColumn` ("move right") / `NewRow` ("move down") moves
/// in — used only by `relocates_to_another_output()` to know which of the
/// workspace's own axes/layout to check.
#[derive(Copy, Clone)]
enum MoveDirection {
    Right,
    Down,
}

/// Whether running NewColumn/NewRow on `container_id` right now would risk
/// Sway relocating it (and its whole workspace) to a different output
/// rather than moving it within the workspace or no-oping.
///
/// This originally checked only "is `container_id` the only window in its
/// workspace" — live testing during this feature's development found that
/// too narrow: a *non-solo* workspace can escalate too, whenever
/// `container_id` is already the trailing child of a workspace whose own
/// layout already matches the move's axis (confirmed live: two windows
/// side by side, `[con_id=<rightmost>] move right` relocated it to another
/// output, not a same-workspace no-op, even with a sibling to its left).
/// Conversely, a solo window whose workspace layout *doesn't* match the
/// axis (e.g. stacked vertically via `splitv`, then moved right) was
/// confirmed live to restructure in place rather than escalate — so
/// checking layout, not just child count, also avoids skipping a move that
/// would actually have been safe. The current check: `container_id` is a
/// *direct* child of its workspace (not nested in a sub-container), the
/// workspace's own `layout` matches the axis (`SplitH` for `NewColumn`,
/// `SplitV` for `NewRow`), and `container_id` is the last child in that
/// list — this subsumes the original solo-window case (trivially both
/// direct- and last-child of its workspace) while also catching the
/// multi-window case that check alone missed. A window nested inside a
/// sub-container is conservatively never flagged. Confirmed live in both an
/// axis-mismatched nesting (a `splitv` sub-container under a `splith`
/// workspace) and the axis-matched worst case (a `splith` sub-container
/// under a `splith` workspace, target as its trailing child) that this
/// conservatism costs nothing: `move right` on the nested target never
/// escalated to a different output either way, it simply popped the target
/// out to become a new direct child of the workspace — see
/// `tests/live_sway.rs`'s
/// `new_column_does_not_relocate_a_nested_window_to_a_different_output`.
/// Returns `false` (safe to proceed) if outputs/tree can't be read or
/// `container_id`/its workspace can't be found, rather than blocking the
/// action on an inconclusive check.
fn relocates_to_another_output(
    container_id: i64,
    direction: MoveDirection,
) -> Result<bool, String> {
    let outputs = match self::new_connection()?.get_outputs() {
        Ok(outputs) => outputs,
        Err(error) => return Err(error.to_string()),
    };
    if outputs.len() < 2 {
        return Ok(false);
    }

    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };

    Ok(self::is_at_the_trailing_workspace_edge(
        &tree,
        container_id,
        direction,
    ))
}

fn is_at_the_trailing_workspace_edge(
    tree: &Node,
    container_id: i64,
    direction: MoveDirection,
) -> bool {
    let Some(workspace) = self::find_workspace_node(tree, container_id) else {
        return false;
    };
    let expected_layout = match direction {
        MoveDirection::Right => NodeLayout::SplitH,
        MoveDirection::Down => NodeLayout::SplitV,
    };
    if workspace.layout != expected_layout {
        return false;
    }
    workspace
        .nodes
        .last()
        .is_some_and(|last| last.id == container_id)
}

fn find_workspace_node(node: &Node, container_id: i64) -> Option<&Node> {
    if node.node_type == NodeType::Workspace {
        return if self::contains_id(node, container_id) {
            Some(node)
        } else {
            None
        };
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| self::find_workspace_node(child, container_id))
}

/// Whether `container_id` is still present anywhere in the current tree —
/// used by `run_wait_time()` to catch a container that closed between an
/// earlier action resolving it and this one about to run its command
/// against it. Needed on Sway 1.9 (still what `apt` installs on Ubuntu
/// 24.04/CI), which treats a `[con_id=N]` criteria matching zero containers
/// as success rather than an error; Sway 1.11 already errors clearly
/// ("No matching node.") on its own, confirmed live, which makes this check
/// redundant there but still required for 1.9 — see `node_is_floating()`'s
/// doc comment for the same version split.
fn container_exists(container_id: i64) -> Result<bool, String> {
    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };
    Ok(self::contains_id(&tree, container_id))
}

fn contains_id(node: &Node, container_id: i64) -> bool {
    node.id == container_id
        || node
            .nodes
            .iter()
            .chain(node.floating_nodes.iter())
            .any(|child| self::contains_id(child, container_id))
}

/// The `layout` field of `container_id`'s *parent* node, or `None` if the
/// container/tree can't be read, or `container_id` has no parent in the
/// tree (e.g. it's the root). `container_id`'s own node never carries its
/// own split direction — confirmed against a live Sway compositor,
/// splitting a window with siblings wraps it in a new split container
/// (whose `layout` is the requested direction) one level up, and splitting
/// a solo window instead sets the `layout` of the workspace it's already
/// the sole child of; the leaf window node's own `layout` field is always
/// `None`/unset either way. Used by `SwayAction::poll_matches()` to confirm
/// a `Split` action actually applied before its `run_poll_then_fallback()`
/// grace period falls back to sleeping the rest of `--wait-time`.
fn parent_node_layout(container_id: i64) -> Option<NodeLayout> {
    let tree = self::new_connection().ok()?.get_tree().ok()?;
    self::find_parent_layout(&tree, container_id)
}

fn find_parent_layout(node: &Node, container_id: i64) -> Option<NodeLayout> {
    if node
        .nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .any(|child| child.id == container_id)
    {
        return Some(node.layout);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| self::find_parent_layout(child, container_id))
}

/// The tree node with id `container_id`, or `None` if it can't be read
/// (transient IPC error, or the container's gone) — used by
/// `SwayAction::poll_matches()`'s `Height`/`Width`/`Position` arms to read
/// a window's own current geometry, as opposed to `parent_node_layout()`,
/// which reads its *parent's* state for `Split`.
fn node_by_id(container_id: i64) -> Option<Node> {
    let tree = self::new_connection().ok()?.get_tree().ok()?;
    self::find_node(&tree, container_id).cloned()
}

fn find_node(node: &Node, container_id: i64) -> Option<&Node> {
    if node.id == container_id {
        return Some(node);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| self::find_node(child, container_id))
}

/// Whether `node`'s current width matches a `resize set width <expected_px>px`
/// command. Confirmed against a live Sway compositor that this needs two
/// candidate formulas, not one fixed offset: a window resized while it's
/// been floating since the very command that floated it matches
/// `rect.width` exactly, but a window that had already been tiled for a
/// while before being floated and resized comes out `2 *
/// current_border_width` short on `rect.width` alone (see
/// `retarget_by_id_layout_floats_the_first_step_by_name` in
/// `tests/live_sway.rs`, and this project's own prior investigation notes
/// in `docs/plan-poll-based-wait-time-actions.md`) — border accounting
/// this project hasn't found a single deterministic rule for, so both
/// candidates are accepted rather than picking one and risking a resize
/// that's actually done never being recognized as such.
///
/// Accepted residual risk: `run_poll_then_fallback()`'s very first poll
/// runs immediately after the command is sent, with no minimum settle
/// time, so if the window's *pre-resize* width already happens to satisfy
/// either formula for the *newly requested* width (the old and new widths
/// would need to coincide, accounting for a possible border offset), this
/// would report "confirmed" on that first poll before Sway has actually
/// processed the new command — indistinguishable from a legitimate
/// already-there match (the same case `Split`'s idempotent re-application
/// relies on confirming instantly). Judged low-risk enough not to warrant
/// the added latency/complexity of requiring a genuine change (like
/// `NewColumn`/`NewRow`'s `poll_baseline()` snapshot) purely for this;
/// revisit if it ever proves problematic in practice.
fn width_matches(node: &Node, expected_px: i32) -> bool {
    node.rect.width == expected_px || node.rect.width + 2 * node.current_border_width == expected_px
}

/// Whether `node`'s current height matches a `resize set height
/// <expected_px>px` command. Unlike `width_matches()`, live testing found
/// only one formula for height across both the freshly-floating and
/// tiled cases: the decoration (title bar) is always excluded from
/// `rect.height`, so the outer, decoration-inclusive height is
/// `rect.height + deco_rect.height`.
fn height_matches(node: &Node, expected_px: i32) -> bool {
    node.rect.height + node.deco_rect.height == expected_px
}

/// Whether `container_id`'s window is currently positioned where `position`
/// (`"center"` or `"<x>,<y>"`) requests. Never propagates a `None`/error —
/// any failure to read the tree/outputs, or to resolve an expected
/// position at all (e.g. `container_id` not found, or not on a known
/// output), folds into `false` ("not confirmed yet"), consistent with
/// `SwayAction::poll_matches()`'s other arms.
///
/// Confirmed live that a fullscreen window's `deco_rect` stays `{0, 0, 0,
/// 0}` permanently (not a transient race — held stable across a multi-second
/// sweep), since Sway never computes decoration geometry for a window with
/// no border/titlebar to draw. Comparing only `deco_rect` would therefore
/// mean `--position` against a fullscreen container (directly, or via
/// `--floating --fullscreen --position` in one invocation) can never be
/// confirmed by polling — `move position` actually succeeds immediately
/// (confirmed live via `rect.x`/`rect.y` landing on the requested target),
/// but every invocation would still burn the full poll grace period before
/// falling back to sleeping `--wait-time`. Falling back to `rect.x`/`rect.y`
/// when `deco_rect` is unset closes that gap, mirroring `width_matches()`'s
/// existing dual-formula tolerance for a different Sway geometry quirk.
fn position_matches(container_id: i64, position: &Position) -> bool {
    let Some((node, output_name)) = self::node_and_output_name(container_id) else {
        return false;
    };
    let Some((expected_x, expected_y)) =
        self::expected_position(position, &node, output_name.as_deref())
    else {
        return false;
    };
    if node.deco_rect.width == 0 && node.deco_rect.height == 0 {
        node.rect.x == expected_x && node.rect.y == expected_y
    } else {
        node.deco_rect.x == expected_x && node.deco_rect.y == expected_y
    }
}

/// `container_id`'s tree node together with the name of the output
/// containing it (`None` if it isn't on any output, e.g. the scratchpad),
/// read via a single `get_tree()` call — used by `position_matches()`
/// rather than combining `node_by_id()` with the existing `current_output()`
/// helper, which would cost a second, redundant tree fetch per poll
/// iteration.
fn node_and_output_name(container_id: i64) -> Option<(Node, Option<String>)> {
    let tree = self::new_connection().ok()?.get_tree().ok()?;
    let node = self::find_node(&tree, container_id)?.clone();
    let output_name = self::find_containing_name(&tree, container_id, NodeType::Output, None);
    Some((node, output_name))
}

/// The geometry of the output named `output_name`, or `None` if it can't be
/// read or no output has that name.
fn output_rect(output_name: &str) -> Option<swayipc::Rect> {
    let outputs = self::new_connection().ok()?.get_outputs().ok()?;
    outputs
        .into_iter()
        .find(|output| output.name == output_name)
        .map(|output| output.rect)
}

/// The `(x, y)` `position_matches()` expects `node` to be at, for `"center"`
/// or a validated `"<x>,<y>"` string (`validate_position_argument`'s regex
/// guarantees the latter parses cleanly by the time this runs) — matches
/// `deco_rect.x`/`deco_rect.y`, the decoration-inclusive frame `move
/// position` actually targets, confirmed live by
/// `position_moves_a_floating_window_to_given_coordinates`/
/// `position_center_centers_a_floating_window` in `tests/live_sway.rs`
/// (`deco_rect.x` and `rect.x` were confirmed equal there too, so using
/// `deco_rect.x` uniformly for both the coordinate and center cases, rather
/// than `rect.x` for one and `deco_rect.x` for the other, doesn't change
/// which value is being compared). `output_name` is only consulted for
/// `"center"`, so a window not on any output (e.g. the scratchpad) can
/// still match a plain `"<x>,<y>"` position.
fn expected_position(
    position: &Position,
    node: &Node,
    output_name: Option<&str>,
) -> Option<(i32, i32)> {
    match position {
        Position::Center => {
            let rect = self::output_rect(output_name?)?;
            Some(self::compute_center_position(
                rect,
                node.rect.width,
                node.rect.height + node.deco_rect.height,
            ))
        }
        Position::Coordinates { x, y } => Some((*x, *y)),
    }
}

/// The top-left `(x, y)` that centers a `window_width` x `window_height`
/// window (its decoration-inclusive outer footprint) within `output_rect`.
/// `output_rect`'s own `x`/`y` are added in since tree/output coordinates
/// are global, not output-relative — only visibly matters once a second
/// output exists to the left of or above this one, but correct
/// unconditionally rather than assuming this is the primary output.
fn compute_center_position(
    output_rect: swayipc::Rect,
    window_width: i32,
    window_height: i32,
) -> (i32, i32) {
    (
        output_rect.x + (output_rect.width - window_width) / 2,
        output_rect.y + (output_rect.height - window_height) / 2,
    )
}

/// What `SwayLaunch::run()` should act on: launch a new window (the
/// original, still-default behavior), a specific already-open window by
/// container id, or an already-open window found by matching
/// `app_id_match`/`class_match` against currently open windows.
pub enum Target<'a> {
    Exec { command: &'a str },
    ConId(i64),
    Existing,
}

pub struct SwayLaunch<'a> {
    pub target: Target<'a>,

    pub app_id_match: &'a str,
    pub class_match: &'a str,

    pub split: Option<Split>,
    pub floating: bool,
    pub sticky: bool,
    pub fullscreen: bool,
    pub focus: bool,
    pub mark: &'a str,
    pub new_column: bool,
    pub new_row: bool,
    pub workspace: Option<&'a str>,
    pub output: Option<&'a str>,
    pub height: Option<Size>,
    pub width: Option<Size>,
    pub position: Option<Position>,
    pub scratchpad: bool,

    pub verbose: bool,
    pub timeout: time::Duration,
    pub wait_time: time::Duration,
}

impl<'a> SwayLaunch<'a> {
    pub fn debug_events(&self) -> Result<(), String> {
        let subscriptions = [
            EventType::Workspace,
            EventType::Mode,
            EventType::Window,
            EventType::BarConfigUpdate,
            EventType::Binding,
            EventType::Shutdown,
            EventType::Tick,
            EventType::BarStateUpdate,
            EventType::Input,
        ];

        for (i, event) in self::event_loop(&subscriptions)?.enumerate() {
            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            println!("Event: {}", i);
            println!("{:?}\n", event);
        }

        Ok(())
    }

    fn resolve_container_id(&self) -> Result<i64, String> {
        match self.target {
            Target::Exec { command } => SwayAction::Exec {
                command,
                app_id_match: self.app_id_match,
                class_match: self.class_match,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run(),
            Target::ConId(container_id) => Ok(container_id),
            Target::Existing => {
                self::find_existing_container_id(self.app_id_match, self.class_match)
            }
        }
    }

    /// Builds the sequence of `SwayAction`s `run()` would apply against
    /// `container_id`, in the same fixed order `run()` always used, without
    /// running any of them — the "plan" half of the plan-then-execute split
    /// `run()` itself is now just the "execute" half of. Exists as its own
    /// method (rather than inlined into `run()`) so `--dry-run` can print
    /// the plan instead of running it.
    ///
    /// `check_relocation` controls whether `NewColumn`/`NewRow` actually
    /// call `relocates_to_another_output()` (a live `get_outputs()`/
    /// `get_tree()` read) to decide whether to skip them — `run()` passes
    /// `true`, needing the real answer since it's about to run the action
    /// for real. `--dry-run` (via `build_actions_for_preview()`) passes
    /// `false`: previewing a plan has no real `container_id` to check
    /// against yet (nothing has launched), so the check can't give a
    /// meaningful answer anyway, and skipping it entirely is what makes
    /// previewing fully IPC-free — `NewColumn`/`NewRow` are always included
    /// in a preview when the flag is set, same as every other action.
    fn build_actions(
        &self,
        container_id: i64,
        check_relocation: bool,
    ) -> Result<Vec<SwayAction<'a>>, String> {
        let mut actions = Vec::new();

        if self.new_column {
            if check_relocation
                && self::relocates_to_another_output(container_id, MoveDirection::Right)?
            {
                if self.verbose {
                    eprintln!(
                        "Skipping new-column: container id {} is at the trailing edge of a \
                         workspace already laid out along that axis, and more than one output \
                         exists — \"move right\" would relocate it to a different output \
                         instead of no-oping",
                        container_id
                    );
                }
            } else {
                actions.push(SwayAction::NewColumn {
                    container_id,
                    verbose: self.verbose,
                    wait_time: self.wait_time,
                });
            }
        }
        if self.new_row {
            if check_relocation
                && self::relocates_to_another_output(container_id, MoveDirection::Down)?
            {
                if self.verbose {
                    eprintln!(
                        "Skipping new-row: container id {} is at the trailing edge of a \
                         workspace already laid out along that axis, and more than one output \
                         exists — \"move down\" would relocate it to a different output \
                         instead of no-oping",
                        container_id
                    );
                }
            } else {
                actions.push(SwayAction::NewRow {
                    container_id,
                    verbose: self.verbose,
                    wait_time: self.wait_time,
                });
            }
        }
        if let Some(workspace) = self.workspace {
            actions.push(SwayAction::Workspace {
                container_id,
                workspace,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }
        if let Some(output) = self.output {
            actions.push(SwayAction::Output {
                container_id,
                output,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }
        if let Some(split) = self.split {
            actions.push(SwayAction::Split {
                container_id,
                split,
                verbose: self.verbose,
                wait_time: self.wait_time,
            });
        }
        if self.floating {
            actions.push(SwayAction::Floating {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }
        if self.sticky {
            actions.push(SwayAction::Sticky {
                container_id,
                verbose: self.verbose,
                wait_time: self.wait_time,
            });
        }
        if self.fullscreen {
            actions.push(SwayAction::Fullscreen {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }
        if self.focus {
            actions.push(SwayAction::Focus {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }
        if let Some(height) = self.height {
            actions.push(SwayAction::Height {
                container_id,
                height,
                verbose: self.verbose,
                wait_time: self.wait_time,
            });
        }
        if let Some(width) = self.width {
            actions.push(SwayAction::Width {
                container_id,
                width,
                verbose: self.verbose,
                wait_time: self.wait_time,
            });
        }
        if let Some(position) = self.position {
            actions.push(SwayAction::Position {
                container_id,
                position,
                verbose: self.verbose,
                wait_time: self.wait_time,
            });
        }
        if !self.mark.is_empty() {
            actions.push(SwayAction::Mark {
                container_id,
                mark: self.mark,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }
        if self.scratchpad {
            actions.push(SwayAction::Scratchpad {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            });
        }

        Ok(actions)
    }

    /// The `--dry-run` entry point: every `SwayAction` this `SwayLaunch`
    /// would apply, in order, without a real `container_id` (nothing has
    /// launched) and without touching Sway IPC at all — see
    /// `build_actions()`'s doc comment for why `check_relocation: false`
    /// is what makes that possible. Infallible (`build_actions()` only
    /// ever errors via the relocation check this skips), so callers don't
    /// need to handle a `--dry-run` preview failing the way a real `run()`
    /// can.
    pub fn build_actions_for_preview(&self) -> Vec<SwayAction<'a>> {
        self.build_actions(0, false).expect(
            "check_relocation: false means build_actions() makes no IPC call, so it can't fail",
        )
    }

    pub fn run(&self) -> Result<RunOutcome, String> {
        let container_id = self.resolve_container_id()?;

        if self.verbose {
            eprintln!("Target container id: {}", container_id);
        }

        let mut actions = Vec::new();
        for action in self.build_actions(container_id, true)? {
            actions.push(action.sway_command_verb());
            action.run()?;
        }

        Ok(RunOutcome {
            container_id,
            actions,
        })
    }
}

/// `SwayLaunch::run()`'s result: the resolved container id, plus every
/// action's verb (`SwayAction::sway_command_verb()`, the same
/// container-id-free text `--dry-run` prints) in the order it actually ran
/// — a real run's richer `--json` shape (`main.rs`) reports this alongside
/// `container_id`. Since `run()` stops at the first action that fails,
/// `actions` on a successful `Ok` is always the *complete* planned list,
/// not a partial one — there's no per-action "confirmed"/"failed" status to
/// report here, because a failed action never returns at all; it's
/// reported as `run()`'s `Err` instead, same as before this existed.
pub struct RunOutcome {
    pub container_id: i64,
    pub actions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_event(
        change: &str,
        container_id: i64,
        app_id: Option<&str>,
        class: Option<&str>,
    ) -> WindowEvent {
        let value = serde_json::json!({
            "change": change,
            "container": {
                "id": container_id,
                "type": "con",
                "border": "normal",
                "current_border_width": 0,
                "layout": "none",
                "orientation": "none",
                "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
                "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
                "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
                "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
                "urgent": false,
                "focused": false,
                "focus": [],
                "floating_nodes": [],
                "sticky": false,
                "app_id": app_id,
                "window_properties": class.map(|class| serde_json::json!({"class": class})),
            }
        });

        serde_json::from_value(value).expect("valid WindowEvent test fixture")
    }

    fn leaf_node_value(
        container_id: i64,
        app_id: Option<&str>,
        class: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": container_id,
            "type": "con",
            "border": "normal",
            "current_border_width": 0,
            "layout": "none",
            "orientation": "none",
            "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
            "urgent": false,
            "focused": false,
            "focus": [],
            "floating_nodes": [],
            "sticky": false,
            "app_id": app_id,
            "window_properties": class.map(|class| serde_json::json!({"class": class})),
        })
    }

    fn container_node_value(
        container_id: i64,
        nodes: Vec<serde_json::Value>,
        floating_nodes: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": container_id,
            "type": "con",
            "border": "normal",
            "current_border_width": 0,
            "layout": "none",
            "orientation": "none",
            "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
            "urgent": false,
            "focused": false,
            "focus": [],
            "nodes": nodes,
            "floating_nodes": floating_nodes,
            "sticky": false,
        })
    }

    fn node_tree(
        container_id: i64,
        nodes: Vec<serde_json::Value>,
        floating_nodes: Vec<serde_json::Value>,
    ) -> Node {
        serde_json::from_value(container_node_value(container_id, nodes, floating_nodes))
            .expect("valid Node test fixture")
    }

    fn workspace_node_tree(
        container_id: i64,
        name: &str,
        nodes: Vec<serde_json::Value>,
        floating_nodes: Vec<serde_json::Value>,
    ) -> Node {
        let mut value = container_node_value(container_id, nodes, floating_nodes);
        value["type"] = serde_json::json!("workspace");
        value["name"] = serde_json::json!(name);
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    // generate_pid_marker_token / process_has_env_var / any_process_has_env_var

    #[test]
    fn generate_pid_marker_token_starts_with_this_processes_id() {
        let token = generate_pid_marker_token();
        let expected_prefix = format!("{}-", std::process::id());
        assert!(
            token.starts_with(&expected_prefix),
            "token {:?} should start with {:?}",
            token,
            expected_prefix
        );
    }

    #[test]
    fn process_has_env_var_true_for_this_processes_own_environment() {
        let pid = std::process::id() as i32;
        let path = std::env::var("PATH").expect("PATH should be set in the test environment");
        assert!(process_has_env_var(pid, "PATH", &path));
    }

    #[test]
    fn process_has_env_var_false_for_wrong_value() {
        let pid = std::process::id() as i32;
        assert!(!process_has_env_var(
            pid,
            "PATH",
            "definitely-not-the-real-path-value"
        ));
    }

    #[test]
    fn process_has_env_var_false_for_nonexistent_pid() {
        assert!(!process_has_env_var(i32::MAX, "PATH", "anything"));
    }

    #[test]
    fn any_process_has_env_var_true_when_this_process_has_it() {
        let path = std::env::var("PATH").expect("PATH should be set in the test environment");
        assert!(any_process_has_env_var("PATH", &path));
    }

    #[test]
    fn any_process_has_env_var_false_for_a_value_nothing_has() {
        assert!(!any_process_has_env_var(
            "SWAY_LAUNCH_DEFINITELY_UNUSED_TEST_VAR_XYZ",
            "nope"
        ));
    }

    // quote_sway_string

    #[test]
    fn quote_sway_string_wraps_plain_value() {
        assert_eq!(quote_sway_string("foo"), "\"foo\"");
    }

    #[test]
    fn quote_sway_string_escapes_embedded_quotes() {
        assert_eq!(quote_sway_string("foo\"bar"), "\"foo\\\"bar\"");
    }

    #[test]
    fn quote_sway_string_escapes_backslashes() {
        assert_eq!(quote_sway_string("foo\\bar"), "\"foo\\\\bar\"");
    }

    #[test]
    fn quote_sway_string_wraps_a_value_containing_a_newline() {
        // Regression test: confirmed live (this project's security review,
        // 2026-08-21) that a literal newline embedded in a --mark value
        // can't break out of the quoting either -- Sway's own parser treats
        // it as part of the quoted literal, not a command separator, the
        // same as the comma/semicolon case below. quote_sway_string()
        // itself needs no special handling for `\n` (only `\`/`"` are
        // escaped) since it's neither of those; this test just pins that
        // the newline survives untouched inside the quotes rather than
        // being stripped or otherwise mishandled. See
        // mark_with_special_characters_is_stored_literally_not_executed in
        // tests/live_sway.rs for the live-Sway proof this is actually safe.
        let injected = "foo\nexec malicious-command";
        let quoted = quote_sway_string(injected);
        assert_eq!(quoted, "\"foo\nexec malicious-command\"");
    }

    #[test]
    fn quote_sway_string_neutralizes_command_separators() {
        // Regression test: an unquoted mark containing a command separator
        // used to let extra Sway commands be injected into the same call.
        let injected = "foo, exec malicious-command";
        let quoted = quote_sway_string(injected);
        assert_eq!(quoted, "\"foo, exec malicious-command\"");
        assert!(!quoted.trim_matches('"').contains('"'));
    }

    // validate_size_argument / validate_position_argument

    #[test]
    fn validate_size_argument_accepts_px() {
        assert_eq!(validate_size_argument("300px"), Ok("300px".to_string()));
    }

    #[test]
    fn validate_size_argument_accepts_ppt() {
        assert_eq!(validate_size_argument("20ppt"), Ok("20ppt".to_string()));
    }

    #[test]
    fn validate_size_argument_accepts_zero() {
        assert_eq!(validate_size_argument("0px"), Ok("0px".to_string()));
    }

    #[test]
    fn validate_size_argument_rejects_missing_unit() {
        assert!(validate_size_argument("300").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_unknown_unit() {
        assert!(validate_size_argument("300pixels").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_negative() {
        assert!(validate_size_argument("-5px").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_decimal() {
        assert!(validate_size_argument("3.5px").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_empty() {
        assert!(validate_size_argument("").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_trailing_garbage() {
        assert!(validate_size_argument("300px ").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_a_value_that_overflows_u32() {
        // Regression test: the regex's \d+ has no digit-count bound, but
        // parse_size() trusts a validated value to parse infallibly --
        // matches the shape (all digits) while overflowing u32 (max
        // 4294967295, 10 digits) must still be rejected here.
        assert!(validate_size_argument("99999999999px").is_err());
    }

    #[test]
    fn validate_position_argument_accepts_center() {
        assert_eq!(
            validate_position_argument("center"),
            Ok("center".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_coordinates() {
        assert_eq!(
            validate_position_argument("100,200"),
            Ok("100,200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_rejects_missing_y() {
        assert!(validate_position_argument("100").is_err());
    }

    #[test]
    fn validate_position_argument_accepts_negative_x() {
        assert_eq!(
            validate_position_argument("-1,200"),
            Ok("-1,200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_negative_y() {
        assert_eq!(
            validate_position_argument("100,-200"),
            Ok("100,-200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_negative_x_and_y() {
        assert_eq!(
            validate_position_argument("-1920,-200"),
            Ok("-1920,-200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_rejects_bare_dash() {
        assert!(validate_position_argument("-,200").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_a_coordinate_that_overflows_i32() {
        // Same reasoning as validate_size_argument's overflow test:
        // parse_position() trusts a validated value to parse infallibly, so
        // a coordinate that overflows i32 (max 2147483647, 10 digits) must
        // be rejected here, not discovered as a panic later.
        assert!(validate_position_argument("99999999999,200").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_unknown_word() {
        assert!(validate_position_argument("middle").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_empty() {
        assert!(validate_position_argument("").is_err());
    }

    // Split

    #[test]
    fn split_display_v_is_vertical() {
        assert_eq!(Split::V.to_string(), "Vertical");
    }

    #[test]
    fn split_display_h_is_horizontal() {
        assert_eq!(Split::H.to_string(), "Horizontal");
    }

    // SwayAction::sway_command

    #[test]
    fn sway_command_exec() {
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "exec foot");
    }

    #[test]
    fn sway_command_floating() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] floating enable");
    }

    #[test]
    fn sway_command_sticky() {
        let action = SwayAction::Sticky {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] sticky enable");
    }

    #[test]
    fn sway_command_fullscreen() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] fullscreen enable");
    }

    #[test]
    fn sway_command_focus() {
        let action = SwayAction::Focus {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] focus");
    }

    #[test]
    fn sway_command_split_v() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::V,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] splitv");
    }

    #[test]
    fn sway_command_split_h() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] splith");
    }

    #[test]
    fn sway_command_mark_quotes_the_mark() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo, exec evil",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] mark \"foo, exec evil\"");
    }

    #[test]
    fn sway_command_new_column() {
        let action = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move right");
    }

    #[test]
    fn sway_command_new_row() {
        let action = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move down");
    }

    #[test]
    fn sway_command_workspace_quotes_the_workspace() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web, exec evil",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.sway_command(),
            "[con_id=42] move workspace \"web, exec evil\""
        );
    }

    #[test]
    fn sway_command_output_quotes_the_output() {
        let action = SwayAction::Output {
            container_id: 42,
            output: "HDMI-A-1, exec evil",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.sway_command(),
            "[con_id=42] move container to output \"HDMI-A-1, exec evil\""
        );
    }

    #[test]
    fn sway_command_height() {
        let action = SwayAction::Height {
            container_id: 42,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] resize set height 300px");
    }

    #[test]
    fn sway_command_width() {
        let action = SwayAction::Width {
            container_id: 42,
            width: Size::Percent(20),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] resize set width 20ppt");
    }

    #[test]
    fn sway_command_position_center() {
        let action = SwayAction::Position {
            container_id: 42,
            position: Position::Center,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move position center");
    }

    #[test]
    fn sway_command_position_coordinates() {
        let action = SwayAction::Position {
            container_id: 42,
            position: Position::Coordinates { x: 100, y: 200 },
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move position 100 200");
    }

    #[test]
    fn sway_command_position_negative_coordinates() {
        let action = SwayAction::Position {
            container_id: 42,
            position: Position::Coordinates { x: -1920, y: -200 },
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.sway_command(),
            "[con_id=42] move position -1920 -200"
        );
    }

    #[test]
    fn sway_command_scratchpad() {
        let action = SwayAction::Scratchpad {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move scratchpad");
    }

    // SwayAction::sway_command_verb

    #[test]
    fn sway_command_verb_omits_the_con_id_prefix() {
        // Regression test for the sway_command()/sway_command_verb() split
        // (--dry-run's foundation): the verb-only form is exactly
        // sway_command() with the "[con_id=N] " prefix stripped, for every
        // variant that has one.
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] floating enable");
        assert_eq!(action.sway_command_verb(), "floating enable");
    }

    #[test]
    fn sway_command_verb_position_matches_sway_command_minus_the_prefix() {
        let action = SwayAction::Position {
            container_id: 42,
            position: Position::Coordinates { x: -1920, y: -200 },
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.sway_command(),
            "[con_id=42] move position -1920 -200"
        );
        assert_eq!(action.sway_command_verb(), "move position -1920 -200");
    }

    // SwayAction::Display

    #[test]
    fn display_exec() {
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "foot",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.to_string(),
            "Exec \"foot\" (app_id_match: \"foot\") (class_match: \"\")"
        );
    }

    #[test]
    fn display_split() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Split (container id: 42) (split: Horizontal)"
        );
    }

    #[test]
    fn display_floating() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Floating (container_id: 42)");
    }

    #[test]
    fn display_sticky() {
        let action = SwayAction::Sticky {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.to_string(), "Sticky (container_id: 42)");
    }

    #[test]
    fn display_fullscreen() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Fullscreen (container_id: 42)");
    }

    #[test]
    fn display_focus() {
        let action = SwayAction::Focus {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Focus (container_id: 42)");
    }

    #[test]
    fn display_new_column() {
        let action = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.to_string(), "New column (container_id: 42)");
    }

    #[test]
    fn display_new_row() {
        let action = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.to_string(), "New row (container_id: 42)");
    }

    #[test]
    fn display_workspace() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.to_string(),
            "Workspace (container id: 42) (workspace: web)"
        );
    }

    #[test]
    fn display_output() {
        let action = SwayAction::Output {
            container_id: 42,
            output: "HDMI-A-1",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.to_string(),
            "Output (container id: 42) (output: HDMI-A-1)"
        );
    }

    #[test]
    fn display_mark() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Mark (container id: 42) (mark: foo)");
    }

    #[test]
    fn display_height() {
        let action = SwayAction::Height {
            container_id: 42,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Height (container id: 42) (height: 300px)"
        );
    }

    #[test]
    fn display_width() {
        let action = SwayAction::Width {
            container_id: 42,
            width: Size::Percent(20),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Width (container id: 42) (width: 20ppt)"
        );
    }

    #[test]
    fn display_position() {
        let action = SwayAction::Position {
            container_id: 42,
            position: Position::Center,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Position (container id: 42) (position: center)"
        );
    }

    #[test]
    fn display_scratchpad() {
        let action = SwayAction::Scratchpad {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Scratchpad (container_id: 42)");
    }

    // SwayAction accessors

    #[test]
    fn verbose_reflects_the_flag() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: true,
            timeout: time::Duration::from_secs(5),
        };
        assert!(action.verbose());
    }

    #[test]
    fn duration_returns_the_timeout_for_an_event_based_action() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(7),
        };
        assert_eq!(action.duration(), time::Duration::from_secs(7));
    }

    #[test]
    fn duration_returns_the_wait_time_for_a_time_based_action() {
        let action = SwayAction::Height {
            container_id: 42,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(42),
        };
        assert_eq!(action.duration(), time::Duration::from_millis(42));
    }

    #[test]
    fn container_id_is_none_for_exec() {
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.container_id(), None);
    }

    #[test]
    fn container_id_is_set_for_other_actions() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.container_id(), Some(42));
    }

    // SwayAction::matching_window_change_events

    #[test]
    fn exec_only_matches_new_window_change() {
        // Regression test: `Move` used to be accepted here too, which meant
        // any pre-existing window being reparented (not just the one we
        // just launched) could be mistaken for a match.
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::New])
        );
    }

    #[test]
    fn floating_matches_floating_window_change() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Floating])
        );
    }

    #[test]
    fn fullscreen_matches_fullscreen_mode_window_change() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::FullscreenMode])
        );
    }

    #[test]
    fn focus_matches_focus_window_change() {
        let action = SwayAction::Focus {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Focus])
        );
    }

    #[test]
    fn workspace_matches_move_window_change() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Move])
        );
    }

    #[test]
    fn output_matches_move_window_change() {
        let action = SwayAction::Output {
            container_id: 42,
            output: "HDMI-A-1",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Move])
        );
    }

    #[test]
    fn scratchpad_matches_move_window_change() {
        let action = SwayAction::Scratchpad {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Move])
        );
    }

    #[test]
    fn mark_matches_mark_window_change() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Mark])
        );
    }

    #[test]
    fn sticky_has_no_matching_window_change() {
        // Regression test: confirmed live that toggling sticky fires no
        // WindowChange event at all, unlike Floating/Fullscreen/Focus,
        // which each have a dedicated variant — Sticky must stay a
        // wait-time action, not accidentally wired up to wait on an event
        // that will never arrive.
        let action = SwayAction::Sticky {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.matching_window_change_events(), None);
    }

    #[test]
    fn split_height_width_have_no_matching_window_change() {
        let split = SwayAction::Split {
            container_id: 42,
            split: Split::V,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let new_column = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let new_row = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let height = SwayAction::Height {
            container_id: 42,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let width = SwayAction::Width {
            container_id: 42,
            width: Size::Percent(20),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let position = SwayAction::Position {
            container_id: 42,
            position: Position::Center,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(split.matching_window_change_events(), None);
        assert_eq!(new_column.matching_window_change_events(), None);
        assert_eq!(new_row.matching_window_change_events(), None);
        assert_eq!(height.matching_window_change_events(), None);
        assert_eq!(width.matching_window_change_events(), None);
        assert_eq!(position.matching_window_change_events(), None);
    }

    // SwayAction::poll_matches

    #[test]
    fn split_has_a_poll_matcher() {
        // container_id 999999 is never in the tree read back by
        // parent_node_layout()'s IPC call in this headless test environment,
        // so the Some(bool) returned is always Some(false) here — this test
        // only asserts Split opts into polling at all, not the match outcome
        // itself (that needs a live Sway tree, covered by
        // tests/live_sway.rs). Both directions are exercised so
        // poll_matches()'s Split::V => NodeLayout::SplitV arm isn't left
        // unhit by cargo test.
        for split in [Split::H, Split::V] {
            let action = SwayAction::Split {
                container_id: 999999,
                split,
                verbose: false,
                wait_time: time::Duration::from_millis(20),
            };
            assert_eq!(action.poll_matches(999999, None), Some(false));
        }
    }

    #[test]
    fn new_column_and_new_row_have_no_poll_matcher_without_a_baseline() {
        // Unlike Split/Height/Width/Position, NewColumn/NewRow have no
        // fixed target to check against — without a poll_baseline()
        // snapshot to compare the current rect to, there's nothing to poll
        // for, so these opt out entirely (None) rather than resolving to
        // Some(false) that could never become true.
        let new_column = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let new_row = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(new_column.poll_matches(42, None), None);
        assert_eq!(new_row.poll_matches(42, None), None);
    }

    #[test]
    fn new_column_and_new_row_have_a_poll_matcher_given_a_baseline() {
        // container_id 999999 is never in the tree read back by
        // node_by_id()'s IPC call in this headless test environment, so
        // node_by_id(999999) is always None and node.rect != baseline can
        // never be observed true here — this test only asserts NewColumn/
        // NewRow opt into polling once a baseline exists, not the match
        // outcome itself (that needs a live Sway tree, covered by
        // tests/live_sway.rs).
        let new_column = SwayAction::NewColumn {
            container_id: 999999,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let new_row = SwayAction::NewRow {
            container_id: 999999,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let baseline = Some(rect(0, 0, 640, 720));
        assert_eq!(new_column.poll_matches(999999, baseline), Some(false));
        assert_eq!(new_row.poll_matches(999999, baseline), Some(false));
    }

    #[test]
    fn only_new_column_and_new_row_have_a_poll_baseline_at_all() {
        // Every other wait-time variant's poll_baseline() arm is `_ =>
        // None` with no IPC call at all — a deterministic "not applicable"
        // answer, unlike NewColumn/NewRow's (which calls node_by_id() and
        // is exempted from headless coverage per CLAUDE.md, since it needs
        // a live tree to return Some).
        let split = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let height = SwayAction::Height {
            container_id: 42,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let width = SwayAction::Width {
            container_id: 42,
            width: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let position = SwayAction::Position {
            container_id: 42,
            position: Position::Center,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let sticky = SwayAction::Sticky {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(split.poll_baseline(42), None);
        assert_eq!(height.poll_baseline(42), None);
        assert_eq!(width.poll_baseline(42), None);
        assert_eq!(position.poll_baseline(42), None);
        assert_eq!(sticky.poll_baseline(42), None);
    }

    #[test]
    fn height_and_width_in_pixels_have_a_poll_matcher() {
        // container_id 999999 is never in the tree read back by
        // node_by_id()'s IPC call in this headless test environment, so the
        // Some(bool) returned is always Some(false) here — this test only
        // asserts Height/Width in px opt into polling at all, not the match
        // outcome itself (that needs a live Sway tree, covered by
        // tests/live_sway.rs).
        let height = SwayAction::Height {
            container_id: 999999,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let width = SwayAction::Width {
            container_id: 999999,
            width: Size::Pixels(400),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(height.poll_matches(999999, None), Some(false));
        assert_eq!(width.poll_matches(999999, None), Some(false));
    }

    #[test]
    fn height_and_width_in_percent_have_no_poll_matcher() {
        // A `ppt` value has no pixel figure to poll for without also
        // resolving the reference dimension it's a percentage of (see
        // parse_pixel_value()'s doc comment), so these opt out entirely
        // rather than polling for something that could never match.
        let height = SwayAction::Height {
            container_id: 42,
            height: Size::Percent(20),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let width = SwayAction::Width {
            container_id: 42,
            width: Size::Percent(20),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(height.poll_matches(42, None), None);
        assert_eq!(width.poll_matches(42, None), None);
    }

    #[test]
    fn sticky_has_a_poll_matcher() {
        // container_id 999999 is never in the tree read back by
        // node_by_id()'s IPC call in this headless test environment, so
        // the Some(bool) returned is always Some(false) here — this test
        // only asserts Sticky opts into polling at all, not the match
        // outcome itself (that needs a live Sway tree, covered by
        // tests/live_sway.rs).
        let action = SwayAction::Sticky {
            container_id: 999999,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.poll_matches(999999, None), Some(false));
    }

    #[test]
    fn position_has_a_poll_matcher() {
        let center = SwayAction::Position {
            container_id: 999999,
            position: Position::Center,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let coords = SwayAction::Position {
            container_id: 999999,
            position: Position::Coordinates { x: 100, y: 200 },
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(center.poll_matches(999999, None), Some(false));
        assert_eq!(coords.poll_matches(999999, None), Some(false));
    }

    // SwayAction::matches_window_event

    #[test]
    fn exec_without_filter_matches_any_new_window() {
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("foot"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::NewWindowMatchWithoutCheck)
        ));
    }

    #[test]
    fn exec_without_filter_rejects_non_new_window_change() {
        // Regression test for the Move-over-matching bug: with no
        // app_id/class filter, only a `New` event may identify the window
        // just launched — not a `Move` (or any other) event belonging to
        // some other, pre-existing window.
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("move", 99, Some("foot"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::EventChangeTypeMismatch)
        ));
    }

    #[test]
    fn exec_with_app_id_match_accepts_matching_app_id() {
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "foot",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("foot"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowAppId)
        ));
    }

    #[test]
    fn exec_with_app_id_match_rejects_different_app_id() {
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "foot",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("alacritty"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowAppIdMismatch)
        ));
    }

    #[test]
    fn exec_with_app_id_match_rejects_missing_app_id() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "firefox",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowAppIdMismatch)
        ));
    }

    #[test]
    fn exec_with_class_match_accepts_matching_class() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "",
            class_match: "Firefox",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, Some("Firefox"));
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowClass)
        ));
    }

    #[test]
    fn exec_with_class_match_rejects_different_class() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "",
            class_match: "Firefox",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, Some("Chromium"));
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowClassMismatch)
        ));
    }

    #[test]
    fn exec_with_class_match_rejects_missing_window_properties() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "",
            class_match: "Firefox",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowClassMismatch)
        ));
    }

    #[test]
    fn exec_with_app_id_match_ignores_class_match_when_both_set() {
        // Documents current behavior: when both are set, app_id_match takes
        // priority and class_match is never consulted (see also
        // Args::app_id's conflicts_with in main.rs, which now prevents a
        // caller from setting both in the first place).
        let action = SwayAction::Exec {
            command: "foot",
            app_id_match: "foot",
            class_match: "SomethingElseEntirely",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("foot"), Some("SomethingElseEntirely"));
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowAppId)
        ));
    }

    #[test]
    fn non_exec_action_matches_on_container_id() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("floating", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn workspace_matches_on_container_id() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("move", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn fullscreen_matches_on_container_id() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("fullscreen_mode", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn scratchpad_matches_on_container_id() {
        let action = SwayAction::Scratchpad {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("move", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn non_exec_action_rejects_different_container_id() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("floating", 99, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::NoMatchingEvent)
        ));
    }

    #[test]
    fn non_exec_action_rejects_wrong_change_type() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("mark", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::EventChangeTypeMismatch)
        ));
    }

    // window_app_id_match / window_class_match

    #[test]
    fn window_app_id_match_true_when_equal() {
        let event = window_event("new", 1, Some("foot"), None);
        assert!(window_app_id_match(&event.container, "foot"));
    }

    #[test]
    fn window_app_id_match_false_when_different() {
        let event = window_event("new", 1, Some("foot"), None);
        assert!(!window_app_id_match(&event.container, "alacritty"));
    }

    #[test]
    fn window_app_id_match_false_when_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_app_id_match(&event.container, "foot"));
    }

    #[test]
    fn window_class_match_true_when_equal() {
        let event = window_event("new", 1, None, Some("Firefox"));
        assert!(window_class_match(&event.container, "Firefox"));
    }

    #[test]
    fn window_class_match_false_when_different() {
        let event = window_event("new", 1, None, Some("Firefox"));
        assert!(!window_class_match(&event.container, "Chromium"));
    }

    #[test]
    fn window_class_match_false_when_window_properties_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_class_match(&event.container, "Firefox"));
    }

    #[test]
    fn window_class_match_false_when_class_absent_but_window_properties_present() {
        let mut value = leaf_node_value(1, None, None);
        value["window_properties"] = serde_json::json!({});
        let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
        assert!(!window_class_match(&node, "Firefox"));
    }

    // matching_container_ids

    #[test]
    fn matching_container_ids_finds_tiling_and_floating_matches() {
        let tree = node_tree(
            1,
            vec![
                leaf_node_value(10, Some("foot"), None),
                leaf_node_value(11, Some("firefox"), None),
            ],
            vec![leaf_node_value(20, Some("foot"), None)],
        );
        let mut ids = matching_container_ids(&tree, "foot", "");
        ids.sort();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn matching_container_ids_empty_when_no_match() {
        let tree = node_tree(1, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
        assert_eq!(
            matching_container_ids(&tree, "nonexistent", ""),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn matching_container_ids_matches_by_class() {
        let tree = node_tree(1, vec![leaf_node_value(10, None, Some("Firefox"))], vec![]);
        assert_eq!(matching_container_ids(&tree, "", "Firefox"), vec![10]);
    }

    #[test]
    fn matching_container_ids_recurses_into_nested_containers() {
        let inner = container_node_value(2, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
        let tree = node_tree(1, vec![inner], vec![]);
        assert_eq!(matching_container_ids(&tree, "foot", ""), vec![10]);
    }

    #[test]
    fn matching_container_ids_prefers_app_id_over_class_when_both_set() {
        let tree = node_tree(
            1,
            vec![leaf_node_value(10, Some("foot"), Some("NoMatch"))],
            vec![],
        );
        assert_eq!(matching_container_ids(&tree, "foot", "NoMatch"), vec![10]);
    }

    // resolve_matches

    #[test]
    fn resolve_matches_errors_on_zero_matches() {
        assert_eq!(
            resolve_matches(vec![], "app_id \"foot\""),
            Err("No existing window matches app_id \"foot\"".to_string())
        );
    }

    #[test]
    fn resolve_matches_ok_on_single_match() {
        assert_eq!(resolve_matches(vec![42], "app_id \"foot\""), Ok(42));
    }

    #[test]
    fn resolve_matches_errors_listing_ids_on_multiple_matches() {
        assert_eq!(
            resolve_matches(vec![42, 91], "app_id \"foot\""),
            Err("2 windows match app_id \"foot\": 42, 91 — retarget with --con-id".to_string())
        );
    }

    // find_containing_name / find_workspace_node / contains_id /
    // is_at_the_trailing_workspace_edge

    #[test]
    fn find_containing_name_finds_the_nearest_ancestor_of_kind() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree(2, "main", vec![leaf], vec![]);
        assert_eq!(
            find_containing_name(&workspace, 10, NodeType::Workspace, None),
            Some("main".to_string())
        );
    }

    #[test]
    fn find_containing_name_returns_none_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert_eq!(
            find_containing_name(&tree, 999, NodeType::Workspace, None),
            None
        );
    }

    #[test]
    fn find_workspace_node_locates_the_workspace_containing_the_id() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree(2, "main", vec![leaf], vec![]);
        let found = find_workspace_node(&workspace, 10).expect("should find workspace");
        assert_eq!(found.name.as_deref(), Some("main"));
    }

    #[test]
    fn find_workspace_node_returns_none_when_id_not_found() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree(2, "main", vec![leaf], vec![]);
        assert!(find_workspace_node(&workspace, 999).is_none());
    }

    #[test]
    fn contains_id_true_for_self_and_descendants() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let tree = node_tree(1, vec![leaf], vec![]);
        assert!(contains_id(&tree, 1));
        assert!(contains_id(&tree, 10));
    }

    #[test]
    fn contains_id_false_when_absent() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(!contains_id(&tree, 42));
    }

    #[test]
    fn find_parent_layout_returns_the_direct_parents_layout() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let mut parent_value = container_node_value(2, vec![leaf], vec![]);
        parent_value["layout"] = serde_json::json!("splitv");
        let parent: Node = serde_json::from_value(parent_value).expect("valid Node test fixture");
        assert_eq!(find_parent_layout(&parent, 10), Some(NodeLayout::SplitV));
    }

    #[test]
    fn find_parent_layout_finds_the_nearest_ancestor_when_nested() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let mut inner_value = container_node_value(2, vec![leaf], vec![]);
        inner_value["layout"] = serde_json::json!("splith");
        let tree = node_tree(1, vec![inner_value], vec![]);
        assert_eq!(find_parent_layout(&tree, 10), Some(NodeLayout::SplitH));
    }

    #[test]
    fn find_parent_layout_checks_floating_children_too() {
        let floating = leaf_node_value(20, Some("foot"), None);
        let mut value = container_node_value(1, vec![], vec![floating]);
        value["layout"] = serde_json::json!("splitv");
        let tree: Node = serde_json::from_value(value).expect("valid Node test fixture");
        assert_eq!(find_parent_layout(&tree, 20), Some(NodeLayout::SplitV));
    }

    #[test]
    fn find_parent_layout_returns_none_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(find_parent_layout(&tree, 42).is_none());
    }

    // find_node

    #[test]
    fn find_node_finds_self_and_nested_children() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let inner = container_node_value(2, vec![leaf], vec![]);
        let tree = node_tree(1, vec![inner], vec![]);
        assert_eq!(find_node(&tree, 1).map(|node| node.id), Some(1));
        assert_eq!(find_node(&tree, 2).map(|node| node.id), Some(2));
        assert_eq!(find_node(&tree, 10).map(|node| node.id), Some(10));
    }

    #[test]
    fn find_node_finds_floating_children() {
        let floating = leaf_node_value(20, Some("foot"), None);
        let tree = node_tree(1, vec![], vec![floating]);
        assert_eq!(find_node(&tree, 20).map(|node| node.id), Some(20));
    }

    #[test]
    fn find_node_returns_none_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(find_node(&tree, 42).is_none());
    }

    // parse_pixel_value

    #[test]
    fn parse_size_parses_pixels() {
        assert_eq!(parse_size("300px"), Size::Pixels(300));
        assert_eq!(parse_size("0px"), Size::Pixels(0));
    }

    #[test]
    fn parse_size_parses_percent() {
        assert_eq!(parse_size("20ppt"), Size::Percent(20));
    }

    #[test]
    fn size_display_matches_the_format_parse_size_accepts() {
        assert_eq!(Size::Pixels(300).to_string(), "300px");
        assert_eq!(Size::Percent(20).to_string(), "20ppt");
    }

    #[test]
    fn parse_position_parses_center() {
        assert_eq!(parse_position("center"), Position::Center);
    }

    #[test]
    fn parse_position_parses_coordinates() {
        assert_eq!(
            parse_position("100,200"),
            Position::Coordinates { x: 100, y: 200 }
        );
    }

    #[test]
    fn parse_position_parses_negative_coordinates() {
        assert_eq!(
            parse_position("-1920,-200"),
            Position::Coordinates { x: -1920, y: -200 }
        );
    }

    #[test]
    fn position_display_matches_the_format_parse_position_accepts() {
        assert_eq!(Position::Center.to_string(), "center");
        assert_eq!(
            Position::Coordinates { x: 100, y: 200 }.to_string(),
            "100,200"
        );
    }

    // width_matches / height_matches

    fn node_with_geometry(width: i32, height: i32, border_width: i32, deco_height: i32) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        value["rect"] = serde_json::json!({"x": 0, "y": 0, "width": width, "height": height});
        value["deco_rect"] =
            serde_json::json!({"x": 0, "y": 0, "width": width, "height": deco_height});
        value["current_border_width"] = serde_json::json!(border_width);
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn width_matches_exact_rect_width() {
        let node = node_with_geometry(400, 300, 2, 25);
        assert!(width_matches(&node, 400));
    }

    #[test]
    fn width_matches_border_adjusted_width() {
        let node = node_with_geometry(396, 300, 2, 25);
        assert!(width_matches(&node, 400));
    }

    #[test]
    fn width_matches_false_when_neither_formula_fits() {
        let node = node_with_geometry(350, 300, 2, 25);
        assert!(!width_matches(&node, 400));
    }

    #[test]
    fn height_matches_decoration_inclusive_height() {
        let node = node_with_geometry(400, 275, 2, 25);
        assert!(height_matches(&node, 300));
    }

    #[test]
    fn height_matches_false_when_short_of_the_expected_value() {
        let node = node_with_geometry(400, 300, 2, 25);
        assert!(!height_matches(&node, 300));
    }

    // node_is_floating

    fn node_with_floating_state(node_type: &str, floating: Option<&str>) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        value["type"] = serde_json::json!(node_type);
        if let Some(floating) = floating {
            value["floating"] = serde_json::json!(floating);
        }
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn node_is_floating_true_for_floating_con_type_even_without_a_floating_field() {
        // Sway 1.9 (confirmed live against a headless compositor) leaves a
        // floating container's own `floating` field null and only reports
        // its state via `type: floating_con` — node_is_floating() must not
        // rely on the `floating` field alone.
        let node = node_with_floating_state("floating_con", None);
        assert!(node_is_floating(&node));
    }

    #[test]
    fn node_is_floating_true_when_the_floating_field_is_set() {
        let node = node_with_floating_state("con", Some("user_on"));
        assert!(node_is_floating(&node));
    }

    #[test]
    fn node_is_floating_false_for_a_plain_tiled_node() {
        let node = node_with_floating_state("con", None);
        assert!(!node_is_floating(&node));
    }

    // node_is_in_scratchpad

    fn node_with_scratchpad_state(state: Option<&str>) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        if let Some(state) = state {
            value["scratchpad_state"] = serde_json::json!(state);
        }
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn node_is_in_scratchpad_false_when_the_field_is_absent() {
        let node = node_with_scratchpad_state(None);
        assert!(!node_is_in_scratchpad(&node));
    }

    #[test]
    fn node_is_in_scratchpad_false_for_scratchpad_state_none() {
        // Present but "no scratchpad state" — an ordinary window, not one
        // in the scratchpad.
        let node = node_with_scratchpad_state(Some("none"));
        assert!(!node_is_in_scratchpad(&node));
    }

    #[test]
    fn node_is_in_scratchpad_true_for_fresh() {
        let node = node_with_scratchpad_state(Some("fresh"));
        assert!(node_is_in_scratchpad(&node));
    }

    #[test]
    fn node_is_in_scratchpad_true_for_changed() {
        let node = node_with_scratchpad_state(Some("changed"));
        assert!(node_is_in_scratchpad(&node));
    }

    // tree_shows_container_in_scratchpad

    #[test]
    fn tree_shows_container_in_scratchpad_true_via_ancestor_workspace_name() {
        // Regression test for a CI failure against Sway 1.9 (still what
        // `apt` installs on Ubuntu 24.04/CI): scratchpad_state stays
        // Some(ScratchpadState::None) there even for a genuinely
        // scratchpadded window, so this must detect the scratchpad via the
        // ancestor workspace name alone, with no scratchpad_state set at
        // all on the leaf node.
        let leaf = leaf_node_value(10, Some("foot"), None);
        let tree = workspace_node_tree(2, "__i3_scratch", vec![leaf], vec![]);
        assert!(tree_shows_container_in_scratchpad(&tree, 10));
    }

    #[test]
    fn tree_shows_container_in_scratchpad_true_via_scratchpad_state_fallback() {
        // The node's own scratchpad_state is still honored as a secondary,
        // redundant signal, independent of the ancestor workspace name.
        let mut leaf = leaf_node_value(10, Some("foot"), None);
        leaf["scratchpad_state"] = serde_json::json!("fresh");
        let tree = workspace_node_tree(2, "1", vec![leaf], vec![]);
        assert!(tree_shows_container_in_scratchpad(&tree, 10));
    }

    #[test]
    fn tree_shows_container_in_scratchpad_false_for_an_ordinary_window() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let tree = workspace_node_tree(2, "1", vec![leaf], vec![]);
        assert!(!tree_shows_container_in_scratchpad(&tree, 10));
    }

    // compute_center_position

    fn rect(x: i32, y: i32, width: i32, height: i32) -> swayipc::Rect {
        serde_json::from_value(
            serde_json::json!({"x": x, "y": y, "width": width, "height": height}),
        )
        .expect("valid Rect test fixture")
    }

    #[test]
    fn compute_center_position_centers_within_a_primary_output() {
        let output_rect = rect(0, 0, 1280, 720);
        assert_eq!(compute_center_position(output_rect, 400, 300), (440, 210));
    }

    #[test]
    fn compute_center_position_accounts_for_a_non_origin_output() {
        let output_rect = rect(1920, 100, 1280, 720);
        assert_eq!(compute_center_position(output_rect, 400, 300), (2360, 310));
    }

    // expected_position

    #[test]
    fn expected_position_parses_explicit_coordinates() {
        let node = node_with_geometry(400, 300, 2, 25);
        assert_eq!(
            expected_position(&Position::Coordinates { x: 100, y: 200 }, &node, None),
            Some((100, 200))
        );
    }

    #[test]
    fn expected_position_center_without_an_output_name_is_none() {
        let node = node_with_geometry(400, 300, 2, 25);
        assert_eq!(expected_position(&Position::Center, &node, None), None);
    }

    #[test]
    fn expected_position_center_with_an_output_name_is_none_without_a_live_socket() {
        // Exercises the "center" branch's output_rect() call (unreachable
        // from the None-output_name test above) — output_rect() degrades
        // to None when no Sway socket is reachable, which is exactly this
        // headless test environment, so this is testable without live
        // Sway: it confirms the whole chain still resolves to None rather
        // than panicking when IPC is unavailable.
        let node = node_with_geometry(400, 300, 2, 25);
        assert_eq!(
            expected_position(&Position::Center, &node, Some("some-output")),
            None
        );
    }

    // is_at_the_trailing_workspace_edge

    fn workspace_node_tree_with_layout(
        container_id: i64,
        name: &str,
        layout: &str,
        nodes: Vec<serde_json::Value>,
    ) -> Node {
        let mut value = container_node_value(container_id, nodes, vec![]);
        value["type"] = serde_json::json!("workspace");
        value["name"] = serde_json::json!(name);
        value["layout"] = serde_json::json!(layout);
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_true_for_a_solo_window() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![leaf]);
        assert!(is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_true_for_the_last_of_several_siblings() {
        // The case the old solo-window-only check missed: container_id has
        // a sibling, but is still the trailing (rightmost) child of a
        // workspace whose own layout already matches the move axis.
        let leaf1 = leaf_node_value(10, Some("foot"), None);
        let leaf2 = leaf_node_value(11, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![leaf1, leaf2]);
        assert!(is_at_the_trailing_workspace_edge(
            &workspace,
            11,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_false_for_a_leading_sibling() {
        let leaf1 = leaf_node_value(10, Some("foot"), None);
        let leaf2 = leaf_node_value(11, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![leaf1, leaf2]);
        assert!(!is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_false_when_the_workspace_layout_does_not_match_the_axis() {
        // Confirmed live: a solo window stacked via splitv, then moved
        // right, restructures in place rather than escalating — the
        // workspace's own layout has to match the move's axis too, not
        // just "container_id is the trailing child".
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splitv", vec![leaf]);
        assert!(!is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_checks_the_down_axis_for_new_row() {
        let leaf1 = leaf_node_value(10, Some("foot"), None);
        let leaf2 = leaf_node_value(11, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splitv", vec![leaf1, leaf2]);
        assert!(is_at_the_trailing_workspace_edge(
            &workspace,
            11,
            MoveDirection::Down
        ));
        assert!(!is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Down
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_false_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(!is_at_the_trailing_workspace_edge(
            &tree,
            999,
            MoveDirection::Right
        ));
    }

    // first_outcome_error

    #[test]
    fn first_outcome_error_ok_when_all_succeed() {
        let outcomes: Vec<Result<(), String>> = vec![Ok(()), Ok(())];
        assert_eq!(first_outcome_error(outcomes, "cmd"), Ok(()));
    }

    #[test]
    fn first_outcome_error_fails_when_empty() {
        let outcomes: Vec<Result<(), String>> = vec![];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("cmd command failed".to_string())
        );
    }

    #[test]
    fn first_outcome_error_surfaces_a_leading_failure() {
        let outcomes: Vec<Result<(), String>> = vec![Err("boom".to_string()), Ok(())];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("boom".to_string())
        );
    }

    #[test]
    fn first_outcome_error_surfaces_a_trailing_failure() {
        // Regression test: a prior version only inspected the first outcome
        // and returned Ok(()) here, silently dropping this failure.
        let outcomes: Vec<Result<(), String>> = vec![Ok(()), Err("boom".to_string())];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("boom".to_string())
        );
    }

    // Display impls for the private matching-result enums

    #[test]
    fn window_event_match_display() {
        assert_eq!(
            WindowEventMatch::WindowAppId.to_string(),
            "Window app_id match"
        );
        assert_eq!(
            WindowEventMatch::WindowClass.to_string(),
            "Window class match"
        );
        assert_eq!(
            WindowEventMatch::NewWindowMatchWithoutCheck.to_string(),
            "New window without app_id or class check"
        );
        assert_eq!(
            WindowEventMatch::WindowContainerIdMatch.to_string(),
            "Window container id match"
        );
    }

    #[test]
    fn window_event_match_error_display() {
        assert_eq!(
            WindowEventMatchError::EventChangeTypeMismatch.to_string(),
            "Event does not match action event matches"
        );
        assert_eq!(
            WindowEventMatchError::WindowAppIdMismatch.to_string(),
            "app_id mismatch"
        );
        assert_eq!(
            WindowEventMatchError::WindowClassMismatch.to_string(),
            "class mismatch"
        );
        assert_eq!(
            WindowEventMatchError::NoMatchingEvent.to_string(),
            "No matching event"
        );
    }

    // SwayLaunch::build_actions

    fn minimal_sway_launch() -> SwayLaunch<'static> {
        SwayLaunch {
            target: Target::ConId(42),
            app_id_match: "",
            class_match: "",
            split: None,
            floating: false,
            sticky: false,
            fullscreen: false,
            focus: false,
            mark: "",
            new_column: false,
            new_row: false,
            workspace: None,
            output: None,
            height: None,
            width: None,
            position: None,
            scratchpad: false,
            verbose: false,
            timeout: time::Duration::from_secs(5),
            wait_time: time::Duration::from_millis(20),
        }
    }

    #[test]
    fn build_actions_is_empty_when_no_flags_are_set() {
        let sway_launch = minimal_sway_launch();
        let actions = sway_launch
            .build_actions(42, true)
            .expect("no flags set means no IPC call at all, so this can't fail");
        assert!(actions.is_empty());
    }

    #[test]
    fn build_actions_includes_every_flag_in_the_documented_fixed_order() {
        // Regression test for the SwayLaunch::run() -> build_actions() split
        // (external-review.md #5/#32): every flag except new_column/new_row
        // (which need a live get_outputs()/get_tree() call inside
        // relocates_to_another_output() even just to build the plan, so
        // they're covered separately by tests/live_sway.rs, not headlessly
        // here) is exercised together, asserting both that each one
        // produces its documented action *and* that the overall order
        // matches CLAUDE.md's Architecture section exactly: Workspace ->
        // Output -> Split -> Floating -> Sticky -> Fullscreen -> Focus ->
        // Height -> Width -> Position -> Mark -> Scratchpad.
        let mut sway_launch = minimal_sway_launch();
        sway_launch.workspace = Some("3");
        sway_launch.output = Some("HDMI-A-1");
        sway_launch.split = Some(Split::H);
        sway_launch.floating = true;
        sway_launch.sticky = true;
        sway_launch.fullscreen = true;
        sway_launch.focus = true;
        sway_launch.height = Some(Size::Pixels(300));
        sway_launch.width = Some(Size::Pixels(400));
        sway_launch.position = Some(Position::Center);
        sway_launch.mark = "pinned";
        sway_launch.scratchpad = true;

        let actions = sway_launch
            .build_actions(42, true)
            .expect("none of these flags touch IPC while building the plan");

        let kinds: Vec<&str> = actions
            .iter()
            .map(|action| match action {
                SwayAction::Workspace { .. } => "workspace",
                SwayAction::Output { .. } => "output",
                SwayAction::Split { .. } => "split",
                SwayAction::Floating { .. } => "floating",
                SwayAction::Sticky { .. } => "sticky",
                SwayAction::Fullscreen { .. } => "fullscreen",
                SwayAction::Focus { .. } => "focus",
                SwayAction::Height { .. } => "height",
                SwayAction::Width { .. } => "width",
                SwayAction::Position { .. } => "position",
                SwayAction::Mark { .. } => "mark",
                SwayAction::Scratchpad { .. } => "scratchpad",
                other => panic!("unexpected action in plan: {:?}", other),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "workspace",
                "output",
                "split",
                "floating",
                "sticky",
                "fullscreen",
                "focus",
                "height",
                "width",
                "position",
                "mark",
                "scratchpad",
            ]
        );
        for action in &actions {
            assert_eq!(action.container_id(), Some(42));
        }
    }

    #[test]
    fn build_actions_omits_mark_when_empty() {
        let sway_launch = minimal_sway_launch();
        let actions = sway_launch
            .build_actions(42, true)
            .expect("no flags set means no IPC call at all, so this can't fail");
        assert!(!actions
            .iter()
            .any(|action| matches!(action, SwayAction::Mark { .. })));
    }

    #[test]
    fn build_actions_propagates_the_relocation_check_error_for_new_column() {
        // Headless environments have no reachable Sway socket, so
        // relocates_to_another_output()'s own get_outputs() call fails —
        // confirming build_actions() surfaces that as an error rather than
        // silently building an incomplete plan. The actual guard behavior
        // (skip vs. include NewColumn) needs a live compositor; see
        // tests/live_sway.rs's new_column_does_not_relocate_*/
        // new_column_combined_with_workspace_* tests for that.
        let mut sway_launch = minimal_sway_launch();
        sway_launch.new_column = true;
        assert!(sway_launch.build_actions(42, true).is_err());
    }

    #[test]
    fn build_actions_propagates_the_relocation_check_error_for_new_row() {
        let mut sway_launch = minimal_sway_launch();
        sway_launch.new_row = true;
        assert!(sway_launch.build_actions(42, true).is_err());
    }

    #[test]
    fn build_actions_with_check_relocation_false_never_touches_ipc() {
        // The whole point of check_relocation: false — confirms
        // new_column/new_row are always included (never skipped, and
        // never erroring) with no reachable Sway socket, unlike the two
        // tests above with check_relocation: true.
        let mut sway_launch = minimal_sway_launch();
        sway_launch.new_column = true;
        sway_launch.new_row = true;
        let actions = sway_launch
            .build_actions(42, false)
            .expect("check_relocation: false makes no IPC call, so this can't fail");
        assert!(actions
            .iter()
            .any(|action| matches!(action, SwayAction::NewColumn { .. })));
        assert!(actions
            .iter()
            .any(|action| matches!(action, SwayAction::NewRow { .. })));
    }

    // SwayLaunch::build_actions_for_preview

    #[test]
    fn build_actions_for_preview_never_touches_ipc_even_with_new_column_set() {
        let mut sway_launch = minimal_sway_launch();
        sway_launch.new_column = true;
        sway_launch.floating = true;
        // Would panic (build_actions_for_preview()'s own .expect()) if this
        // somehow tried an IPC call in this headless test environment —
        // passing at all is the assertion.
        let actions = sway_launch.build_actions_for_preview();
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn build_actions_for_preview_verbs_are_container_id_free() {
        let mut sway_launch = minimal_sway_launch();
        sway_launch.floating = true;
        sway_launch.mark = "pinned";
        let actions = sway_launch.build_actions_for_preview();
        let verbs: Vec<String> = actions.iter().map(|a| a.sway_command_verb()).collect();
        assert_eq!(verbs, vec!["floating enable", "mark \"pinned\""]);
    }
}
