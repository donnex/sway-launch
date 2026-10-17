//! Deciding whether an action actually took effect.
//!
//! Sway confirms different actions in completely different ways, and this
//! module is where that lives: which `WindowChange` event (if any) would
//! confirm a given action, whether the requested state is in effect, whether a
//! poll of the tree shows the change, and how long to keep looking before
//! reporting the weaker `Unconfirmed`. The `run_*` drivers here are what
//! actually send each command and wait on its confirmation.
//!
//! The split between what's here and what's in `action.rs` is what an action
//! *is* versus how its effect is established: `action.rs` renders the command,
//! this module decides whether it worked. `ExecAction` and `SwayAction` each
//! get their own driver here, since launching a window and acting on one that
//! already exists are confirmed in different ways.

use super::action::{ActionKind, ExecAction, SwayAction};
use super::ipc::{
    event_loop, new_connection, run_single_sway_command, run_sway_command, spawn_event_forwarder,
};
use super::outcome::{ActionOutcome, LaunchOwnership, RunError, UnmatchedWindow};
use super::process_marker::{
    generate_pid_marker_token, process_has_env_var, scan_marker_presence, MarkerPresence,
    MarkerTracker, MARKER_SCAN_INTERVAL, PID_MARKER_FALLBACK_GRACE, PID_MARKER_VAR,
};
use super::query::{
    awaited_existing_container_id, container_state, container_state_on, ensure_container_exists,
    existing_container, node_by_id, parent_node_layout, position_matches,
    recheck_launched_windows_now,
};
use super::target::{LaunchFilter, WindowCriteria};
use super::tree::{height_matches, is_window, width_matches, ContainerState};
use super::values::{escape_control_characters, Size, Split, TitlePattern};
use super::{MOVE_POLL_GRACE, WAIT_TIME_POLL_GRACE, WAIT_TIME_POLL_INTERVAL};
use std::sync::mpsc;
use std::{fmt, thread, time};
use swayipc::{Connection, Event, EventType, NodeLayout, WindowChange, WindowEvent};

enum WindowEventMatch {
    WindowAppId,
    WindowClass,
    NewWindowMatchWithoutCheck,
    WindowContainerIdMatch,
}

impl fmt::Display for WindowEventMatch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowEventMatch::WindowAppId => {
                write!(f, "Window app_id match")
            }
            WindowEventMatch::WindowClass => {
                write!(f, "Window class match")
            }
            WindowEventMatch::NewWindowMatchWithoutCheck => {
                write!(f, "New window without app_id or class check")
            }
            WindowEventMatch::WindowContainerIdMatch => {
                write!(f, "Window container id match")
            }
        }
    }
}

enum WindowEventMatchError {
    EventChangeTypeMismatch,
    WindowAppIdMismatch,
    WindowClassMismatch,
    WindowTitleMismatch,
    NoMatchingEvent,
}

impl fmt::Display for WindowEventMatchError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowEventMatchError::EventChangeTypeMismatch => {
                write!(f, "Event does not match action event matches")
            }
            WindowEventMatchError::WindowAppIdMismatch => {
                write!(f, "app_id mismatch")
            }
            WindowEventMatchError::WindowClassMismatch => {
                write!(f, "class mismatch")
            }
            WindowEventMatchError::WindowTitleMismatch => {
                write!(f, "title mismatch")
            }
            WindowEventMatchError::NoMatchingEvent => {
                write!(f, "No matching event")
            }
        }
    }
}

impl ActionKind<'_> {
    /// The `WindowChange` event that would confirm this kind of action, or
    /// `None` for one Sway fires no event for, which confirms by polling
    /// instead.
    ///
    /// Answering `Some` here commits a kind to recognising its own end state
    /// in `state_satisfies()`: `run_wait_matching_events()` accepts a matching
    /// event only once that returns `true`, so a kind without a real arm there
    /// sees its event arrive, rejects it every time, and times out after a
    /// command that worked. Answering `None` commits it to a `polls()`/
    /// `poll_matches()` arm instead, or to being reported unconfirmed on every
    /// run. Every match on `ActionKind` in this file names each variant rather
    /// than ending in `_`, so a new kind can't skip either decision silently.
    fn matching_window_change_event(&self) -> Option<WindowChange> {
        match self {
            ActionKind::Floating => Some(WindowChange::Floating),
            ActionKind::Fullscreen => Some(WindowChange::FullscreenMode),
            ActionKind::Focus => Some(WindowChange::Focus),
            // Unlike NewColumn/NewRow's "move right"/"move down" below,
            // "move workspace"/"move container to output" reliably fire
            // WindowChange::Move for an actual move (tests/live_sway.rs's
            // workspace_moves_window_to_named_workspace/
            // output_moves_window_to_named_output) — but live testing also
            // showed they *do* have an "already there" no-op case after
            // all (moving to the workspace/output the window is already
            // on doesn't reparent anything, so no Move event fires either):
            // `already_at_target()` below checks for and short-circuits
            // that case inside run_wait_matching_events(), once the
            // subscription is open but before the command is sent (that
            // ordering is load-bearing — see its doc comment), rather than
            // hanging until --timeout for a move that was never going to
            // happen.
            ActionKind::Workspace(_) | ActionKind::Output(_) => Some(WindowChange::Move),
            ActionKind::Mark(_) => Some(WindowChange::Mark),
            // NewColumn/NewRow ("move right"/"move down") were event-based
            // via WindowChange::Move too, until live-Sway testing showed
            // "move right" doesn't fire it when the window is already at
            // the tree's rightmost position (the ordinary two-window case)
            // — it silently no-ops instead, hanging until --timeout.
            // "move down" happened to work in that same test, but relies on
            // the identical unverified assumption for a different tree
            // shape, so both moved to the same wait-time pattern as
            // Split/Height/Width/Position rather than leaving one exposed.
            //
            // On a multi-monitor setup, "move right"/"move down" on a
            // window with no sibling to move past within its own workspace
            // don't always no-op the way they do with a single output —
            // Sway's own move-direction semantics can instead escalate and
            // relocate the whole workspace to the next output in that
            // direction. `SwayLaunch::run()` guards against this (see
            // `tree::move_skip_reason()`) by skipping NewColumn/NewRow
            // rather than silently moving the window to a different
            // monitor when that's the situation.
            // No WindowChange variant exists for sticky at all — confirmed
            // live by subscribing to window events while toggling
            // `sticky enable`/`sticky disable` on a live container: zero
            // events fired either way, unlike Floating/Fullscreen/Focus
            // above, which each have a dedicated WindowChange variant.
            ActionKind::Sticky => None,
            // Moving a workspace fires a workspace event, not a window one,
            // and only when the workspace actually changes output, so this
            // is polled against the tree instead.
            ActionKind::WorkspaceToOutput { .. } => None,
            ActionKind::Split(_)
            | ActionKind::NewColumn
            | ActionKind::NewRow
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_) => None,
            // "move scratchpad" reparents the container into the special
            // __i3_scratch workspace, which — like Workspace/Output above —
            // fires WindowChange::Move. Confirmed live: for a previously
            // *tiled* window, Sway also auto-floats it first, firing a
            // WindowChange::Floating event before the Move — matches_window_event()
            // simply doesn't match that one (its change type isn't the one
            // returned here) and the loop keeps waiting for the Move that follows.
            // Re-scratchpadding an already-scratchpadded window is a no-op
            // that fires no event at all (confirmed live), so
            // already_at_target() below checks for and short-circuits that
            // case the same way it does for Workspace/Output/Floating/
            // Fullscreen/Focus.
            ActionKind::Scratchpad => Some(WindowChange::Move),
        }
    }

    /// Whether this kind can only be confirmed when its target is a window.
    /// Its confirming event is a window event, and Sway sends those only for
    /// views: on a marked split container the command applies, nothing
    /// arrives, and the action used to time out over a move that worked
    /// (confirmed live). `Floating`, `Fullscreen`, `Scratchpad` and `Mark`
    /// were confirmed live to work on a split container, so they're left be.
    pub(super) fn needs_a_window(&self) -> bool {
        match self {
            ActionKind::Workspace(_) | ActionKind::Output(_) | ActionKind::Focus => true,
            ActionKind::Split(_)
            | ActionKind::Floating
            | ActionKind::Sticky
            | ActionKind::Fullscreen
            | ActionKind::NewColumn
            | ActionKind::NewRow
            | ActionKind::WorkspaceToOutput { .. }
            | ActionKind::Mark(_)
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_)
            | ActionKind::Scratchpad => false,
        }
    }

    /// Whether this kind of action is confirmed by an IPC event (and so waits
    /// on `--timeout`) rather than by polling (and so waits on `--wait-time`).
    pub(super) fn is_event_confirmed(&self) -> bool {
        self.matching_window_change_event().is_some()
    }
}

