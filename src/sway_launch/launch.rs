//! Turning one invocation into an ordered plan, and running it.
//!
//! `SwayLaunch` is a whole invocation: what to target (launch it, or retarget
//! an existing window), which actions were asked for, and the settings they
//! share. `build_actions()` turns that into a fixed-order plan without running
//! any of it — which is what makes `--dry-run` possible and the ordering
//! unit-testable — and `run()` executes the plan, folding each action's
//! outcome into the `RunOutcome` `main.rs` reports.
//!
//! The order actions run in is defined here, and it is deliberate rather than
//! incidental; see `build_actions()`.

use super::action::{ActionKind, ExecAction, SwayAction};
use super::ipc::event_loop;
use super::ipc::{quote_sway_string, run_sway_command};
use super::outcome::{
    ActionOutcome, ActionRecord, ActionStatus, LaunchOwnership, RunError, RunOutcome,
};
use super::query::{
    current_move_skip_reason, ensure_con_id_target, find_existing_container_id,
    focused_workspace_name,
};
use super::target::Target;
use super::tree::MoveDirection;
use super::values::{Position, Size, Split};
use std::io::Write;
use std::time;
use swayipc::EventType;

/// Every event type `--debug-events` subscribes to, which its help text
/// promises is all of them. Mirrors `swayipc`'s `EventType` by hand: that enum
/// is `#[non_exhaustive]`, so nothing outside the crate can match on it
/// exhaustively, and a variant added upstream compiles here silently. Recheck
/// this list against it whenever `swayipc` is bumped (`Output` was missing
/// once, so `--debug-events` never showed an output being added or removed).
pub(super) const DEBUG_EVENT_SUBSCRIPTIONS: [EventType; 10] = [
    EventType::Workspace,
    EventType::Output,
    EventType::Mode,
    EventType::Window,
    EventType::BarConfigUpdate,
    EventType::Binding,
    EventType::Shutdown,
    EventType::Tick,
    EventType::BarStateUpdate,
    EventType::Input,
];

pub struct SwayLaunch<'a> {
    pub target: Target<'a>,

    pub split: Option<Split>,
    pub floating: bool,
    pub sticky: bool,
    pub fullscreen: bool,
    pub focus: bool,
    pub mark: Option<&'a str>,
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

    /// Fail a launch if focus left the workspace it started on before its
    /// window appeared — `--layout`/`--template` set this, since their shapes
    /// are built where focus is, and every step launched after such a switch
    /// would open on the other workspace, splitting the shape in two.
    pub require_launch_on_focused_workspace: bool,
}

/// Where a `--layout`/`--template` run builds: a workspace and/or output
/// focused once before the first step, so every window opens there and the
/// shape is built in place. Moving each window there after building it
/// scrambled the shape, since a moved window lands next to whatever the
/// target workspace has focused.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildLocation<'a> {
    pub workspace: Option<&'a str>,
    pub output: Option<&'a str>,
}

impl BuildLocation<'_> {
    /// The Sway commands that focus this location, in order: the output
    /// first, so a new workspace is created there.
    pub fn commands(&self) -> Vec<String> {
        let output = self
            .output
            .map(|output| format!("focus output {}", quote_sway_string(output)));
        let workspace = self
            .workspace
            .map(|workspace| format!("workspace {}", quote_sway_string(workspace)));
        output.into_iter().chain(workspace).collect()
    }

    pub fn apply(&self, verbose: bool) -> Result<(), String> {
        for command in self.commands() {
            if verbose {
                diagnostic!(
                    "Sway command: {}",
                    super::values::escape_control_characters(&command, false)
                );
            }
            run_sway_command(&command).map_err(|error| format!("{}: {}", command, error))?;
        }
        Ok(())
    }
}

