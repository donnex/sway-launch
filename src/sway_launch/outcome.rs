//! What a run reports back: per-action outcomes, and the record of them.
//!
//! Two audiences, kept apart on purpose. `RunOutcome`, `ActionRecord`,
//! `ActionStatus` and `LaunchOwnership` are public, and `main.rs` renders them
//! into `--json`, so their shape is a contract with anyone scripting against
//! this tool. `ActionOutcome` is internal, and exists because the distinction
//! the executor draws while working (observed versus merely sent) is finer
//! than the one worth publishing.

use std::fmt;

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
    /// What went wrong with where the window opened, when
    /// `SwayLaunch::warn_on_misplaced_launch` asked (#160): each a complete
    /// sentence, for `main.rs` to print and put in `--json`'s `"warnings"`.
    pub warnings: Vec<String>,
}

/// Whether a launched window is one this invocation can prove it created.
///
/// `ExecAction` tags the process it spawns with a per-invocation marker
/// and prefers a window whose pid carries it, but deliberately falls back to a
/// content match (app_id/class) when no marked window appears — see
/// `run_wait_matching_exec_event()`. That fallback covers two very different
/// situations, and only one of them is evidence of ownership:
///
/// - The marked process is *gone*, and the window's process was already
///   running before the launch — the single-instance application case (a
///   browser or editor forwarding the request to an already-running process,
///   which then maps the window). Nothing marker-confirmed can still be
///   coming, and the window most likely appeared because we asked for it:
///   `Launched`. This is an inference, not proof: the older process could
///   have opened a window for its own reasons at the same moment.
/// - Anything else: the marked process is still *running* when the grace
///   period elapses, or it's gone but the window's process started after the
///   launch (the launched command failed and exited, #124). The matching
///   window came from somewhere else — another `sway-launch`, another
///   launcher, a user opening something at the wrong moment — and this
///   invocation adopted it only because it matched the requested
///   app_id/class: `Adopted`.
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

/// Why `SwayLaunch::run()` failed: the message, plus any windows a launch saw
/// appear while it waited but rejected because they didn't pass its
/// `--app-id`/`--class` filter, plus the container the run had already
/// resolved when a later action failed.
///
/// The windows are carried separately from the message, rather than only
/// mentioned in it, because they are real windows left open on screen —
/// usually the one the user meant, under an app_id they didn't expect — and a
/// `--json` caller needs their ids to deal with them without parsing prose.
/// Every other failure leaves `unmatched_windows` empty, which is what the
/// `From<String>` conversion produces.
///
/// `container_id`/`launch_ownership` exist for the same reason. A run can
/// launch its window successfully and *then* fail on a follow-up action (a
/// confirmation timeout, `--position` on a tiled window), leaving that window
/// on screen; without its id on the error path, neither a script cleaning up
/// after a failed single invocation nor `--rollback-on-error` for the failing
/// layout step could find it. Both are `None` when the failure happened before
/// a container was resolved at all, and `launch_ownership` is also `None` for
/// a retargeted window, exactly as on `RunOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    pub message: String,
    pub unmatched_windows: Vec<UnmatchedWindow>,
    pub container_id: Option<i64>,
    pub launch_ownership: Option<LaunchOwnership>,
    /// Further windows a timed-out launch proved it opened, beyond
    /// `container_id`, each `LaunchOwnership::Launched`: an application can
    /// open several under an app_id its filter didn't name (#168). Empty
    /// for every other failure.
    pub also_launched: Vec<i64>,
    /// The run's window never appeared within `--timeout`: a launch that saw
    /// no matching window, or `--existing --wait` that saw none open. The one
    /// failure `--continue-on-timeout` lets a layout carry on past (#19),
    /// since nothing was resolved for a later step to act on by mistake.
    pub window_timed_out: bool,
}

impl From<String> for RunError {
    fn from(message: String) -> Self {
        RunError {
            message,
            unmatched_windows: Vec::new(),
            container_id: None,
            launch_ownership: None,
            also_launched: Vec::new(),
            window_timed_out: false,
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// A new window that appeared while a launch was waiting, but didn't match
/// its `--app-id`/`--class` filter. `launched_by_this_run` is whether the
/// window's process carried this invocation's PID marker — when it did, the
/// filter is almost certainly what's wrong, not the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmatchedWindow {
    pub container_id: i64,
    pub app_id: Option<String>,
    pub class: Option<String>,
    pub title: Option<String>,
    pub launched_by_this_run: bool,
}

/// One entry in `RunOutcome.actions`: either an action that actually ran, or
/// a `NewColumn`/`NewRow` the executor chose not to send at all (see
/// `tree::move_skip_reason()`).
///
/// `kind` is the action's stable identity, the same for both cases: the
/// `snake_case` name of the CLI flag that produced it
/// (`ActionKind::flag_name()`). `command` is the Sway command text it ran
/// (`SwayAction::sway_command_verb()`, the same container-id-free text
/// `--dry-run` prints), and `None` for a skip, which was never rendered into
/// a command at all. They are kept apart because the command is no identity:
/// it carries user data (a mark, a size) and Sway quoting, and changes
/// whenever its rendering does, so a caller matching on it would be parsing
/// Sway syntax — and a skipped `--new-column` would answer to a different
/// name than a run one.
///
/// Folding skips into the same list as the actions that ran, alongside the
/// `Changed`/`AlreadySatisfied`/`Unconfirmed` distinction `run_batch()`
/// computes (`ActionOutcome`), is what lets a `--json` caller tell every
/// outcome apart in its actual run order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRecord {
    pub kind: &'static str,
    pub command: Option<String>,
    pub status: ActionStatus,
}

/// What actually happened to one planned action. `reason` on `Skipped` is a
/// short, stable, machine-readable identifier (not prose) — the same spirit
/// as `SwayAction::sway_command_verb()`'s stable text, not a human-facing
/// message (that's what the existing `--verbose` `diagnostic!` next to each
/// skip is for). Only one skip mechanism exists today (the `NewColumn`/
/// `NewRow` guard, whose reasons are `"floating"`, `"fullscreen"` and
/// `"trailing_workspace_edge"`), so `reason` is a plain `&'static str` rather
/// than a nested enum — revisit if a second one ever appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    Changed,
    AlreadySatisfied,
    Unconfirmed,
    Skipped { reason: &'static str },
}

/// What `run_batch()` observed, before `SwayLaunch::run()` folds it
/// together with the skip case into an `ActionStatus`. Separate from
/// `ActionStatus` because a skipped action is never sent, so
/// `run_batch()` never sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionOutcome {
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
