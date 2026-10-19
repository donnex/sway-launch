//! Turning one invocation into an ordered plan, and running it.
//!
//! `SwayLaunch` is a whole invocation: what to target (launch it, or retarget
//! an existing window), which actions were asked for (its `ActionRequest`),
//! and the settings they share. `build_actions()` turns that into a fixed-order plan without running
//! any of it — which is what makes `--dry-run` possible and the ordering
//! unit-testable — and `run()` executes the plan, folding each action's
//! outcome into the `RunOutcome` `main.rs` reports.
//!
//! The order actions run in is defined here, and it is deliberate rather than
//! incidental; see `build_actions()`.

use super::action::{ActionKind, ExecAction, SwayAction};
use super::confirmation::{run_batch, run_wait_for_existing_window};
use super::ipc::event_loop;
use super::ipc::{quote_sway_string, run_sway_command};
use super::outcome::{
    ActionOutcome, ActionRecord, ActionStatus, LaunchOwnership, RunError, RunOutcome,
};
use super::query::{
    build_workspace_anchor_now, current_move_skip_reason, ensure_con_id_target,
    find_existing_container_id, launch_placement_problems_now, workspace_now,
};
use super::target::Target;
use super::tree::{MoveDirection, SkipReason, WorkspaceIdentity, WorkspaceLookup};
use super::values::{Position, Size, Split, WorkspaceTarget};
use std::io::Write;
use std::time;
use swayipc::{Event, EventType};

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

    /// What to do to the window once it is resolved. Its own struct so a
    /// launch can drop every action at once (its `Default`), as
    /// `--concurrent`'s staging does, with no list of fields to keep in step
    /// with the actions (#180).
    pub actions: ActionRequest<'a>,

    pub verbose: bool,
    pub timeout: time::Duration,
    pub wait_time: time::Duration,
    /// The workspace a `--layout`/`--template` run builds on, recorded once
    /// after `BuildLocation::apply()` (`BuildLocation::anchor()`, #156). A
    /// launch fails if focus isn't on it as the step starts, or left it before
    /// the window appeared: their shapes are built where focus is, and every
    /// step launched after such a switch would open on the other workspace,
    /// splitting the shape in two, or building into another run's.
    pub build_workspace: Option<BuildWorkspace>,

    /// Before the first action, check that the window opened tiled on the
    /// focused workspace, and report in `RunOutcome::warnings` when a config
    /// rule put it elsewhere (#160) — `--layout`/`--template` set this for a
    /// step that launches, since the rest of the shape is built around it.
    pub warn_on_misplaced_launch: bool,

    /// After a launch times out or is interrupted, keep watching for
    /// `LATE_WINDOW_GRACE` for a window carrying this launch's marker, and
    /// report it as the failed run's container (#15, #127). Set by `--rollback-on-error`, whose rollback
    /// then closes it rather than leaving it to open after the run gave up.
    pub catch_late_window: bool,

    /// `cd`/`export` run before a launched command, from `--cwd`/`--env`
    /// (#113), already rendered by `render_launch_setup()`. Only meaningful
    /// with `Target::Exec`; the front ends refuse it otherwise. Sent through
    /// a `SetupFile`, not on the launch's command line (#167).
    pub launch_setup: Option<String>,

    /// Containers `Target::Existing` never resolves to, whatever they match:
    /// a `--concurrent` run's windows still waiting on the staging workspace
    /// for a later step (#190), which a sequential run wouldn't have opened
    /// yet.
    pub excluded_containers: Vec<i64>,
}

/// Every action flag of one invocation, plus `--settle`, which only changes
/// how they are applied: what `build_actions()` plans from. The default asks
/// for nothing.
#[derive(Clone, Debug, Default)]
pub struct ActionRequest<'a> {
    pub split: Option<Split>,
    pub floating: bool,
    pub sticky: bool,
    pub fullscreen: bool,
    pub focus: bool,
    pub mark: Option<&'a str>,
    pub new_column: bool,
    pub new_row: bool,
    pub workspace: Option<WorkspaceTarget<'a>>,
    pub output: Option<&'a str>,
    pub height: Option<Size>,
    pub width: Option<Size>,
    pub position: Option<Position>,
    pub scratchpad: bool,
    /// `--sway-command` (#90), each already checked by
    /// `validate_sway_command()`.
    pub sway_commands: &'a [String],
    /// `--settle` (#17): how long to keep holding the confirmed pixel
    /// height/width and position against the client, if at all. See
    /// `run_batch()`.
    pub settle: Option<time::Duration>,
}

