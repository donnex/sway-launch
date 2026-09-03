//! What a run reports back: per-action outcomes, and the record of them.
//!
//! Two audiences, kept apart on purpose. `RunOutcome`, `ActionRecord`,
//! `ActionStatus` and `LaunchOwnership` are public, and `main.rs` renders them
//! into `--json`, so their shape is a contract with anyone scripting against
//! this tool. `ActionResult`, `ActionOutcome`, `PlannedAction` and
//! `SkippedAction` are internal, and exist because the distinctions the
//! executor draws while working (planned versus skipped, observed versus
//! merely sent) are finer than the ones worth publishing.

use super::SwayAction;

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
pub(super) struct ActionResult {
    pub(super) container_id: i64,
    pub(super) outcome: ActionOutcome,
    pub(super) launch_ownership: Option<LaunchOwnership>,
}

impl ActionResult {
    /// The result of an action applied to an already-resolved container, which
    /// is every action except `Exec`.
    pub(super) fn acted(container_id: i64, outcome: ActionOutcome) -> Self {
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

/// One `SwayAction` `SwayLaunch::build_actions()` planned to run, or one it
/// decided to skip instead (interleaved in the same `Vec` so the overall
/// fixed order survives — see `build_actions()`'s doc comment for why a
/// skip needs a defined position at all, not just a separate side list).
#[derive(Debug)]
pub(super) enum PlannedAction<'a> {
    Run(SwayAction<'a>),
    Skip(SkippedAction),
}

/// One action `SwayLaunch::build_actions()` decided not to include in the
/// plan, and why — see `ActionStatus::Skipped`, which is what actually
/// carries this into `RunOutcome`/`--json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SkippedAction {
    pub(super) action: &'static str,
    pub(super) reason: &'static str,
}
