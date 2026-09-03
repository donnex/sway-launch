use std::io::Write;
use std::{fmt, time};
use swayipc::EventType;

mod confirmation;
mod ipc;
mod outcome;
mod process_marker;
mod query;
#[cfg(test)]
mod test_support;
mod tree;
mod values;

pub use ipc::kill_container;
use ipc::{event_loop, quote_sway_string};
use outcome::{ActionOutcome, PlannedAction, SkippedAction};
pub use outcome::{ActionRecord, ActionStatus, LaunchOwnership, RunOutcome};
use query::{find_existing_container_id, relocates_to_another_output};
use tree::MoveDirection;
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
