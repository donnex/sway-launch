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
use super::outcome::{
    ActionOutcome, ActionRecord, ActionStatus, LaunchOwnership, PlannedAction, RunError,
    RunOutcome, SkippedAction,
};
use super::query::{find_existing_container_id, relocates_to_another_output};
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
}

impl<'a> SwayLaunch<'a> {
    pub fn debug_events(&self) -> Result<(), String> {
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

    /// The container every action in this run will target, plus how it was
    /// arrived at — `Some(..)` only for `Target::Exec`, the one mode that
    /// creates a window rather than picking an existing one out.
    fn resolve_container_id(&self) -> Result<(i64, Option<LaunchOwnership>), RunError> {
        match self.target {
            Target::Exec { command } => ExecAction {
                command,
                app_id_match: self.app_id_match,
                class_match: self.class_match,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()
            .map(|(container_id, ownership)| (container_id, Some(ownership))),
            Target::ConId(container_id) => Ok((container_id, None)),
            Target::Existing => {
                find_existing_container_id(self.app_id_match, self.class_match, self.mark_match)
                    .map(|container_id| (container_id, None))
                    .map_err(RunError::from)
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
    ///
    /// Each entry the guard above skips is interleaved in-place as
    /// `PlannedAction::Skip`, alongside why — `--verbose` already logged
    /// this via `diagnostic!` before this existed; this is what lets `run()`
    /// also surface it in `RunOutcome`/`--json`, in its correct position in
    /// the fixed order, instead of a skip being visible only in a
    /// `--verbose` log line. Never appears when `check_relocation` is
    /// `false` (a preview has nothing to actually check).
    fn build_actions(
        &self,
        container_id: i64,
        check_relocation: bool,
    ) -> Result<Vec<PlannedAction<'a>>, String> {
        let planned = |kind| {
            PlannedAction::Run(SwayAction::new(
                container_id,
                kind,
                self.verbose,
                self.timeout,
                self.wait_time,
            ))
        };
        let mut actions = Vec::new();

        if self.new_column {
            if check_relocation && relocates_to_another_output(container_id, MoveDirection::Right)?
            {
                if self.verbose {
                    diagnostic!(
                        "Skipping new-column: container id {} is at the trailing edge of \
                         its workspace along that axis, and more than one output exists — \
                         \"move right\" would relocate it to a different output instead of \
                         moving it within the workspace",
                        container_id
                    );
                }
                actions.push(PlannedAction::Skip(SkippedAction {
                    kind: ActionKind::NewColumn.flag_name(),
                    reason: "trailing_workspace_edge",
                }));
            } else {
                actions.push(planned(ActionKind::NewColumn));
            }
        }
        if self.new_row {
            if check_relocation && relocates_to_another_output(container_id, MoveDirection::Down)? {
                if self.verbose {
                    diagnostic!(
                        "Skipping new-row: container id {} is at the trailing edge of \
                         its workspace along that axis, and more than one output exists — \
                         \"move down\" would relocate it to a different output instead of \
                         moving it within the workspace",
                        container_id
                    );
                }
                actions.push(PlannedAction::Skip(SkippedAction {
                    kind: ActionKind::NewRow.flag_name(),
                    reason: "trailing_workspace_edge",
                }));
            } else {
                actions.push(planned(ActionKind::NewRow));
            }
        }

        let requested = [
            self.workspace.map(ActionKind::Workspace),
            self.output.map(ActionKind::Output),
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
        actions.extend(requested.into_iter().flatten().map(planned));

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

    pub fn run(&self) -> Result<RunOutcome, RunError> {
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

        let mut actions = Vec::new();
        for planned in self
            .build_actions(container_id, true)
            .map_err(|error| resolved(error.into()))?
        {
            let record = match planned {
                PlannedAction::Run(action) => {
                    let kind = action.kind_name();
                    let command = action.sway_command_verb();
                    let outcome = action.run().map_err(resolved)?;
                    ActionRecord {
                        kind,
                        command: Some(command),
                        status: match outcome {
                            ActionOutcome::Changed => ActionStatus::Changed,
                            ActionOutcome::AlreadySatisfied => ActionStatus::AlreadySatisfied,
                            ActionOutcome::Unconfirmed => ActionStatus::Unconfirmed,
                        },
                    }
                }
                PlannedAction::Skip(skipped) => ActionRecord {
                    kind: skipped.kind,
                    command: None,
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

#[cfg(test)]
mod tests;