/// `--debug-events`: subscribes to every event type and prints each event
/// until interrupted. A free function rather than a `SwayLaunch` method
/// because it acts on no window and reads none of an invocation's settings,
/// so `main()` dispatches it with the other standalone modes, before any
/// target is resolved.
pub fn debug_events() -> Result<(), String> {
    for (i, event) in event_loop(&DEBUG_EVENT_SUBSCRIPTIONS)?.enumerate() {
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

impl<'a> SwayLaunch<'a> {
    /// The container every action in this run will target, plus how it was
    /// arrived at — `Some(..)` only for `Target::Exec`, the one mode that
    /// creates a window rather than picking an existing one out.
    fn resolve_container_id(&self) -> Result<(i64, Option<LaunchOwnership>), RunError> {
        match self.target {
            Target::Exec { command, filter } => ExecAction {
                command,
                filter,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()
            .map(|(container_id, ownership)| (container_id, Some(ownership))),
            Target::ConId(container_id) => Ok((container_id, None)),
            Target::Existing(window_match) => find_existing_container_id(window_match)
                .map(|container_id| (container_id, None))
                .map_err(RunError::from),
        }
    }

    /// Builds the sequence of `SwayAction`s `run()` would apply against
    /// `container_id`, in the same fixed order `run()` always used, without
    /// running any of them — the "plan" half of the plan-then-execute split
    /// `run()` itself is the "execute" half of. Exists as its own method
    /// (rather than inlined into `run()`) so `--dry-run` can print the plan
    /// instead of running it.
    ///
    /// Pure: the multi-output/floating guard for `NewColumn`/`NewRow` is not
    /// consulted here but by `execute_plan()`, immediately before each move
    /// is sent. Deciding it while planning meant judging `--new-row` against
    /// a tree its own `--new-column` was about to change.
    /// `--output` alone moves the container to the output's visible
    /// workspace. With `--workspace` too, that would undo the workspace move,
    /// so the workspace itself is moved to the output instead.
    fn output_action(&self) -> Option<ActionKind<'a>> {
        let output = self.output?;
        Some(match self.workspace {
            Some(workspace) => ActionKind::WorkspaceToOutput { workspace, output },
            None => ActionKind::Output(output),
        })
    }

    fn build_actions(&self, container_id: i64) -> Vec<SwayAction<'a>> {
        let requested = [
            self.new_column.then_some(ActionKind::NewColumn),
            self.new_row.then_some(ActionKind::NewRow),
            self.workspace.map(ActionKind::Workspace),
            self.output_action(),
            self.split.map(ActionKind::Split),
            self.floating.then_some(ActionKind::Floating),
            self.sticky.then_some(ActionKind::Sticky),
            self.fullscreen.then_some(ActionKind::Fullscreen),
            self.focus.then_some(ActionKind::Focus),
            self.height.map(ActionKind::Height),
            self.width.map(ActionKind::Width),
            self.position.map(ActionKind::Position),
            self.mark.map(ActionKind::Mark),
            self.scratchpad.then_some(ActionKind::Scratchpad),
        ];
        requested
            .into_iter()
            .flatten()
            .map(|kind| {
                SwayAction::new(
                    container_id,
                    kind,
                    self.verbose,
                    self.timeout,
                    self.wait_time,
                )
            })
            .collect()
    }

    /// The `--dry-run` entry point: every `SwayAction` this `SwayLaunch`
    /// would apply, in order, without a real `container_id` (nothing has
    /// launched) and without touching Sway IPC at all. A preview always
    /// lists `NewColumn`/`NewRow` when the flag is set: whether a real run
    /// skips one depends on the tree at the moment it would be sent.
    pub fn build_actions_for_preview(&self) -> Vec<SwayAction<'a>> {
        self.build_actions(0)
    }

    /// Runs `plan` in order, stopping at the first failure, and records what
    /// happened to each action. Before a `NewColumn`/`NewRow` is sent,
    /// `move_skip_reason` is asked whether that move is safe *now* — after
    /// every earlier action in the plan has run — and a `Some(reason)` skips
    /// it with that reason instead (see `tree::move_skip_reason()`).
    ///
    /// Both the guard and the action runner are parameters only so the order
    /// of checks and runs can be pinned headlessly; `run()` passes the real
    /// `current_move_skip_reason()` and `SwayAction::run()`.
    fn execute_plan(
        &self,
        plan: Vec<SwayAction<'a>>,
        mut move_skip_reason: impl FnMut(MoveDirection) -> Result<Option<&'static str>, String>,
        mut run_action: impl FnMut(&SwayAction<'a>) -> Result<ActionOutcome, RunError>,
    ) -> Result<Vec<ActionRecord>, RunError> {
        let mut records = Vec::new();
        for action in plan {
            let kind = action.kind_name();
            let direction = match action.kind {
                ActionKind::NewColumn => Some(MoveDirection::Right),
                ActionKind::NewRow => Some(MoveDirection::Down),
                _ => None,
            };
            if let Some(direction) = direction {
                if let Some(reason) = move_skip_reason(direction)? {
                    if self.verbose {
                        diagnostic!(
                            "Skipping {}: container id {} {}",
                            kind,
                            action.container_id,
                            skip_explanation(reason, direction)
                        );
                    }
                    records.push(ActionRecord {
                        kind,
                        command: None,
                        status: ActionStatus::Skipped { reason },
                    });
                    continue;
                }
            }

            let command = action.sway_command_verb();
            let outcome = run_action(&action)?;
            records.push(ActionRecord {
                kind,
                command: Some(command),
                status: match outcome {
                    ActionOutcome::Changed => ActionStatus::Changed,
                    ActionOutcome::AlreadySatisfied => ActionStatus::AlreadySatisfied,
                    ActionOutcome::Unconfirmed => ActionStatus::Unconfirmed,
                },
            });
        }
        Ok(records)
    }

    pub fn run(&self) -> Result<RunOutcome, RunError> {
        let launching = matches!(self.target, Target::Exec { .. });
        let expected_workspace = if launching && self.require_launch_on_focused_workspace {
            focused_workspace_name()?
        } else {
            None
        };

        let (container_id, launch_ownership) = self.resolve_container_id()?;

        if self.verbose {
            diagnostic!("Target container id: {}", container_id);
        }

        // From here on a container exists, so every failure carries its id:
        // see RunError's doc comment.
        let resolved = |error: RunError| RunError {
            container_id: Some(container_id),
            launch_ownership,
            ..error
        };

        if let Some(expected) = expected_workspace {
            let focused_now = focused_workspace_name().map_err(|error| resolved(error.into()))?;
            if let Some(message) = focus_moved_during_launch(&expected, focused_now.as_deref()) {
                return Err(resolved(RunError::from(message)));
            }
        }

        // A bare `--con-id N` stays unchecked, so it still works without a
        // compositor; with actions to apply, the socket is needed anyway and
        // a wrong id fails here, before the first one, with a clear reason.
        let plan = self.build_actions(container_id);
        if matches!(self.target, Target::ConId(_)) && !plan.is_empty() {
            ensure_con_id_target(container_id).map_err(|error| resolved(error.into()))?;
        }

        let actions = self
            .execute_plan(
                plan,
                |direction| current_move_skip_reason(container_id, direction),
                SwayAction::run,
            )
            .map_err(resolved)?;

        Ok(RunOutcome {
            container_id,
            actions,
            launch_ownership,
        })
    }
}

/// Why a launch fails when focus was on `expected` as it started and is on
/// `focused_now` once its window appeared, or `None` when it didn't move.
///
/// Sway places a window on the workspace its `exec` ran on, wherever focus
/// has gone since, so the launched window itself is still where it belongs
/// (confirmed live). What a switch breaks is every step after it: those are
/// launched on the new workspace. Checking where the window opened therefore
/// can't see the problem; checking where focus went can.
fn focus_moved_during_launch(expected: &str, focused_now: Option<&str>) -> Option<String> {
    match focused_now {
        Some(workspace) if workspace == expected => None,
        Some(workspace) => Some(format!(
            "focus moved from workspace {:?} to {:?} while this step's window was opening, so \
             later steps would build on the wrong workspace",
            expected, workspace
        )),
        None => Some(format!(
            "focus left workspace {:?} while this step's window was opening",
            expected
        )),
    }
}

/// The `--verbose` prose for one of `tree::move_skip_reason()`'s reasons,
/// completing "container id N ...".
fn skip_explanation(reason: &str, direction: MoveDirection) -> String {
    let command = match direction {
        MoveDirection::Right => "move right",
        MoveDirection::Down => "move down",
    };
    match reason {
        "floating" => format!(
            "is floating — \"{}\" would only nudge it 10 px, not give it a new column or row",
            command
        ),
        "fullscreen" => format!(
            "is fullscreen and more than one output exists — \"{}\" would send it to a \
             different output",
            command
        ),
        _ => format!(
            "is at the trailing edge of its workspace along that axis, and more than one output \
             exists — \"{}\" would relocate it to a different output instead of moving it \
             within the workspace",
            command
        ),
    }
}

#[cfg(test)]
mod tests;
