//! Reading answers out of a Sway tree, without ever fetching one.
//!
//! Every function here takes a `&Node` someone else already read and returns a
//! plain answer: which container matches, what workspace or output contains
//! it, whether a node is floating, whether a rect is the size that was asked
//! for. Nothing in this module names a `Connection`, which is exactly the
//! point — it keeps the *interpretation* of Sway's state unit-testable
//! headlessly, and leaves only the fetching (see `query.rs`) needing a live
//! compositor.
//!
//! Prefer this shape for any new predicate: take the resolved values as
//! arguments and let the caller do the reading, rather than burying a
//! comparison behind a `&mut Connection` where the coverage gate can't see it.

use super::values::Position;
use swayipc::{Node, NodeLayout, NodeType};

/// `node`'s direct children, tiling (`nodes`) first, then floating
/// (`floating_nodes`) — the order every walk in this module visits them in.
fn children(node: &Node) -> impl DoubleEndedIterator<Item = &Node> {
    node.nodes.iter().chain(node.floating_nodes.iter())
}

/// `node` itself followed by every node below it, tiling and floating, in
/// depth-first pre-order.
pub(super) fn descendants(node: &Node) -> impl Iterator<Item = &Node> {
    let mut stack = vec![node];
    std::iter::from_fn(move || {
        let current = stack.pop()?;
        stack.extend(children(current).rev());
        Some(current)
    })
}

/// The path from `tree` down to `container_id`, `tree` first and the container
/// last, through tiling and floating children alike — or `None` when the
/// container isn't in `tree`. The one walk every "where is this container"
/// question below is answered from: each of them is a lookup on this path.
pub(super) fn ancestors_of(tree: &Node, container_id: i64) -> Option<Vec<&Node>> {
    fn walk<'a>(node: &'a Node, container_id: i64, path: &mut Vec<&'a Node>) -> bool {
        path.push(node);
        if node.id == container_id || children(node).any(|child| walk(child, container_id, path)) {
            return true;
        }
        path.pop();
        false
    }

    let mut path = Vec::new();
    walk(tree, container_id, &mut path).then_some(path)
}

/// The nearest node on `path` (the target itself included) whose `node_type`
/// is `kind`.
fn nearest_of_kind<'a>(path: &[&'a Node], kind: NodeType) -> Option<&'a Node> {
    path.iter()
        .rev()
        .copied()
        .find(|node| node.node_type == kind)
}

fn name_of_nearest(path: &[&Node], kind: NodeType) -> Option<String> {
    nearest_of_kind(path, kind).and_then(|node| node.name.clone())
}

pub(super) fn window_app_id_match(node: &Node, app_id_match: &str) -> bool {
    node.app_id.as_deref() == Some(app_id_match)
}

pub(super) fn window_class_match(node: &Node, class_match: &str) -> bool {
    node.window_properties
        .as_ref()
        .and_then(|window_properties| window_properties.class.as_deref())
        == Some(class_match)
}

pub(super) fn window_mark_match(node: &Node, mark_match: &str) -> bool {
    node.marks.iter().any(|mark| mark == mark_match)
}

/// Collects the container ids of every node in `tree` (tiling
/// and floating children at every level) whose app_id/class/mark matches,
/// used to target an already-open window instead of launching a new one.
/// The three criteria are mutually exclusive by construction (enforced at
/// the CLI/layout/template level, mirroring `--app-id`/`--class`/
/// `--mark-match`'s own `conflicts_with_all`), but the precedence order
/// here — app_id, then class, then mark — still needs to be defined
/// regardless, matching `matches_window_event`'s Exec-matching precedence.
pub(super) fn matching_container_ids(
    tree: &Node,
    app_id_match: &str,
    class_match: &str,
    mark_match: &str,
) -> Vec<i64> {
    let matches = |node: &Node| {
        if !app_id_match.is_empty() {
            window_app_id_match(node, app_id_match)
        } else if !class_match.is_empty() {
            window_class_match(node, class_match)
        } else if !mark_match.is_empty() {
            window_mark_match(node, mark_match)
        } else {
            false
        }
    };

    descendants(tree)
        .filter(|node| matches(node))
        .map(|node| node.id)
        .collect()
}

/// Turns the container ids `matching_container_ids()` found into a single
/// target, erroring — rather than silently picking one — on zero or more
/// than one match.
pub(super) fn resolve_matches(matches: Vec<i64>, criteria: &str) -> Result<i64, String> {
    match matches.len() {
        0 => Err(format!("No existing window matches {}", criteria)),
        1 => Ok(matches[0]),
        _ => {
            let ids = matches
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "{} windows match {}: {} — retarget with --con-id",
                matches.len(),
                criteria,
                ids
            ))
        }
    }
}

