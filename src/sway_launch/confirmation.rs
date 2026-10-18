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
use super::interrupt::{self, WaitEnd};
use super::ipc::{
    event_loop, new_connection, run_single_sway_command, run_sway_commands, spawn_event_forwarder,
};
use super::outcome::{ActionOutcome, LaunchOwnership, RunError, UnmatchedWindow};
use super::process_marker::{
    generate_pid_marker_token, is_sibling_launchs_window, launch_evidence, process_start_ticks,
    register_launch, scan_marker_presence, LaunchEvidence, MarkerPresence, MarkerTracker,
    MARKER_SCAN_INTERVAL, PID_MARKER_FALLBACK_GRACE, PID_MARKER_VAR,
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
use std::{fmt, time};
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
    /// in `state_satisfies()`: `run_batch()` accepts a matching event only
    /// once that returns `true`, so a kind without a real arm there
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
            // run_batch() checks for and short-circuits that case once the
            // subscription is open but before the command is sent (that
            // ordering is load-bearing — see
            // is_skipped_when_already_in_effect()'s doc comment), rather than
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
            | ActionKind::SwayCommand(_)
            | ActionKind::Position(_) => None,
            // "move scratchpad" reparents the container into the special
            // __i3_scratch workspace, which — like Workspace/Output above —
            // fires WindowChange::Move. Confirmed live: for a previously
            // *tiled* window, Sway also auto-floats it first, firing a
            // WindowChange::Floating event before the Move — matches_window_event()
            // simply doesn't match that one (its change type isn't the one
            // returned here) and the loop keeps waiting for the Move that follows.
            // Re-scratchpadding an already-scratchpadded window is a no-op
            // that fires no event at all (confirmed live), so run_batch()
            // short-circuits that case the same way it does for
            // Workspace/Output/Floating/Fullscreen/Focus.
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
            | ActionKind::SwayCommand(_)
            | ActionKind::Position(_)
            | ActionKind::Scratchpad => false,
        }
    }

    /// Whether this kind of action is confirmed by an IPC event (and so waits
    /// on `--timeout`) rather than by polling (and so waits on `--wait-time`).
    pub(super) fn is_event_confirmed(&self) -> bool {
        self.matching_window_change_event().is_some()
    }

    /// Whether `run_batch()` leaves this kind out of the command when its
    /// state already holds, reporting it `AlreadySatisfied`. Sway fires no
    /// event for a command that turns out to be a no-op, so without the check
    /// the action would wait out `--timeout` for an event that was never
    /// coming.
    ///
    /// `Workspace`/`Output` were the first two found needing this (moving to
    /// the workspace or output the window is already on reparents nothing).
    /// `Floating`/`Fullscreen`/`Focus` have the identical failure mode,
    /// confirmed live: re-running `--floating`/`--fullscreen`/`--focus` on a
    /// window already in that state hung 5s and then errored out. `Scratchpad`
    /// needs it as well: `[con_id] move scratchpad` on a window already in the
    /// scratchpad fires no event at all, confirmed live, and is checked
    /// against the ancestor workspace rather than `floating`, since a
    /// scratchpad window also reports as floating (see
    /// `matching_window_change_event()`).
    ///
    /// `Mark` is the one event-confirmed kind deliberately excluded even
    /// though `state_satisfies()` can answer for it: re-applying a mark the
    /// container already has *does* fire `WindowChange::Mark`, confirmed live,
    /// so there's no hang to avoid, and short-circuiting would report it as
    /// `AlreadySatisfied` when the command genuinely does re-apply. A polled
    /// kind has nothing to hang on, and is always sent.
    ///
    /// **The check runs only once the subscription is open.** It reads state
    /// another client can change a moment later, and the no-op it exists to
    /// detect is precisely the case Sway fires no event for, so if the state
    /// flipped between the read and the subscription, the command would be a
    /// no-op, no event would arrive, and the action would wait out the full
    /// `--timeout`. Subscribing first closes the window completely rather than
    /// narrowing it: a change landing before the read is caught by the read,
    /// one landing after it by the subscription. Measured against a live
    /// compositor while the check still ran ahead of the subscription:
    /// sweeping a competing `floating enable` across a 0-20ms window after
    /// spawn, 5 of 40 trials hung the full timeout, clustered 2.5-4ms in.
    pub(super) fn is_skipped_when_already_in_effect(&self) -> bool {
        self.is_event_confirmed() && !matches!(self, ActionKind::Mark(_))
    }
}

