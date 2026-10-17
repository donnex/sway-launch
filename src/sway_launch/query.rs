//! Asking the compositor a question, then handing the answer to `tree.rs`.
//!
//! Every function here is the same shape: open or take a `Connection`, read
//! the tree or the outputs, and delegate the actual decision to a pure
//! predicate. That is deliberately all they do: a decision here can only be
//! tested by scripting a compositor (`tests/fake_sway.rs`, or a real one in
//! `tests/live_sway.rs`), while one in `tree.rs` is a one-line unit test. A
//! new query belongs here only if it can't answer its question from a `&Node`
//! alone.

use super::ipc::{ipc_error, new_connection};
use super::target::{LaunchFilter, WindowMatch};
use super::tree::{
    con_id_target_problem, expected_position, find_node, find_parent_layout, find_workspace_node,
    focused_workspace, matching_container_ids, move_skip_reason, node_position, present_ids,
    recheck_launched_windows, resolve_matches, tiled_window_count, ContainerState, MoveDirection,
    RecheckedWindow,
};
use super::values::Position;
use swayipc::{Connection, Node, NodeLayout};

/// Finds exactly one already-open window `window_match` accepts via
/// `get_tree()`, for `Target::Existing`.
/// Errors — rather than silently picking one — if zero or more than one
/// window matches, since guessing which of several matches the caller meant
/// would be a worse default than asking them to retarget with `--con-id`.
pub(super) fn find_existing_container_id(window_match: WindowMatch) -> Result<i64, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    resolve_matches(
        matching_container_ids(&tree, window_match),
        &window_match.to_string(),
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
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

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
/// `is_at_the_trailing_workspace_edge()`, and CLAUDE.md's Orchestration
/// section for the shape rules that preceded it and the cases each missed.
///
/// A failed `get_outputs()`/`get_tree()` propagates as an error rather than
/// being treated as "safe to proceed": the caller is about to run a command
/// whose safety this check exists to establish, so an unanswerable check is a
/// reason to stop, not to guess.
pub(super) fn current_move_skip_reason(
    container_id: i64,
    direction: MoveDirection,
) -> Result<Option<&'static str>, String> {
    // One connection for both reads: they answer halves of a single question.
    let mut connection = new_connection()?;

    let outputs = match connection.get_outputs() {
        Ok(outputs) => outputs,
        Err(error) => return Err(ipc_error(error)),
    };
    let tree = match connection.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    Ok(move_skip_reason(
        &tree,
        container_id,
        direction,
        outputs.len() > 1,
    ))
}

/// Errors with the one message every action driver reports for a container
/// that closed since it was resolved, so the three call sites in
/// `confirmation.rs` can't drift into different wording for the same
/// condition. Needed on Sway 1.9, which treats a `[con_id=N]` criteria matching zero containers
/// as success rather than an error; Sway 1.11 already errors clearly ("No
/// matching node."), confirmed live, which makes this redundant there but
/// still required for 1.9 — see `node_is_floating()`'s doc comment for the
/// same version split.
pub(super) fn ensure_container_exists(container_id: i64) -> Result<(), String> {
    existing_container(container_id).map(|_| ())
}

/// Fails unless `container_id` names a window or split container, for a
/// `Target::ConId` run with actions to apply (see `con_id_target_problem()`).
pub(super) fn ensure_con_id_target(container_id: i64) -> Result<(), String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
    con_id_target_problem(&tree, container_id).map_or(Ok(()), Err)
}

/// `ensure_container_exists()`, handing back the container's node for a
/// caller that needs to know what kind of container it is.
pub(super) fn existing_container(container_id: i64) -> Result<Node, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
    find_node(&tree, container_id).cloned().ok_or_else(|| {
        format!(
            "container id {} no longer exists — window may have closed",
            container_id
        )
    })
}

/// Which of `container_ids` are still present in the current tree, in the
/// order given. Used by `main.rs`'s `--rollback-on-error` to tell a window
/// that actually closed from one whose close request Sway merely accepted:
/// `kill` asks the client to close and replies before it has, and a client
/// is free to keep its window open (an unsaved-changes prompt, say).
pub fn containers_still_present(container_ids: &[i64]) -> Result<Vec<i64>, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
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
/// a `Split` action actually applied before its `run_poll_then_fallback()`
/// grace period falls back to sleeping the rest of `--wait-time`.
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
) -> Option<Vec<RecheckedWindow>> {
    let tree = connection.get_tree().ok()?;
    Some(recheck_launched_windows(&tree, container_ids, filter))
}

/// The tree node with id `container_id`, or `None` if it can't be read
/// (transient IPC error, or the container's gone) — used by
/// `SwayAction::poll_matches()`'s `Height`/`Width`/`Position` arms to read
/// a window's own current geometry, as opposed to `parent_node_layout()`,
/// which reads its *parent's* state for `Split`.
/// The name of the workspace Sway's focus is on, or `None` when the tree has
/// none focused.
pub(super) fn focused_workspace_name() -> Result<Option<String>, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
    Ok(focused_workspace(&tree).and_then(|workspace| workspace.name.clone()))
}

/// The focused workspace's name and how many tiled windows it already holds,
/// for `--layout`/`--template`'s warning that a run is building around
/// existing windows.
pub fn focused_workspace_occupancy() -> Result<Option<(String, usize)>, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
    Ok(focused_workspace(&tree).and_then(|workspace| {
        workspace
            .name
            .clone()
            .map(|name| (name, tiled_window_count(workspace)))
    }))
}

pub(super) fn container_state_on(
    connection: &mut Connection,
    container_id: i64,
) -> Option<ContainerState> {
    let tree = connection.get_tree().ok()?;
    ContainerState::from_tree(&tree, container_id)
}

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
    let Ok(tree) = connection.get_tree() else {
        return false;
    };
    let Some(node) = find_node(&tree, container_id) else {
        return false;
    };
    let Some(workspace) = find_workspace_node(&tree, container_id) else {
        return false;
    };
    node_position(node, workspace.rect) == expected_position(position, node, workspace.rect)
}