/// Where a `--layout`/`--template` run builds: a workspace and/or output
/// focused once before the first step, so every window opens there and the
/// shape is built in place. Moving each window there after building it
/// scrambled the shape, since a moved window lands next to whatever the
/// target workspace has focused.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildLocation<'a> {
    pub workspace: Option<WorkspaceTarget<'a>>,
    pub output: Option<&'a str>,
}

impl BuildLocation<'_> {
    /// The Sway commands that focus this location, in order: the output
    /// first, so a new workspace is created there.
    pub fn commands(&self) -> Vec<String> {
        let output = self
            .output
            .map(|output| format!("focus output {}", quote_sway_string(output)));
        let workspace = self.workspace.map(|workspace| workspace.focus_command());
        output.into_iter().chain(workspace).collect()
    }

    /// The workspace the run builds on, read once right after `apply()`, and
    /// refused when it isn't the one this location names
    /// (`tree::build_workspace_anchor()`, #156).
    pub fn anchor(&self) -> Result<BuildWorkspace, String> {
        build_workspace_anchor_now(self.workspace, self.output).map(BuildWorkspace)
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

/// A layout run's build workspace, by node id (`WorkspaceIdentity`), as
/// `BuildLocation::anchor()` recorded it: what every launching step and
/// `Staging::enter()` check focus against, rather than against focus as each
/// one starts (#156).
#[derive(Clone, Debug)]
pub struct BuildWorkspace(WorkspaceIdentity);

impl BuildWorkspace {
    /// Fails, naming both workspaces, unless focus is on this workspace now.
    fn check_focus(&self, when: FocusCheck) -> Result<(), String> {
        let focused_now = workspace_now(WorkspaceLookup::Focused)?;
        if focused_now.as_ref().map(|workspace| workspace.id) == Some(self.0.id) {
            return Ok(());
        }
        // Only for the message: the workspace may have been renamed since.
        let expected_now = workspace_now(WorkspaceLookup::Id(self.0.id))
            .ok()
            .flatten()
            .unwrap_or_else(|| self.0.clone());
        match focus_moved_during_launch(&expected_now, focused_now.as_ref(), when) {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }
}

/// When `BuildWorkspace::check_focus()` runs, for its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusCheck {
    BeforeLaunch,
    WhileOpening,
    BeforeStaging,
}

impl FocusCheck {
    fn when(self) -> &'static str {
        match self {
            FocusCheck::BeforeLaunch => "before this step's window was launched",
            FocusCheck::WhileOpening => "while this step's window was opening",
            FocusCheck::BeforeStaging => "before this run's windows were launched",
        }
    }
}

/// Where `--concurrent` (#73) launches a run's windows: a workspace of its
/// own, so they open out of sight of the build workspace and are then moved
/// there one at a time, in step order, each landing where it would have
/// opened. Sway puts a window on the workspace its `exec` ran on, whatever
/// has focus by the time it maps, so the staging workspace holds them even
/// if focus moves meanwhile.
///
/// The build workspace is tracked by its node id, and each command naming it
/// uses the name it has right then (`build_workspace_name()`): a
/// workspace-renaming daemon renames it as the windows arrive, and moving the
/// next one by the old name would make Sway create a second workspace under
/// that name and build the rest of the shape there (#178).
pub struct Staging {
    build_workspace: std::cell::RefCell<WorkspaceIdentity>,
    staging_workspace: String,
}

impl Staging {
    /// Checks focus is still on the run's build workspace, then switches to
    /// a staging workspace unique to this process.
    pub fn enter(build_workspace: &BuildWorkspace, verbose: bool) -> Result<Staging, String> {
        build_workspace.check_focus(FocusCheck::BeforeStaging)?;
        let staging = Staging {
            build_workspace: std::cell::RefCell::new(build_workspace.0.clone()),
            staging_workspace: format!("sway-launch-staging-{}", std::process::id()),
        };
        staging.send(
            &WorkspaceTarget::Name(&staging.staging_workspace).focus_command(),
            verbose,
        )?;
        Ok(staging)
    }

    /// Focuses the build workspace again. Sway destroys an empty workspace
    /// once focus leaves it, so the one focused here can be a new workspace
    /// under the old name; it is the build workspace from now on.
    pub fn leave(&self, verbose: bool) -> Result<(), String> {
        let name = self.build_workspace_name()?;
        self.send(&WorkspaceTarget::Name(&name).focus_command(), verbose)?;
        self.remember(WorkspaceLookup::Named(&name))
    }

