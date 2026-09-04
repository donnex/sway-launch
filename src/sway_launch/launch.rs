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

use super::action::SwayAction;
use super::ipc::event_loop;
use super::outcome::{
    ActionOutcome, ActionRecord, ActionStatus, LaunchOwnership, PlannedAction, RunOutcome,
    SkippedAction,
};
use super::query::{find_existing_container_id, relocates_to_another_output};
use super::tree::MoveDirection;
use super::values::{Position, Size, Split};
use std::io::Write;
use std::time;
use swayipc::EventType;

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
            Target::Existing => {
                find_existing_container_id(self.app_id_match, self.class_match, self.mark_match)
                    .map(|container_id| (container_id, None))
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
            if check_relocation && relocates_to_another_output(container_id, MoveDirection::Right)?
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
            if check_relocation && relocates_to_another_output(container_id, MoveDirection::Down)? {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    // build_actions()'s error propagation when relocates_to_another_output()
    // can't read the tree is covered by tests/json_output.rs's
    // new_column_and_new_row_report_the_relocation_checks_error, not here.
    // Two unit tests used to assert it directly, by assuming the machine
    // running them had no reachable Sway socket — which is true on a CI runner
    // and false in a Sway session, where both failed outright. A unit test
    // can't state that precondition safely (std::env::set_var is
    // process-global, and this harness is multi-threaded), so the assertion
    // moved to a test that can set it per-invocation on the child process.
    //
    // The guard's actual behavior (skip vs. include) needs a live compositor
    // either way: see tests/live_sway.rs's new_column_does_not_relocate_*.

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