/// A container's state, as far as any `SwayAction` cares about it: everything
/// `SwayAction::state_satisfies()` needs to decide whether what an action asked
/// for is what's in effect, and nothing else.
///
/// Exists so that decision can be pure: reading each field through its own IPC
/// helper from the arm that needs it would wedge the interpretation ("is 300px
/// what this window is?", "is this the workspace asked for?") behind a socket,
/// reachable only by the live-Sway suite. Fetching once, up front, leaves the
/// fetch as the only part needing a compositor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ContainerState {
    pub(super) workspace: Option<String>,
    pub(super) output: Option<String>,
    pub(super) floating: bool,
    pub(super) fullscreen: bool,
    pub(super) focused: bool,
    pub(super) in_scratchpad: bool,
    pub(super) marks: Vec<String>,
}

impl ContainerState {
    /// Derives `container_id`'s state from an already-fetched tree, or `None`
    /// when the container isn't in it.
    pub(super) fn from_tree(tree: &Node, container_id: i64) -> Option<Self> {
        let path = ancestors_of(tree, container_id)?;
        let node = *path.last()?;
        let workspace = name_of_nearest(&path, NodeType::Workspace);

        Some(ContainerState {
            in_scratchpad: is_scratchpad_workspace(workspace.as_deref()),
            workspace,
            output: name_of_nearest(&path, NodeType::Output),
            floating: node_is_floating(node),
            fullscreen: node.fullscreen_mode.is_some_and(|mode| mode != 0),
            focused: node.focused,
            marks: node.marks.clone(),
        })
    }
}

/// Whether `node` is currently floating. Sway 1.9 (still what `apt` installs
/// on Ubuntu 24.04, confirmed live against a headless compositor during a
/// CI-failure investigation) never populates a floating container's own
/// `floating` field — it stays `null` even though the container's `type` is
/// correctly `floating_con` — while Sway 1.11 populates both. Checking
/// `node_type` alone therefore covers both versions; the `floating` field is
/// checked too only so a version that reverses this (populates `floating`
/// but not `node_type`, unconfirmed but not ruled out) still works.
pub(super) fn node_is_floating(node: &Node) -> bool {
    node.node_type == NodeType::FloatingCon
        || matches!(
            node.floating,
            Some(swayipc::Floating::UserOn) | Some(swayipc::Floating::AutoOn)
        )
}

/// Whether a container whose nearest ancestor workspace is `workspace_name` is
/// currently hidden in Sway's scratchpad: whether that workspace is the fixed
/// internal workspace Sway names `__i3_scratch`, confirmed live to be
/// populated reliably on both Sway 1.9 and 1.11. Deliberately not
/// `node_is_floating()`: a window Sway auto-floats as part of moving it to
/// the scratchpad (see `SwayAction::matching_window_change_events()`'s doc
/// comment on `Scratchpad`) would otherwise be misreported as already in the
/// scratchpad the moment it's floating, before it actually is.
///
/// Deliberately not the node's own `scratchpad_state` field either, not even
/// as a secondary signal. That field means "this window belongs to the
/// scratchpad", not "it is hidden there": it stays `fresh`/`changed` while the
/// window is *shown* on an ordinary workspace via `scratchpad show`, so on
/// Sway 1.11 (which populates it) a shown dropdown window would look
/// already-hidden and `--scratchpad` would short-circuit without sending
/// anything. Sway 1.9 leaves the field at `none` even for a hidden window, so
/// it isn't reliable in the other direction either.
fn is_scratchpad_workspace(workspace_name: Option<&str>) -> bool {
    workspace_name == Some("__i3_scratch")
}

/// The direction `NewColumn` ("move right") / `NewRow` ("move down") moves
/// in — used only by `is_at_the_trailing_workspace_edge()` to know which
/// layouts count as parallel to the move.
#[derive(Copy, Clone)]
pub(super) enum MoveDirection {
    Right,
    Down,
}