impl SwayAction<'_> {
    /// Runs this action for real, returning how confidently it can say what
    /// happened — which `SwayLaunch::run()` turns into the `ActionStatus`
    /// reported in `RunOutcome`/`--json`.
    ///
    /// The three outcomes are genuinely different claims, and collapsing them
    /// was misleading: an event-confirmed action that observed its state, an
    /// action that short-circuited because the state already held, and a
    /// wait-time action that sent its command and never saw the change appear
    /// are not equally strong results.
    ///
    /// `already_at_target()`'s no-op short-circuit is deliberately *not* run
    /// here, ahead of the dispatch below: it belongs inside
    /// `run_wait_matching_events()`, after that function has subscribed. See
    /// its doc comment for why the ordering is load-bearing. The seven kinds
    /// that confirm by polling answer `false` from `state_satisfied()`
    /// unconditionally, so they're unaffected by where it runs.
    ///
    /// Every error is prefixed with the Sway command verb, so that a timeout in
    /// an invocation with several actions, or a layout's `step N: ...`, says
    /// which one failed rather than only how long it waited.
    pub(super) fn run(&self) -> Result<ActionOutcome, RunError> {
        if self.verbose {
            diagnostic!(
                "Sway action: {}",
                escape_control_characters(&self.to_string(), false)
            );
        }

        let outcome = if self.kind.is_event_confirmed() {
            self.run_wait_matching_events()
        } else {
            self.run_wait_time().map(|confirmed| {
                if confirmed {
                    ActionOutcome::Changed
                } else {
                    ActionOutcome::Unconfirmed
                }
            })
        };
        outcome.map_err(|error| format!("{}: {}", self.sway_command_verb(), error).into())
    }

    /// Checks whether the container is already where/how a given action
    /// would put it — Sway doesn't fire the event each of these actions
    /// waits on when the command turns out to be a no-op, so
    /// `run_wait_matching_events()` would otherwise hang until `--timeout`
    /// for an event that was never coming. Returns `true` to
    /// short-circuit `run_wait_matching_events()` with success, or `false`
    /// to proceed normally.
    ///
    /// `Workspace`/`Output` were the first two found needing this (checked
    /// against the containing workspace/output name). `Floating`/
    /// `Fullscreen`/`Focus` were found to have the identical failure mode —
    /// confirmed live: re-running `--floating`/`--fullscreen`/`--focus` on
    /// a window already in that state hangs 5s and then errors out, rather
    /// than completing promptly — so they're checked the same way, against
    /// the container's own `floating`/`fullscreen_mode`/`focused` fields.
    /// `Mark` was checked live too and found *not* to need this:
    /// re-applying a mark the container already has still fires
    /// `WindowChange::Mark`, unlike these three. `Scratchpad` was found to
    /// need it as well — re-running `[con_id] move scratchpad` on a window
    /// already in the scratchpad fires no event at all, confirmed live —
    /// checked against the ancestor workspace name rather than `floating`,
    /// since a scratchpad window also reports as floating for the unrelated
    /// reason `matching_window_change_event()`'s doc comment covers. Every
    /// state above is read in one pass by `ContainerState::from_tree()`; every
    /// other action falls through to `false` unconditionally, which this never
    /// touches.
    ///
    /// `Mark` is the one event-confirmed action deliberately excluded here
    /// even though `state_satisfied()` below can answer for it: re-applying a
    /// mark the container already has *does* fire `WindowChange::Mark`, so
    /// there's no hang to avoid, and short-circuiting would report it as
    /// `AlreadySatisfied` when the command genuinely does re-apply.
    ///
    /// **Call this only once a subscription is already open.** This check reads
    /// state that another client can change a moment later, and the no-op it
    /// exists to detect is precisely the case Sway fires no event for — so if
    /// the state flips between reading it here and subscribing, the command
    /// becomes a no-op, no event ever arrives, and the action waits out the
    /// full `--timeout` before erroring. Subscribing first closes the window
    /// completely rather than narrowing it: a change landing before this read
    /// is caught by the read, and one landing after it is caught by the
    /// subscription, with no instant belonging to neither. Measured against a
    /// live compositor while the check still ran ahead of the subscription:
    /// sweeping a competing `floating enable` across a 0-20ms window after
    /// spawn, 5 of 40 trials hung the full timeout, clustered 2.5-4ms in.
    fn already_at_target(&self) -> Result<bool, String> {
        if matches!(self.kind, ActionKind::Mark(_)) {
            return Ok(false);
        }
        self.state_satisfied()
    }

    /// Whether this action's requested end state currently holds, read from
    /// Sway's tree. `false` for every action that confirms by polling instead
    /// (`Split`, `Sticky`, `Height`, `Width`, `Position`, `NewColumn`,
    /// `NewRow`), none of which reach this.
    ///
    /// Used twice, for two different questions. Before the command runs,
    /// `already_at_target()` asks it to detect a no-op Sway would fire no
    /// event for. *After* a matching event arrives,
    /// `run_wait_matching_events()` asks it again — because an event proves
    /// only that Sway emitted that event type for this container, not that the
    /// state this action asked for is the one that ended up applying.
    ///
    /// The gap is reachable whenever something else is driving the same
    /// window: another `sway-launch` process, a keybinding, a `swaymsg` in the
    /// same script. Two invocations sending `move workspace 2` and
    /// `move workspace 3` at once each see a `WindowChange::Move` for their
    /// container and, on the event alone, both report success — while the
    /// window is on exactly one of them. The same reasoning applies to
    /// floating, fullscreen, focus (inherently global, so the most exposed),
    /// output, mark and scratchpad. Checking the state turns the event from
    /// the confirmation into what it actually is: a wake-up telling us it's
    /// worth looking.
    ///
    /// Safe to require rather than merely prefer, because Sway applies a
    /// command before emitting its event and serves IPC requests in order, so
    /// a `get_tree()` issued after receiving the event already reflects it —
    /// this is not a settle race. If the state genuinely isn't there, the
    /// event was someone else's and waiting for the next one is correct.
    ///
    /// Split in two: this half fetches, `state_satisfies()` below decides. The
    /// decision is the part worth testing exhaustively and the part with no
    /// business talking to a socket, so it takes a `ContainerState` and stays
    /// ordinary pure logic — the same shape `position_matches()`/
    /// `expected_position()` already use, and what this project asks of any new
    /// matcher (see CLAUDE.md's coverage note). One `get_tree()` answers for
    /// every variant, where the previous per-arm helpers each fetched their
    /// own.
    fn state_satisfied(&self) -> Result<bool, String> {
        // Every kind `state_satisfies()` answers `false` for regardless of
        // state is one with no confirming event, so there's nothing to read a
        // tree for.
        if !self.kind.is_event_confirmed() {
            return Ok(false);
        }
        Ok(self.state_satisfies(container_state(self.container_id)?.as_ref()))
    }

    /// Whether `state` is the state this action asked for. `None` — the
    /// container isn't in the tree at all — is never satisfied: a window that
    /// has closed isn't floating, focused or on the target workspace, and
    /// reporting it as already-there would short-circuit an action into
    /// claiming success over a container that no longer exists.
    fn state_satisfies(&self, state: Option<&ContainerState>) -> bool {
        let Some(state) = state else {
            return false;
        };

        match self.kind {
            // Sway matches both names case-insensitively, so `--workspace
            // web` lands on an existing `Web`.
            ActionKind::Workspace(workspace) => {
                workspace.matches(state.workspace.as_deref(), state.workspace_number)
            }
            // Sway's `move container to output` lands on the output's
            // visible workspace, so a window on a hidden workspace of the
            // right output still has somewhere to go.
            ActionKind::Output(output) => {
                state.workspace_visible
                    && state
                        .output
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(output))
            }
            ActionKind::Floating => state.floating,
            ActionKind::Fullscreen => state.fullscreen,
            ActionKind::Focus => state.focused,
            ActionKind::Scratchpad => state.in_scratchpad,
            ActionKind::Mark(mark) => state.marks.iter().any(|held| held == mark),
            // Both halves of the combined request at once; whether the
            // workspace is the one the output is showing doesn't matter here.
            ActionKind::WorkspaceToOutput { workspace, output } => {
                workspace.matches(state.workspace.as_deref(), state.workspace_number)
                    && state
                        .output
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(output))
            }
            ActionKind::Split(_)
            | ActionKind::Sticky
            | ActionKind::NewColumn
            | ActionKind::NewRow
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_) => false,
        }
    }

    /// How long `run_poll_then_fallback()` may keep polling for this
    /// variant's own confirmation. Pure, so the choice stays unit-testable
    /// headlessly; the caller still caps it at the actual `--wait-time`.
    ///
    /// `NewColumn`/`NewRow` get a much shorter window than everything else
    /// because their matcher answers a weaker question — "did the geometry
    /// change?" rather than "is it what was asked for?" — and only for as
    /// long as it keeps looking. See `MOVE_POLL_GRACE`.
    fn poll_grace(&self) -> time::Duration {
        match self.kind {
            ActionKind::NewColumn | ActionKind::NewRow => MOVE_POLL_GRACE,
            ActionKind::Split(_)
            | ActionKind::Sticky
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_)
            | ActionKind::Floating
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::Workspace(_)
            | ActionKind::Output(_)
            | ActionKind::WorkspaceToOutput { .. }
            | ActionKind::Mark(_)
            | ActionKind::Scratchpad => WAIT_TIME_POLL_GRACE,
        }
    }

    /// Whether this variant confirms its own command via polling at all.
    /// Pure — no IPC — so `run_wait_time()` can decide whether to enter the
    /// poll loop before opening a connection for it, and so the decision
    /// stays unit-testable headlessly (which is most of what the poll
    /// matchers' own tests were ever asserting).
    ///
    /// `baseline` matters only to `NewColumn`/`NewRow`: without a snapshot
    /// to compare against there is nothing for them to poll for, so they opt
    /// out (see `poll_baseline()`'s doc comment). A `ppt` (percent) `Size`
    /// opts out for a different reason — there's no pixel figure to poll for
    /// without also resolving the reference dimension it's a percentage of.
    fn polls(&self, baseline: Option<swayipc::Rect>) -> bool {
        match self.kind {
            ActionKind::Split(_)
            | ActionKind::Sticky
            | ActionKind::Position(_)
            | ActionKind::WorkspaceToOutput { .. } => true,
            ActionKind::Height(Size::Pixels(_)) | ActionKind::Width(Size::Pixels(_)) => true,
            ActionKind::Height(Size::Percent(_)) | ActionKind::Width(Size::Percent(_)) => false,
            ActionKind::NewColumn | ActionKind::NewRow => baseline.is_some(),
            ActionKind::Floating
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::Workspace(_)
            | ActionKind::Output(_)
            | ActionKind::Mark(_)
            | ActionKind::Scratchpad => false,
        }
    }

    /// Whether the tree currently shows this action's command as having
    /// taken effect (see each variant's notes under CLAUDE.md's "Core model"
    /// section). Only
    /// meaningful for variants `polls()` returns `true` for; every other
    /// variant answers `false` unconditionally.
    ///
    /// Reads through the caller's `connection` rather than opening its own:
    /// `run_poll_then_fallback()` calls this up to 20 times inside a 200ms
    /// grace period, so opening a fresh Sway IPC connection per iteration
    /// would cost ~20 connects and handshakes for a single action.
    ///
    /// Errors reading the tree (transient IPC hiccup, container gone) fold
    /// into "not confirmed yet" rather than propagating: this only ever
    /// exists to return *faster* than the unconditional sleep already does,
    /// never to turn success into failure.
    fn poll_matches(&self, connection: &mut Connection, baseline: Option<swayipc::Rect>) -> bool {
        let container_id = self.container_id;
        match self.kind {
            ActionKind::Split(split) => {
                let expected = match split {
                    Split::V => NodeLayout::SplitV,
                    Split::H => NodeLayout::SplitH,
                };
                parent_node_layout(connection, container_id) == Some(expected)
            }
            ActionKind::Height(Size::Pixels(pixels)) => node_by_id(connection, container_id)
                .is_some_and(|node| height_matches(&node, pixels)),
            ActionKind::Width(Size::Pixels(pixels)) => node_by_id(connection, container_id)
                .is_some_and(|node| width_matches(&node, pixels)),
            ActionKind::Position(position) => position_matches(connection, container_id, &position),
            // Unlike Floating's `floating`/node-type split (see
            // node_is_floating()'s doc comment), `sticky` is a plain `bool`
            // on `Node` with no version-dependent quirk found — confirmed
            // live that `sticky enable` sets it directly and immediately,
            // even on a still-tiled container.
            ActionKind::Sticky => {
                node_by_id(connection, container_id).is_some_and(|node| node.sticky)
            }
            // No fixed target exists for "move right"/"move down" — a
            // successful move can land the window almost anywhere in the
            // tree. Instead, `poll_baseline()` snapshots the window's own
            // `rect` just before the command runs, and this compares the
            // current `rect` against it: any real move changes it (a
            // sibling swap shifts x/y, a row/column change shifts
            // width/height too), confirmed live across both an ordinary
            // sibling swap and a row-changing move that also rewrites the
            // surrounding split structure. Critically, this does *not*
            // compare parent/sibling structure directly — live testing
            // (see tests/live_sway.rs's
            // new_column_falls_back_gracefully_at_the_edge_with_a_large_wait_time)
            // found that
            // the documented "already at the edge" no-op still incidentally
            // restructures the tree around *other* siblings (Sway wraps
            // one in a new split container) even though the target window's
            // own `rect` never changes, so a parent/sibling-list comparison
            // would false-positive on exactly the no-op case this whole
            // mechanism exists to fall back gracefully on.
            ActionKind::NewColumn | ActionKind::NewRow => baseline.is_some_and(|baseline| {
                node_by_id(connection, container_id).is_some_and(|node| node.rect != baseline)
            }),
            ActionKind::WorkspaceToOutput { .. } => {
                self.state_satisfies(container_state_on(connection, container_id).as_ref())
            }
            ActionKind::Height(Size::Percent(_)) | ActionKind::Width(Size::Percent(_)) => false,
            ActionKind::Floating
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::Workspace(_)
            | ActionKind::Output(_)
            | ActionKind::Mark(_)
            | ActionKind::Scratchpad => false,
        }
    }

    /// Snapshots whatever state a wait-time action's `poll_matches()` needs
    /// to compare against *after* the command runs — captured *before* it
    /// runs. Only `NewColumn`/`NewRow` need one (their own current `rect`);
    /// every other action checks against a fixed target derived from its
    /// own fields, with no prior snapshot needed. Returns `None` if the
    /// tree can't be read, which `poll_matches()` treats as "no matcher
    /// applies" for these two variants — without a baseline there's nothing
    /// to compare against, so falling back to the unconditional sleep is
    /// the only sound choice.
    fn poll_baseline(&self, connection: &mut Connection) -> Option<swayipc::Rect> {
        match self.kind {
            ActionKind::NewColumn | ActionKind::NewRow => {
                node_by_id(connection, self.container_id).map(|node| node.rect)
            }
            ActionKind::Split(_)
            | ActionKind::Sticky
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::Position(_)
            | ActionKind::Floating
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::Workspace(_)
            | ActionKind::Output(_)
            | ActionKind::WorkspaceToOutput { .. }
            | ActionKind::Mark(_)
            | ActionKind::Scratchpad => None,
        }
    }

    /// Polls for up to `poll_grace()` for `poll_matches()` to confirm
    /// the command `run_wait_time()` just sent took effect, returning as
    /// soon as it does (the fast path). Several wait-time actions have
    /// legitimate no-op outcomes where confirmation never arrives (a solo
    /// window's clamped resize, a move already at the workspace edge) —
    /// telling that apart from "hasn't happened yet" is impossible from tree
    /// state alone, so once the grace period elapses this falls back to
    /// assuming success and sleeping out the rest of `wait_time`, mirroring
    /// `run_wait_matching_exec_event()`'s `PID_MARKER_FALLBACK_GRACE`
    /// fallback.
    /// Returns whether the change was actually observed —
    /// `false` means the grace period elapsed and this fell back to sleeping,
    /// which the caller reports as `ActionStatus::Unconfirmed` rather than
    /// letting it pass as an ordinary success.
    fn run_poll_then_fallback(
        &self,
        connection: &mut Connection,
        baseline: Option<swayipc::Rect>,
    ) -> bool {
        let wait_time = self.timing;
        // Capped at `wait_time`: `WAIT_TIME_POLL_GRACE` (200ms) is only a
        // sound upper bound when `--wait-time` is at least that — at the
        // CLI's own 20ms default, polling for the full 200ms before
        // falling back would make the fallback path (a legitimate, common
        // case — a solo-window resize clamp, a tiled NewColumn/NewRow
        // already at the edge) take ~220ms total instead of ~40ms. With the
        // cap, the fallback path costs `wait_time` (pre-sleep) + at most
        // `wait_time` (grace plus whatever's left of the sleep) — roughly
        // `2 * wait_time`, confirmed live.
        let grace = self.poll_grace().min(wait_time);
        let poll_started = time::Instant::now();
        loop {
            if self.poll_matches(connection, baseline) {
                if self.verbose {
                    diagnostic!("Confirmed via poll (container id: {})", self.container_id);
                }
                return true;
            }
            if poll_started.elapsed() >= grace {
                break;
            }
            thread::sleep(WAIT_TIME_POLL_INTERVAL);
        }

        if self.verbose {
            diagnostic!(
                "Poll grace period elapsed without confirmation, falling back to wait-time \
                 (container id: {})",
                self.container_id
            );
        }
        thread::sleep(wait_time.saturating_sub(poll_started.elapsed()));
        false
    }

    /// Returns whether the requested change was actually observed. `false` covers every route to "we sent the command and waited,
    /// but never saw it take effect": the poll grace period elapsing, a variant
    /// with no poll matcher at all (a `ppt` size has no pixel figure to check),
    /// and failing to open the poll connection. The caller reports that as
    /// `ActionStatus::Unconfirmed` — it is not the same claim as a confirmed
    /// change, and reporting it as one is what made a wait-time action's
    /// "success" weaker than an event-confirmed action's without saying so.
    fn run_wait_time(&self) -> Result<bool, String> {
        let wait_time = self.timing;
        let container_id = self.container_id;

        if self.verbose {
            diagnostic!(
                "No matching event types for action. Will run Sway command and wait {} ms.",
                wait_time.as_millis()
            );
        }

        // Checked before the sleep as well as after it, and both are wanted.
        //
        // This one is a fast fail: --wait-time is the caller's own knob and is
        // deliberately unbounded, so sleeping it out in full only to report
        // that the target had already closed before the wait even began is
        // pure delay in front of a foregone answer (`wait_time = 3600000`
        // would otherwise sit for an hour before saying so).
        //
        // It doesn't replace the post-sleep check below, which covers the
        // different case of a window closing *during* the wait, and is the one
        // guaranteeing the container still exists at the moment the command is
        // actually sent. The extra get_tree() runs a handful of times per
        // invocation, never in a loop.
        ensure_container_exists(container_id)?;

        // Wait before and after running the Sway command: before, to let
        // other running IPC clients finish their own commands; after, to
        // let this command finish before the next action runs.
        thread::sleep(wait_time);

        // On Sway 1.9 (see node_is_floating()'s doc comment for the same
        // version split), a
        // wait-time action's [con_id=N] criteria matching zero containers is
        // "success" as far as Sway's concerned (there's simply nothing to
        // apply the command to) — unlike an event-confirmed action, which
        // would visibly hang until --timeout instead. Without this check, a
        // container that closed between an earlier action resolving it and
        // this one running would silently no-op rather than error on 1.9.
        // Sway 1.11 already errors clearly ("No matching node.") on its own,
        // confirmed live, making this check redundant there — it's kept for
        // 1.9 compatibility, not because it's still needed on every version.
        ensure_container_exists(container_id)?;

        // One connection for the whole poll cycle — the baseline snapshot
        // and every poll iteration below share it, rather than each tree
        // read opening its own. Failing to open one isn't fatal: there's
        // simply nothing to poll with, so this falls back to the
        // unconditional sleep, exactly as a variant with no matcher does.
        let mut poll_connection = new_connection().ok();

        // Captured before the command runs, not after — see
        // poll_baseline()'s doc comment for why NewColumn/NewRow need a
        // "before" snapshot while every other poll-matched action doesn't.
        let baseline = poll_connection
            .as_mut()
            .and_then(|connection| self.poll_baseline(connection));

        let sway_command = self.sway_command();
        if self.verbose {
            diagnostic!(
                "Sway command: {}",
                escape_control_characters(&sway_command, false)
            );
        }

        run_sway_command(&sway_command)?;

        if self.polls(baseline) {
            if let Some(connection) = poll_connection.as_mut() {
                return Ok(self.run_poll_then_fallback(connection, baseline));
            }
        }

        thread::sleep(wait_time);

        // Nothing was polled for, so nothing was observed. Honest rather than
        // optimistic: this is the ppt-size case, and the no-connection case.
        Ok(false)
    }

    /// Subscribes, confirms the requested state isn't already in effect, sends
    /// the command, and waits for an event that — together with a second state
    /// read — confirms it applied.
    ///
    /// The subscription is opened before the no-op check and before the
    /// command, and that ordering is the whole point rather than an incidental
    /// detail: it's what makes the check-then-command sequence safe against
    /// another client racing it. See `already_at_target()`'s doc comment. The
    /// cost is one Sway IPC connection opened and immediately dropped on the
    /// short-circuit path, which is the cheap half of the trade — that path
    /// already pays for a `get_tree()`.
    ///
    /// The container-exists check stays ahead of the subscription only to
    /// fail fast on a closed window before opening a subscription for it;
    /// `event_loop()` bounds the subscribe handshake itself (by
    /// `IPC_ROUND_TRIP_TIMEOUT`, like every other request).
    fn run_wait_matching_events(&self) -> Result<ActionOutcome, String> {
        // Same check and same reason as run_wait_time()'s: on Sway 1.9 a
        // [con_id=N] criteria matching zero containers is
        // "success", so without this an action whose container closed since
        // it was resolved would send its command successfully and then block
        // for the full --timeout waiting on a confirmation event that can
        // never arrive, reporting a timeout — pointing the
        // user at --timeout rather than at the closed window. Sway 1.11
        // already errors clearly ("No matching node.") from
        // run_sway_command() below, confirmed live, making this redundant
        // there but still required for 1.9.
        //
        // Costs one extra get_tree() per event-confirmed action, partly
        // duplicating the one already_at_target() makes for six of the seven
        // variants. Accepted deliberately: one check in one place covers
        // every variant (including Mark, which has no already_at_target()
        // arm at all), and this runs a handful of times per invocation, not
        // in a loop.
        let container_id = self.container_id;
        let container = existing_container(container_id)?;
        if self.kind.needs_a_window() && !is_window(&container) {
            return Err(split_container_error(container_id));
        }

        let event_loop = event_loop(&[EventType::Window])?;

        if self.already_at_target()? {
            if self.verbose {
                diagnostic!(
                    "Already at target, nothing to move (container id: {})",
                    container_id
                );
            }
            return Ok(ActionOutcome::AlreadySatisfied);
        }

        let deadline = deadline_after(self.timing)?;
        let sway_command = self.sway_command();
        if self.verbose {
            diagnostic!(
                "Sway command: {}",
                escape_control_characters(&sway_command, false)
            );
        }
        run_sway_command(&sway_command)?;

        // spawn_event_forwarder() reads events on a separate thread and
        // forwards them through a channel, so recv_timeout() below enforces a real deadline even if
        // the event stream itself never produces another event (a blocking
        // iterator has no way to time out on its own).
        //
        // The thread outlives this function whenever it's still blocked on the
        // socket as we return: there's no public way to interrupt a blocking
        // EventStream read from another thread, so it can only notice its
        // receiver is gone the next time an event actually arrives, at which
        // point send() fails, the loop breaks, and the thread and its Sway IPC
        // socket are released.
        //
        // That drains itself in practice rather than accumulating, because the
        // events that wake a stale reader are the very ones the *next*
        // event-confirmed action produces: Sway broadcasts every window event
        // to every subscription, so step N's own confirming event is what
        // retires step N-1's reader. Measured against a live compositor rather
        // than assumed — a 300-step layout of event-confirmed actions peaked at
        // 2 threads and 3 sockets, and a 15-step layout launching a real window
        // per step at 3 and 3. Pinned by tests/live_sway.rs's
        // `event_reader_threads_stay_bounded_across_a_long_layout`.
        //
        // The bound depends on later events arriving: the *last*
        // event-confirmed action's reader stays blocked until the process
        // exits, which is why it's a small constant rather than zero. One
        // owned event reader per SwayLaunch run, with an explicit shutdown
        // protocol, would buy nothing for a CLI that exits moments later, and
        // would replace a per-action subscription (which is also what makes
        // each action's wait independent, see Scratchpad's shared
        // WindowChange::Move in matching_window_change_event()) with shared
        // mutable state to coordinate. Revisit if this ever becomes a
        // library, gains a long-lived mode, or needs cancellation.
        let event_receiver = spawn_event_forwarder(event_loop);

        loop {
            let remaining = deadline.saturating_duration_since(time::Instant::now());
            if remaining.is_zero() {
                return Err(event_timeout_message(self.timing));
            }

            let event = match event_receiver.recv_timeout(remaining) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(event_timeout_message(self.timing));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Event stream closed unexpectedly".to_string());
                }
            };

            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            let window = match event {
                Event::Window(window) => window,
                _ => continue,
            };

            match self.matches_window_event(&window) {
                Ok(result) => {
                    // The event identifies the right container and the right
                    // event type, which is not the same as the requested state
                    // having been what applied — another IPC client driving the
                    // same window produces events indistinguishable from ours.
                    // See state_satisfied()'s doc comment.
                    if !self.state_satisfied()? {
                        if self.verbose {
                            diagnostic!(
                                "Event match: {:?} container id {} ({}), but the requested state \
                                 is not in effect — another client may be driving this window; \
                                 waiting for the next event",
                                window.change,
                                window.container.id,
                                result
                            );
                        }
                        continue;
                    }

                    if self.verbose {
                        diagnostic!(
                            "Event match: {:?} container id {} ({})",
                            window.change,
                            window.container.id,
                            result
                        );
                    }

                    return Ok(ActionOutcome::Changed);
                }
                Err(error_result) => {
                    if self.verbose {
                        diagnostic!(
                            "Event mismatch: {:?} container id {} ({})",
                            window.change,
                            window.container.id,
                            error_result
                        );
                    }
                }
            }
        }
    }

    /// Whether `window` is this action's confirming event: its kind's event
    /// type, for its own container. Never matches for a kind that has
    /// no confirming event at all.
    fn matches_window_event(
        &self,
        window: &WindowEvent,
    ) -> Result<WindowEventMatch, WindowEventMatchError> {
        if self.kind.matching_window_change_event() != Some(window.change) {
            return Err(WindowEventMatchError::EventChangeTypeMismatch);
        }

        if self.container_id == window.container.id {
            return Ok(WindowEventMatch::WindowContainerIdMatch);
        }

        Err(WindowEventMatchError::NoMatchingEvent)
    }
}

