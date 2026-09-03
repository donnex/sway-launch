use std::io::Write;
use std::sync::mpsc;
use std::{fmt, thread, time};
use swayipc::{
    Connection, Event, EventType, Node, NodeLayout, NodeType, WindowChange, WindowEvent,
};

mod ipc;
mod process_marker;
#[cfg(test)]
mod test_support;
mod tree;
mod values;

pub use ipc::kill_container;
use ipc::{event_loop, ipc_error, new_connection, quote_sway_string, run_sway_command};
use process_marker::{
    any_process_has_env_var, generate_pid_marker_token, process_has_env_var,
    PID_MARKER_FALLBACK_GRACE, PID_MARKER_VAR,
};
use tree::{
    contains_id, expected_position, find_containing_name, find_node, find_parent_layout,
    height_matches, is_at_the_trailing_workspace_edge, matching_container_ids, node_position,
    resolve_matches, width_matches, window_app_id_match, window_class_match, ContainerState,
    MoveDirection,
};
pub use values::{
    parse_position, parse_size, require_non_blank, validate_non_blank_argument,
    validate_position_argument, validate_size_argument, validate_sway_string_argument, Position,
    Size, Split,
};

/// The upper bound `run_poll_then_fallback()` polls `get_tree()` for a
/// wait-time action's own confirmation before giving up and falling back to
/// the original blind sleep-the-rest-of-`--wait-time` behavior — capped at
/// the actual `--wait-time` in play (`run_poll_then_fallback()` computes
/// `self.poll_grace().min(wait_time)`), not used directly, and not the bound
/// every variant gets — `NewColumn`/`NewRow` use the much shorter
/// `MOVE_POLL_GRACE` instead, for the reason documented there. Several of
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

/// The same bound for `NewColumn`/`NewRow` specifically, which need a much
/// shorter one — see `SwayAction::poll_grace()`.
///
/// Every other matcher compares against a fixed target the action itself
/// asked for ("is it 300px?", "is the parent splith?"), so time spent waiting
/// can only ever confirm that exact request. These two have no fixed target —
/// a relative move can land the window anywhere — so they compare the
/// container's `rect` against a snapshot taken before the command and treat
/// *any* difference as confirmation. That predicate can't tell our move apart
/// from someone else's change to the same window, so every millisecond the
/// window stays open is a millisecond in which an unrelated resize or move
/// gets credited to this action.
///
/// It can't be closed, only narrowed: Sway offers no way to attribute a
/// geometry change to a specific command. 25ms is where the measurements put
/// it. Against a live compositor, a real move's `rect` change was visible on
/// the very first tree read after Sway acknowledged the command — 0.29, 0.39,
/// 0.41, 0.47, 0.59ms across six moves, with one 3.42ms outlier — because
/// Sway arranges before it replies. 25ms keeps ~7x margin over the worst of
/// those while cutting the exposure window 8x. A move that somehow takes
/// longer than this reports `Unconfirmed` rather than failing, which is the
/// safe direction: the command still ran, and the fallback sleep still
/// happens, so nothing about the layout changes — only how confidently the
/// result is described.
const MOVE_POLL_GRACE: time::Duration = time::Duration::from_millis(25);