    /// The build workspace's name as of now: the current name of the
    /// workspace with the remembered id, or the remembered name once that
    /// workspace is gone (an empty one Sway destroyed when focus left it).
    fn build_workspace_name(&self) -> Result<String, String> {
        let remembered = self.build_workspace.borrow().clone();
        Ok(workspace_now(WorkspaceLookup::Id(remembered.id))?
            .map_or(remembered.name, |workspace| workspace.name))
    }

    /// Makes the workspace `lookup` finds the build workspace, when there is
    /// one.
    fn remember(&self, lookup: WorkspaceLookup) -> Result<(), String> {
        if let Some(workspace) = workspace_now(lookup)? {
            *self.build_workspace.borrow_mut() = workspace;
        }
        Ok(())
    }

    /// Moves a staged window to the build workspace, and focuses it when
    /// `focus` is set, as a newly opened window would be, so the next one
    /// moved there lands after it. A window moved there lands next to the
    /// workspace's focused one, where it would have opened (every built-in
    /// template builds its recorded shape this way, checked live).
    pub fn bring(&self, container_id: i64, focus: bool, verbose: bool) -> Result<(), String> {
        let name = self.build_workspace_name()?;
        self.send(
            &format!(
                "[con_id={}] move container to workspace {}",
                container_id,
                WorkspaceTarget::Name(&name).sway_argument()
            ),
            verbose,
        )?;
        self.remember(WorkspaceLookup::Holding(container_id))?;
        if focus {
            self.send(&format!("[con_id={}] focus", container_id), verbose)?;
        }
        Ok(())
    }

    pub fn staging_workspace(&self) -> &str {
        &self.staging_workspace
    }

    fn send(&self, command: &str, verbose: bool) -> Result<(), String> {
        if verbose {
            diagnostic!(
                "Sway command: {}",
                super::values::escape_control_characters(command, false)
            );
        }
        run_sway_command(command).map_err(|error| format!("{}: {}", command, error))
    }
}

/// `--debug-events`: subscribes to every event type and prints each event
/// until interrupted. A free function rather than a `SwayLaunch` method
/// because it acts on no window and reads none of an invocation's settings,
/// so `main()` dispatches it with the other standalone modes, before any
/// target is resolved.
pub fn debug_events(json: bool) -> Result<(), String> {
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
        let written = if json {
            writeln!(std::io::stdout(), "{}", debug_event_json(&event))
        } else {
            writeln!(std::io::stdout(), "Event: {}\n{:?}\n", i, event)
        };
        if let Err(error) = written {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(format!("failed writing to stdout: {}", error));
        }
    }

    Ok(())
}