impl ExecAction<'_> {
    /// Launches this action's command and waits for its window, returning the
    /// window's container id and whether this run can prove it launched it
    /// (see `LaunchOwnership`). A freshly launched window is inherently a
    /// change, so unlike `SwayAction::run()` there's no outcome to report.
    ///
    /// Every error is prefixed with `launch "<command>"`, for the same reason
    /// `SwayAction::run()` prefixes its own with the command verb.
    pub(super) fn run(&self) -> Result<(i64, LaunchOwnership), RunError> {
        if self.verbose {
            diagnostic!(
                "Sway action: {}",
                escape_control_characters(&self.to_string(), false)
            );
        }

        self.run_wait_matching_exec_event()
            .map_err(|error| RunError {
                message: format!("launch \"{}\": {}", self.command, error.message),
                ..error
            })
    }

    /// The launch counterpart of `run_wait_matching_events()`: matching purely
    /// on event content (app_id/class, or nothing at all with no filter) is
    /// ambiguous when more than one qualifying `New` window can appear
    /// around the same time — a concurrently-running second `sway-launch`
    /// process, or any other coincidentally-timed window — since Sway
    /// broadcasts window events to every IPC connection. To disambiguate,
    /// the launched command's environment is tagged with a random,
    /// per-invocation marker (`env <PID_MARKER_VAR>=<token> <command>`,
    /// prepended without otherwise touching the user's command), and a
    /// content-matching event is only accepted outright once
    /// `/proc/<event pid>/environ` confirms that marker.
    ///
    /// A content-matching event whose marker doesn't confirm isn't rejected
    /// outright, though: some applications (browsers, editors) are
    /// single-instance and forward a second invocation's request to an
    /// already-running process before exiting, so the window that
    /// eventually appears is legitimately the right one, owned by a PID
    /// that was never given our marker. The first such event is kept as a
    /// fallback candidate, used once either the marked process (or a marked
    /// descendant) is known to be gone — nothing marker-confirmed is coming —
    /// or `PID_MARKER_FALLBACK_GRACE` elapses, whichever comes first, bounding
    /// how long a genuinely ambiguous case can add to the wait.
    ///
    /// "Gone" means *seen, then gone*: `/proc` is scanned for the marker every
    /// `MARKER_SCAN_INTERVAL` from the moment the command is sent until it's
    /// been seen, and, while a fallback candidate is held, until it disappears
    /// (`MarkerTracker`). A marker that was never seen is not evidence of
    /// anything, because that's also what the moments before the marked
    /// process exists look like (Sway's `exec` returns once it has forked),
    /// and what a launcher that drops the environment looks like. Reading
    /// "absent" as "gone" there would claim whichever matching window arrived
    /// first — measured live as two concurrent `--template` runs reporting
    /// the same container id in 4 of 7 trials, each taking the other's window
    /// during its own startup gap. A marker that's never seen therefore always
    /// ends through the grace period, as `Adopted`.
    ///
    /// Which of those two ends the wait is reported back as the returned
    /// `LaunchOwnership` (see its own doc comment): a marked process that's
    /// gone means the window is ours by elimination, while the grace period
    /// merely elapsing means we adopted a window we never confirmed was ours.
    /// Only `--rollback-on-error` reads this, and only to decide whether
    /// killing the window is this run's business.
    ///
    /// Same background-thread/socket lifetime as `run_wait_matching_events()`:
    /// the thread reading this action's event stream may outlive this function
    /// if still blocked on the socket when we return, but is retired by the
    /// next window event to arrive — which, in a multi-step run, is the next
    /// step's own. Measured bounded, not accumulating; see the comment at that
    /// function's `spawn_event_forwarder()` call for the mechanism and the
    /// numbers.
    ///
    /// A `New` window rejected by the `--app-id`/`--class` filter is recorded
    /// rather than silently dropped, and a timeout is reported through
    /// `exec_timeout_message()` using what was seen: the rejected windows, or,
    /// with none, whether the launched command is still running at all. A typo'd
    /// command and a wrong `--app-id` otherwise both end in the same bare
    /// timeout, pointing the user at `--timeout` when raising it only makes the
    /// failure slower.
    ///
    /// A rejected window whose process carries this run's marker is ours, so
    /// it isn't given up on: its app_id/class are re-read from the tree every
    /// `LATE_PROPERTY_RECHECK_INTERVAL` until they match (returned as
    /// `Launched`), it closes, or the deadline passes. Some clients set
    /// these only after mapping — XWayland apps that set `WM_CLASS` late, or a
    /// Wayland client calling `set_app_id` after its first commit — and Sway
    /// sends no window event when they change, confirmed live with foot's
    /// runtime app-id escape sequence. Without the recheck such an app waited
    /// out the whole `--timeout` beside the very window it was waiting for.
    fn run_wait_matching_exec_event(&self) -> Result<(i64, LaunchOwnership), RunError> {
        let ExecAction {
            verbose,
            timeout,
            filter,
            title,
            ..
        } = *self;

        let event_loop = event_loop(&[EventType::Window])?;

        let deadline = deadline_after(timeout)?;
        let token = generate_pid_marker_token();
        let sway_command = self.sway_command(&token);
        if verbose {
            diagnostic!(
                "Sway command: {}",
                escape_control_characters(&sway_command, false)
            );
        }
        run_single_sway_command(&sway_command)?;

        let event_receiver = spawn_event_forwarder(event_loop);

        // Every New window the app_id/class filter turned away, reported if
        // the wait ends without a match — see exec_timeout_message().
        let mut unmatched_windows: Vec<UnmatchedWindow> = Vec::new();

        // The first content-matching-but-unconfirmed event seen, and when —
        // used both to cap how long the grace period below can run and as
        // the value returned once it's used.
        let mut fallback: Option<(i64, time::Instant)> = None;

        let mut marker = MarkerTracker::default();
        let mut next_scan = time::Instant::now();

        // Windows the filter turned away that carried this run's marker:
        // rechecked through the tree until one matches, since an app can set
        // its app_id/class after mapping and Sway sends no event when it does.
        let mut rechecks: Vec<i64> = Vec::new();
        let mut next_recheck = time::Instant::now();
        let mut recheck_connection: Option<Connection> = None;

        // Every window that mapped during this run. Only these get a second
        // look when their title changes: an older window renaming itself is
        // never the one this command opened.
        let mut opened_this_run: Vec<i64> = Vec::new();

        loop {
            let effective_deadline = match fallback {
                Some((_, first_seen)) => first_seen
                    .checked_add(PID_MARKER_FALLBACK_GRACE)
                    .map_or(deadline, |capped| deadline.min(capped)),
                None => deadline,
            };
            let now = time::Instant::now();
            let remaining = effective_deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                if let Some((container_id, _)) = fallback {
                    if verbose {
                        diagnostic!(
                            "No PID-marker-confirmed match arrived; using earlier \
                             content-matched container id {} (adopted, not confirmed as \
                             launched by this run){}",
                            container_id,
                            unfiltered_fallback_hint(filter)
                        );
                    }
                    return Ok((container_id, LaunchOwnership::Adopted));
                }
                // Only worth asking when nothing else explains the timeout: a
                // rejected window already says what went wrong.
                let command_running = if unmatched_windows.is_empty() {
                    match scan_marker_presence(PID_MARKER_VAR, &token, marker.previously_seen()) {
                        MarkerPresence::Present(_) => Some(true),
                        MarkerPresence::Absent => Some(false),
                        MarkerPresence::Unknown | MarkerPresence::Unavailable => None,
                    }
                } else {
                    None
                };
                return Err(RunError {
                    message: exec_timeout_message(
                        timeout,
                        filter,
                        title,
                        &unmatched_windows,
                        command_running,
                    ),
                    unmatched_windows,
                    container_id: None,
                    launch_ownership: None,
                    window_timed_out: true,
                });
            }

            if marker.needs_scan(fallback.is_some()) && now >= next_scan {
                let presence =
                    scan_marker_presence(PID_MARKER_VAR, &token, marker.previously_seen());
                if marker.observe(presence) && verbose {
                    diagnostic!(
                        "Could not read /proc — PID-marker correlation is degraded on this system"
                    );
                }
                next_scan = now + MARKER_SCAN_INTERVAL;
                if let (true, Some((container_id, _))) = (marker.is_gone(), fallback) {
                    if verbose {
                        diagnostic!(
                            "Marked process no longer running; using fallback container id {} \
                             (nothing marker-confirmed can still arrive, so this window is this \
                             run's own){}",
                            container_id,
                            unfiltered_fallback_hint(filter)
                        );
                    }
                    return Ok((container_id, LaunchOwnership::Launched));
                }
            }

            if !rechecks.is_empty() && now >= next_recheck {
                next_recheck = now + LATE_PROPERTY_RECHECK_INTERVAL;
                if let Some(container_id) = self.recheck_launched_windows(
                    &mut recheck_connection,
                    &mut rechecks,
                    &mut unmatched_windows,
                ) {
                    if verbose {
                        diagnostic!(
                            "Recheck match: container id {} set a matching app_id/class/title \
                             after mapping (PID-marker-confirmed)",
                            container_id
                        );
                    }
                    return Ok((container_id, LaunchOwnership::Launched));
                }
            }

            let mut wait = remaining;
            if marker.needs_scan(fallback.is_some()) {
                wait = wait.min(next_scan.saturating_duration_since(time::Instant::now()));
            }
            if !rechecks.is_empty() {
                wait = wait.min(next_recheck.saturating_duration_since(time::Instant::now()));
            }

            let event = match event_receiver.recv_timeout(wait) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Event stream closed unexpectedly".to_string().into());
                }
            };

            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string().into()),
            };

            let window = match event {
                Event::Window(window) => window,
                _ => continue,
            };

            let container_id = window.container.id;
            let retitled = window.change == WindowChange::Title
                && title.is_some()
                && opened_this_run.contains(&container_id)
                && fallback.is_none_or(|(fallback_id, _)| fallback_id != container_id);
            if window.change == WindowChange::New {
                opened_this_run.push(container_id);
            }

            let matched = if retitled {
                self.matches_window(&window.container)
            } else {
                self.matches_window_event(&window)
            };
            if let Err(mismatch) = matched {
                if matches!(
                    mismatch,
                    WindowEventMatchError::WindowAppIdMismatch
                        | WindowEventMatchError::WindowClassMismatch
                        | WindowEventMatchError::WindowTitleMismatch
                ) {
                    if let Some(seen) = unmatched_windows
                        .iter_mut()
                        .find(|unmatched| unmatched.container_id == container_id)
                    {
                        seen.title = window.container.name.clone();
                        continue;
                    }
                    let unmatched = UnmatchedWindow {
                        container_id,
                        app_id: window.container.app_id.clone(),
                        class: window
                            .container
                            .window_properties
                            .as_ref()
                            .and_then(|properties| properties.class.clone()),
                        title: window.container.name.clone(),
                        launched_by_this_run: window
                            .container
                            .pid
                            .is_some_and(|pid| process_has_env_var(pid, PID_MARKER_VAR, &token)),
                    };
                    if verbose {
                        diagnostic!(
                            "Event mismatch: {:?} {} ({}){}",
                            window.change,
                            describe_unmatched_window(&unmatched, title.is_some()),
                            mismatch,
                            if unmatched.launched_by_this_run {
                                " — rechecking it in case it sets its app_id/class/title after \
                                 mapping"
                            } else {
                                ""
                            }
                        );
                    }
                    if unmatched.launched_by_this_run {
                        rechecks.push(unmatched.container_id);
                        next_recheck = time::Instant::now() + LATE_PROPERTY_RECHECK_INTERVAL;
                    }
                    unmatched_windows.push(unmatched);
                }
                continue;
            }
            // A window that failed its title when it mapped and matches now
            // is no longer one to report.
            unmatched_windows.retain(|unmatched| unmatched.container_id != container_id);

            let pid_confirmed = window
                .container
                .pid
                .is_some_and(|pid| process_has_env_var(pid, PID_MARKER_VAR, &token));

            if pid_confirmed {
                if verbose {
                    diagnostic!(
                        "Event match: {:?} container id {} (PID-marker-confirmed)",
                        window.change,
                        window.container.id
                    );
                }
                return Ok((window.container.id, LaunchOwnership::Launched));
            }

            if fallback.is_none() {
                if verbose {
                    diagnostic!(
                        "Event content-matches but PID marker unconfirmed (container id {}) \
                         — keeping as a fallback candidate",
                        window.container.id
                    );
                }
                fallback = Some((window.container.id, time::Instant::now()));
                // Re-check straight away rather than at the next interval: if
                // the marked process was already seen and has since exited
                // (the single-instance case), this is the moment to say so.
                next_scan = time::Instant::now();
            }
        }
    }

    /// One recheck of the windows in `rechecks` (see the loop above): the
    /// first one whose app_id/class now passes the filter, if any. Refreshes
    /// each window's recorded app_id/class in `unmatched_windows`, so a
    /// timeout reports what the window ended up with rather than what it
    /// mapped with, and drops windows that have closed from `rechecks`. A
    /// tree that can't be read is skipped until the next interval, with the
    /// connection reopened then.
    fn recheck_launched_windows(
        &self,
        connection: &mut Option<Connection>,
        rechecks: &mut Vec<i64>,
        unmatched_windows: &mut [UnmatchedWindow],
    ) -> Option<i64> {
        if connection.is_none() {
            *connection = new_connection().ok();
        }
        let Some(rechecked) = connection.as_mut().and_then(|connection| {
            recheck_launched_windows_now(connection, rechecks, self.filter, self.title)
        }) else {
            *connection = None;
            return None;
        };

        for window in &rechecked {
            if let Some(unmatched) = unmatched_windows
                .iter_mut()
                .find(|unmatched| unmatched.container_id == window.container_id)
            {
                unmatched.app_id = window.app_id.clone();
                unmatched.class = window.class.clone();
                unmatched.title = window.title.clone();
            }
        }
        *rechecks = rechecked.iter().map(|window| window.container_id).collect();
        rechecked
            .iter()
            .find(|window| window.matches)
            .map(|window| window.container_id)
    }

    /// Whether `window` is the `New` window this launch is waiting for,
    /// filtered on `--app-id`/`--class` and `--title` when set.
    ///
    /// Only `New` identifies a window as the one just launched. `Move` ("the
    /// view has been reparented in the tree") fires for any window whenever
    /// Sway's tree is restructured, including pre-existing windows unrelated to
    /// this exec — accepting it here let sway-launch return the wrong container
    /// id.
    fn matches_window_event(
        &self,
        window: &WindowEvent,
    ) -> Result<WindowEventMatch, WindowEventMatchError> {
        if window.change != WindowChange::New {
            return Err(WindowEventMatchError::EventChangeTypeMismatch);
        }
        self.matches_window(&window.container)
    }

    /// `matches_window_event()`'s check of the window itself, which a window
    /// opened during this run also gets again when its title changes.
    fn matches_window(
        &self,
        node: &swayipc::Node,
    ) -> Result<WindowEventMatch, WindowEventMatchError> {
        let matched = match self.filter {
            None => WindowEventMatch::NewWindowMatchWithoutCheck,
            Some(filter @ LaunchFilter::AppId(_)) => match filter.matches(node) {
                true => WindowEventMatch::WindowAppId,
                false => return Err(WindowEventMatchError::WindowAppIdMismatch),
            },
            Some(filter @ LaunchFilter::Class(_)) => match filter.matches(node) {
                true => WindowEventMatch::WindowClass,
                false => return Err(WindowEventMatchError::WindowClassMismatch),
            },
        };
        match self.title {
            Some(title) if !title.matches(node.name.as_deref()) => {
                Err(WindowEventMatchError::WindowTitleMismatch)
            }
            _ => Ok(matched),
        }
    }
}