/// Whether Sway's own `move right`/`move down` on `container_id` would end in
/// its "move to the next output" branch rather than moving the window within
/// its workspace. A step-for-step model of `container_move_in_direction()` in
/// Sway's `sway/commands/move.c`, run over a tree read rather than performed:
///
/// - Walk up from the container until an ancestor's parent is laid out along
///   the move's axis. `tabbed` counts as horizontal and `stacked` as
///   vertical, the way Sway's own `is_parallel()` treats them. Reaching the
///   workspace with no such parent means Sway re-orients the workspace
///   instead, which never escalates.
/// - The container itself being the trailing child of a parallel *workspace*
///   is the original escalating case.
/// - A container at the trailing edge of its own parallel parent climbs one
///   level. When it finds an ancestor with nothing after it, Sway promotes the
///   container next to that ancestor — unless the container is the *only*
///   child of a sub-container sitting directly on the workspace. Sway treats
///   that "singleton" like a window at workspace level (mimicking i3) and
///   escalates. That case covers a lone window wrapped by `layout tabbed`, a
///   window left alone in a `splitv` container after its sibling closed, and
///   every first window on a workspace under `workspace_layout tabbed` or
///   `stacking`.
///
/// Floating windows (and anything inside a floating container) are never
/// flagged: Sway refuses to move them out of their floating parent at all.
pub(super) fn is_at_the_trailing_workspace_edge(
    tree: &Node,
    container_id: i64,
    direction: MoveDirection,
) -> bool {
    let Some(path) = ancestors_of(tree, container_id) else {
        return false;
    };
    let Some(workspace_index) = path
        .iter()
        .rposition(|node| node.node_type == NodeType::Workspace)
    else {
        return false;
    };
    let chain = &path[workspace_index..];
    let reached_through_tiling_children_only = chain
        .windows(2)
        .all(|pair| pair[0].nodes.iter().any(|child| child.id == pair[1].id));
    if !reached_through_tiling_children_only {
        return false;
    }
    let container_depth = chain.len() - 1;
    if container_depth == 0 {
        return false;
    }

    let mut depth = container_depth;
    loop {
        let current = chain[depth];
        let parent = chain[depth - 1];
        if !is_parallel(parent.layout, direction) {
            if depth == 1 {
                return false;
            }
            depth -= 1;
            continue;
        }

        let has_following_sibling = parent
            .nodes
            .iter()
            .position(|sibling| sibling.id == current.id)
            .is_some_and(|index| index + 1 < parent.nodes.len());
        if has_following_sibling {
            return false;
        }
        if depth == container_depth {
            if depth == 1 {
                return true;
            }
            depth -= 1;
            continue;
        }
        return container_depth == 2 && chain[1].nodes.len() == 1;
    }
}

fn is_parallel(layout: NodeLayout, direction: MoveDirection) -> bool {
    match direction {
        MoveDirection::Right => matches!(layout, NodeLayout::SplitH | NodeLayout::Tabbed),
        MoveDirection::Down => matches!(layout, NodeLayout::SplitV | NodeLayout::Stacked),
    }
}

pub(super) fn find_workspace_node(tree: &Node, container_id: i64) -> Option<&Node> {
    nearest_of_kind(&ancestors_of(tree, container_id)?, NodeType::Workspace)
}

/// Which of `container_ids` are still anywhere in `tree`, in the order given —
/// the pure half of `containers_still_present()`.
pub(super) fn present_ids(tree: &Node, container_ids: &[i64]) -> Vec<i64> {
    container_ids
        .iter()
        .copied()
        .filter(|&container_id| find_node(tree, container_id).is_some())
        .collect()
}

pub(super) fn find_parent_layout(tree: &Node, container_id: i64) -> Option<NodeLayout> {
    let path = ancestors_of(tree, container_id)?;
    let parent_index = path.len().checked_sub(2)?;
    Some(path[parent_index].layout)
}

pub(super) fn find_node(tree: &Node, container_id: i64) -> Option<&Node> {
    descendants(tree).find(|node| node.id == container_id)
}

/// Whether `node`'s current width matches a `resize set width <expected_px>px`
/// command. Confirmed against a live Sway compositor that this needs two
/// candidate formulas, not one fixed offset: a window resized while it's
/// been floating since the very command that floated it matches
/// `rect.width` exactly, but a window that had already been tiled for a
/// while before being floated and resized comes out `2 *
/// current_border_width` short on `rect.width` alone (see
/// `retarget_by_id_layout_floats_the_first_step_by_name` in
/// `tests/live_sway.rs`, and the `Height`/`Width` notes under CLAUDE.md's
/// "Core model" section) — border accounting
/// this project hasn't found a single deterministic rule for, so both
/// candidates are accepted rather than picking one and risking a resize
/// that's actually done never being recognized as such.
///
/// Accepted residual risk: `run_poll_then_fallback()`'s very first poll
/// runs immediately after the command is sent, with no minimum settle
/// time, so if the window's *pre-resize* width already happens to satisfy
/// either formula for the *newly requested* width (the old and new widths
/// would need to coincide, accounting for a possible border offset), this
/// would report "confirmed" on that first poll before Sway has actually
/// processed the new command — indistinguishable from a legitimate
/// already-there match (the same case `Split`'s idempotent re-application
/// relies on confirming instantly). Judged low-risk enough not to warrant
/// the added latency/complexity of requiring a genuine change (like
/// `NewColumn`/`NewRow`'s `poll_baseline()` snapshot) purely for this;
/// revisit if it ever proves problematic in practice.
pub(super) fn width_matches(node: &Node, expected_px: i32) -> bool {
    node.rect.width == expected_px || node.rect.width + 2 * node.current_border_width == expected_px
}

