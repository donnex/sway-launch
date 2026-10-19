//! What an action is, and the Sway command it renders to.
//!
//! Two types, because there are two genuinely different things this tool does
//! to a window. `ExecAction` launches one: it has no container yet, and its
//! whole job is to find the one it creates. `SwayAction` acts on a container
//! that's already resolved: the context every such action shares (the target
//! container, `--verbose`, and how long to wait) lives on the struct once, and
//! what differs between them lives in `ActionKind`, one variant per CLI flag.
//!
//! Keeping them apart is what makes "this action has a container" a fact the
//! compiler checks rather than a promise each call site keeps by reading a
//! comment: there is no `SwayAction` without a `container_id`, and no way to
//! ask an `ExecAction` for one.
//!
//! `sway_command_verb()` is the single place an action's command text is
//! defined, so `--dry-run`'s preview and a real run can't drift apart.
//!
//! How an action's effect is *confirmed* lives in `confirmation.rs`, which
//! holds the rest of both impls.

use super::ipc::quote_sway_string;
use super::process_marker::PID_MARKER_VAR;
use super::target::LaunchFilter;
use super::values::{quote_shell_word, Position, Size, Split, TitlePattern, WorkspaceTarget};
use std::path::Path;
use std::{fmt, time};

/// Launches `command` and waits for the window it opens.
#[derive(Copy, Clone, Debug)]
pub(super) struct ExecAction<'a> {
    pub(super) command: &'a str,
    /// `cd`/`export` run before `command`, from `--cwd`/`--env` (#113).
    pub(super) setup: Option<&'a str>,
    pub(super) filter: Option<LaunchFilter<'a>>,
    /// `--title`: the window's title must match too (#14).
    pub(super) title: Option<&'a TitlePattern>,
    pub(super) verbose: bool,
    pub(super) timeout: time::Duration,
    /// See `SwayLaunch::catch_late_window`.
    pub(super) catch_late_window: bool,
}

/// One action against an already-resolved container.
///
/// `timing` is the `--timeout` for an event-confirmed kind and the
/// `--wait-time` for one that confirms by polling — chosen once, by
/// `SwayAction::new()`, from `ActionKind::is_event_confirmed()`, rather than
/// each kind carrying whichever of the two it happens to use.
#[derive(Copy, Clone, Debug)]
pub struct SwayAction<'a> {
    pub(super) container_id: i64,
    pub(super) verbose: bool,
    pub(super) timing: time::Duration,
    pub(super) kind: ActionKind<'a>,
}

/// What a `SwayAction` does — one variant per CLI flag, holding only what
/// that flag itself specifies.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum ActionKind<'a> {
    Split(Split),
    Floating,
    Sticky,
    Fullscreen,
    Focus,
    NewColumn,
    NewRow,
    Workspace(WorkspaceTarget<'a>),
    Output(&'a str),
    /// `--workspace` and `--output` together: after the window moved to the
    /// workspace, move that whole workspace to the output, so the window
    /// ends up on the named workspace *and* the named output. A plain
    /// container move to an output would land on whatever workspace that
    /// output is showing, undoing the workspace move.
    WorkspaceToOutput {
        workspace: WorkspaceTarget<'a>,
        output: &'a str,
    },
    Mark(&'a str),
    Height(Size),
    Width(Size),
    Position(Position),
    /// `--sway-command` (#90): one command from `SWAY_WINDOW_COMMANDS`,
    /// sent as given, for a property no other flag models. Nothing it does
    /// can be confirmed, so it reports `unconfirmed`.
    SwayCommand(&'a str),
    Scratchpad,
}

impl ActionKind<'_> {
    /// The stable, machine-readable name of this kind: the CLI flag that
    /// produces it, in `snake_case` (`"new_column"` for `--new-column`).
    ///
    /// This is `--json`'s `"kind"` field, and it exists so a caller can
    /// identify an action without parsing Sway command text: the command
    /// carries user data (a mark, a size) and Sway quoting, and changes
    /// whenever its rendering does, while this never does. A skipped action
    /// reports the same name, since a skip is decided per kind before any
    /// `SwayAction` exists to render a command.
    pub(super) fn flag_name(&self) -> &'static str {
        match self {
            ActionKind::Split(_) => "split",
            ActionKind::Floating => "floating",
            ActionKind::Sticky => "sticky",
            ActionKind::Fullscreen => "fullscreen",
            ActionKind::Focus => "focus",
            ActionKind::NewColumn => "new_column",
            ActionKind::NewRow => "new_row",
            ActionKind::Workspace(_) => "workspace",
            ActionKind::Output(_) | ActionKind::WorkspaceToOutput { .. } => "output",
            ActionKind::Mark(_) => "mark",
            ActionKind::Height(_) => "height",
            ActionKind::Width(_) => "width",
            ActionKind::Position(_) => "position",
            ActionKind::SwayCommand(_) => "sway_command",
            ActionKind::Scratchpad => "scratchpad",
        }
    }
}

impl fmt::Display for ExecAction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Exec \"{}\"", self.command)?;
        if let Some(filter) = self.filter {
            write!(f, " (filter: {})", filter)?;
        }
        match self.title {
            Some(title) => write!(f, " ({})", title),
            None => Ok(()),
        }
    }
}