/// The instant a timeout-bounded action gives up, computed *before* its
/// command is sent rather than after. `checked_add` rather than `+`, so an
/// unrepresentable deadline errors instead of panicking — and doing it first
/// means that error arrives before anything has happened: computed afterwards
/// (as it once was), it reported a timeout over a command that had already
/// taken effect, leaving a launched window behind with no container id
/// printed. `validate_timeout_seconds()` already rejects any value this could
/// fail on, so this is the backstop for a caller that builds a `SwayLaunch`
/// directly.
fn deadline_after(timeout: time::Duration) -> Result<time::Instant, String> {
    time::Instant::now().checked_add(timeout).ok_or_else(|| {
        format!(
            "timeout of {} sec is too large to represent",
            timeout.as_secs()
        )
    })
}

/// Why a window-only action refuses a split container. `SwayAction::run()`
/// prefixes it with the action's verb.
fn split_container_error(container_id: i64) -> String {
    format!(
        "container {} is a split container, not a window — Sway sends no window event for it, \
         so this can't be confirmed; target a window inside it instead",
        container_id
    )
}

/// An event-confirmed action's timeout. `SwayAction::run()` prefixes it with
/// the action's own verb, which is what tells several such actions apart.
fn event_timeout_message(timeout: time::Duration) -> String {
    format!("no matching window event within {} sec", timeout.as_secs())
}