/// One event as `--debug-events --json` prints it: the event's payload as
/// `swayipc` models it (so a field it doesn't know is dropped, and one Sway
/// left out reads `null`; `swaymsg -t subscribe -m` shows the raw bytes),
/// with an `event` field naming its type (`window`, `output`,
/// `barconfig_update`, ...), since the payload alone doesn't say. A payload
/// that isn't an object is kept under `payload`.
fn debug_event_json(event: &Event) -> serde_json::Value {
    let name = debug_event_type(event)
        .and_then(|event_type| serde_json::to_value(event_type).ok())
        .and_then(|name| name.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let tagged = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    let payload = match tagged {
        serde_json::Value::Object(mut variant) if variant.len() == 1 => variant
            .values_mut()
            .next()
            .map(serde_json::Value::take)
            .unwrap_or_default(),
        other => other,
    };
    match payload {
        serde_json::Value::Object(fields) => {
            let mut object = serde_json::Map::new();
            object.insert("event".to_string(), serde_json::Value::String(name));
            object.extend(fields);
            serde_json::Value::Object(object)
        }
        other => serde_json::json!({ "event": name, "payload": other }),
    }
}

/// The `EventType` an event arrived as, whose serialization is Sway's own
/// name for it (`barconfig_update`, not the variant's `bar_config_update`).
/// `None` for a variant a newer `swayipc` adds, since `Event` is
/// `#[non_exhaustive]`; recheck this alongside `DEBUG_EVENT_SUBSCRIPTIONS`.
fn debug_event_type(event: &Event) -> Option<EventType> {
    Some(match event {
        Event::Workspace(_) => EventType::Workspace,
        Event::Output(_) => EventType::Output,
        Event::Mode(_) => EventType::Mode,
        Event::Window(_) => EventType::Window,
        Event::BarConfigUpdate(_) => EventType::BarConfigUpdate,
        Event::Binding(_) => EventType::Binding,
        Event::Shutdown(_) => EventType::Shutdown,
        Event::Tick(_) => EventType::Tick,
        Event::BarStateUpdate(_) => EventType::BarStateUpdate,
        Event::Input(_) => EventType::Input,
        _ => return None,
    })
}

impl<'a> SwayLaunch<'a> {
    /// The container every action in this run will target, plus how it was
    /// arrived at — `Some(..)` only for `Target::Exec`, the one mode that
    /// creates a window rather than picking an existing one out.
    fn resolve_container_id(&self) -> Result<(i64, Option<LaunchOwnership>), RunError> {
        match self.target {
            Target::Exec {
                command,
                filter,
                title,
            } => ExecAction {
                command,
                setup: self.launch_setup.as_deref(),
                filter,
                title,
                verbose: self.verbose,
                timeout: self.timeout,
                catch_late_window: self.catch_late_window,
            }
            .run()
            .map(|(container_id, ownership)| (container_id, Some(ownership))),
            Target::ConId(container_id) => Ok((container_id, None)),
            Target::Existing {
                criteria,
                wait: false,
            } => find_existing_container_id(criteria, &self.excluded_containers)
                .map(|container_id| (container_id, None))
                .map_err(RunError::from),
            Target::Existing {
                criteria,
                wait: true,
            } => run_wait_for_existing_window(
                criteria,
                &self.excluded_containers,
                self.timeout,
                self.verbose,
            )
            .map(|container_id| (container_id, None)),
        }
    }

    /// `--output` alone moves the container to the output's visible
    /// workspace. With `--workspace` too, that would undo the workspace move,
    /// so the workspace itself is moved to the output instead.
    fn output_action(&self) -> Option<ActionKind<'a>> {
        let output = self.actions.output?;
        Some(match self.actions.workspace {
            Some(workspace) => ActionKind::WorkspaceToOutput { workspace, output },
            None => ActionKind::Output(output),
        })
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
    fn build_actions(&self, container_id: i64) -> Vec<SwayAction<'a>> {
        let actions = &self.actions;
        let requested = [
            actions.new_column.then_some(ActionKind::NewColumn),
            actions.new_row.then_some(ActionKind::NewRow),
            actions.workspace.map(ActionKind::Workspace),
            self.output_action(),
            actions.split.map(ActionKind::Split),
            actions.floating.then_some(ActionKind::Floating),
            actions.sticky.then_some(ActionKind::Sticky),
            actions.fullscreen.then_some(ActionKind::Fullscreen),
            actions.focus.then_some(ActionKind::Focus),
            actions.height.map(ActionKind::Height),
            actions.width.map(ActionKind::Width),
            actions.position.map(ActionKind::Position),
            actions.mark.map(ActionKind::Mark),
        ];
        let sway_commands = actions
            .sway_commands
            .iter()
            .map(|command| ActionKind::SwayCommand(command));
        requested
            .into_iter()
            .flatten()
            .chain(sway_commands)
            .chain(actions.scratchpad.then_some(ActionKind::Scratchpad))
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

    /// Whether `run()` talks to the compositor at all: everything but a bare
    /// `--con-id N` with no actions, which only prints the id (#107).
    pub fn needs_compositor(&self) -> bool {
        match self.target {
            Target::ConId(container_id) => !self.build_actions(container_id).is_empty(),
            _ => true,
        }
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
    /// happened to each action. Consecutive actions that `joins_batch()` are
    /// handed to `run_actions` together, to be sent as one command (#32);
    /// every other action is handed over alone. Before a `NewColumn`/`NewRow`
    /// is sent, `move_skip_reason` is asked whether that move is safe *now* —
    /// after every earlier action in the plan has run — and a `Some(reason)`
    /// skips it with that reason instead (see `tree::move_skip_reason()`).
    ///
    /// Both the guard and the action runner are parameters only so the order
    /// of checks and runs can be pinned headlessly; `run()` passes the real
    /// `current_move_skip_reason()` and `run_batch()`. `run_actions`
    /// returns one outcome per action it was given.
    fn execute_plan(
        &self,
        plan: Vec<SwayAction<'a>>,
        mut move_skip_reason: impl FnMut(MoveDirection) -> Result<Option<SkipReason>, String>,
        mut run_actions: impl FnMut(&[SwayAction<'a>]) -> Result<Vec<ActionOutcome>, RunError>,
    ) -> Result<Vec<ActionRecord>, RunError> {
        let mut records = Vec::new();
        for group in plan_groups(&plan) {
            if let [action] = group {
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
                                action.kind_name(),
                                action.container_id,
                                reason.explanation(direction)
                            );
                        }
                        records.push(ActionRecord {
                            kind: action.kind_name(),
                            command: None,
                            status: ActionStatus::Skipped { reason },
                        });
                        continue;
                    }
                }
            }

            let outcomes = run_actions(group)?;
            records.extend(
                group
                    .iter()
                    .zip(outcomes)
                    .map(|(action, outcome)| ActionRecord {
                        kind: action.kind_name(),
                        command: Some(action.sway_command_verb()),
                        status: match outcome {
                            ActionOutcome::Changed => ActionStatus::Changed,
                            ActionOutcome::AlreadySatisfied => ActionStatus::AlreadySatisfied,
                            ActionOutcome::Unconfirmed => ActionStatus::Unconfirmed,
                        },
                    }),
            );
        }
        Ok(records)
    }

    pub fn run(&self) -> Result<RunOutcome, RunError> {
        let launching = matches!(self.target, Target::Exec { .. });
        let build_workspace = self.build_workspace.as_ref().filter(|_| launching);
        if let Some(build_workspace) = build_workspace {
            build_workspace.check_focus(FocusCheck::BeforeLaunch)?;
        }

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

        if let Some(build_workspace) = build_workspace {
            build_workspace
                .check_focus(FocusCheck::WhileOpening)
                .map_err(|error| resolved(error.into()))?;
        }

        let warnings = if self.warn_on_misplaced_launch {
            launch_placement_problems_now(
                container_id,
                self.actions.workspace.is_some()
                    || self.actions.output.is_some()
                    || self.actions.scratchpad,
                self.actions.floating || self.actions.scratchpad,
            )
            .map_err(|error| resolved(error.into()))?
        } else {
            Vec::new()
        };

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
                |group| run_batch(group, self.actions.settle),
            )
            .map_err(resolved)?;

        Ok(RunOutcome {
            container_id,
            actions,
            launch_ownership,
            warnings,
        })
    }
}