impl fmt::Display for SwayAction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let name = match self.kind {
            ActionKind::Split(_) => "Split",
            ActionKind::Floating => "Floating",
            ActionKind::Sticky => "Sticky",
            ActionKind::Fullscreen => "Fullscreen",
            ActionKind::Focus => "Focus",
            ActionKind::NewColumn => "New column",
            ActionKind::NewRow => "New row",
            ActionKind::Workspace(_) => "Workspace",
            ActionKind::Output(_) => "Output",
            ActionKind::WorkspaceToOutput { .. } => "Workspace to output",
            ActionKind::Mark(_) => "Mark",
            ActionKind::Height(_) => "Height",
            ActionKind::Width(_) => "Width",
            ActionKind::Position(_) => "Position",
            ActionKind::SwayCommand(_) => "Sway command",
            ActionKind::Scratchpad => "Scratchpad",
        };
        write!(f, "{} (container id: {})", name, self.container_id)?;
        match self.kind {
            ActionKind::Split(split) => write!(f, " (split: {})", split),
            ActionKind::Workspace(workspace) => write!(f, " (workspace: {})", workspace),
            ActionKind::Output(output) => write!(f, " (output: {})", output),
            ActionKind::WorkspaceToOutput { workspace, output } => {
                write!(f, " (workspace: {}) (output: {})", workspace, output)
            }
            ActionKind::Mark(mark) => write!(f, " (mark: {})", mark),
            ActionKind::Height(height) => write!(f, " (height: {})", height),
            ActionKind::Width(width) => write!(f, " (width: {})", width),
            ActionKind::Position(position) => write!(f, " (position: {})", position),
            ActionKind::SwayCommand(command) => write!(f, " (command: {})", command),
            ActionKind::Floating
            | ActionKind::Sticky
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::NewColumn
            | ActionKind::NewRow
            | ActionKind::Scratchpad => Ok(()),
        }
    }
}

impl ExecAction<'_> {
    /// The Sway command that launches this action's command: the user's
    /// command with a per-invocation marker prepended to its environment, so
    /// the window it eventually produces can be correlated back to the process
    /// this invocation spawned (see `process_marker.rs`, and
    /// `run_wait_matching_exec_event()`'s doc comment for the full mechanism).
    /// Takes the marker as an argument because it only exists once the action
    /// is actually running.
    ///
    /// The marker is set with `export ... &&` rather than an `env VAR=value`
    /// prefix. Sway hands an `exec` line to `sh -c`, and `env` can only run an
    /// executable, so a prefix broke any command starting with a shell builtin,
    /// keyword or compound command (`cd dir && editor .`, `exec app`, `( ... )`)
    /// — nothing launched and the wait timed out. `export` keeps the user's
    /// text a shell command list exactly as `swaymsg exec` would run it, and
    /// every command in that list inherits the marker, not just the first.
    /// `&&` rather than `;`, because Sway's own parser splits on an unquoted
    /// `;`; `export` always succeeds, so `&&` never changes the outcome.
    ///
    /// The marker and the command are deliberately raw, unquoted text rather
    /// than passed through `quote_sway_string()`: Sway's own variable
    /// substitution mangles a literal `$` inside a quoted argument, which broke
    /// an earlier design, and the command is the one value this tool exists to
    /// run as given. The cost is that Sway's parser sees the command first and
    /// would split it on an unquoted `,`/`;` or a newline, which is why every
    /// command is checked by `validate_exec_command()` before it gets here.
    ///
    /// The command runs in a `( ... )` subshell so that the shell Sway starts,
    /// whose command line carries the token, stays alive as an ancestor of
    /// everything the command starts: `launch_evidence()` walks up to it to
    /// recognise a window whose program never saw the marker (#96). Without
    /// the parentheses that held only where `sh` is dash; bash execs the last
    /// command of an `&&` list in place of the shell (confirmed live). A
    /// command can't end the group early: an unquoted `#` that would comment
    /// out the `)` is refused by `validate_exec_command()`.
    ///
    /// Lives here, alongside every other action's command text, rather than
    /// inline at the call site: when the two were apart, a separate rendering
    /// of `exec <command>` drifted into something nothing ever sent.
    ///
    /// `--cwd`/`--env`'s setup goes into `setup_file` when there is one, and
    /// the command line only sources it and removes it (#167): the launch
    /// shell's command line is readable by every local user for as long as
    /// the program runs, which a variable's value must not be. `Err` only
    /// when the file's path can't be quoted for Sway.
    pub(super) fn sway_command(
        &self,
        token: &str,
        setup_file: Option<&Path>,
    ) -> Result<String, String> {
        let setup = match (self.setup, setup_file) {
            (None, _) => None,
            (Some(setup), None) => Some(setup.to_string()),
            (Some(_), Some(path)) => {
                let path = path
                    .to_str()
                    .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
                    .and_then(quote_shell_word)
                    .map_err(|error| format!("--cwd/--env setup file: {}", error))?;
                Some(format!(". {} && rm -f {}", path, path))
            }
        };
        Ok(match setup {
            Some(setup) => format!(
                "exec export {}={} && {} && ( {} )",
                PID_MARKER_VAR, token, setup, self.command
            ),
            None => format!(
                "exec export {}={} && ( {} )",
                PID_MARKER_VAR, token, self.command
            ),
        })
    }
}