/// What a `--verbose` fallback line adds when the launch had no
/// `--app-id`/`--class` filter: without one, any new window that appears
/// during the wait qualifies, so an unrelated one can be picked up (#18).
fn unfiltered_fallback_hint(filter: Option<LaunchFilter>) -> &'static str {
    match filter {
        Some(_) => "",
        None => {
            "; this launch has no --app-id/--class filter, so any new window could have \
                 matched; pass -a/-c (app_id/class in a file) to pin it to its app"
        }
    }
}

/// A launch's timeout, saying as much as the wait observed about why.
///
/// The common causes otherwise look identical: a `--app-id`/`--class` that
/// doesn't match what the application actually reports (it varies with
/// version, packaging and backend — an XWayland window has no app_id at all),
/// and a command that never started (Sway's `exec` gives no feedback). So:
/// rejected windows are named, with the filter to check; with none, a launched
/// command that's no longer running (`command_running == Some(false)`) is
/// called out as such; and only a command still running gets the hint that it
/// may simply be slow. `None` means the marker scan couldn't tell, and adds
/// nothing.
fn exec_timeout_message(
    timeout: time::Duration,
    filter: Option<LaunchFilter>,
    title: Option<&TitlePattern>,
    unmatched_windows: &[UnmatchedWindow],
    command_running: Option<bool>,
) -> String {
    let criteria = match (filter, title) {
        (Some(filter), Some(title)) => format!(" with {} and {}", filter, title),
        (Some(filter), None) => format!(" with {}", filter),
        (None, Some(title)) => format!(" with {}", title),
        (None, None) => String::new(),
    };
    let base = format!(
        "no window{} appeared within {} sec",
        criteria,
        timeout.as_secs()
    );

    if !unmatched_windows.is_empty() {
        let described: Vec<String> = unmatched_windows
            .iter()
            .map(|window| format!("({})", describe_unmatched_window(window, title.is_some())))
            .collect();
        let noun = if unmatched_windows.len() == 1 {
            "window"
        } else {
            "windows"
        };
        let properties = if title.is_some() {
            "app_id/class/title"
        } else {
            "app_id/class"
        };
        let recheck_note = if unmatched_windows
            .iter()
            .any(|window| window.launched_by_this_run)
        {
            format!(
                " (a window opened by this command was rechecked until the timeout, in case the \
                 app sets its {} after mapping)",
                properties
            )
        } else {
            String::new()
        };
        return format!(
            "{}; {} other new {} did {} — check {}{}",
            base,
            unmatched_windows.len(),
            noun,
            described.join(", "),
            if title.is_some() {
                "--app-id/--class/--title"
            } else {
                "--app-id/--class"
            },
            recheck_note
        );
    }

    match command_running {
        Some(false) => format!(
            "{}: the launched command exited without opening a window — check that it is on \
             PATH in Sway's environment",
            base
        ),
        Some(true) => format!(
            "{}; the launched command is still running — it may be slow to start (see \
             --timeout) or not open a window at all",
            base
        ),
        None => base,
    }
}

