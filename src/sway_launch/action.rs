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
use super::values::{Position, Size, Split};
use std::{fmt, time};

/// Launches `command` and waits for the window it opens.
#[derive(Copy, Clone, Debug)]
pub(super) struct ExecAction<'a> {
    pub(super) command: &'a str,
    pub(super) app_id_match: &'a str,
    pub(super) class_match: &'a str,
    pub(super) verbose: bool,
    pub(super) timeout: time::Duration,
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
    Workspace(&'a str),
    Output(&'a str),
    Mark(&'a str),
    Height(Size),
    Width(Size),
    Position(Position),
    Scratchpad,
}

impl fmt::Display for ExecAction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Exec \"{}\" (app_id_match: \"{}\") (class_match: \"{}\")",
            self.command, self.app_id_match, self.class_match
        )
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
            ActionKind::Mark(_) => "Mark",
            ActionKind::Height(_) => "Height",
            ActionKind::Width(_) => "Width",
            ActionKind::Position(_) => "Position",
            ActionKind::Scratchpad => "Scratchpad",
        };
        write!(f, "{} (container id: {})", name, self.container_id)?;
        match self.kind {
            ActionKind::Split(split) => write!(f, " (split: {})", split),
            ActionKind::Workspace(workspace) => write!(f, " (workspace: {})", workspace),
            ActionKind::Output(output) => write!(f, " (output: {})", output),
            ActionKind::Mark(mark) => write!(f, " (mark: {})", mark),
            ActionKind::Height(height) => write!(f, " (height: {})", height),
            ActionKind::Width(width) => write!(f, " (width: {})", width),
            ActionKind::Position(position) => write!(f, " (position: {})", position),
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
    /// Lives here, alongside every other action's command text, rather than
    /// inline at the call site: when the two were apart, a separate rendering
    /// of `exec <command>` drifted into something nothing ever sent.
    pub(super) fn sway_command(&self, token: &str) -> String {
        format!(
            "exec export {}={} && {}",
            PID_MARKER_VAR, token, self.command
        )
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

    /// The full `swaymsg` command string, `[con_id=N] <verb>`. Just wraps
    /// `sway_command_verb()` with the target prefix; kept as a separate method
    /// (rather than inlining that wrap at every `run_sway_command()` call site)
    /// so there's exactly one place that does it.
    pub(super) fn sway_command(&self) -> String {
        format!(
            "[con_id={}] {}",
            self.container_id,
            self.sway_command_verb()
        )
    }

    /// The `swaymsg` command *without* the `[con_id=N]` target prefix —
    /// `sway_command()`'s own building block, and also what `--dry-run`
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
                format!("move workspace {}", quote_sway_string(workspace))
            }
            ActionKind::Output(output) => {
                format!("move container to output {}", quote_sway_string(output))
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
            ActionKind::Scratchpad => "move scratchpad".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{exec_action, targeted};
    use super::*;

    // SwayAction::new

    #[test]
    fn new_picks_the_timeout_for_an_event_confirmed_kind() {
        let action = SwayAction::new(
            42,
            ActionKind::Floating,
            true,
            time::Duration::from_secs(7),
            time::Duration::from_millis(42),
        );
        assert_eq!(action.timing, time::Duration::from_secs(7));
        assert_eq!(action.container_id, 42);
        assert!(action.verbose);
    }

    #[test]
    fn new_picks_the_wait_time_for_a_polled_kind() {
        let action = SwayAction::new(
            42,
            ActionKind::Height(Size::Pixels(300)),
            false,
            time::Duration::from_secs(7),
            time::Duration::from_millis(42),
        );
        assert_eq!(action.timing, time::Duration::from_millis(42));
    }

    // ExecAction::sway_command

    #[test]
    fn exec_sway_command_prepends_the_marker_to_the_users_command() {
        // Asserts the text that actually reaches Sway. An earlier test asserted
        // a separately rendered "exec foot" that nothing ever sent.
        let action = exec_action("foot --app-id=probe", "", "");
        assert_eq!(
            action.sway_command("1234-5678"),
            "exec export SWAY_LAUNCH_PID_MARKER=1234-5678 && foot --app-id=probe"
        );
    }

    #[test]
    fn exec_sway_command_keeps_a_command_led_by_a_shell_builtin_intact() {
        let action = exec_action("cd /tmp && exec foot", "", "");
        assert_eq!(
            action.sway_command("1234-5678"),
            "exec export SWAY_LAUNCH_PID_MARKER=1234-5678 && cd /tmp && exec foot"
        );
    }

    // SwayAction::sway_command

    #[test]
    fn sway_command_renders_every_kind() {
        let cases = [
            (ActionKind::Floating, "[con_id=42] floating enable"),
            (ActionKind::Sticky, "[con_id=42] sticky enable"),
            (ActionKind::Fullscreen, "[con_id=42] fullscreen enable"),
            (ActionKind::Focus, "[con_id=42] focus"),
            (ActionKind::Split(Split::V), "[con_id=42] splitv"),
            (ActionKind::Split(Split::H), "[con_id=42] splith"),
            (ActionKind::NewColumn, "[con_id=42] move right"),
            (ActionKind::NewRow, "[con_id=42] move down"),
            (
                ActionKind::Height(Size::Pixels(300)),
                "[con_id=42] resize set height 300px",
            ),
            (
                ActionKind::Width(Size::Percent(20)),
                "[con_id=42] resize set width 20ppt",
            ),
            (
                ActionKind::Position(Position::Center),
                "[con_id=42] move position center",
            ),
            (
                ActionKind::Position(Position::Coordinates { x: 100, y: 200 }),
                "[con_id=42] move position 100 200",
            ),
            (
                ActionKind::Position(Position::Coordinates { x: -1920, y: -200 }),
                "[con_id=42] move position -1920 -200",
            ),
            (ActionKind::Scratchpad, "[con_id=42] move scratchpad"),
        ];
        for (kind, expected) in cases {
            assert_eq!(targeted(kind).sway_command(), expected, "{kind:?}");
        }
    }

    #[test]
    fn sway_command_mark_quotes_the_mark() {
        assert_eq!(
            targeted(ActionKind::Mark("foo, exec evil")).sway_command(),
            "[con_id=42] mark \"foo, exec evil\""
        );
    }

    #[test]
    fn sway_command_workspace_quotes_the_workspace() {
        assert_eq!(
            targeted(ActionKind::Workspace("web, exec evil")).sway_command(),
            "[con_id=42] move workspace \"web, exec evil\""
        );
    }

    #[test]
    fn sway_command_output_quotes_the_output() {
        assert_eq!(
            targeted(ActionKind::Output("HDMI-A-1, exec evil")).sway_command(),
            "[con_id=42] move container to output \"HDMI-A-1, exec evil\""
        );
    }

    // SwayAction::sway_command_verb

    #[test]
    fn sway_command_verb_omits_the_con_id_prefix() {
        // Regression test for the sway_command()/sway_command_verb() split
        // (--dry-run's foundation): the verb-only form is exactly
        // sway_command() with the "[con_id=N] " prefix stripped.
        let floating = targeted(ActionKind::Floating);
        assert_eq!(floating.sway_command(), "[con_id=42] floating enable");
        assert_eq!(floating.sway_command_verb(), "floating enable");

        let position = targeted(ActionKind::Position(Position::Coordinates {
            x: -1920,
            y: -200,
        }));
        assert_eq!(
            position.sway_command(),
            "[con_id=42] move position -1920 -200"
        );
        assert_eq!(position.sway_command_verb(), "move position -1920 -200");
    }

    // Display

    #[test]
    fn display_exec() {
        assert_eq!(
            exec_action("foot", "foot", "").to_string(),
            "Exec \"foot\" (app_id_match: \"foot\") (class_match: \"\")"
        );
    }

    #[test]
    fn display_names_every_kind_with_the_same_container_id_wording() {
        let cases = [
            (
                ActionKind::Split(Split::H),
                "Split (container id: 42) (split: Horizontal)",
            ),
            (ActionKind::Floating, "Floating (container id: 42)"),
            (ActionKind::Sticky, "Sticky (container id: 42)"),
            (ActionKind::Fullscreen, "Fullscreen (container id: 42)"),
            (ActionKind::Focus, "Focus (container id: 42)"),
            (ActionKind::NewColumn, "New column (container id: 42)"),
            (ActionKind::NewRow, "New row (container id: 42)"),
            (
                ActionKind::Workspace("web"),
                "Workspace (container id: 42) (workspace: web)",
            ),
            (
                ActionKind::Output("HDMI-A-1"),
                "Output (container id: 42) (output: HDMI-A-1)",
            ),
            (
                ActionKind::Mark("foo"),
                "Mark (container id: 42) (mark: foo)",
            ),
            (
                ActionKind::Height(Size::Pixels(300)),
                "Height (container id: 42) (height: 300px)",
            ),
            (
                ActionKind::Width(Size::Percent(20)),
                "Width (container id: 42) (width: 20ppt)",
            ),
            (
                ActionKind::Position(Position::Center),
                "Position (container id: 42) (position: center)",
            ),
            (ActionKind::Scratchpad, "Scratchpad (container id: 42)"),
        ];
        for (kind, expected) in cases {
            assert_eq!(targeted(kind).to_string(), expected);
        }
    }
}
