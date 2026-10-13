//! Asking the compositor a question, then handing the answer to `tree.rs`.
//!
//! Every function here is the same shape: open or take a `Connection`, read
//! the tree or the outputs, and delegate the actual decision to a pure
//! predicate. That is deliberately all they do — the thinner this layer is,
//! the less logic sits where `cargo test` cannot reach it.
//!
//! This whole module is exempt from the coverage target for that reason (see
//! CLAUDE.md's Rust conventions); `tests/live_sway.rs` is what exercises it,
//! against a real compositor. A new query belongs here only if it can't answer
//! its question from a `&Node` alone.

use super::ipc::{ipc_error, new_connection};
use super::tree::{
    expected_position, find_node, find_parent_layout, find_workspace_node,
    is_at_the_trailing_workspace_edge, matching_container_ids, node_position, present_ids,
    resolve_matches, ContainerState, MoveDirection,
};
use super::values::Position;
use swayipc::{Connection, Node, NodeLayout};

/// Finds exactly one already-open window matching `app_id_match`/
/// `class_match`/`mark_match` via `get_tree()`, for `Target::Existing`.
/// Errors — rather than silently picking one — if zero or more than one
/// window matches, since guessing which of several matches the caller meant
/// would be a worse default than asking them to retarget with `--con-id`.
pub(super) fn find_existing_container_id(
    app_id_match: &str,
    class_match: &str,
    mark_match: &str,
) -> Result<i64, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    let criteria = if !app_id_match.is_empty() {
        format!("app_id \"{}\"", app_id_match)
    } else if !class_match.is_empty() {
        format!("class \"{}\"", class_match)
    } else {
        format!("mark \"{}\"", mark_match)
    };

    resolve_matches(
        matching_container_ids(&tree, app_id_match, class_match, mark_match),
        &criteria,
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

/// Whether running NewColumn/NewRow on `container_id` right now would risk
/// Sway relocating it (and its whole workspace) to a different output
/// rather than moving it within the workspace or no-oping.
///
/// The tree half is `is_at_the_trailing_workspace_edge()`, which replays
/// Sway's own `move right`/`move down` walk rather than approximating it
/// with a shape rule — see its doc comment and CLAUDE.md's Orchestration
/// section for the two shape rules that preceded it and the cases each one
/// missed (a trailing window with a sibling to its left; a window that is
/// the only child of a sub-container at the workspace's trailing edge).
///
/// A failed `get_outputs()`/`get_tree()` propagates as an error rather than
/// being treated as "safe to proceed": the caller (`build_actions()`) is about
/// to run a command whose safety this check exists to establish, so an
/// unanswerable check is a reason to stop, not to guess. Only an unfindable
/// `container_id`/workspace yields `false`, since that genuinely means the
/// escalation this guards against can't apply.
pub(super) fn relocates_to_another_output(
    container_id: i64,
    direction: MoveDirection,
) -> Result<bool, String> {
    // One connection for both reads. They answer halves of a single question,
    // and the early return below means the tree fetch only happens on a
    // multi-output setup anyway — same reasoning as `run_wait_time()`'s shared
    // poll connection, just without a loop to amplify the cost.
    let mut connection = new_connection()?;

    let outputs = match connection.get_outputs() {
        Ok(outputs) => outputs,
        Err(error) => return Err(ipc_error(error)),
    };
    if outputs.len() < 2 {
        return Ok(false);
    }

    let tree = match connection.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };

    Ok(is_at_the_trailing_workspace_edge(
        &tree,
        container_id,
        direction,
    ))
}

/// Whether `container_id` is still present anywhere in the current tree —
/// used by `run_wait_time()` to catch a container that closed between an
/// earlier action resolving it and this one about to run its command
/// against it. Needed on Sway 1.9 (still what `apt` installs on Ubuntu
/// 24.04/CI), which treats a `[con_id=N]` criteria matching zero containers
/// as success rather than an error; Sway 1.11 already errors clearly
/// ("No matching node.") on its own, confirmed live, which makes this check
/// redundant there but still required for 1.9 — see `node_is_floating()`'s
/// doc comment for the same version split.
pub(super) fn container_exists(container_id: i64) -> Result<bool, String> {
    let tree = match new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(ipc_error(error)),
    };
    Ok(find_node(&tree, container_id).is_some())
}

/// `container_exists()` as a guard: errors with the one message every action
/// driver reports for a container that closed since it was resolved, so the
/// three call sites in `confirmation.rs` can't drift into different wording
/// for the same condition.
pub(super) fn ensure_container_exists(container_id: i64) -> Result<(), String> {
    if container_exists(container_id)? {
        Ok(())
    } else {
        Err(format!(
            "container id {} no longer exists — window may have closed",
            container_id
        ))
    }
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
