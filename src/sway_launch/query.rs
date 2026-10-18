//! Asking the compositor a question, then handing the answer to `tree.rs`.
//!
//! Every function here is the same shape: open or take a `Connection`, read
//! the tree or the outputs, and delegate the actual decision to a pure
//! predicate. That is deliberately all they do: a decision here can only be
//! tested by scripting a compositor (`tests/fake_sway.rs`, or a real one in
//! `tests/live_sway.rs`), while one in `tree.rs` is a one-line unit test. A
//! new query belongs here only if it can't answer its question from a `&Node`
//! alone.

use super::capture::capture;
use super::ipc::{ipc_error, new_connection};
use super::target::{LaunchFilter, WindowCriteria};
use super::tree::{
    con_id_target_problem, existing_candidates, find_node, find_parent_layout, find_workspace,
    focused_workspace, launch_placement_problems, move_skip_reason, position_shows, present_ids,
    recheck_launched_windows, resolve_awaited_matches, resolve_matches, tiled_window_count,
    unnamed_build_workspace, window_listing, workspace_named, ContainerState, ListedWindow,
    MoveDirection, RecheckedWindow, SkipReason, WorkspaceIdentity, WorkspaceLookup,
};
use super::values::{Position, TitlePattern};
use swayipc::{Connection, Node, NodeLayout};

/// The whole tree, on a connection of its own, with a failure already
/// worded by `ipc_error()`.
fn fetch_tree() -> Result<Node, String> {
    new_connection()?.get_tree().map_err(ipc_error)
}

/// Finds exactly one already-open window `criteria` accepts via
/// `get_tree()`, for `Target::Existing`.
/// Errors — rather than silently picking one — if zero or more than one
/// window matches, since guessing which of several matches the caller meant
/// would be a worse default than asking them to retarget with `--con-id`.
/// `excluded` are never candidates (see `existing_candidates()`).
pub(super) fn find_existing_container_id(
    criteria: WindowCriteria,
    excluded: &[i64],
) -> Result<i64, String> {
    let tree = fetch_tree()?;

    resolve_matches(
        existing_candidates(&tree, criteria, excluded),
        &criteria.to_string(),
    )
}

/// Every open window `criteria` accepts, for `--list-windows` (see
/// `window_listing()`).
pub fn list_windows(criteria: WindowCriteria) -> Result<Vec<ListedWindow>, String> {
    let tree = fetch_tree()?;
    Ok(window_listing(&tree, criteria))
}

/// `--capture` (#31): the `--layout` file rebuilding the workspace named
/// `name`, or the focused one.
pub fn capture_workspace(name: Option<&str>) -> Result<String, String> {
    let tree = fetch_tree()?;
    let workspace = match name {
        Some(name) => workspace_named(&tree, name)
            .ok_or_else(|| format!("no workspace is named {:?}", name))?,
        None => focused_workspace(&tree).ok_or_else(|| "no workspace is focused".to_string())?,
    };
    capture(workspace)
}

/// The one window `criteria` accepts right now, `None` while there is
/// none yet, or the ambiguity error when there are several — for
/// `run_wait_for_existing_window()`, on its one connection.
pub(super) fn awaited_existing_container_id(
    connection: &mut Connection,
    criteria: WindowCriteria,
    excluded: &[i64],
) -> Result<Option<i64>, String> {
    let tree = connection.get_tree().map_err(ipc_error)?;
    resolve_awaited_matches(
        existing_candidates(&tree, criteria, excluded),
        &criteria.to_string(),
    )
}

/// Reads `container_id`'s `ContainerState`, or `None` if it isn't in the tree.
///
/// Unlike `node_by_id()` (used by the poll-then-fallback machinery, where a
/// transient IPC failure is deliberately swallowed into "not confirmed yet"),
/// this propagates a genuine connection/`get_tree()` failure as an error: it's
/// used to check state *before* deciding whether to act, where silently
/// treating a real IPC failure as "not already there" would let a later step
/// fail with a confusing timeout instead of surfacing the actual problem
/// immediately.
pub(super) fn container_state(container_id: i64) -> Result<Option<ContainerState>, String> {
    let tree = fetch_tree()?;

    Ok(ContainerState::from_tree(&tree, container_id))
}