/// How often `run_wait_time()`'s poll loop re-queries `get_tree()` while
/// inside `WAIT_TIME_POLL_GRACE`. Cheap enough on a local Unix socket to run
/// this often without meaningfully loading the compositor, while still
/// avoiding a zero-sleep busy loop.
const WAIT_TIME_POLL_INTERVAL: time::Duration = time::Duration::from_millis(10);

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

    /// Runs this action for real, returning the container id it acted on and
    /// how confidently it can say what happened — which `SwayLaunch::run()`
    /// turns into the `ActionStatus` reported in `RunOutcome`/`--json`.
    ///
    /// The three outcomes are genuinely different claims, and collapsing them
    /// was misleading: an event-confirmed action that observed its state, an
    /// action that short-circuited because the state already held, and a
    /// wait-time action that sent its command and never saw the change appear
    /// are not equally strong results. `Exec` is always `Changed` — a freshly
    /// launched window is inherently a change, and `already_at_target()` never
    /// short-circuits it.
    ///
    /// `already_at_target()`'s no-op short-circuit is deliberately *not* run
    /// here, ahead of the dispatch below: it belongs inside
    /// `run_wait_matching_events()`, after that function has subscribed. See
    /// its doc comment for why the ordering is load-bearing. Every variant this
    /// function routes elsewhere — `Exec`, and the seven that confirm by
    /// polling — answers `false` from `state_satisfied()` unconditionally, so
    /// moving the check has no effect on them.
    fn run(&self) -> Result<ActionResult, String> {
        if self.verbose() {
            eprintln!("Sway action: {}", self);
        }

        match self {
            SwayAction::Exec { .. } => {
                self.run_wait_matching_exec_event()
                    .map(|(container_id, ownership)| ActionResult {
                        container_id,
                        outcome: ActionOutcome::Changed,
                        launch_ownership: Some(ownership),
                    })
            }
            _ => match self.matching_window_change_events() {
                Some(_) => self
                    .run_wait_matching_events()
                    .map(|(container_id, outcome)| ActionResult::acted(container_id, outcome)),
                None => self.run_wait_time().map(|(container_id, confirmed)| {
                    let outcome = if confirmed {
                        ActionOutcome::Changed
                    } else {
                        ActionOutcome::Unconfirmed
                    };
                    ActionResult::acted(container_id, outcome)
                }),
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
    ///
    /// `Mark` is the one event-confirmed action deliberately excluded here
    /// even though `state_satisfied()` below can answer for it: re-applying a
    /// mark the container already has *does* fire `WindowChange::Mark`, so
    /// there's no hang to avoid, and short-circuiting would report it as
    /// `AlreadySatisfied` when the command genuinely does re-apply.
    ///
    /// **Call this only once a subscription is already open.** This check reads
    /// state that another client can change a moment later, and the no-op it
    /// exists to detect is precisely the case Sway fires no event for — so if
    /// the state flips between reading it here and subscribing, the command
    /// becomes a no-op, no event ever arrives, and the action waits out the
    /// full `--timeout` before erroring. Subscribing first closes the window
    /// completely rather than narrowing it: a change landing before this read
    /// is caught by the read, and one landing after it is caught by the
    /// subscription, with no instant belonging to neither. Measured against a
    /// live compositor while the check still ran ahead of the subscription:
    /// sweeping a competing `floating enable` across a 0-20ms window after
    /// spawn, 5 of 40 trials hung the full timeout, clustered 2.5-4ms in.
    fn already_at_target(&self) -> Result<Option<i64>, String> {
        if matches!(self, SwayAction::Mark { .. }) {
            return Ok(None);
        }
        let Some(container_id) = self.container_id() else {
            return Ok(None);
        };
        if self.state_satisfied()? {
            Ok(Some(container_id))
        } else {
            Ok(None)
        }
    }

    /// Whether this action's requested end state currently holds, read from
    /// Sway's tree. `false` for every action that confirms by polling instead
    /// (`Split`, `Sticky`, `Height`, `Width`, `Position`, `NewColumn`,
    /// `NewRow`) and for `Exec`, none of which reach this.
    ///
    /// Used twice, for two different questions. Before the command runs,
    /// `already_at_target()` asks it to detect a no-op Sway would fire no
    /// event for. *After* a matching event arrives,
    /// `run_wait_matching_events()` asks it again — because an event proves
    /// only that Sway emitted that event type for this container, not that the
    /// state this action asked for is the one that ended up applying.
    ///
    /// The gap is reachable whenever something else is driving the same
    /// window: another `sway-launch` process, a keybinding, a `swaymsg` in the
    /// same script. Two invocations sending `move workspace 2` and
    /// `move workspace 3` at once each see a `WindowChange::Move` for their
    /// container and, on the event alone, both report success — while the
    /// window is on exactly one of them. The same reasoning applies to
    /// floating, fullscreen, focus (inherently global, so the most exposed),
    /// output, mark and scratchpad. Checking the state turns the event from
    /// the confirmation into what it actually is: a wake-up telling us it's
    /// worth looking.
    ///
    /// Safe to require rather than merely prefer, because Sway applies a
    /// command before emitting its event and serves IPC requests in order, so
    /// a `get_tree()` issued after receiving the event already reflects it —
    /// this is not a settle race. If the state genuinely isn't there, the
    /// event was someone else's and waiting for the next one is correct.
    ///
    /// Split in two: this half fetches, `state_satisfies()` below decides. The
    /// decision is the part worth testing exhaustively and the part with no
    /// business talking to a socket, so it takes a `ContainerState` and stays
    /// ordinary pure logic — the same shape `position_matches()`/
    /// `expected_position()` already use, and what this project asks of any new
    /// matcher (see CLAUDE.md's coverage note). One `get_tree()` answers for
    /// every variant, where the previous per-arm helpers each fetched their
    /// own.
    fn state_satisfied(&self) -> Result<bool, String> {
        let Some(container_id) = self.container_id() else {
            return Ok(false);
        };
        // Every variant `state_satisfies()` answers `false` for regardless of
        // state is one with no confirming event, so there's nothing to read a
        // tree for.
        if self.matching_window_change_events().is_none() {
            return Ok(false);
        }
        Ok(self.state_satisfies(self::container_state(container_id)?.as_ref()))
    }

    /// Whether `state` is the state this action asked for. `None` — the
    /// container isn't in the tree at all — is never satisfied: a window that
    /// has closed isn't floating, focused or on the target workspace, and
    /// reporting it as already-there would short-circuit an action into
    /// claiming success over a container that no longer exists.
    fn state_satisfies(&self, state: Option<&ContainerState>) -> bool {
        let Some(state) = state else {
            return false;
        };

        match self {
            SwayAction::Workspace { workspace, .. } => {
                state.workspace.as_deref() == Some(*workspace)
            }
            SwayAction::Output { output, .. } => state.output.as_deref() == Some(*output),
            SwayAction::Floating { .. } => state.floating,
            SwayAction::Fullscreen { .. } => state.fullscreen,
            SwayAction::Focus { .. } => state.focused,
            SwayAction::Scratchpad { .. } => state.in_scratchpad,
            SwayAction::Mark { mark, .. } => state.marks.iter().any(|held| held == mark),
            _ => false,
        }
    }

    /// How long `run_poll_then_fallback()` may keep polling for this
    /// variant's own confirmation. Pure, so the choice stays unit-testable
    /// headlessly; the caller still caps it at the actual `--wait-time`.
    ///
    /// `NewColumn`/`NewRow` get a much shorter window than everything else
    /// because their matcher answers a weaker question — "did the geometry
    /// change?" rather than "is it what was asked for?" — and only for as
    /// long as it keeps looking. See `MOVE_POLL_GRACE`.
    fn poll_grace(&self) -> time::Duration {
        match self {
            SwayAction::NewColumn { .. } | SwayAction::NewRow { .. } => MOVE_POLL_GRACE,
            _ => WAIT_TIME_POLL_GRACE,
        }
    }

    /// Whether this variant confirms its own command via polling at all.
    /// Pure — no IPC — so `run_wait_time()` can decide whether to enter the
    /// poll loop before opening a connection for it, and so the decision
    /// stays unit-testable headlessly (which is most of what the poll
    /// matchers' own tests were ever asserting).
    ///
    /// `baseline` matters only to `NewColumn`/`NewRow`: without a snapshot
    /// to compare against there is nothing for them to poll for, so they opt
    /// out (see `poll_baseline()`'s doc comment). A `ppt` (percent) `Size`
    /// opts out for a different reason — there's no pixel figure to poll for
    /// without also resolving the reference dimension it's a percentage of.
    fn polls(&self, baseline: Option<swayipc::Rect>) -> bool {
        match self {
            SwayAction::Split { .. } | SwayAction::Sticky { .. } | SwayAction::Position { .. } => {
                true
            }
            SwayAction::Height {
                height: Size::Pixels(_),
                ..
            }
            | SwayAction::Width {
                width: Size::Pixels(_),
                ..
            } => true,
            SwayAction::Height {
                height: Size::Percent(_),
                ..
            }
            | SwayAction::Width {
                width: Size::Percent(_),
                ..
            } => false,
            SwayAction::NewColumn { .. } | SwayAction::NewRow { .. } => baseline.is_some(),
            _ => false,
        }
    }

    /// Whether the tree currently shows this action's command as having
    /// taken effect (see docs/plan-poll-based-wait-time-actions.md). Only
    /// meaningful for variants `polls()` returns `true` for; every other
    /// variant answers `false` unconditionally.
    ///
    /// Reads through the caller's `connection` rather than opening its own:
    /// `run_poll_then_fallback()` calls this up to 20 times inside a 200ms
    /// grace period, and each variant used to open one or two fresh Sway IPC
    /// connections per iteration (`Position` two, for `get_tree()` plus
    /// `get_outputs()`) — ~40 connects and handshakes for a single action.
    ///
    /// Errors reading the tree (transient IPC hiccup, container gone) fold
    /// into "not confirmed yet" rather than propagating: this only ever
    /// exists to return *faster* than the unconditional sleep already does,
    /// never to turn success into failure.
    fn poll_matches(
        &self,
        connection: &mut Connection,
        container_id: i64,
        baseline: Option<swayipc::Rect>,
    ) -> bool {
        match self {
            SwayAction::Split { split, .. } => {
                let expected = match split {
                    Split::V => NodeLayout::SplitV,
                    Split::H => NodeLayout::SplitH,
                };
                self::parent_node_layout(connection, container_id) == Some(expected)
            }
            SwayAction::Height {
                height: Size::Pixels(pixels),
                ..
            } => self::node_by_id(connection, container_id)
                .is_some_and(|node| height_matches(&node, *pixels)),
            SwayAction::Width {
                width: Size::Pixels(pixels),
                ..
            } => self::node_by_id(connection, container_id)
                .is_some_and(|node| width_matches(&node, *pixels)),
            SwayAction::Position { position, .. } => {
                self::position_matches(connection, container_id, position)
            }
            // Unlike Floating's `floating`/node-type split (see
            // node_is_floating()'s doc comment), `sticky` is a plain `bool`
            // on `Node` with no version-dependent quirk found — confirmed
            // live that `sticky enable` sets it directly and immediately,
            // even on a still-tiled container.
            SwayAction::Sticky { .. } => {
                self::node_by_id(connection, container_id).is_some_and(|node| node.sticky)
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
                baseline.is_some_and(|baseline| {
                    self::node_by_id(connection, container_id)
                        .is_some_and(|node| node.rect != baseline)
                })
            }
            _ => false,
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
    fn poll_baseline(
        &self,
        connection: &mut Connection,
        container_id: i64,
    ) -> Option<swayipc::Rect> {
        match self {
            SwayAction::NewColumn { .. } | SwayAction::NewRow { .. } => {
                self::node_by_id(connection, container_id).map(|node| node.rect)
            }
            _ => None,
        }
    }

    /// Polls for up to `poll_grace()` for `poll_matches()` to confirm
    /// the command `run_wait_time()` just sent took effect, returning as
    /// soon as it does (the fast path). Several wait-time actions have
    /// legitimate no-op outcomes where confirmation never arrives (see
    /// docs/plan-poll-based-wait-time-actions.md) — telling that apart from
    /// "hasn't happened yet" is impossible from tree state alone, so once
    /// the grace period elapses this falls back to today's original
    /// behavior: assume success and sleep out the rest of `wait_time`,
    /// mirroring `run_wait_matching_exec_event()`'s
    /// `PID_MARKER_FALLBACK_GRACE` fallback.
    /// Returns the container id and whether the change was actually observed —
    /// `false` means the grace period elapsed and this fell back to sleeping,
    /// which the caller reports as `ActionStatus::Unconfirmed` rather than
    /// letting it pass as an ordinary success.
    fn run_poll_then_fallback(
        &self,
        connection: &mut Connection,
        container_id: i64,
        wait_time: time::Duration,
        baseline: Option<swayipc::Rect>,
    ) -> (i64, bool) {
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
        let grace = self.poll_grace().min(wait_time);
        let poll_started = time::Instant::now();
        loop {
            if self.poll_matches(connection, container_id, baseline) {
                if self.verbose() {
                    eprintln!("Confirmed via poll (container id: {})", container_id);
                }
                return (container_id, true);
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
        (container_id, false)
    }

    /// Returns the container id and whether the requested change was actually
    /// observed. `false` covers every route to "we sent the command and waited,
    /// but never saw it take effect": the poll grace period elapsing, a variant
    /// with no poll matcher at all (a `ppt` size has no pixel figure to check),
    /// and failing to open the poll connection. The caller reports that as
    /// `ActionStatus::Unconfirmed` — it is not the same claim as a confirmed
    /// change, and reporting it as one is what made a wait-time action's
    /// "success" weaker than an event-confirmed action's without saying so.
    fn run_wait_time(&self) -> Result<(i64, bool), String> {
        let wait_time = self.duration();

        if self.verbose() {
            eprintln!(
                "No matching event types for action. Will run Sway command and wait {} ms.",
                wait_time.as_millis()
            );
        }

        let container_id = self
            .container_id()
            .expect("run_wait_time() is only ever called for variants other than Exec");

        // Checked before the sleep as well as after it, and both are wanted.
        //
        // This one is a fast fail: --wait-time is the caller's own knob and is
        // deliberately unbounded, so sleeping it out in full only to report
        // that the target had already closed before the wait even began is
        // pure delay in front of a foregone answer. A step with
        // `wait_time = 3600000` against a window that's already gone used to
        // sit there for an hour before saying so.
        //
        // It doesn't replace the post-sleep check below, which covers the
        // different case of a window closing *during* the wait, and is the one
        // guaranteeing the container still exists at the moment the command is
        // actually sent. Two get_tree() calls per wait-time action, then,
        // where there used to be one — a handful per invocation, none of them
        // in a loop.
        if !self::container_exists(container_id)? {
            return Err(format!(
                "container id {} no longer exists — window may have closed",
                container_id
            ));
        }

        // Wait before and after running the Sway command: before, to let
        // other running IPC clients finish their own commands; after, to
        // let this command finish before the next action runs.
        thread::sleep(wait_time);

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

        // One connection for the whole poll cycle — the baseline snapshot
        // and every poll iteration below share it, rather than each tree
        // read opening its own. Failing to open one isn't fatal: there's
        // simply nothing to poll with, so this falls back to the original
        // unconditional sleep, exactly as a variant with no matcher does.
        let mut poll_connection = new_connection().ok();

        // Captured before the command runs, not after — see
        // poll_baseline()'s doc comment for why NewColumn/NewRow need a
        // "before" snapshot while every other poll-matched action doesn't.
        let baseline = poll_connection
            .as_mut()
            .and_then(|connection| self.poll_baseline(connection, container_id));

        let sway_command = self.sway_command();
        if self.verbose() {
            eprintln!("Sway command: {}", sway_command);
        }

        run_sway_command(&sway_command)?;

        if self.polls(baseline) {
            if let Some(connection) = poll_connection.as_mut() {
                return Ok(self.run_poll_then_fallback(
                    connection,
                    container_id,
                    wait_time,
                    baseline,
                ));
            }
        }

        thread::sleep(wait_time);

        // Nothing was polled for, so nothing was observed. Honest rather than
        // optimistic: this is the ppt-size case, and the no-connection case.
        Ok((container_id, false))
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
    ///
    /// Which of those two ends the wait is reported back as the returned
    /// `LaunchOwnership` (see its own doc comment): a marked process that's
    /// gone means the window is ours by elimination, while the grace period
    /// merely elapsing means we adopted a window we never confirmed was ours.
    /// Only `--rollback-on-error` reads this, and only to decide whether
    /// killing the window is this run's business.
    ///
    /// Same background-thread/socket lifetime as `run_wait_matching_events()`:
    /// the thread reading this action's event stream may outlive this function
    /// if still blocked on the socket when we return, but is retired by the
    /// next window event to arrive — which, in a multi-step run, is the next
    /// step's own. Measured bounded, not accumulating; see the comment at that
    /// function's `thread::spawn` call for the mechanism and the numbers.
    fn run_wait_matching_exec_event(&self) -> Result<(i64, LaunchOwnership), String> {
        let SwayAction::Exec {
            command,
            verbose,
            timeout,
            ..
        } = *self
        else {
            unreachable!("run_wait_matching_exec_event is only called for SwayAction::Exec");
        };

        let event_loop = event_loop(&[EventType::Window])?;

        let token = generate_pid_marker_token();
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
                             content-matched container id {} (adopted, not confirmed as \
                             launched by this run)",
                            container_id
                        );
                    }
                    return Ok((container_id, LaunchOwnership::Adopted));
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
                .is_some_and(|pid| process_has_env_var(pid, PID_MARKER_VAR, &token));

            if pid_confirmed {
                if verbose {
                    eprintln!(
                        "Event match: {:?} container id {} (PID-marker-confirmed)",
                        window.change, window.container.id
                    );
                }
                return Ok((window.container.id, LaunchOwnership::Launched));
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
                || !any_process_has_env_var(PID_MARKER_VAR, &token, verbose);
            if marked_process_confirmed_gone {
                let (container_id, _) = fallback.expect("just set above if it wasn't already");
                if verbose {
                    eprintln!(
                        "Marked process no longer running; using fallback container id {} \
                         (nothing marker-confirmed can still arrive, so this window is this \
                         run's own)",
                        container_id
                    );
                }
                return Ok((container_id, LaunchOwnership::Launched));
            }
        }
    }

    /// Subscribes, confirms the requested state isn't already in effect, sends
    /// the command, and waits for an event that — together with a second state
    /// read — confirms it applied.
    ///
    /// The subscription is opened before the no-op check and before the
    /// command, and that ordering is the whole point rather than an incidental
    /// detail: it's what makes the check-then-command sequence safe against
    /// another client racing it. See `already_at_target()`'s doc comment. The
    /// cost is one Sway IPC connection opened and immediately dropped on the
    /// short-circuit path, which is the cheap half of the trade — that path
    /// already pays for a `get_tree()`.
    ///
    /// The container-exists check stays ahead of the subscription, though, and
    /// that ordering is load-bearing too, for an unrelated reason:
    /// `new_event_connection()` deliberately sets no socket timeout (a
    /// subscription's job is to block), but `subscribe()` itself is a
    /// request/response handshake, so a compositor that accepts a connection
    /// and then stops answering would hang there forever. Every other IPC call
    /// this function makes goes through `new_connection()`, which is bounded by
    /// `IPC_ROUND_TRIP_TIMEOUT` — so keeping a bounded read first is what makes
    /// a wedged compositor fail with a message instead of hanging. Found by
    /// `tests/live_sway.rs`'s `a_stalled_sway_socket_fails_instead_of_hanging_forever`
    /// after an earlier version of this ordering fix put the subscription first.
    fn run_wait_matching_events(&self) -> Result<(i64, ActionOutcome), String> {
        // Same check, same reason, and the same error text as
        // run_wait_time()'s: on Sway 1.9 (still what `apt` installs on
        // Ubuntu 24.04/CI) a [con_id=N] criteria matching zero containers is
        // "success", so without this an action whose container closed since
        // it was resolved would send its command successfully and then block
        // for the full --timeout waiting on a confirmation event that can
        // never arrive, reporting "N sec timeout reached" — pointing the
        // user at --timeout rather than at the closed window. Sway 1.11
        // already errors clearly ("No matching node.") from
        // run_sway_command() below, confirmed live, making this redundant
        // there but still required for 1.9.
        //
        // Costs one extra get_tree() per event-confirmed action, partly
        // duplicating the one already_at_target() makes for six of the seven
        // variants. Accepted deliberately: one check in one place covers
        // every variant (including Mark, which has no already_at_target()
        // arm at all), and this runs a handful of times per invocation, not
        // in a loop.
        let container_id = self
            .container_id()
            .expect("run_wait_matching_events() is only ever called for variants other than Exec");
        if !self::container_exists(container_id)? {
            return Err(format!(
                "container id {} no longer exists — window may have closed",
                container_id
            ));
        }

        let event_loop = event_loop(&[EventType::Window])?;

        if let Some(container_id) = self.already_at_target()? {
            if self.verbose() {
                eprintln!(
                    "Already at target, nothing to move (container id: {})",
                    container_id
                );
            }
            return Ok((container_id, ActionOutcome::AlreadySatisfied));
        }

        let sway_command = self.sway_command();
        if self.verbose() {
            eprintln!("Sway command: {}", sway_command);
        }
        run_sway_command(&sway_command)?;

        // Read events on a separate thread and forward them through a
        // channel, so recv_timeout() below enforces a real deadline even if
        // the event stream itself never produces another event (a blocking
        // iterator has no way to time out on its own).
        //
        // The thread outlives this function whenever it's still blocked on the
        // socket as we return: there's no public way to interrupt a blocking
        // EventStream read from another thread, so it can only notice its
        // receiver is gone the next time an event actually arrives, at which
        // point send() fails, the loop breaks, and the thread and its Sway IPC
        // socket are released.
        //
        // That drains itself in practice rather than accumulating, because the
        // events that wake a stale reader are the very ones the *next*
        // event-confirmed action produces: Sway broadcasts every window event
        // to every subscription, so step N's own confirming event is what
        // retires step N-1's reader. Measured against a live compositor rather
        // than assumed — a 300-step layout of event-confirmed actions peaked at
        // 2 threads and 3 sockets, and a 15-step layout launching a real window
        // per step at 3 and 3. Pinned by tests/live_sway.rs's
        // `event_reader_threads_stay_bounded_across_a_long_layout`.
        //
        // An earlier version of this comment claimed the opposite ("for a large
        // generated layout ... this accumulates rather than staying bounded")
        // and was believed for long enough to reach an external reviewer, who
        // reasonably rated it a high-severity resource leak on the strength of
        // it. The bound is a real property worth stating, but note it depends
        // on later events arriving: the *last* event-confirmed action's reader
        // does stay blocked until the process exits, which is why the bound is
        // a small constant rather than zero.
        //
        // That last reader was raised again (2026-09-02) as a lifetime problem
        // rather than a leak: an operation returns while resources it opened
        // are still held, released only by process exit. Accurate, and left as
        // is. The suggested fix — one owned event reader per SwayLaunch run,
        // with an explicit shutdown protocol — buys nothing for a CLI that
        // exits moments later, and would replace a per-action subscription
        // (which is also what makes each action's wait independent, see
        // Scratchpad's shared WindowChange::Move in
        // matching_window_change_events()) with shared mutable state to
        // coordinate. Revisit if this ever becomes a library, gains a
        // long-lived mode, or needs cancellation — at which point the reader's
        // lifetime stops being bounded by the process's.
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
                    // The event identifies the right container and the right
                    // event type, which is not the same as the requested state
                    // having been what applied — another IPC client driving the
                    // same window produces events indistinguishable from ours.
                    // See state_satisfied()'s doc comment.
                    if !self.state_satisfied()? {
                        if self.verbose() {
                            eprintln!(
                                "Event match: {:?} container id {} ({}), but the requested state \
                                 is not in effect — another client may be driving this window; \
                                 waiting for the next event",
                                window.change, window.container.id, result
                            );
                        }
                        continue;
                    }

                    if self.verbose() {
                        eprintln!(
                            "Event match: {:?} container id {} ({})",
                            window.change, window.container.id, result
                        );
                    }

                    return Ok((window.container.id, ActionOutcome::Changed));
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

/// Finds exactly one already-open window matching `app_id_match`/
/// `class_match`/`mark_match` via `get_tree()`, for `Target::Existing`.
/// Errors — rather than silently picking one — if zero or more than one
/// window matches, since guessing which of several matches the caller meant
/// would be a worse default than asking them to retarget with `--con-id`.
fn find_existing_container_id(
    app_id_match: &str,
    class_match: &str,
    mark_match: &str,
) -> Result<i64, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    let criteria = if !app_id_match.is_empty() {
        format!("app_id \"{}\"", app_id_match)
    } else if !class_match.is_empty() {
        format!("class \"{}\"", class_match)
    } else {
        format!("mark \"{}\"", mark_match)
    };

    resolve_matches(
        matching_container_ids(&tree, app_id_match, class_match, mark_match),
        &criteria,
    )
}

/// Reads `container_id`'s `ContainerState`, or `None` if it isn't in the tree.
///
/// Unlike `node_by_id()` (used by the poll-then-fallback machinery, where a
/// transient IPC failure is deliberately swallowed into "not confirmed yet"),
/// this propagates a genuine connection/`get_tree()` failure as an error: it's
/// used to check state *before* deciding whether to act, where silently
/// treating a real IPC failure as "not already there" would let a later step
/// fail with a confusing timeout instead of surfacing the actual problem
/// immediately.
fn container_state(container_id: i64) -> Result<Option<ContainerState>, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    Ok(ContainerState::from_tree(&tree, container_id))
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
    // One connection for both reads. They answer halves of a single question,
    // and the early return below means the tree fetch only happens on a
    // multi-output setup anyway — same reasoning as `run_wait_time()`'s shared
    // poll connection, just without a loop to amplify the cost.
    let mut connection = new_connection()?;

    let outputs = match connection.get_outputs() {
        Ok(outputs) => outputs,
        Err(error) => return Err(ipc_error(error)),
    };
    if outputs.len() < 2 {
        return Ok(false);
    }

    let tree = match connection.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    Ok(is_at_the_trailing_workspace_edge(
        &tree,
        container_id,
        direction,
    ))
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
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
    Ok(contains_id(&tree, container_id))
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
fn parent_node_layout(connection: &mut Connection, container_id: i64) -> Option<NodeLayout> {
    let tree = connection.get_tree().ok()?;
    find_parent_layout(&tree, container_id)
}

/// The tree node with id `container_id`, or `None` if it can't be read
/// (transient IPC error, or the container's gone) — used by
/// `SwayAction::poll_matches()`'s `Height`/`Width`/`Position` arms to read
/// a window's own current geometry, as opposed to `parent_node_layout()`,
/// which reads its *parent's* state for `Split`.
fn node_by_id(connection: &mut Connection, container_id: i64) -> Option<Node> {
    let tree = connection.get_tree().ok()?;
    find_node(&tree, container_id).cloned()
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
fn position_matches(connection: &mut Connection, container_id: i64, position: &Position) -> bool {
    let Some((node, output_name)) = self::node_and_output_name(connection, container_id) else {
        return false;
    };
    // Only `center` needs the output's own geometry, so an explicit
    // `<x>,<y>` costs one tree read per poll iteration rather than a tree
    // read plus a get_outputs().
    let output_rect = match position {
        Position::Center => output_name
            .as_deref()
            .and_then(|name| self::output_rect(connection, name)),
        Position::Coordinates { .. } => None,
    };
    let Some(expected) = expected_position(position, &node, output_rect) else {
        return false;
    };
    node_position(&node) == expected
}

/// `container_id`'s tree node together with the name of the output
/// containing it (`None` if it isn't on any output, e.g. the scratchpad),
/// read via a single `get_tree()` call — used by `position_matches()`
/// rather than combining `node_by_id()` with the existing `current_output()`
/// helper, which would cost a second, redundant tree fetch per poll
/// iteration.
fn node_and_output_name(
    connection: &mut Connection,
    container_id: i64,
) -> Option<(Node, Option<String>)> {
    let tree = connection.get_tree().ok()?;
    let node = find_node(&tree, container_id)?.clone();
    let output_name = find_containing_name(&tree, container_id, NodeType::Output, None);
    Some((node, output_name))
}

/// The geometry of the output named `output_name`, or `None` if it can't be
/// read or no output has that name.
fn output_rect(connection: &mut Connection, output_name: &str) -> Option<swayipc::Rect> {
    let outputs = connection.get_outputs().ok()?;
    outputs
        .into_iter()
        .find(|output| output.name == output_name)
        .map(|output| output.rect)
}

/// What `SwayLaunch::run()` should act on: launch a new window (the
/// original, still-default behavior), a specific already-open window by
/// container id, or an already-open window found by matching
/// `app_id_match`/`class_match`/`mark_match` against currently open windows.
pub enum Target<'a> {
    Exec { command: &'a str },
    ConId(i64),
    Existing,
}

pub struct SwayLaunch<'a> {
    pub target: Target<'a>,

    pub app_id_match: &'a str,
    pub class_match: &'a str,
    pub mark_match: &'a str,

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

        for (i, event) in event_loop(&subscriptions)?.enumerate() {
            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            // Written through `writeln!` rather than `println!` so a closed
            // stdout is an ordinary end-of-output rather than a panic: this
            // is the one mode that writes until killed, so
            // `--debug-events | head` is a normal way to use it, and Rust
            // ignores SIGPIPE. Reported as success — the reader got what it
            // asked for and went away.
            let written = writeln!(std::io::stdout(), "Event: {}\n{:?}\n", i, event);
            if let Err(error) = written {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(format!("failed writing to stdout: {}", error));
            }
        }

        Ok(())
    }

    /// The container every action in this run will target, plus how it was
    /// arrived at — `Some(..)` only for `Target::Exec`, the one mode that
    /// creates a window rather than picking an existing one out.
    fn resolve_container_id(&self) -> Result<(i64, Option<LaunchOwnership>), String> {
        match self.target {
            Target::Exec { command } => SwayAction::Exec {
                command,
                app_id_match: self.app_id_match,
                class_match: self.class_match,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()
            .map(|result| (result.container_id, result.launch_ownership)),
            Target::ConId(container_id) => Ok((container_id, None)),
            Target::Existing => self::find_existing_container_id(
                self.app_id_match,
                self.class_match,
                self.mark_match,
            )
            .map(|container_id| (container_id, None)),
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
    ///
    /// Each entry the guard above skips is interleaved in-place as
    /// `PlannedAction::Skip`, alongside why — `--verbose` already logged
    /// this via `eprintln!` before this existed; this is what lets `run()`
    /// also surface it in `RunOutcome`/`--json`, in its correct position in
    /// the fixed order, instead of a skip being visible only in a
    /// `--verbose` log line. Never appears when `check_relocation` is
    /// `false` (a preview has nothing to actually check).
    fn build_actions(
        &self,
        container_id: i64,
        check_relocation: bool,
    ) -> Result<Vec<PlannedAction<'a>>, String> {
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
                actions.push(PlannedAction::Skip(SkippedAction {
                    action: "new_column",
                    reason: "trailing_workspace_edge",
                }));
            } else {
                actions.push(PlannedAction::Run(SwayAction::NewColumn {
                    container_id,
                    verbose: self.verbose,
                    wait_time: self.wait_time,
                }));
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
                actions.push(PlannedAction::Skip(SkippedAction {
                    action: "new_row",
                    reason: "trailing_workspace_edge",
                }));
            } else {
                actions.push(PlannedAction::Run(SwayAction::NewRow {
                    container_id,
                    verbose: self.verbose,
                    wait_time: self.wait_time,
                }));
            }
        }
        if let Some(workspace) = self.workspace {
            actions.push(PlannedAction::Run(SwayAction::Workspace {
                container_id,
                workspace,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
        }
        if let Some(output) = self.output {
            actions.push(PlannedAction::Run(SwayAction::Output {
                container_id,
                output,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
        }
        if let Some(split) = self.split {
            actions.push(PlannedAction::Run(SwayAction::Split {
                container_id,
                split,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }));
        }
        if self.floating {
            actions.push(PlannedAction::Run(SwayAction::Floating {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
        }
        if self.sticky {
            actions.push(PlannedAction::Run(SwayAction::Sticky {
                container_id,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }));
        }
        if self.fullscreen {
            actions.push(PlannedAction::Run(SwayAction::Fullscreen {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
        }
        if self.focus {
            actions.push(PlannedAction::Run(SwayAction::Focus {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
        }
        if let Some(height) = self.height {
            actions.push(PlannedAction::Run(SwayAction::Height {
                container_id,
                height,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }));
        }
        if let Some(width) = self.width {
            actions.push(PlannedAction::Run(SwayAction::Width {
                container_id,
                width,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }));
        }
        if let Some(position) = self.position {
            actions.push(PlannedAction::Run(SwayAction::Position {
                container_id,
                position,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }));
        }
        if !self.mark.is_empty() {
            actions.push(PlannedAction::Run(SwayAction::Mark {
                container_id,
                mark: self.mark,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
        }
        if self.scratchpad {
            actions.push(PlannedAction::Run(SwayAction::Scratchpad {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }));
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
    /// can. `check_relocation: false` never produces a `PlannedAction::Skip`
    /// entry, so filtering down to just the `Run` half loses nothing here.
    pub fn build_actions_for_preview(&self) -> Vec<SwayAction<'a>> {
        self.build_actions(0, false)
            .expect(
                "check_relocation: false means build_actions() makes no IPC call, so it can't fail",
            )
            .into_iter()
            .filter_map(|planned| match planned {
                PlannedAction::Run(action) => Some(action),
                PlannedAction::Skip(_) => None,
            })
            .collect()
    }

    pub fn run(&self) -> Result<RunOutcome, String> {
        let (container_id, launch_ownership) = self.resolve_container_id()?;

        if self.verbose {
            eprintln!("Target container id: {}", container_id);
        }

        let mut actions = Vec::new();
        for planned in self.build_actions(container_id, true)? {
            let record = match planned {
                PlannedAction::Run(action) => {
                    let verb = action.sway_command_verb();
                    let result = action.run()?;
                    ActionRecord {
                        action: verb,
                        status: match result.outcome {
                            ActionOutcome::Changed => ActionStatus::Changed,
                            ActionOutcome::AlreadySatisfied => ActionStatus::AlreadySatisfied,
                            ActionOutcome::Unconfirmed => ActionStatus::Unconfirmed,
                        },
                    }
                }
                PlannedAction::Skip(skipped) => ActionRecord {
                    action: skipped.action.to_string(),
                    status: ActionStatus::Skipped {
                        reason: skipped.reason,
                    },
                },
            };
            actions.push(record);
        }

        Ok(RunOutcome {
            container_id,
            actions,
            launch_ownership,
        })
    }
}

/// `SwayLaunch::run()`'s result: the resolved container id, plus every
/// planned action's outcome, in the fixed order `build_actions()` produced
/// it in — a real run's richer `--json` shape (`main.rs`) reports this
/// alongside `container_id`. Since `run()` stops at the first action that
/// fails, `actions` on a successful `Ok` is always the *complete* planned
/// list, not a partial one — a failed action never produces an `ActionRecord`
/// at all; it's reported as `run()`'s `Err` instead, same as before this
/// existed.
pub struct RunOutcome {
    pub container_id: i64,
    pub actions: Vec<ActionRecord>,
    /// How `container_id` came to be this run's target — `None` for a window
    /// that was retargeted rather than launched (`--con-id`/`--existing`, and
    /// a layout step's `target_id`). Only `Some(LaunchOwnership::Launched)`
    /// means this invocation can prove the window is its own, which is what
    /// `main.rs`'s `--rollback-on-error` keys its destructive cleanup off.
    pub launch_ownership: Option<LaunchOwnership>,
}

/// Whether a launched window is one this invocation can prove it created.
///
/// `SwayAction::Exec` tags the process it spawns with a per-invocation marker
/// and prefers a window whose pid carries it, but deliberately falls back to a
/// content match (app_id/class) when no marked window appears — see
/// `run_wait_matching_exec_event()`. That fallback covers two very different
/// situations, and only one of them is evidence of ownership:
///
/// - The marked process is *gone*. Nothing marker-confirmed can still be
///   coming, and a matching window appeared anyway — the single-instance
///   application case (a browser or editor forwarding the request to an
///   already-running process, which then maps the window). Our command caused
///   that window, as directly as can be observed from outside: `Launched`.
/// - The marked process is still *running* and the grace period simply
///   elapsed. The matching window came from somewhere else — another
///   `sway-launch`, another launcher, a user opening something at the wrong
///   moment — and this invocation adopted it only because it matched the
///   requested app_id/class: `Adopted`.
///
/// The distinction exists because `--rollback-on-error` kills windows, and a
/// kill can't be undone. Reporting an adopted window as this run's own would
/// mean destroying a window some other process launched, on the strength of a
/// match this code already knows it couldn't confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOwnership {
    Launched,
    Adopted,
}

/// One entry in `RunOutcome.actions`: either an action that actually ran
/// (`action` is `SwayAction::sway_command_verb()`, the same
/// container-id-free text `--dry-run` prints), or one `build_actions()`
/// chose not to run at all (the multi-output relocation guard — see its own
/// doc comment; `action` is then the short, stable, machine-readable flag
/// name `SkippedAction.action` already used, not a Sway command verb, since
/// a skipped action was never turned into a real `SwayAction` to have one).
/// Previously a skip was visible only via a `--verbose` log line or a
/// separate `"skipped"` field; folding both into one `actions` list,
/// alongside the `Changed`/`AlreadySatisfied` distinction
/// `SwayAction::already_at_target()` already computed internally but
/// discarded, is what makes a `--json` caller able to tell all three
/// outcomes apart in their actual run order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRecord {
    pub action: String,
    pub status: ActionStatus,
}

/// What actually happened to one planned action. `reason` on `Skipped` is a
/// short, stable, machine-readable identifier (not prose) — the same spirit
/// as `SwayAction::sway_command_verb()`'s stable text, not a human-facing
/// message (that's what the existing `--verbose` `eprintln!` next to each
/// skip is for). Only one skip mechanism exists today (the multi-output
/// relocation guard), so `reason` is a plain `&'static str` rather than a
/// nested enum — revisit if a second one ever appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    Changed,
    AlreadySatisfied,
    Unconfirmed,
    Skipped { reason: &'static str },
}

/// What one `SwayAction::run()` call reports back: which container it acted
/// on, how confidently, and — for `Exec` alone — whether the window it settled
/// on is one this invocation can prove it launched. Every other action targets
/// a container that was already resolved before it ran, so it has no launch to
/// claim and leaves `launch_ownership` unset.
struct ActionResult {
    container_id: i64,
    outcome: ActionOutcome,
    launch_ownership: Option<LaunchOwnership>,
}

impl ActionResult {
    /// The result of an action applied to an already-resolved container, which
    /// is every action except `Exec`.
    fn acted(container_id: i64, outcome: ActionOutcome) -> Self {
        ActionResult {
            container_id,
            outcome,
            launch_ownership: None,
        }
    }
}

/// What `SwayAction::run()` observed, before `SwayLaunch::run()` folds it
/// together with the skip case into an `ActionStatus`. Separate from
/// `ActionStatus` because a skip is decided while *planning* and never reaches
/// `run()` at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionOutcome {
    /// The change was observed: a confirmation event arrived and the requested
    /// state was in effect, or a poll saw it take hold.
    Changed,
    /// The state already held, so the command was never sent.
    AlreadySatisfied,
    /// The command was sent and the wait elapsed, but the change was never
    /// observed. Not an error — several wait-time actions have legitimate
    /// outcomes where the expected state never arrives (a solo window's resize
    /// is silently clamped by Sway, a move at the edge of a workspace is a
    /// no-op), and failing on those would turn a working layout into a broken
    /// one. It is, however, a weaker claim than `Changed`, and saying so is
    /// the point.
    Unconfirmed,
}

/// One `SwayAction` `SwayLaunch::build_actions()` planned to run, or one it
/// decided to skip instead (interleaved in the same `Vec` so the overall
/// fixed order survives — see `build_actions()`'s doc comment for why a
/// skip needs a defined position at all, not just a separate side list).
#[derive(Debug)]
enum PlannedAction<'a> {
    Run(SwayAction<'a>),
    Skip(SkippedAction),
}

/// One action `SwayLaunch::build_actions()` decided not to include in the
/// plan, and why — see `ActionStatus::Skipped`, which is what actually
/// carries this into `RunOutcome`/`--json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SkippedAction {
    action: &'static str,
    reason: &'static str,
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

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

    // SwayAction::poll_grace

    #[test]
    fn move_actions_poll_for_a_shorter_grace_than_everything_else() {
        // NewColumn/NewRow confirm on "the rect changed at all", which can't
        // distinguish this action's own move from another client's change to
        // the same window — so the window in which that confusion is possible
        // is deliberately much shorter for them. See MOVE_POLL_GRACE.
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
        let split = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };

        assert_eq!(new_column.poll_grace(), MOVE_POLL_GRACE);
        assert_eq!(new_row.poll_grace(), MOVE_POLL_GRACE);
        assert_eq!(split.poll_grace(), WAIT_TIME_POLL_GRACE);
        assert!(MOVE_POLL_GRACE < WAIT_TIME_POLL_GRACE);
    }

    #[test]
    fn every_other_polling_variant_keeps_the_full_grace() {
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

        for action in [height, width, position, sticky] {
            assert_eq!(action.poll_grace(), WAIT_TIME_POLL_GRACE);
        }
    }

    // SwayAction::polls
    //
    // These assert which variants opt into polling at all — a pure decision
    // with no IPC. They previously went through poll_matches(), whose
    // Some/None return carried the same information but needed a live tree
    // read to produce it, so headlessly they could only ever observe
    // Some(false) and asserted the opt-in indirectly. The match outcome
    // itself still needs a real compositor: see tests/live_sway.rs.

    #[test]
    fn split_polls() {
        for split in [Split::H, Split::V] {
            let action = SwayAction::Split {
                container_id: 42,
                split,
                verbose: false,
                wait_time: time::Duration::from_millis(20),
            };
            assert!(action.polls(None));
        }
    }

    #[test]
    fn new_column_and_new_row_do_not_poll_without_a_baseline() {
        // Unlike Split/Height/Width/Position, NewColumn/NewRow have no fixed
        // target to check against — without a poll_baseline() snapshot to
        // compare the current rect to there is nothing to poll for, so these
        // opt out entirely.
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
        assert!(!new_column.polls(None));
        assert!(!new_row.polls(None));
    }

    #[test]
    fn new_column_and_new_row_poll_given_a_baseline() {
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
        let baseline = Some(rect(0, 0, 640, 720));
        assert!(new_column.polls(baseline));
        assert!(new_row.polls(baseline));
    }

    #[test]
    fn height_and_width_in_pixels_poll() {
        let height = SwayAction::Height {
            container_id: 42,
            height: Size::Pixels(300),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let width = SwayAction::Width {
            container_id: 42,
            width: Size::Pixels(400),
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert!(height.polls(None));
        assert!(width.polls(None));
    }

    #[test]
    fn height_and_width_in_percent_do_not_poll() {
        // A `ppt` value has no pixel figure to poll for without also
        // resolving the reference dimension it's a percentage of (see Size's
        // doc comment), so these opt out rather than polling for something
        // that could never match.
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
        assert!(!height.polls(None));
        assert!(!width.polls(None));
    }

    #[test]
    fn sticky_polls() {
        let action = SwayAction::Sticky {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert!(action.polls(None));
    }

    #[test]
    fn position_polls() {
        for position in [Position::Center, Position::Coordinates { x: 100, y: 200 }] {
            let action = SwayAction::Position {
                container_id: 42,
                position,
                verbose: false,
                wait_time: time::Duration::from_millis(20),
            };
            assert!(action.polls(None));
        }
    }

    #[test]
    fn event_confirmed_actions_do_not_poll() {
        // The `_ => false` arm: everything dispatched through an IPC event
        // confirms that way instead and never enters the poll loop.
        let floating = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let mark = SwayAction::Mark {
            container_id: 42,
            mark: "pinned",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert!(!floating.polls(None));
        assert!(!mark.polls(None));
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

    // SwayAction::state_satisfies

    fn state_with(mutate: impl FnOnce(&mut ContainerState)) -> ContainerState {
        let mut state = ContainerState::default();
        mutate(&mut state);
        state
    }

    fn workspace_action(workspace: &str) -> SwayAction<'_> {
        SwayAction::Workspace {
            container_id: 42,
            workspace,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        }
    }

    fn output_action(output: &str) -> SwayAction<'_> {
        SwayAction::Output {
            container_id: 42,
            output,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        }
    }

    fn mark_action(mark: &str) -> SwayAction<'_> {
        SwayAction::Mark {
            container_id: 42,
            mark,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        }
    }

    fn floating_action() -> SwayAction<'static> {
        SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        }
    }

    #[test]
    fn state_satisfies_workspace_only_for_the_requested_workspace() {
        let state = state_with(|state| state.workspace = Some("3".to_string()));

        assert!(workspace_action("3").state_satisfies(Some(&state)));
        assert!(!workspace_action("2").state_satisfies(Some(&state)));
    }

    #[test]
    fn state_satisfies_workspace_false_when_the_workspace_is_unknown() {
        let state = ContainerState::default();
        assert!(!workspace_action("3").state_satisfies(Some(&state)));
    }

    #[test]
    fn state_satisfies_output_only_for_the_requested_output() {
        let state = state_with(|state| state.output = Some("HEADLESS-1".to_string()));

        assert!(output_action("HEADLESS-1").state_satisfies(Some(&state)));
        assert!(!output_action("HEADLESS-2").state_satisfies(Some(&state)));
    }

    #[test]
    fn state_satisfies_floating_fullscreen_focus_and_scratchpad_read_their_own_flag() {
        let floating = state_with(|state| state.floating = true);
        let fullscreen = state_with(|state| state.fullscreen = true);
        let focused = state_with(|state| state.focused = true);
        let scratchpadded = state_with(|state| state.in_scratchpad = true);

        let fullscreen_action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let focus_action = SwayAction::Focus {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let scratchpad_action = SwayAction::Scratchpad {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };

        assert!(floating_action().state_satisfies(Some(&floating)));
        assert!(!floating_action().state_satisfies(Some(&fullscreen)));
        assert!(fullscreen_action.state_satisfies(Some(&fullscreen)));
        assert!(!fullscreen_action.state_satisfies(Some(&floating)));
        assert!(focus_action.state_satisfies(Some(&focused)));
        assert!(!focus_action.state_satisfies(Some(&floating)));
        assert!(scratchpad_action.state_satisfies(Some(&scratchpadded)));
        assert!(!scratchpad_action.state_satisfies(Some(&floating)));
    }

    #[test]
    fn state_satisfies_mark_matches_any_held_mark() {
        let state = state_with(|state| {
            state.marks = vec!["other".to_string(), "dropdown-term".to_string()]
        });

        assert!(mark_action("dropdown-term").state_satisfies(Some(&state)));
        assert!(!mark_action("missing").state_satisfies(Some(&state)));
    }

    #[test]
    fn state_satisfies_is_false_for_an_action_with_no_state_to_check() {
        let state = state_with(|state| state.floating = true);
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };

        assert!(!action.state_satisfies(Some(&state)));
    }

    #[test]
    fn state_satisfies_is_false_when_the_container_is_gone() {
        // A window that has closed isn't floating, focused, or on the target
        // workspace — short-circuiting an action into success over a container
        // that no longer exists would be worse than letting the command fail.
        assert!(!floating_action().state_satisfies(None));
        assert!(!workspace_action("3").state_satisfies(None));
        assert!(!mark_action("dropdown-term").state_satisfies(None));
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
            mark_match: "",
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
            .map(|planned| match planned {
                PlannedAction::Run(SwayAction::Workspace { .. }) => "workspace",
                PlannedAction::Run(SwayAction::Output { .. }) => "output",
                PlannedAction::Run(SwayAction::Split { .. }) => "split",
                PlannedAction::Run(SwayAction::Floating { .. }) => "floating",
                PlannedAction::Run(SwayAction::Sticky { .. }) => "sticky",
                PlannedAction::Run(SwayAction::Fullscreen { .. }) => "fullscreen",
                PlannedAction::Run(SwayAction::Focus { .. }) => "focus",
                PlannedAction::Run(SwayAction::Height { .. }) => "height",
                PlannedAction::Run(SwayAction::Width { .. }) => "width",
                PlannedAction::Run(SwayAction::Position { .. }) => "position",
                PlannedAction::Run(SwayAction::Mark { .. }) => "mark",
                PlannedAction::Run(SwayAction::Scratchpad { .. }) => "scratchpad",
                other => panic!("unexpected planned action: {:?}", other),
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
        for planned in &actions {
            match planned {
                PlannedAction::Run(action) => assert_eq!(action.container_id(), Some(42)),
                PlannedAction::Skip(skipped) => panic!("unexpected skip: {:?}", skipped),
            }
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
            .any(|planned| matches!(planned, PlannedAction::Run(SwayAction::Mark { .. }))));
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
            .any(|planned| matches!(planned, PlannedAction::Run(SwayAction::NewColumn { .. }))));
        assert!(actions
            .iter()
            .any(|planned| matches!(planned, PlannedAction::Run(SwayAction::NewRow { .. }))));
        assert!(!actions
            .iter()
            .any(|planned| matches!(planned, PlannedAction::Skip(_))));
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