impl<'a> SwayAction<'a> {
    /// An action of `kind` against `container_id`, waiting `timeout` if `kind`
    /// is confirmed by an IPC event and `wait_time` if it isn't — the one
    /// place that choice is made.
    pub(super) fn new(
        container_id: i64,
        kind: ActionKind<'a>,
        verbose: bool,
        timeout: time::Duration,
        wait_time: time::Duration,
    ) -> Self {
        let timing = if kind.is_event_confirmed() {
            timeout
        } else {
            wait_time
        };
        SwayAction {
            container_id,
            verbose,
            timing,
            kind,
        }
    }

    /// This action's stable kind name — see `ActionKind::flag_name()`.
    pub fn kind_name(&self) -> &'static str {
        self.kind.flag_name()
    }

    /// Whether this action goes to Sway in one command with its neighbours
    /// — see `ActionKind::joins_batch()`.
    pub fn joins_batch(&self) -> bool {
        self.kind.joins_batch()
    }

    /// The `swaymsg` command *without* the `[con_id=N]` target prefix — the
    /// one place an action is serialized, which `batch_command()` puts the
    /// prefix in front of, and also what `--dry-run`
    /// prints for a planned action whose container id isn't resolved yet
    /// (a not-yet-launched window has no id to show).
    pub fn sway_command_verb(&self) -> String {
        match self.kind {
            ActionKind::Floating => "floating enable".to_string(),
            ActionKind::Sticky => "sticky enable".to_string(),
            ActionKind::Fullscreen => "fullscreen enable".to_string(),
            ActionKind::Focus => "focus".to_string(),
            ActionKind::Split(Split::V) => "splitv".to_string(),
            ActionKind::Split(Split::H) => "splith".to_string(),
            ActionKind::Mark(mark) => format!("mark {}", quote_sway_string(mark)),
            ActionKind::NewColumn => "move right".to_string(),
            ActionKind::NewRow => "move down".to_string(),
            ActionKind::Workspace(workspace) => {
                format!("move workspace {}", workspace.sway_argument())
            }
            ActionKind::Output(output) => {
                format!("move container to output {}", quote_sway_string(output))
            }
            ActionKind::WorkspaceToOutput { output, .. } => {
                format!("move workspace to output {}", quote_sway_string(output))
            }
            ActionKind::Height(height) => format!("resize set height {}", height),
            ActionKind::Width(width) => format!("resize set width {}", width),
            ActionKind::Position(position) => {
                let position_command = match position {
                    Position::Center => "center".to_string(),
                    Position::Coordinates { x, y } => format!("{} {}", x, y),
                };
                format!("move position {}", position_command)
            }
            ActionKind::SwayCommand(command) => command.to_string(),
            ActionKind::Scratchpad => "move scratchpad".to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