impl SwayAction<'_> {
    /// Whether `state` is the state this action asked for. `None` — the
    /// container isn't in the tree at all — is never satisfied: a window that
    /// has closed isn't floating, focused or on the target workspace, and
    /// reporting it as already-there would short-circuit an action into
    /// claiming success over a container that no longer exists. `false` for
    /// every kind that confirms by polling instead, whatever the state, except
    /// `WorkspaceToOutput`, whose `poll_matches()` arm asks it.
    ///
    /// `run_batch()` asks it twice, for two different questions. Before the
    /// command runs, to detect a no-op Sway would fire no event for (see
    /// `ActionKind::is_skipped_when_already_in_effect()`). *After* a matching
    /// event arrives, again, because an event proves only that Sway emitted
    /// that event type for this container, not that the state this action
    /// asked for is the one that ended up applying.
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
    /// Pure: `run_batch()` fetches one `ContainerState` and this decides, so
    /// every case is a unit test rather than a scripted compositor exchange
    /// (see CLAUDE.md's coverage note).
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
            | ActionKind::SwayCommand(_)
            | ActionKind::Position(_) => false,
        }
    }

    /// How long `run_batch()` may keep polling for this variant's own
    /// confirmation after sending its command. Pure, so the choice stays unit-testable
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
            | ActionKind::SwayCommand(_)
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
    /// Pure — no IPC — so `run_batch()` can decide whether an action is
    /// worth polling for at all once the command is sent, and so the decision
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
            | ActionKind::SwayCommand(_)
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
    /// `run_batch()` calls this up to 20 times per action inside a 200ms
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
            | ActionKind::SwayCommand(_)
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
            | ActionKind::SwayCommand(_)
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

impl ActionKind<'_> {
    /// Whether this kind is sent in one command with its neighbours (#32)
    /// rather than on its own. Sway runs a `,`-joined command's parts in
    /// order and commits them together, so the window takes every one of
    /// them at once instead of floating, then resizing, then jumping into
    /// place, and a client sees no intermediate geometry to react to (#17).
    ///
    /// What stays out is whatever the next action can't be judged without:
    /// `NewColumn`/`NewRow` need the move guard read against the tree the
    /// previous action left, `Workspace`/`Output`/`WorkspaceToOutput` change
    /// where the window is, which a later action's already-there check
    /// depends on (a window already on output A before a `--workspace` move
    /// to output B), and `Scratchpad` hides the window, so it goes last on
    /// its own.
    pub(super) fn joins_batch(&self) -> bool {
        match self {
            ActionKind::Split(_)
            | ActionKind::Floating
            | ActionKind::Sticky
            | ActionKind::Fullscreen
            | ActionKind::Focus
            | ActionKind::Mark(_)
            | ActionKind::Height(_)
            | ActionKind::Width(_)
            | ActionKind::SwayCommand(_)
            | ActionKind::Position(_) => true,
            ActionKind::NewColumn
            | ActionKind::NewRow
            | ActionKind::Workspace(_)
            | ActionKind::Output(_)
            | ActionKind::WorkspaceToOutput { .. }
            | ActionKind::Scratchpad => false,
        }
    }
}

