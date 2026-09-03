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
    contains_id, expected_position, find_containing_name, find_node, find_parent_layout,
    is_at_the_trailing_workspace_edge, matching_container_ids, node_position, resolve_matches,
    ContainerState, MoveDirection,
};
use super::values::Position;
use swayipc::{Connection, Node, NodeLayout, NodeType};

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
/// This originally checked only "is `container_id` the only window in its
/// workspace" — live testing during this feature's development found that
/// too narrow: a *non-solo* workspace can escalate too, whenever
/// `container_id` is already the trailing child of a workspace whose own
/// layout already matches the move's axis (confirmed live: two windows
/// side by side, `[con_id=<rightmost>] move right` relocated it to another
/// output, not a same-workspace no-op, even with a sibling to its left).
/// Conversely, a solo window whose workspace layout *doesn't* match the
/// axis (e.g. stacked vertically via `splitv`, then moved right) was
/// confirmed live to restructure in place rather than escalate — so
/// checking layout, not just child count, also avoids skipping a move that
/// would actually have been safe. The current check: `container_id` is a
/// *direct* child of its workspace (not nested in a sub-container), the
/// workspace's own `layout` matches the axis (`SplitH` for `NewColumn`,
/// `SplitV` for `NewRow`), and `container_id` is the last child in that
/// list — this subsumes the original solo-window case (trivially both
/// direct- and last-child of its workspace) while also catching the
/// multi-window case that check alone missed. A window nested inside a
/// sub-container is conservatively never flagged. Confirmed live in both an
/// axis-mismatched nesting (a `splitv` sub-container under a `splith`
/// workspace) and the axis-matched worst case (a `splith` sub-container
/// under a `splith` workspace, target as its trailing child) that this
/// conservatism costs nothing: `move right` on the nested target never
/// escalated to a different output either way, it simply popped the target
/// out to become a new direct child of the workspace — see
/// `tests/live_sway.rs`'s
/// `new_column_does_not_relocate_a_nested_window_to_a_different_output`.
/// Returns `false` (safe to proceed) if outputs/tree can't be read or
/// `container_id`/its workspace can't be found, rather than blocking the
/// action on an inconclusive check.
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
    Ok(contains_id(&tree, container_id))
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
/// (`"center"` or `"<x>,<y>"`) requests. Never propagates a `None`/error —
/// any failure to read the tree/outputs, or to resolve an expected
/// position at all (e.g. `container_id` not found, or not on a known
/// output), folds into `false` ("not confirmed yet"), consistent with
/// `SwayAction::poll_matches()`'s other arms.
///
/// Confirmed live that a fullscreen window's `deco_rect` stays `{0, 0, 0,
/// 0}` permanently (not a transient race — held stable across a multi-second
/// sweep), since Sway never computes decoration geometry for a window with
/// no border/titlebar to draw. Comparing only `deco_rect` would therefore
/// mean `--position` against a fullscreen container (directly, or via
/// `--floating --fullscreen --position` in one invocation) can never be
/// confirmed by polling — `move position` actually succeeds immediately
/// (confirmed live via `rect.x`/`rect.y` landing on the requested target),
/// but every invocation would still burn the full poll grace period before
/// falling back to sleeping `--wait-time`. Falling back to `rect.x`/`rect.y`
/// when `deco_rect` is unset closes that gap, mirroring `width_matches()`'s
/// existing dual-formula tolerance for a different Sway geometry quirk.
pub(super) fn position_matches(
    connection: &mut Connection,
    container_id: i64,
    position: &Position,
) -> bool {
    let Some((node, output_name)) = node_and_output_name(connection, container_id) else {
        return false;
    };
    // Only `center` needs the output's own geometry, so an explicit
    // `<x>,<y>` costs one tree read per poll iteration rather than a tree
    // read plus a get_outputs().
    let output_rect = match position {
        Position::Center => output_name
            .as_deref()
            .and_then(|name| output_rect(connection, name)),
        Position::Coordinates { .. } => None,
    };
    let Some(expected) = expected_position(position, &node, output_rect) else {
        return false;
    };
    node_position(&node) == expected
}

/// `container_id`'s tree node together with the name of the output
/// containing it (`None` if it isn't on any output, e.g. the scratchpad),
/// read via a single `get_tree()` call — used by `position_matches()`
/// rather than combining `node_by_id()` with the existing `current_output()`
/// helper, which would cost a second, redundant tree fetch per poll
/// iteration.
pub(super) fn node_and_output_name(
    connection: &mut Connection,
    container_id: i64,
) -> Option<(Node, Option<String>)> {
    let tree = connection.get_tree().ok()?;
    let node = find_node(&tree, container_id)?.clone();
    let output_name = find_containing_name(&tree, container_id, NodeType::Output, None);
    Some((node, output_name))
}

/// The geometry of the output named `output_name`, or `None` if it can't be
/// read or no output has that name.
pub(super) fn output_rect(connection: &mut Connection, output_name: &str) -> Option<swayipc::Rect> {
    let outputs = connection.get_outputs().ok()?;
    outputs
        .into_iter()
        .find(|output| output.name == output_name)
        .map(|output| output.rect)
}