/// Why NewColumn/NewRow must not send its move to `container_id` right now,
/// or `None` when it's safe — `move_skip_reason()` over a fresh tree and
/// output count. `SwayLaunch::run()` asks immediately before each move rather
/// than while planning, so the answer reflects whatever the actions before it
/// (a `--new-column` ahead of a `--new-row`, say) did to the tree.
///
/// The tree half replays Sway's own `move right`/`move down` rather than
/// approximating it with a shape rule — see `move_skip_reason()` and
/// `is_at_the_trailing_workspace_edge()`, and `docs/design-notes.md`'s
/// "Orchestration" section (history) for the shape rules that preceded it
/// and the cases each missed.
///
/// A failed `get_outputs()`/`get_tree()` propagates as an error rather than
/// being treated as "safe to proceed": the caller is about to run a command
/// whose safety this check exists to establish, so an unanswerable check is a
/// reason to stop, not to guess.
pub(super) fn current_move_skip_reason(
    container_id: i64,
    direction: MoveDirection,
) -> Result<Option<SkipReason>, String> {
    // One connection for both reads: they answer halves of a single question.
    let mut connection = new_connection()?;

    let outputs = connection.get_outputs().map_err(ipc_error)?;
    let tree = connection.get_tree().map_err(ipc_error)?;

    Ok(move_skip_reason(
        &tree,
        container_id,
        direction,
        outputs.len() > 1,
    ))
}

/// Errors with the one message every action driver reports for a container
/// that closed since it was resolved, so every existence check in
/// `confirmation.rs` (this and `existing_container()`) can't drift into
/// different wording for the same condition. Not a correctness requirement:
/// every supported Sway answers a `[con_id=N]` command for a container that
/// is gone with `No matching node.` (confirmed live on 1.9 and 1.11, and
/// recorded in `tests/fake-sway/`), so the command would fail anyway. The
/// checks earn their tree reads by failing fast (before a subscription is
/// opened, before `--wait-time` is slept out) and with a message that names
/// the container and says why, which Sway's own doesn't.
pub(super) fn ensure_container_exists(container_id: i64) -> Result<(), String> {
    existing_container(container_id).map(|_| ())
}

/// Fails unless `container_id` names a window or split container, for a
/// `Target::ConId` run with actions to apply (see `con_id_target_problem()`).
pub(super) fn ensure_con_id_target(container_id: i64) -> Result<(), String> {
    let tree = fetch_tree()?;
    con_id_target_problem(&tree, container_id).map_or(Ok(()), Err)
}

/// `ensure_container_exists()`, handing back the container's node for a
/// caller that needs to know what kind of container it is.
pub(super) fn existing_container(container_id: i64) -> Result<Node, String> {
    let tree = fetch_tree()?;
    find_node(&tree, container_id)
        .cloned()
        .ok_or_else(|| missing_container_error(container_id))
}

/// The message every existence check reports for a container that closed
/// since it was resolved (see `ensure_container_exists()`).
pub(super) fn missing_container_error(container_id: i64) -> String {
    format!(
        "container id {} no longer exists — window may have closed",
        container_id
    )
}

/// Which of `container_ids` are still present in the current tree, in the
/// order given. Used by `run.rs`'s `--rollback-on-error` to tell a window
/// that actually closed from one whose close request Sway merely accepted:
/// `kill` asks the client to close and replies before it has, and a client
/// is free to keep its window open (an unsaved-changes prompt, say).
pub fn containers_still_present(container_ids: &[i64]) -> Result<Vec<i64>, String> {
    let tree = fetch_tree()?;
    Ok(present_ids(&tree, container_ids))
}

/// The `layout` field of `container_id`'s *parent* node, or `None` if the
/// container/tree can't be read, or `container_id` has no parent in the
/// tree (e.g. it's the root). `container_id`'s own node never carries its
/// own split direction — confirmed against a live Sway compositor,
/// splitting a window with siblings wraps it in a new split container
/// (whose `layout` is the requested direction) one level up, and splitting
/// a solo window instead sets the `layout` of the workspace it's already
/// the sole child of; the leaf window node's own `layout` field is always
/// `None`/unset either way. Used by `SwayAction::poll_matches()` to confirm
/// a `Split` action actually applied before its `run_batch()`
/// poll grace period falls back to sleeping the rest of `--wait-time`.
pub(super) fn parent_node_layout(
    connection: &mut Connection,
    container_id: i64,
) -> Option<NodeLayout> {
    let tree = connection.get_tree().ok()?;
    find_parent_layout(&tree, container_id)
}