/// Whether `node`'s current height matches a `resize set height
/// <expected_px>px` command. Unlike `width_matches()`, live testing found
/// only one formula for height across both the freshly-floating and
/// tiled cases: the decoration (title bar) is always excluded from
/// `rect.height`, so the outer, decoration-inclusive height is
/// `rect.height + deco_rect.height`.
pub(super) fn height_matches(node: &Node, expected_px: i32) -> bool {
    node.rect.height + node.deco_rect.height == expected_px
}

/// Where `node`'s frame sits relative to `workspace_rect` (the workspace
/// containing it), in the same space `move position <x> <y>` targets: Sway
/// adds the workspace's own origin to the requested coordinates, so the
/// workspace — not the output, and not the global layout — is the origin to
/// compare in. The workspace starts below or beside a bar and inside outer
/// gaps, so on most real desktops it isn't at `(0, 0)`.
///
/// Sway reports the two candidate fields in different spaces, which is why
/// they're handled differently rather than read interchangeably:
///
/// - `deco_rect` is the decoration-inclusive frame origin *relative to the
///   workspace* for a floating window, so it's already in the right space.
/// - `rect` is *absolute*, so the workspace origin is subtracted from it.
///   It's what's left when `deco_rect` is all zero, which Sway reports for any
///   window drawing no title bar: a fullscreen window, and every window whose
///   border isn't `normal` (`border pixel <N>`, `border none`, or the same set
///   through `default_border`). Such a window has no decoration above its `rect`,
///   so `rect`'s origin *is* the frame origin.
/// - A freshly mapped window also reads all-zero `deco_rect` until Sway has
///   applied its first layout transaction, since Sway tests the *applied*
///   border while serving geometry from pending state. Its `rect` then
///   includes the title bar, so the fallback still yields the frame origin
///   and the position confirms against the pending state later commands act
///   on — but a client reading `deco_rect` straight afterwards can see zeros.
///
/// Split out from `position_matches()` so this comparison is ordinary pure
/// logic with its own tests rather than something only reachable through a
/// live `get_tree()`; `expected_position()` is split out for the same reason.
pub(super) fn node_position(node: &Node, workspace_rect: swayipc::Rect) -> (i32, i32) {
    if node.deco_rect.width == 0 && node.deco_rect.height == 0 {
        (
            node.rect.x - workspace_rect.x,
            node.rect.y - workspace_rect.y,
        )
    } else {
        (node.deco_rect.x, node.deco_rect.y)
    }
}

/// The workspace-relative `(x, y)` `position_matches()` expects `node`'s frame
/// to be at (see `node_position()` for why the workspace is the origin): the
/// requested coordinates as given, or for `Position::Center`, the frame
/// centered within `workspace_rect` — Sway's own `move position center`
/// centers on the workspace, not on the output.
pub(super) fn expected_position(
    position: &Position,
    node: &Node,
    workspace_rect: swayipc::Rect,
) -> (i32, i32) {
    match position {
        Position::Center => compute_center_position(
            workspace_rect,
            node.rect.width,
            node.rect.height + node.deco_rect.height,
        ),
        Position::Coordinates { x, y } => (*x, *y),
    }
}

/// The workspace-relative top-left `(x, y)` that centers a `window_width` x
/// `window_height` frame (the decoration-inclusive outer footprint) within
/// `workspace_rect`. Only the workspace's size matters here, not its origin,
/// since the result is relative to that origin.
///
/// Sway computes the same figure in floating point, and an odd difference
/// leaves a half pixel. Integer division here truncates it the way Sway's own
/// conversion to `deco_rect` does, so the two agree. The one known mismatch is
/// the absolute-`rect` path on a workspace with a *negative* origin, where
/// Sway's truncation toward zero rounds the other way: that case reports
/// `unconfirmed` after the poll grace rather than a false `changed`.
pub(super) fn compute_center_position(
    workspace_rect: swayipc::Rect,
    window_width: i32,
    window_height: i32,
) -> (i32, i32) {
    (
        (workspace_rect.width - window_width) / 2,
        (workspace_rect.height - window_height) / 2,
    )
}

#[cfg(test)]
mod tests;