/// `plan` split into what `execute_plan()` runs at once: each run of
/// consecutive actions whose kind `joins_batch()`, and every other action on
/// its own, in plan order.
fn plan_groups<'p, 'a>(plan: &'p [SwayAction<'a>]) -> Vec<&'p [SwayAction<'a>]> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < plan.len() {
        let end = if plan[start].kind.joins_batch() {
            start
                + plan[start..]
                    .iter()
                    .take_while(|action| action.kind.joins_batch())
                    .count()
        } else {
            start + 1
        };
        groups.push(&plan[start..end]);
        start = end;
    }
    groups
}

/// Why a launch fails when focus should be on the build workspace `expected`
/// and is on `focused_now`, or `None` when it didn't move. Checked as each
/// launching step starts, once its window appeared, and before `--concurrent`
/// staging (`FocusCheck`), always against the workspace the run recorded
/// after focusing its build location (#156), never against focus as the step
/// started: another run focusing its own workspace in between would
/// otherwise have both runs build there.
///
/// Sway places a window on the workspace its `exec` ran on, wherever focus
/// has gone since, so the launched window itself is still where it belongs
/// (confirmed live). What a switch breaks is every step after it: those are
/// launched on the new workspace. Checking where the window opened therefore
/// can't see the problem; checking where focus went can.
///
/// The two are compared by node id, never by name: a workspace-renaming
/// daemon renames the workspace as the window opens, and focus hasn't moved
/// (#178). The message names each workspace as it is called now.
fn focus_moved_during_launch(
    expected: &WorkspaceIdentity,
    focused_now: Option<&WorkspaceIdentity>,
    when: FocusCheck,
) -> Option<String> {
    match focused_now {
        Some(workspace) if workspace.id == expected.id => None,
        Some(workspace) => Some(format!(
            "focus moved from workspace {:?} to {:?} {}, so the layout would be built on the \
             wrong workspace",
            expected.name,
            workspace.name,
            when.when()
        )),
        None => Some(format!(
            "focus left workspace {:?} {}",
            expected.name,
            when.when()
        )),
    }
}

#[cfg(test)]
mod tests;