/// `--existing --wait` (#108): the one window `criteria` accepts, waiting
/// up to `timeout` for it to open when none has yet, for a window something
/// other than this run is starting (an autostart entry, a launcher).
///
/// Subscribes before the first tree read, the same order
/// `run_wait_matching_events()` uses, so a window mapping between the two
/// can't be missed. Every window event then prompts another read, and so does
/// every `LATE_PROPERTY_RECHECK_INTERVAL` without one, since a client can
/// set its app_id/class after mapping and Sway sends no event when it does.
/// Several matches end the wait with the same error `--existing` gives:
/// waiting longer can't make the choice less ambiguous.
pub(super) fn run_wait_for_existing_window(
    criteria: WindowCriteria,
    timeout: time::Duration,
    verbose: bool,
) -> Result<i64, RunError> {
    let events = spawn_event_forwarder(event_loop(&[EventType::Window])?);
    let deadline = deadline_after(timeout)?;
    let mut connection = new_connection()?;
    let mut announced = false;

    loop {
        if let Some(container_id) = awaited_existing_container_id(&mut connection, criteria)? {
            return Ok(container_id);
        }
        if verbose && !announced {
            diagnostic!("No window matches {} yet; waiting for one", criteria);
            announced = true;
        }
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        if remaining.is_zero() {
            return Err(RunError {
                window_timed_out: true,
                ..existing_wait_timeout_message(criteria, timeout).into()
            });
        }
        match events.recv_timeout(remaining.min(LATE_PROPERTY_RECHECK_INTERVAL)) {
            Ok(Ok(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(Err(error)) => return Err(error.to_string().into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Event stream closed unexpectedly".to_string().into());
            }
        }
        // One read covers every event that arrived meanwhile.
        while let Ok(Ok(_)) = events.try_recv() {}
    }
}

fn existing_wait_timeout_message(criteria: WindowCriteria, timeout: time::Duration) -> String {
    format!(
        "no window with {} appeared within {} sec (--list-windows shows every open window)",
        criteria,
        timeout.as_secs()
    )
}

/// How often `run_wait_matching_exec_event()` re-reads the tree for a window
/// that carried this run's marker but failed the `--app-id`/`--class` filter
/// when it mapped. Slower than `WAIT_TIME_POLL_INTERVAL`, since this can run
/// for the whole `--timeout` rather than a 200ms grace period, and a client
/// setting its app_id/class late is waiting on its own startup, not on Sway.
const LATE_PROPERTY_RECHECK_INTERVAL: time::Duration = time::Duration::from_millis(50);

/// `con_id 7, app_id "foot"`, with the window's title too when the launch
/// was matching on one.
fn describe_unmatched_window(window: &UnmatchedWindow, with_title: bool) -> String {
    let mut identity = match (&window.app_id, &window.class) {
        (Some(app_id), _) => format!("app_id \"{}\"", app_id),
        (None, Some(class)) => format!("class \"{}\"", class),
        (None, None) => "no app_id or class".to_string(),
    };
    if with_title {
        match &window.title {
            Some(title) => identity.push_str(&format!(
                ", title \"{}\"",
                escape_control_characters(title, false)
            )),
            None => identity.push_str(", no title"),
        }
    }
    let ownership = if window.launched_by_this_run {
        ", opened by this command"
    } else {
        ""
    };
    format!("con_id {}, {}{}", window.container_id, identity, ownership)
}

#[cfg(test)]
mod tests;