/// The one command a batch sends: every action's verb behind a single
/// `[con_id=N]`, which Sway applies to each `,`-separated part.
pub(super) fn batch_command(container_id: i64, actions: &[&SwayAction<'_>]) -> String {
    let verbs: Vec<String> = actions
        .iter()
        .map(|action| action.sway_command_verb())
        .collect();
    format!("[con_id={}] {}", container_id, verbs.join(", "))
}

/// The error for the first part of a batch Sway refused, prefixed with that
/// action's verb as a lone action's error is, or `None` when every part
/// succeeded. Sway stops at a part it can't parse, so a reply shorter than
/// the batch with no failure in it still means something didn't run.
fn batch_failure(sent: &[&SwayAction<'_>], outcomes: &[Result<(), String>]) -> Option<String> {
    if let Some((action, error)) = sent
        .iter()
        .zip(outcomes)
        .find_map(|(action, outcome)| outcome.as_ref().err().map(|error| (action, error)))
    {
        return Some(format!("{}: {}", action.sway_command_verb(), error));
    }
    if outcomes.len() != sent.len() {
        return Some(format!(
            "Sway answered {} of the {} commands sent together; the rest never ran",
            outcomes.len(),
            sent.len()
        ));
    }
    None
}

/// How far one action of a batch has got while `run_batch()` waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatchProgress {
    /// Its state held before anything was sent, so it wasn't.
    AlreadySatisfied,
    /// Sent; waiting on its event (event-confirmed) or a poll (polled).
    Waiting,
    /// Sent, and its event plus a state read confirmed it.
    EventSeen,
    Changed,
    Unconfirmed,
}

impl BatchProgress {
    fn outcome(self) -> ActionOutcome {
        match self {
            BatchProgress::AlreadySatisfied => ActionOutcome::AlreadySatisfied,
            BatchProgress::Changed => ActionOutcome::Changed,
            BatchProgress::Waiting | BatchProgress::EventSeen | BatchProgress::Unconfirmed => {
                ActionOutcome::Unconfirmed
            }
        }
    }
}

/// Runs `actions` (all against one container: a run of kinds that
/// `joins_batch()`, or one action of any other kind on its own) as one Sway
/// command, and confirms each: an event-confirmed kind needs its own event and
/// then the requested state, a polled kind is polled for, and a kind that
/// can't be confirmed is `Unconfirmed`. Returns one outcome per action, in
/// order. This is the one place an action's effect is confirmed (#133), so a
/// change to how that works is made once.
///
/// The order is load-bearing: the existence check (failing fast on a closed
/// window, before a subscription is opened for it), then the subscription,
/// then the already-there check (one state read, for every kind
/// `is_skipped_when_already_in_effect()`; the subscription has to come first,
/// see there), then, if anything polled is sent, `--wait-time` with the
/// existence check after it, then the command. An action already in its
/// state is left out of the command, and a batch left with nothing to send
/// sends nothing. `NewColumn`/`NewRow` take their `poll_baseline()` just
/// before the command, from the poll connection.
///
/// Every error is prefixed with the failing action's Sway command verb, so a
/// timeout in an invocation with several actions, or a layout's
/// `step N: ...`, says which one failed rather than only how long it waited.
///
/// Every event wakes one state read that settles every action whose event
/// has arrived; polled actions are polled every `WAIT_TIME_POLL_INTERVAL` in
/// the same loop, up to `poll_grace()` capped at `--wait-time`, and then
/// reported `Unconfirmed` once the rest of `--wait-time` has passed. An
/// event-confirmed action still unconfirmed at `--timeout` fails the batch,
/// with its own verb in front, as it would alone.
///
/// With `settle` (#17), the pixel sizes and the position confirmed this way
/// are then held for that long: polled every `WAIT_TIME_POLL_INTERVAL`, and
/// sent again together if the window stops showing them, as a browser does
/// when it sets its own size just after mapping, which restarts the hold.
/// See `hold_geometry()`.
pub(super) fn run_batch(
    actions: &[SwayAction<'_>],
    settle: Option<time::Duration>,
) -> Result<Vec<ActionOutcome>, RunError> {
    let Some(first) = actions.first() else {
        return Ok(Vec::new());
    };
    let container_id = first.container_id;
    let verbose = first.verbose;
    let fail = |action: &SwayAction<'_>, error: String| -> RunError {
        format!("{}: {}", action.sway_command_verb(), error).into()
    };

    if verbose {
        for action in actions {
            diagnostic!(
                "Sway action: {}",
                escape_control_characters(&action.to_string(), false)
            );
        }
    }

    let container = existing_container(container_id).map_err(|error| fail(first, error))?;
    if let Some(action) = actions
        .iter()
        .find(|action| action.kind.needs_a_window() && !is_window(&container))
    {
        return Err(fail(action, split_container_error(container_id)));
    }

    let event_timeout = actions
        .iter()
        .find(|action| action.kind.is_event_confirmed())
        .map(|action| action.timing);
    let wait_time = actions
        .iter()
        .find(|action| !action.kind.is_event_confirmed())
        .map(|action| action.timing);

    let event_stream = match event_timeout {
        Some(_) => Some(event_loop(&[EventType::Window]).map_err(|error| fail(first, error))?),
        None => None,
    };

    let mut progress = vec![BatchProgress::Waiting; actions.len()];
    if event_timeout.is_some() {
        let state = container_state(container_id).map_err(|error| fail(first, error))?;
        for (action, progress) in actions.iter().zip(progress.iter_mut()) {
            if action.kind.is_skipped_when_already_in_effect()
                && action.state_satisfies(state.as_ref())
            {
                if verbose {
                    diagnostic!(
                        "Already in effect, not sent: {} (container id: {})",
                        action.sway_command_verb(),
                        container_id
                    );
                }
                *progress = BatchProgress::AlreadySatisfied;
            }
        }
    }

    let sent: Vec<usize> = (0..actions.len())
        .filter(|&index| progress[index] == BatchProgress::Waiting)
        .collect();
    if sent.is_empty() {
        return Ok(progress.iter().map(|progress| progress.outcome()).collect());
    }
    let sent_actions: Vec<&SwayAction<'_>> = sent.iter().map(|&index| &actions[index]).collect();
    let sends_polled = sent_actions
        .iter()
        .any(|action| !action.kind.is_event_confirmed());

    let deadline = event_timeout
        .map(deadline_after)
        .transpose()
        .map_err(|error| fail(first, error))?;

    // The existence check above is a fast fail: --wait-time is the caller's
    // own knob and deliberately unbounded, so sleeping it out only to report
    // that the target had already closed is pure delay (`wait_time = 3600000`
    // would sit for an hour before saying so). The one after the sleep covers
    // a window closing *during* it: on Sway 1.9 a [con_id=N] criteria matching
    // nothing is "success" for a polled command, which would silently no-op.
    // Sway 1.11 errors on its own ("No matching node."), confirmed live.
    let mut poll_connection = None;
    if let (true, Some(wait_time)) = (sends_polled, wait_time) {
        if verbose {
            diagnostic!(
                "Some actions have no matching event type. Will wait {} ms, then run the Sway \
                 command.",
                wait_time.as_millis()
            );
        }
        interrupt::sleep(wait_time).map_err(|error| fail(first, error))?;
        ensure_container_exists(container_id).map_err(|error| fail(first, error))?;
        poll_connection = new_connection().ok();
    }
    // Before the command, not after: see poll_baseline().
    let baselines: Vec<Option<swayipc::Rect>> = actions
        .iter()
        .map(|action| {
            poll_connection
                .as_mut()
                .and_then(|connection| action.poll_baseline(connection))
        })
        .collect();

    let command = batch_command(container_id, &sent_actions);
    if verbose {
        diagnostic!(
            "Sway command: {}",
            escape_control_characters(&command, false)
        );
    }
    let outcomes =
        run_sway_commands(&command, sent_actions.len()).map_err(|error| fail(first, error))?;
    let sent_at = time::Instant::now();
    if let Some(error) = batch_failure(&sent_actions, &outcomes) {
        return Err(error.into());
    }

    for &index in &sent {
        let action = &actions[index];
        if !action.kind.is_event_confirmed() && !action.polls(baselines[index]) {
            progress[index] = BatchProgress::Unconfirmed;
        }
    }
    if poll_connection.is_none() {
        for &index in &sent {
            if !actions[index].kind.is_event_confirmed() {
                progress[index] = BatchProgress::Unconfirmed;
            }
        }
    }

    // spawn_event_forwarder() reads events on a separate thread, so
    // recv_timeout() below enforces a real deadline even if the stream never
    // produces another event.
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
    // event-confirmed action produces: Sway broadcasts every window event to
    // every subscription, so step N's own confirming event is what retires
    // step N-1's reader. Measured against a live compositor rather than
    // assumed — a 300-step layout of event-confirmed actions peaked at 2
    // threads and 3 sockets, and a 15-step layout launching a real window per
    // step at 3 and 3. Pinned by tests/live_sway.rs's
    // `event_reader_threads_stay_bounded_across_a_long_layout`.
    //
    // The *last* reader stays blocked until the process exits, which is why
    // it's a small constant rather than zero. One owned event reader per run,
    // with an explicit shutdown protocol, would buy nothing for a CLI that
    // exits moments later, and would replace a per-action subscription (which
    // is also what keeps each wait independent, see Scratchpad's shared
    // WindowChange::Move in matching_window_change_event()) with shared
    // mutable state. Revisit if this ever becomes a library, gains a
    // long-lived mode, or needs cancellation.
    let event_receiver = event_stream.map(spawn_event_forwarder);
    loop {
        if let Some(connection) = poll_connection.as_mut() {
            for &index in &sent {
                let action = &actions[index];
                if action.kind.is_event_confirmed() || progress[index] != BatchProgress::Waiting {
                    continue;
                }
                if action.poll_matches(connection, baselines[index]) {
                    if verbose {
                        diagnostic!(
                            "Confirmed via poll: {} (container id: {})",
                            action.sway_command_verb(),
                            container_id
                        );
                    }
                    progress[index] = BatchProgress::Changed;
                } else if sent_at.elapsed() >= action.poll_grace().min(action.timing) {
                    if verbose {
                        diagnostic!(
                            "Poll grace period elapsed without confirmation: {} (container id: \
                             {})",
                            action.sway_command_verb(),
                            container_id
                        );
                    }
                    progress[index] = BatchProgress::Unconfirmed;
                }
            }
        }

        let polling = sent.iter().any(|&index| {
            !actions[index].kind.is_event_confirmed() && progress[index] == BatchProgress::Waiting
        });
        let awaiting_event = sent.iter().find(|&&index| {
            actions[index].kind.is_event_confirmed() && progress[index] != BatchProgress::Changed
        });

        let (Some(&waiting_index), Some(receiver), Some(deadline)) =
            (awaiting_event, event_receiver.as_ref(), deadline)
        else {
            if !polling {
                break;
            }
            interrupt::sleep(WAIT_TIME_POLL_INTERVAL).map_err(|error| fail(first, error))?;
            continue;
        };

        let remaining = deadline.saturating_duration_since(time::Instant::now());
        let waiting = &actions[waiting_index];
        if remaining.is_zero() {
            return Err(fail(waiting, event_timeout_message(waiting.timing)));
        }
        let window = match interrupt::recv(
            receiver,
            if polling {
                remaining.min(WAIT_TIME_POLL_INTERVAL)
            } else {
                remaining
            },
        ) {
            Ok(Ok(Event::Window(window))) => window,
            Ok(Ok(_)) => continue,
            Ok(Err(error)) => return Err(fail(waiting, error.to_string())),
            Err(WaitEnd::Timeout) if polling => continue,
            Err(WaitEnd::Timeout) => {
                return Err(fail(waiting, event_timeout_message(waiting.timing)))
            }
            Err(WaitEnd::Disconnected) => {
                return Err(fail(
                    waiting,
                    "Event stream closed unexpectedly".to_string(),
                ))
            }
            Err(WaitEnd::Interrupted(message)) => return Err(fail(waiting, message)),
        };

        let mut woken = false;
        for &index in &sent {
            if progress[index] == BatchProgress::Waiting
                && actions[index].matches_window_event(&window).is_ok()
            {
                progress[index] = BatchProgress::EventSeen;
                woken = true;
            }
        }
        if !woken {
            continue;
        }
        // The event is a wake-up, not the confirmation: see
        // state_satisfies()'s doc comment.
        let state = container_state(container_id).map_err(|error| fail(waiting, error))?;
        for &index in &sent {
            let action = &actions[index];
            if progress[index] != BatchProgress::EventSeen {
                continue;
            }
            if action.state_satisfies(state.as_ref()) {
                if verbose {
                    diagnostic!(
                        "Event match: {:?} container id {} ({})",
                        window.change,
                        container_id,
                        action.sway_command_verb()
                    );
                }
                progress[index] = BatchProgress::Changed;
            } else {
                if verbose {
                    diagnostic!(
                        "Event match: {:?} container id {} ({}), but the requested state is not \
                         in effect — another client may be driving this window; waiting for the \
                         next event",
                        window.change,
                        container_id,
                        action.sway_command_verb()
                    );
                }
                progress[index] = BatchProgress::Waiting;
            }
        }
    }

    if let (Some(settle), Some(connection)) = (settle, poll_connection.as_mut()) {
        hold_geometry(actions, &mut progress, settle, connection)
            .map_err(|error| fail(first, error))?;
    }

    // What couldn't be confirmed (a solo window's clamped resize, a move
    // already at the workspace edge: no-ops tree state can't tell apart from
    // "not yet") is assumed to have worked, and given the rest of --wait-time
    // to finish before the next action runs.
    if let Some(wait_time) = wait_time {
        if sent
            .iter()
            .any(|&index| progress[index] == BatchProgress::Unconfirmed)
        {
            interrupt::sleep(wait_time.saturating_sub(sent_at.elapsed()))
                .map_err(|error| fail(first, error))?;
        }
    }

    Ok(progress.iter().map(|progress| progress.outcome()).collect())
}

/// How many times `hold_geometry()` sends the geometry again before giving
/// up on a client that keeps undoing it. A browser resizes itself once or
/// twice while it starts; one that never stops is fighting the request, and
/// sending it forever would only fight back.
const MAX_GEOMETRY_REAPPLIES: usize = 3;

/// Whether `kind` is geometry `hold_geometry()` can hold: a pixel size or a
/// position, the kinds `poll_matches()` can check a figure for.
fn is_held_geometry(kind: ActionKind<'_>) -> bool {
    match kind {
        ActionKind::Height(Size::Pixels(_))
        | ActionKind::Width(Size::Pixels(_))
        | ActionKind::Position(_) => true,
        ActionKind::Height(Size::Percent(_))
        | ActionKind::Width(Size::Percent(_))
        | ActionKind::Split(_)
        | ActionKind::Floating
        | ActionKind::Sticky
        | ActionKind::Fullscreen
        | ActionKind::Focus
        | ActionKind::NewColumn
        | ActionKind::NewRow
        | ActionKind::Workspace(_)
        | ActionKind::Output(_)
        | ActionKind::WorkspaceToOutput { .. }
        | ActionKind::Mark(_)
        | ActionKind::SwayCommand(_)
        | ActionKind::Scratchpad => false,
    }
}

/// `--settle` (#17): holds every confirmed geometry action in `actions` for
/// `settle`, re-sending all of them together whenever one stops showing in
/// the tree, which restarts the hold. Browsers set their own window size in
/// the first fraction of a second after mapping, often after the requested
/// one was already applied and confirmed; measured with Firefox and
/// Chromium, the size they settle on varies from run to run, while the same
/// command sent again a moment later always stuck.
///
/// An action that was never confirmed (a clamped resize) isn't held, since
/// nothing says it could ever show. After `MAX_GEOMETRY_REAPPLIES`, or once
/// the tree can't be read, whatever still doesn't match is reported
/// `Unconfirmed` rather than failing the run: the window exists and was
/// sent what was asked, but the client has the last word.
fn hold_geometry(
    actions: &[SwayAction<'_>],
    progress: &mut [BatchProgress],
    settle: time::Duration,
    connection: &mut Connection,
) -> Result<(), String> {
    let held: Vec<usize> = (0..actions.len())
        .filter(|&index| {
            progress[index] == BatchProgress::Changed && is_held_geometry(actions[index].kind)
        })
        .collect();
    let Some(&first) = held.first() else {
        return Ok(());
    };
    let container_id = actions[first].container_id;
    let verbose = actions[first].verbose;
    let held_actions: Vec<&SwayAction<'_>> = held.iter().map(|&index| &actions[index]).collect();
    let command = batch_command(container_id, &held_actions);

    let mut reapplies = 0;
    let mut quiet_since = time::Instant::now();
    while quiet_since.elapsed() < settle {
        interrupt::sleep(
            WAIT_TIME_POLL_INTERVAL.min(settle.saturating_sub(quiet_since.elapsed())),
        )?;
        let drifted: Vec<usize> = held
            .iter()
            .copied()
            .filter(|&index| !actions[index].poll_matches(connection, None))
            .collect();
        if drifted.is_empty() {
            continue;
        }
        if reapplies == MAX_GEOMETRY_REAPPLIES {
            if verbose {
                diagnostic!(
                    "The window changed its geometry again after {} re-applies; leaving it \
                     (container id: {})",
                    reapplies,
                    container_id
                );
            }
            for index in drifted {
                progress[index] = BatchProgress::Unconfirmed;
            }
            return Ok(());
        }
        reapplies += 1;
        if verbose {
            diagnostic!(
                "The window changed its geometry; re-applying ({} of {}): {}",
                reapplies,
                MAX_GEOMETRY_REAPPLIES,
                escape_control_characters(&command, false)
            );
        }
        let outcomes = run_sway_commands(&command, held_actions.len())?;
        if let Some(error) = batch_failure(&held_actions, &outcomes) {
            return Err(error);
        }
        quiet_since = time::Instant::now();
    }
    if verbose {
        diagnostic!(
            "Geometry held for {} ms (container id: {})",
            settle.as_millis(),
            container_id
        );
    }
    Ok(())
}

impl ExecAction<'_> {
    /// Launches this action's command and waits for its window, returning the
    /// window's container id and whether this run can prove it launched it
    /// (see `LaunchOwnership`). A freshly launched window is inherently a
    /// change, so unlike `run_batch()` there's no outcome to report.
    ///
    /// Every error is prefixed with `launch "<command>"`, for the same reason
    /// `run_batch()` prefixes its own with the command verb.
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

    /// The launch counterpart of `run_batch()`: matching purely
    /// on event content (app_id/class, or nothing at all with no filter) is
    /// ambiguous when more than one qualifying `New` window can appear
    /// around the same time — a concurrently-running second `sway-launch`
    /// process, or any other coincidentally-timed window — since Sway
    /// broadcasts window events to every IPC connection. To disambiguate,
    /// the launched command's environment is tagged with a random,
    /// per-invocation marker (`export <PID_MARKER_VAR>=<token> && ( <command> )`,
    /// see `ExecAction::sway_command()`), and a content-matching event is only
    /// accepted outright once `launch_evidence()` confirms it: the marker in
    /// `/proc/<event pid>/environ`, or an ancestor of that process that is
    /// this launch's own (#96).
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
    /// `LaunchOwnership` (see its own doc comment). A marked process that's
    /// gone makes the window `Launched` only when the window's process was
    /// already running before the launch (`MarkerTracker::predates_launch()`),
    /// as the instance a single-instance application hands over to is. A
    /// command that simply failed and exited is gone too, and a window from a
    /// process started after the launch is then someone else's: measured live
    /// as `--rollback-on-error` closing a terminal the user opened while a
    /// mistyped launch was failing (#124). That window, like one the grace
    /// period merely ran out on, is `Adopted`. Only `--rollback-on-error`
    /// reads this, and only to decide whether killing the window is this
    /// run's business.
    ///
    /// Same background-thread/socket lifetime as `run_batch()`'s:
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
        register_launch(&token);
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
        // the value returned once it's used — plus when its process started,
        // for MarkerTracker::predates_launch().
        let mut fallback: Option<(i64, time::Instant)> = None;
        let mut fallback_start: Option<u64> = None;

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
                let mut message = exec_timeout_message(
                    timeout,
                    filter,
                    title,
                    &unmatched_windows,
                    command_running,
                );
                let late_window = if self.catch_late_window {
                    late_marked_window(&event_receiver, &token)
                } else {
                    None
                };
                if let Some(container_id) = late_window {
                    message.push_str(&format!(
                        "; its window (con_id {}) opened within {} sec after that",
                        container_id,
                        LATE_WINDOW_GRACE.as_secs()
                    ));
                }
                return Err(RunError {
                    message,
                    unmatched_windows,
                    container_id: late_window,
                    launch_ownership: late_window.map(|_| LaunchOwnership::Launched),
                    window_timed_out: true,
                });
            }

            if marker.needs_scan(fallback.is_some()) && now >= next_scan {
                let presence =
                    scan_marker_presence(PID_MARKER_VAR, &token, marker.previously_seen());
                if let MarkerPresence::Present(pids) = &presence {
                    for &pid in pids {
                        if let Some(ticks) = process_start_ticks(pid) {
                            marker.record_start(ticks);
                        }
                    }
                }
                if marker.observe(presence) && verbose {
                    diagnostic!(
                        "Could not read /proc — PID-marker correlation is degraded on this system"
                    );
                }
                next_scan = now + MARKER_SCAN_INTERVAL;
                if let (true, Some((container_id, _))) = (marker.is_gone(), fallback) {
                    if !marker.predates_launch(fallback_start) {
                        if verbose {
                            diagnostic!(
                                "Marked process no longer running; using fallback container id \
                                 {} (adopted, not confirmed as launched by this run: its process \
                                 started after the launch, so the launched command exited \
                                 without a window of its own){}",
                                container_id,
                                unfiltered_fallback_hint(filter)
                            );
                        }
                        return Ok((container_id, LaunchOwnership::Adopted));
                    }
                    if verbose {
                        diagnostic!(
                            "Marked process no longer running; using fallback container id {} \
                             (nothing marker-confirmed can still arrive, and its process was \
                             already running before the launch, so this window is this run's \
                             own){}",
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

            let event = match interrupt::recv(&event_receiver, wait) {
                Ok(event) => event,
                Err(WaitEnd::Timeout) => continue,
                Err(WaitEnd::Disconnected) => {
                    return Err("Event stream closed unexpectedly".to_string().into());
                }
                Err(WaitEnd::Interrupted(message)) => {
                    // The likeliest reason to interrupt a run is an
                    // application slow to start, so its window gets the same
                    // grace a timed-out launch's does, for rollback to close.
                    let late_window = if self.catch_late_window {
                        late_marked_window(&event_receiver, &token)
                    } else {
                        None
                    };
                    let message = match late_window {
                        Some(container_id) => format!(
                            "{} while waiting for its window; its window (con_id {}) opened \
                             within {} sec after that",
                            message,
                            container_id,
                            LATE_WINDOW_GRACE.as_secs()
                        ),
                        None => format!(
                            "{} while waiting for its window — the launched command may still \
                             open one",
                            message
                        ),
                    };
                    return Err(RunError {
                        message,
                        unmatched_windows: Vec::new(),
                        container_id: late_window,
                        launch_ownership: late_window.map(|_| LaunchOwnership::Launched),
                        window_timed_out: false,
                    });
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
                            .is_some_and(|pid| launch_evidence(pid, &token).is_some()),
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

            let evidence = window
                .container
                .pid
                .and_then(|pid| launch_evidence(pid, &token));

            if let Some(evidence) = evidence {
                if verbose {
                    diagnostic!(
                        "Event match: {:?} container id {} (PID-marker-confirmed{})",
                        window.change,
                        window.container.id,
                        match evidence {
                            LaunchEvidence::OwnEnvironment => String::new(),
                            LaunchEvidence::Ancestor(ancestor) => {
                                format!(" through its ancestor process {}", ancestor)
                            }
                        }
                    );
                }
                return Ok((window.container.id, LaunchOwnership::Launched));
            }

            if window
                .container
                .pid
                .is_some_and(|pid| is_sibling_launchs_window(pid, &token))
            {
                if verbose {
                    diagnostic!(
                        "Event content-matches but container id {} is another launch's, still \
                         running alongside this one — not a fallback candidate",
                        window.container.id
                    );
                }
                continue;
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
                fallback_start = window.container.pid.and_then(process_start_ticks);
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

/// How long a timed-out or interrupted launch keeps watching for its window
/// when `catch_late_window` is set: long enough for an application that was
/// only just too slow, short enough not to drag out a run that has already
/// failed.
const LATE_WINDOW_GRACE: time::Duration = time::Duration::from_secs(2);

/// The first window to open within `LATE_WINDOW_GRACE` whose process carries
/// this launch's marker, whatever its app_id/class: carrying the marker is
/// what makes it this launch's, and it's only being found so it can be
/// closed. `None` when none does.
///
/// Not `interrupt::recv()`: after an interrupt that would end the watch at
/// once, and a signal during it is a second one, which exits on the spot.
fn late_marked_window(
    events: &mpsc::Receiver<swayipc::Fallible<Event>>,
    token: &str,
) -> Option<i64> {
    let end = time::Instant::now() + LATE_WINDOW_GRACE;
    loop {
        let left = end.saturating_duration_since(time::Instant::now());
        match events.recv_timeout(left) {
            Ok(Ok(Event::Window(window)))
                if window.change == WindowChange::New
                    && window
                        .container
                        .pid
                        .is_some_and(|pid| launch_evidence(pid, token).is_some()) =>
            {
                return Some(window.container.id);
            }
            Ok(_) => {}
            Err(_) => return None,
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

/// Why a window-only action refuses a split container. `run_batch()`
/// prefixes it with the action's verb.
fn split_container_error(container_id: i64) -> String {
    format!(
        "container {} is a split container, not a window — Sway sends no window event for it, \
         so this can't be confirmed; target a window inside it instead",
        container_id
    )
}

/// An event-confirmed action's timeout. `run_batch()` prefixes it with
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
/// `run_batch()` uses, so a window mapping between the two
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
        match interrupt::recv(&events, remaining.min(LATE_PROPERTY_RECHECK_INTERVAL)) {
            Ok(Ok(_)) | Err(WaitEnd::Timeout) => {}
            Ok(Err(error)) => return Err(error.to_string().into()),
            Err(WaitEnd::Disconnected) => {
                return Err("Event stream closed unexpectedly".to_string().into());
            }
            Err(WaitEnd::Interrupted(message)) => return Err(message.into()),
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
