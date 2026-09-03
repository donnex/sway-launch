//! Launching and arranging windows in Sway.
//!
//! The work is layered, and each layer is a module:
//!
//! - `values` — the validated types the CLI and TOML layers hand in.
//! - `ipc` — reaching the compositor: connections, commands, events.
//! - `tree` — reading answers out of a tree, purely.
//! - `query` — fetching a tree so `tree` can read it.
//! - `process_marker` — correlating a launched window to the process we spawned.
//! - `action` — what a single action is, and the command it renders to.
//! - `confirmation` — deciding whether that command took effect.
//! - `outcome` — what a run reports back.
//! - `launch` — turning one invocation into an ordered plan, and running it.
//!
//! The `tree`/`query` boundary is the load-bearing one: everything that needs
//! a live compositor is confined to `ipc` and `query`, so every decision made
//! about Sway's state stays unit-testable without one. See CLAUDE.md's
//! Architecture section for the reasoning behind each of these.
//!
//! This file itself is just the seam: module declarations, and the public
//! surface `main.rs`, `layout.rs` and `template.rs` use.

use std::time;

mod action;
mod confirmation;
mod ipc;
mod launch;
mod outcome;
mod process_marker;
mod query;
#[cfg(test)]
mod test_support;
mod tree;
mod values;

pub use action::SwayAction;
pub use ipc::kill_container;
pub use launch::{SwayLaunch, Target};
pub use outcome::{ActionRecord, ActionStatus, LaunchOwnership};
pub use values::{
    parse_position, parse_size, require_non_blank, validate_non_blank_argument,
    validate_position_argument, validate_size_argument, validate_sway_string_argument, Split,
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