/// `recheck_launched_windows()` against a fresh tree, or `None` if the tree
/// can't be read — used by `ExecAction::run_wait_matching_exec_event()` to
/// notice a launched window setting its app_id/class after mapping.
pub(super) fn recheck_launched_windows_now(
    connection: &mut Connection,
    container_ids: &[i64],
    filter: Option<LaunchFilter>,
    title: Option<&TitlePattern>,
) -> Option<Vec<RecheckedWindow>> {
    let tree = connection.get_tree().ok()?;
    Some(recheck_launched_windows(
        &tree,
        container_ids,
        filter,
        title,
    ))
}

/// `find_workspace()` over a fresh tree: the workspace `lookup` names, by id
/// and current name, or `None` when there is none.
pub(super) fn workspace_now(lookup: WorkspaceLookup) -> Result<Option<WorkspaceIdentity>, String> {
    let tree = fetch_tree()?;
    Ok(find_workspace(&tree, lookup))
}

/// `launch_placement_problems()` over a fresh tree: what a user's config
/// rules did to a step's freshly opened window, if anything (#160).
pub(super) fn launch_placement_problems_now(
    container_id: i64,
    moves_itself: bool,
    floats_itself: bool,
) -> Result<Vec<String>, String> {
    let tree = fetch_tree()?;
    Ok(launch_placement_problems(
        &tree,
        container_id,
        moves_itself,
        floats_itself,
    ))
}

/// `unnamed_build_workspace()` over a fresh tree: the name of the workspace
/// a run with no `--on-workspace` builds on, for its run lock (#129).
pub fn unnamed_build_workspace_name(output: Option<&str>) -> Result<Option<String>, String> {
    let tree = fetch_tree()?;
    Ok(unnamed_build_workspace(&tree, output).and_then(|workspace| workspace.name.clone()))
}

/// The focused workspace's name and how many tiled windows it already holds,
/// for `--layout`/`--template`'s warning that a run is building around
/// existing windows.
pub fn focused_workspace_occupancy() -> Result<Option<(String, usize)>, String> {
    let tree = fetch_tree()?;
    Ok(focused_workspace(&tree).and_then(|workspace| {
        workspace
            .name
            .clone()
            .map(|name| (name, tiled_window_count(workspace)))
    }))
}

/// `ContainerState::from_tree()` on the caller's `connection`, or `None` if
/// the tree can't be read or `container_id` isn't in it.
pub(super) fn container_state_on(
    connection: &mut Connection,
    container_id: i64,
) -> Option<ContainerState> {
    let tree = connection.get_tree().ok()?;
    ContainerState::from_tree(&tree, container_id)
}

/// The whole tree, read on the caller's `connection`, with a failure worded
/// by `ipc_error()`: for `hold_geometry()`, which has to tell a tree it
/// couldn't read from a window that no longer matches.
pub(super) fn tree_on(connection: &mut Connection) -> Result<Node, String> {
    connection.get_tree().map_err(ipc_error)
}

/// The tree node with id `container_id`, or `None` if it can't be read
/// (transient IPC error, or the container's gone) — used by
/// `SwayAction::poll_matches()`'s `Height`/`Width`/`Position` arms to read
/// a window's own current geometry, as opposed to `parent_node_layout()`,
/// which reads its *parent's* state for `Split`.
pub(super) fn node_by_id(connection: &mut Connection, container_id: i64) -> Option<Node> {
    let tree = connection.get_tree().ok()?;
    find_node(&tree, container_id).cloned()
}

/// Whether `container_id`'s window is currently positioned where `position`
/// (`Position::Center` or `Position::Coordinates`) requests. Never propagates
/// a `None`/error — any failure to read the tree, or to find the container or
/// its workspace, folds into `false` ("not confirmed yet"), consistent with
/// `SwayAction::poll_matches()`'s other arms.
///
/// Both sides of the comparison are workspace-relative, and both come from
/// the one `get_tree()`: the workspace node's `rect` is its origin and size.
/// `node_position()` explains why the window's own fields need converting
/// into that space and `expected_position()` why `center` is computed from
/// the workspace rather than the output.
pub(super) fn position_matches(
    connection: &mut Connection,
    container_id: i64,
    position: &Position,
) -> bool {
    connection
        .get_tree()
        .is_ok_and(|tree| position_shows(&tree, container_id, position))
}
