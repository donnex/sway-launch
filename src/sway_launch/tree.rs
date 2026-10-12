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

pub(super) fn window_app_id_match(node: &Node, app_id_match: &str) -> bool {
    let node_app_id = match node.app_id.as_ref().ok_or(()) {
        Ok(app_id) => app_id,
        Err(_) => return false,
    };

    node_app_id == app_id_match
}

pub(super) fn window_class_match(node: &Node, class_match: &str) -> bool {
    let window_properties = match node.window_properties.as_ref().ok_or(()) {
        Ok(window_properties) => window_properties,
        Err(_) => return false,
    };

    let node_class = match window_properties.class.as_ref().ok_or(()) {
        Ok(class) => class,
        Err(_) => return false,
    };

    node_class == class_match
}

pub(super) fn window_mark_match(node: &Node, mark_match: &str) -> bool {
    node.marks.iter().any(|mark| mark == mark_match)
}

/// Recursively collects the container ids of every node in `tree` (tiling
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
    let matches = if !app_id_match.is_empty() {
        window_app_id_match(tree, app_id_match)
    } else if !class_match.is_empty() {
        window_class_match(tree, class_match)
    } else if !mark_match.is_empty() {
        window_mark_match(tree, mark_match)
    } else {
        false
    };

    let mut ids = if matches { vec![tree.id] } else { vec![] };

    for child in tree.nodes.iter().chain(tree.floating_nodes.iter()) {
        ids.extend(matching_container_ids(
            child,
            app_id_match,
            class_match,
            mark_match,
        ));
    }

    ids
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
/// Exists so that decision can be pure. Each of these fields used to be read by
/// its own IPC helper called straight from the arm that needed it, which left
/// the interpretation ("is 300px what this window is?", "is this the workspace
/// asked for?") wedged behind a socket and therefore only reachable by the
/// live-Sway suite. Fetching once, up front, leaves the fetch as the only part
/// needing a compositor.
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
        let node = find_node(tree, container_id)?;

        Some(ContainerState {
            workspace: find_containing_name(tree, container_id, NodeType::Workspace, None),
            output: find_containing_name(tree, container_id, NodeType::Output, None),
            floating: node_is_floating(node),
            fullscreen: node.fullscreen_mode.is_some_and(|mode| mode != 0),
            focused: node.focused,
            in_scratchpad: tree_shows_container_in_scratchpad(tree, container_id),
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

/// Whether `container_id` is currently hidden in Sway's scratchpad, given an
/// already-fetched `tree`: whether its ancestor workspace is the fixed
/// internal workspace Sway names `__i3_scratch`, confirmed live to be
/// populated reliably on both Sway 1.9 and 1.11. Deliberately not
/// `node_is_floating()`: a window Sway auto-floats as part of moving it to
/// the scratchpad (see `SwayAction::matching_window_change_events()`'s doc
/// comment on `Scratchpad`) would otherwise be misreported as already in the
/// scratchpad the moment it's floating, before it actually is.
///
/// Deliberately not the node's own `scratchpad_state` field either, which an
/// earlier version OR'd in as a secondary signal. That field means "this
/// window belongs to the scratchpad", not "it is hidden there": it stays
/// `fresh`/`changed` while the window is *shown* on an ordinary workspace via
/// `scratchpad show`, so on Sway 1.11 (which populates it) a shown dropdown
/// window looked already-hidden and `--scratchpad` short-circuited without
/// sending anything. Sway 1.9 leaves the field at `none` even for a hidden
/// window, so it was never reliable in the other direction either.
pub(super) fn tree_shows_container_in_scratchpad(tree: &Node, container_id: i64) -> bool {
    find_containing_name(tree, container_id, NodeType::Workspace, None).as_deref()
        == Some("__i3_scratch")
}

/// Recursively walks `node` tracking the name of the nearest ancestor whose
/// `node_type` matches `kind`, returning that name once `con_id` is found —
/// e.g. with `kind: NodeType::Workspace`, the name of the workspace
/// containing `con_id`.
pub(super) fn find_containing_name(
    node: &Node,
    con_id: i64,
    kind: NodeType,
    current: Option<&str>,
) -> Option<String> {
    let current = if node.node_type == kind {
        node.name.as_deref()
    } else {
        current
    };
    if node.id == con_id {
        return current.map(String::from);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| find_containing_name(child, con_id, kind, current))
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
    let Some(workspace) = find_workspace_node(tree, container_id) else {
        return false;
    };
    let Some(chain) = tiling_ancestry(workspace, container_id) else {
        return false;
    };
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

/// The path from `node` down to `container_id` through tiling children
/// (`nodes`) only, `node` first and the container last — `None` when the
/// container isn't reachable that way, which includes every floating window.
fn tiling_ancestry(node: &Node, container_id: i64) -> Option<Vec<&Node>> {
    if node.id == container_id {
        return Some(vec![node]);
    }
    node.nodes.iter().find_map(|child| {
        tiling_ancestry(child, container_id).map(|mut path| {
            path.insert(0, node);
            path
        })
    })
}

pub(super) fn find_workspace_node(node: &Node, container_id: i64) -> Option<&Node> {
    if node.node_type == NodeType::Workspace {
        return if contains_id(node, container_id) {
            Some(node)
        } else {
            None
        };
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| find_workspace_node(child, container_id))
}

pub(super) fn contains_id(node: &Node, container_id: i64) -> bool {
    node.id == container_id
        || node
            .nodes
            .iter()
            .chain(node.floating_nodes.iter())
            .any(|child| contains_id(child, container_id))
}

pub(super) fn find_parent_layout(node: &Node, container_id: i64) -> Option<NodeLayout> {
    if node
        .nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .any(|child| child.id == container_id)
    {
        return Some(node.layout);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| find_parent_layout(child, container_id))
}

pub(super) fn find_node(node: &Node, container_id: i64) -> Option<&Node> {
    if node.id == container_id {
        return Some(node);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| find_node(child, container_id))
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

/// The `(x, y)` a `move position` command actually lands `node` at — the
/// decoration-inclusive frame origin, which is `deco_rect` for an ordinary
/// window and `rect` for one that has no decoration geometry at all.
///
/// Split out from `position_matches()` so this comparison is ordinary pure
/// logic with its own tests, rather than something only reachable through a
/// live `get_tree()`: an external review pointed out that burying decision
/// logic behind a `&mut Connection` is what pushes correctness-critical code
/// below the coverage gate. `expected_position()` was separated from its own
/// output lookup for the same reason.
///
/// The `deco_rect`-unset case is a fullscreen window: confirmed live that its
/// `deco_rect` stays `{0, 0, 0, 0}` permanently — stable across a multi-second
/// sweep, not a transient — since Sway never computes decoration geometry for
/// a window with no border or titlebar to draw. `move position` still lands
/// `rect.x`/`rect.y` on the requested target immediately, so without this
/// fallback such a position could never be confirmed by polling and every
/// invocation burned the full grace period before falling back.
pub(super) fn node_position(node: &Node) -> (i32, i32) {
    if node.deco_rect.width == 0 && node.deco_rect.height == 0 {
        (node.rect.x, node.rect.y)
    } else {
        (node.deco_rect.x, node.deco_rect.y)
    }
}

/// The `(x, y)` `position_matches()` expects `node` to be at, for
/// `Position::Center` or explicit `Position::Coordinates` — matches
/// `deco_rect.x`/`deco_rect.y`, the decoration-inclusive frame `move
/// position` actually targets, confirmed live by
/// `position_moves_a_floating_window_to_given_coordinates`/
/// `position_center_centers_a_floating_window` in `tests/live_sway.rs`
/// (`deco_rect.x` and `rect.x` were confirmed equal there too, so using
/// `deco_rect.x` uniformly for both the coordinate and center cases, rather
/// than `rect.x` for one and `deco_rect.x` for the other, doesn't change
/// which value is being compared). `output_rect` is only consulted for
/// `Position::Center`, so a window not on any output (e.g. the scratchpad)
/// can still match explicit coordinates.
pub(super) fn expected_position(
    position: &Position,
    node: &Node,
    output_rect: Option<swayipc::Rect>,
) -> Option<(i32, i32)> {
    match position {
        Position::Center => Some(compute_center_position(
            output_rect?,
            node.rect.width,
            node.rect.height + node.deco_rect.height,
        )),
        Position::Coordinates { x, y } => Some((*x, *y)),
    }
}

/// The top-left `(x, y)` that centers a `window_width` x `window_height`
/// window (its decoration-inclusive outer footprint) within `output_rect`.
/// `output_rect`'s own `x`/`y` are added in since tree/output coordinates
/// are global, not output-relative — only visibly matters once a second
/// output exists to the left of or above this one, but correct
/// unconditionally rather than assuming this is the primary output.
pub(super) fn compute_center_position(
    output_rect: swayipc::Rect,
    window_width: i32,
    window_height: i32,
) -> (i32, i32) {
    (
        output_rect.x + (output_rect.width - window_width) / 2,
        output_rect.y + (output_rect.height - window_height) / 2,
    )
}

#[cfg(test)]
mod tests;
