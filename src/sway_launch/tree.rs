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

/// Whether `node` is currently in the scratchpad, per its own
/// `scratchpad_state` field. `Some(ScratchpadState::None)` means an ordinary
/// window (present but "no scratchpad state", not the same as the field
/// being absent/`None` outright — both fold into "not in the scratchpad"
/// here) and `Some(ScratchpadState::Fresh)`/`Some(ScratchpadState::Changed)`
/// means one that's actually been moved there, confirmed live. Deliberately
/// not `node_is_floating()`: a window Sway auto-floats as part of moving it
/// to the scratchpad (see `SwayAction::matching_window_change_events()`'s
/// doc comment on `Scratchpad`) would otherwise be misreported as already in
/// the scratchpad the moment it's floating, before it actually is.
///
/// A CI-failure investigation found this field alone isn't reliable enough
/// to gate `already_at_target()` on, though: Sway 1.9 (still what `apt`
/// installs on Ubuntu 24.04/CI) leaves `scratchpad_state` at
/// `Some(ScratchpadState::None)` even for a container genuinely in the
/// scratchpad — the same kind of version-dependent gap `node_is_floating()`'s
/// doc comment documents for the `floating` field, just not caught locally
/// beforehand since this project's own dev/CI environments so far have only
/// ever run Sway 1.11, where the field *is* populated correctly. Kept as a
/// secondary, OR'd check in `container_is_in_scratchpad()` below, alongside
/// the version-independent ancestor-workspace-name check that function uses
/// as its primary signal.
pub(super) fn node_is_in_scratchpad(node: &Node) -> bool {
    !matches!(
        node.scratchpad_state,
        None | Some(swayipc::ScratchpadState::None)
    )
}

/// Whether `container_id` is currently in Sway's scratchpad, given an
/// already-fetched `tree`. Checks the container's ancestor workspace name
/// first: the scratchpad is always the fixed internal workspace Sway names
/// `__i3_scratch`, confirmed live to be populated reliably on both Sway 1.9
/// and 1.11 — unlike `node_is_in_scratchpad()`'s own `scratchpad_state`
/// field check (see that function's doc comment), which is kept here only
/// as a secondary, redundant signal.
pub(super) fn tree_shows_container_in_scratchpad(tree: &Node, container_id: i64) -> bool {
    let in_scratchpad_workspace =
        find_containing_name(tree, container_id, NodeType::Workspace, None).as_deref()
            == Some("__i3_scratch");
    let node_flagged = find_node(tree, container_id).is_some_and(node_is_in_scratchpad);
    in_scratchpad_workspace || node_flagged
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
/// in — used only by `relocates_to_another_output()` to know which of the
/// workspace's own axes/layout to check.
#[derive(Copy, Clone)]
pub(super) enum MoveDirection {
    Right,
    Down,
}

pub(super) fn is_at_the_trailing_workspace_edge(
    tree: &Node,
    container_id: i64,
    direction: MoveDirection,
) -> bool {
    let Some(workspace) = find_workspace_node(tree, container_id) else {
        return false;
    };
    let expected_layout = match direction {
        MoveDirection::Right => NodeLayout::SplitH,
        MoveDirection::Down => NodeLayout::SplitV,
    };
    if workspace.layout != expected_layout {
        return false;
    }
    workspace
        .nodes
        .last()
        .is_some_and(|last| last.id == container_id)
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
/// `tests/live_sway.rs`, and this project's own prior investigation notes
/// in `docs/plan-poll-based-wait-time-actions.md`) — border accounting
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

/// The `(x, y)` `position_matches()` expects `node` to be at, for `"center"`
/// or a validated `"<x>,<y>"` string (`validate_position_argument`'s regex
/// guarantees the latter parses cleanly by the time this runs) — matches
/// `deco_rect.x`/`deco_rect.y`, the decoration-inclusive frame `move
/// position` actually targets, confirmed live by
/// `position_moves_a_floating_window_to_given_coordinates`/
/// `position_center_centers_a_floating_window` in `tests/live_sway.rs`
/// (`deco_rect.x` and `rect.x` were confirmed equal there too, so using
/// `deco_rect.x` uniformly for both the coordinate and center cases, rather
/// than `rect.x` for one and `deco_rect.x` for the other, doesn't change
/// which value is being compared). `output_name` is only consulted for
/// `"center"`, so a window not on any output (e.g. the scratchpad) can
/// still match a plain `"<x>,<y>"` position.
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
mod tests {
    use super::super::test_support::*;
    use super::*;

    // window_app_id_match / window_class_match

    #[test]
    fn window_app_id_match_true_when_equal() {
        let event = window_event("new", 1, Some("foot"), None);
        assert!(window_app_id_match(&event.container, "foot"));
    }

    #[test]
    fn window_app_id_match_false_when_different() {
        let event = window_event("new", 1, Some("foot"), None);
        assert!(!window_app_id_match(&event.container, "alacritty"));
    }

    #[test]
    fn window_app_id_match_false_when_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_app_id_match(&event.container, "foot"));
    }

    #[test]
    fn window_class_match_true_when_equal() {
        let event = window_event("new", 1, None, Some("Firefox"));
        assert!(window_class_match(&event.container, "Firefox"));
    }

    #[test]
    fn window_class_match_false_when_different() {
        let event = window_event("new", 1, None, Some("Firefox"));
        assert!(!window_class_match(&event.container, "Chromium"));
    }

    #[test]
    fn window_class_match_false_when_window_properties_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_class_match(&event.container, "Firefox"));
    }

    #[test]
    fn window_class_match_false_when_class_absent_but_window_properties_present() {
        let mut value = leaf_node_value(1, None, None);
        value["window_properties"] = serde_json::json!({});
        let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
        assert!(!window_class_match(&node, "Firefox"));
    }

    #[test]
    fn window_mark_match_true_when_present() {
        let mut value = leaf_node_value(1, None, None);
        value["marks"] = serde_json::json!(["dropdown-term"]);
        let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
        assert!(window_mark_match(&node, "dropdown-term"));
    }

    #[test]
    fn window_mark_match_false_when_different() {
        let mut value = leaf_node_value(1, None, None);
        value["marks"] = serde_json::json!(["other-mark"]);
        let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
        assert!(!window_mark_match(&node, "dropdown-term"));
    }

    #[test]
    fn window_mark_match_false_when_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_mark_match(&event.container, "dropdown-term"));
    }

    // matching_container_ids

    #[test]
    fn matching_container_ids_finds_tiling_and_floating_matches() {
        let tree = node_tree(
            1,
            vec![
                leaf_node_value(10, Some("foot"), None),
                leaf_node_value(11, Some("firefox"), None),
            ],
            vec![leaf_node_value(20, Some("foot"), None)],
        );
        let mut ids = matching_container_ids(&tree, "foot", "", "");
        ids.sort();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn matching_container_ids_empty_when_no_match() {
        let tree = node_tree(1, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
        assert_eq!(
            matching_container_ids(&tree, "nonexistent", "", ""),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn matching_container_ids_matches_by_class() {
        let tree = node_tree(1, vec![leaf_node_value(10, None, Some("Firefox"))], vec![]);
        assert_eq!(matching_container_ids(&tree, "", "Firefox", ""), vec![10]);
    }

    #[test]
    fn matching_container_ids_matches_by_mark() {
        let mut value = leaf_node_value(10, None, None);
        value["marks"] = serde_json::json!(["dropdown-term"]);
        let tree = node_tree(1, vec![value], vec![]);
        assert_eq!(
            matching_container_ids(&tree, "", "", "dropdown-term"),
            vec![10]
        );
    }

    #[test]
    fn matching_container_ids_recurses_into_nested_containers() {
        let inner = container_node_value(2, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
        let tree = node_tree(1, vec![inner], vec![]);
        assert_eq!(matching_container_ids(&tree, "foot", "", ""), vec![10]);
    }

    #[test]
    fn matching_container_ids_prefers_app_id_over_class_when_both_set() {
        let tree = node_tree(
            1,
            vec![leaf_node_value(10, Some("foot"), Some("NoMatch"))],
            vec![],
        );
        assert_eq!(
            matching_container_ids(&tree, "foot", "NoMatch", ""),
            vec![10]
        );
    }

    #[test]
    fn matching_container_ids_prefers_class_over_mark_when_both_set() {
        let mut value = leaf_node_value(10, None, Some("Firefox"));
        value["marks"] = serde_json::json!(["no-match"]);
        let tree = node_tree(1, vec![value], vec![]);
        assert_eq!(
            matching_container_ids(&tree, "", "Firefox", "no-match"),
            vec![10]
        );
    }

    // resolve_matches

    #[test]
    fn resolve_matches_errors_on_zero_matches() {
        assert_eq!(
            resolve_matches(vec![], "app_id \"foot\""),
            Err("No existing window matches app_id \"foot\"".to_string())
        );
    }

    #[test]
    fn resolve_matches_ok_on_single_match() {
        assert_eq!(resolve_matches(vec![42], "app_id \"foot\""), Ok(42));
    }

    #[test]
    fn resolve_matches_errors_listing_ids_on_multiple_matches() {
        assert_eq!(
            resolve_matches(vec![42, 91], "app_id \"foot\""),
            Err("2 windows match app_id \"foot\": 42, 91 — retarget with --con-id".to_string())
        );
    }

    // find_containing_name / find_workspace_node / contains_id /
    // is_at_the_trailing_workspace_edge

    #[test]
    fn find_containing_name_finds_the_nearest_ancestor_of_kind() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree(2, "main", vec![leaf], vec![]);
        assert_eq!(
            find_containing_name(&workspace, 10, NodeType::Workspace, None),
            Some("main".to_string())
        );
    }

    #[test]
    fn find_containing_name_returns_none_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert_eq!(
            find_containing_name(&tree, 999, NodeType::Workspace, None),
            None
        );
    }

    #[test]
    fn find_workspace_node_locates_the_workspace_containing_the_id() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree(2, "main", vec![leaf], vec![]);
        let found = find_workspace_node(&workspace, 10).expect("should find workspace");
        assert_eq!(found.name.as_deref(), Some("main"));
    }

    #[test]
    fn find_workspace_node_returns_none_when_id_not_found() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree(2, "main", vec![leaf], vec![]);
        assert!(find_workspace_node(&workspace, 999).is_none());
    }

    #[test]
    fn contains_id_true_for_self_and_descendants() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let tree = node_tree(1, vec![leaf], vec![]);
        assert!(contains_id(&tree, 1));
        assert!(contains_id(&tree, 10));
    }

    #[test]
    fn contains_id_false_when_absent() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(!contains_id(&tree, 42));
    }

    #[test]
    fn find_parent_layout_returns_the_direct_parents_layout() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let mut parent_value = container_node_value(2, vec![leaf], vec![]);
        parent_value["layout"] = serde_json::json!("splitv");
        let parent: Node = serde_json::from_value(parent_value).expect("valid Node test fixture");
        assert_eq!(find_parent_layout(&parent, 10), Some(NodeLayout::SplitV));
    }

    #[test]
    fn find_parent_layout_finds_the_nearest_ancestor_when_nested() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let mut inner_value = container_node_value(2, vec![leaf], vec![]);
        inner_value["layout"] = serde_json::json!("splith");
        let tree = node_tree(1, vec![inner_value], vec![]);
        assert_eq!(find_parent_layout(&tree, 10), Some(NodeLayout::SplitH));
    }

    #[test]
    fn find_parent_layout_checks_floating_children_too() {
        let floating = leaf_node_value(20, Some("foot"), None);
        let mut value = container_node_value(1, vec![], vec![floating]);
        value["layout"] = serde_json::json!("splitv");
        let tree: Node = serde_json::from_value(value).expect("valid Node test fixture");
        assert_eq!(find_parent_layout(&tree, 20), Some(NodeLayout::SplitV));
    }

    #[test]
    fn find_parent_layout_returns_none_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(find_parent_layout(&tree, 42).is_none());
    }

    // find_node

    #[test]
    fn find_node_finds_self_and_nested_children() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let inner = container_node_value(2, vec![leaf], vec![]);
        let tree = node_tree(1, vec![inner], vec![]);
        assert_eq!(find_node(&tree, 1).map(|node| node.id), Some(1));
        assert_eq!(find_node(&tree, 2).map(|node| node.id), Some(2));
        assert_eq!(find_node(&tree, 10).map(|node| node.id), Some(10));
    }

    #[test]
    fn find_node_finds_floating_children() {
        let floating = leaf_node_value(20, Some("foot"), None);
        let tree = node_tree(1, vec![], vec![floating]);
        assert_eq!(find_node(&tree, 20).map(|node| node.id), Some(20));
    }

    #[test]
    fn find_node_returns_none_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(find_node(&tree, 42).is_none());
    }

    // width_matches / height_matches

    fn node_with_geometry(width: i32, height: i32, border_width: i32, deco_height: i32) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        value["rect"] = serde_json::json!({"x": 0, "y": 0, "width": width, "height": height});
        value["deco_rect"] =
            serde_json::json!({"x": 0, "y": 0, "width": width, "height": deco_height});
        value["current_border_width"] = serde_json::json!(border_width);
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn width_matches_exact_rect_width() {
        let node = node_with_geometry(400, 300, 2, 25);
        assert!(width_matches(&node, 400));
    }

    #[test]
    fn width_matches_border_adjusted_width() {
        let node = node_with_geometry(396, 300, 2, 25);
        assert!(width_matches(&node, 400));
    }

    #[test]
    fn width_matches_false_when_neither_formula_fits() {
        let node = node_with_geometry(350, 300, 2, 25);
        assert!(!width_matches(&node, 400));
    }

    #[test]
    fn height_matches_decoration_inclusive_height() {
        let node = node_with_geometry(400, 275, 2, 25);
        assert!(height_matches(&node, 300));
    }

    #[test]
    fn height_matches_false_when_short_of_the_expected_value() {
        let node = node_with_geometry(400, 300, 2, 25);
        assert!(!height_matches(&node, 300));
    }

    // node_is_floating

    fn node_with_floating_state(node_type: &str, floating: Option<&str>) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        value["type"] = serde_json::json!(node_type);
        if let Some(floating) = floating {
            value["floating"] = serde_json::json!(floating);
        }
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn node_is_floating_true_for_floating_con_type_even_without_a_floating_field() {
        // Sway 1.9 (confirmed live against a headless compositor) leaves a
        // floating container's own `floating` field null and only reports
        // its state via `type: floating_con` — node_is_floating() must not
        // rely on the `floating` field alone.
        let node = node_with_floating_state("floating_con", None);
        assert!(node_is_floating(&node));
    }

    #[test]
    fn node_is_floating_true_when_the_floating_field_is_set() {
        let node = node_with_floating_state("con", Some("user_on"));
        assert!(node_is_floating(&node));
    }

    #[test]
    fn node_is_floating_false_for_a_plain_tiled_node() {
        let node = node_with_floating_state("con", None);
        assert!(!node_is_floating(&node));
    }

    // node_is_in_scratchpad

    fn node_with_scratchpad_state(state: Option<&str>) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        if let Some(state) = state {
            value["scratchpad_state"] = serde_json::json!(state);
        }
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn node_is_in_scratchpad_false_when_the_field_is_absent() {
        let node = node_with_scratchpad_state(None);
        assert!(!node_is_in_scratchpad(&node));
    }

    #[test]
    fn node_is_in_scratchpad_false_for_scratchpad_state_none() {
        // Present but "no scratchpad state" — an ordinary window, not one
        // in the scratchpad.
        let node = node_with_scratchpad_state(Some("none"));
        assert!(!node_is_in_scratchpad(&node));
    }

    #[test]
    fn node_is_in_scratchpad_true_for_fresh() {
        let node = node_with_scratchpad_state(Some("fresh"));
        assert!(node_is_in_scratchpad(&node));
    }

    #[test]
    fn node_is_in_scratchpad_true_for_changed() {
        let node = node_with_scratchpad_state(Some("changed"));
        assert!(node_is_in_scratchpad(&node));
    }

    // tree_shows_container_in_scratchpad

    #[test]
    fn tree_shows_container_in_scratchpad_true_via_ancestor_workspace_name() {
        // Regression test for a CI failure against Sway 1.9 (still what
        // `apt` installs on Ubuntu 24.04/CI): scratchpad_state stays
        // Some(ScratchpadState::None) there even for a genuinely
        // scratchpadded window, so this must detect the scratchpad via the
        // ancestor workspace name alone, with no scratchpad_state set at
        // all on the leaf node.
        let leaf = leaf_node_value(10, Some("foot"), None);
        let tree = workspace_node_tree(2, "__i3_scratch", vec![leaf], vec![]);
        assert!(tree_shows_container_in_scratchpad(&tree, 10));
    }

    #[test]
    fn tree_shows_container_in_scratchpad_true_via_scratchpad_state_fallback() {
        // The node's own scratchpad_state is still honored as a secondary,
        // redundant signal, independent of the ancestor workspace name.
        let mut leaf = leaf_node_value(10, Some("foot"), None);
        leaf["scratchpad_state"] = serde_json::json!("fresh");
        let tree = workspace_node_tree(2, "1", vec![leaf], vec![]);
        assert!(tree_shows_container_in_scratchpad(&tree, 10));
    }

    #[test]
    fn tree_shows_container_in_scratchpad_false_for_an_ordinary_window() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let tree = workspace_node_tree(2, "1", vec![leaf], vec![]);
        assert!(!tree_shows_container_in_scratchpad(&tree, 10));
    }

    // ContainerState::from_tree

    /// A root → output → workspace → leaf tree, the shape `get_tree()`
    /// actually returns, so the ancestor lookups have real ancestors to walk.

    #[test]
    fn container_state_from_tree_reads_the_containing_workspace_and_output() {
        let state = ContainerState::from_tree(
            &state_tree("3", leaf_node_value(10, Some("foot"), None)),
            10,
        )
        .expect("the container is in the tree");

        assert_eq!(state.workspace.as_deref(), Some("3"));
        assert_eq!(state.output.as_deref(), Some("HEADLESS-1"));
    }

    #[test]
    fn container_state_from_tree_is_none_for_a_container_not_in_the_tree() {
        let tree = state_tree("3", leaf_node_value(10, Some("foot"), None));
        assert_eq!(ContainerState::from_tree(&tree, 99), None);
    }

    #[test]
    fn container_state_from_tree_reads_the_containers_own_flags_and_marks() {
        let mut leaf = leaf_node_value(10, Some("foot"), None);
        leaf["type"] = serde_json::json!("floating_con");
        leaf["fullscreen_mode"] = serde_json::json!(1);
        leaf["focused"] = serde_json::json!(true);
        leaf["marks"] = serde_json::json!(["dropdown-term"]);

        let state = ContainerState::from_tree(&state_tree("3", leaf), 10)
            .expect("the container is in the tree");

        assert!(state.floating);
        assert!(state.fullscreen);
        assert!(state.focused);
        assert_eq!(state.marks, vec!["dropdown-term".to_string()]);
        assert!(!state.in_scratchpad);
    }

    #[test]
    fn container_state_from_tree_defaults_a_plain_tiled_window_to_every_flag_unset() {
        let state = ContainerState::from_tree(
            &state_tree("3", leaf_node_value(10, Some("foot"), None)),
            10,
        )
        .expect("the container is in the tree");

        assert!(!state.floating);
        assert!(!state.fullscreen);
        assert!(!state.focused);
        assert!(!state.in_scratchpad);
        assert!(state.marks.is_empty());
    }

    #[test]
    fn container_state_from_tree_reports_a_scratchpadded_container() {
        let state = ContainerState::from_tree(
            &state_tree("__i3_scratch", leaf_node_value(10, Some("foot"), None)),
            10,
        )
        .expect("the container is in the tree");

        assert!(state.in_scratchpad);
    }

    #[test]
    fn container_state_from_tree_reports_fullscreen_mode_zero_as_not_fullscreen() {
        // Sway populates the field with 0 for an ordinary window rather than
        // leaving it unset, so "present" is not the same as "fullscreen".
        let mut leaf = leaf_node_value(10, Some("foot"), None);
        leaf["fullscreen_mode"] = serde_json::json!(0);

        let state = ContainerState::from_tree(&state_tree("3", leaf), 10)
            .expect("the container is in the tree");

        assert!(!state.fullscreen);
    }

    // compute_center_position

    #[test]
    fn compute_center_position_centers_within_a_primary_output() {
        let output_rect = rect(0, 0, 1280, 720);
        assert_eq!(compute_center_position(output_rect, 400, 300), (440, 210));
    }

    #[test]
    fn compute_center_position_accounts_for_a_non_origin_output() {
        let output_rect = rect(1920, 100, 1280, 720);
        assert_eq!(compute_center_position(output_rect, 400, 300), (2360, 310));
    }

    // node_position

    fn node_with_rects(rect_x: i32, rect_y: i32, deco_x: i32, deco_y: i32, deco_set: bool) -> Node {
        let mut value = leaf_node_value(10, Some("foot"), None);
        value["rect"] = serde_json::json!({"x": rect_x, "y": rect_y, "width": 400, "height": 300});
        value["deco_rect"] = if deco_set {
            serde_json::json!({"x": deco_x, "y": deco_y, "width": 400, "height": 25})
        } else {
            serde_json::json!({"x": 0, "y": 0, "width": 0, "height": 0})
        };
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn node_position_uses_the_decoration_frame_for_an_ordinary_window() {
        let node = node_with_rects(100, 225, 100, 200, true);
        assert_eq!(node_position(&node), (100, 200));
    }

    #[test]
    fn node_position_falls_back_to_rect_when_decoration_geometry_is_unset() {
        // A fullscreen window: Sway never computes deco_rect for a window
        // with no border/titlebar, so it stays {0,0,0,0} permanently while
        // `move position` still lands rect.x/rect.y on the target. Comparing
        // deco_rect alone would make such a position unconfirmable.
        let node = node_with_rects(100, 200, 0, 0, false);
        assert_eq!(node_position(&node), (100, 200));
    }

    #[test]
    fn node_position_does_not_treat_a_zero_origin_decoration_as_unset() {
        // Only zero *width and height* means "no decoration geometry". A
        // window legitimately positioned at the origin still has a real
        // deco_rect, and must not be misread as the fullscreen case.
        let node = node_with_rects(0, 25, 0, 0, true);
        assert_eq!(node_position(&node), (0, 0));
    }

    // expected_position

    #[test]
    fn expected_position_parses_explicit_coordinates() {
        let node = node_with_geometry(400, 300, 0, 0);
        assert_eq!(
            expected_position(&Position::Coordinates { x: 100, y: 200 }, &node, None),
            Some((100, 200))
        );
    }

    #[test]
    fn expected_position_center_without_an_output_rect_is_none() {
        // A window on no known output (the scratchpad, say) has nothing to
        // centre against.
        let node = node_with_geometry(400, 300, 0, 0);
        assert_eq!(expected_position(&Position::Center, &node, None), None);
    }

    #[test]
    fn expected_position_centers_against_the_given_output_rect() {
        // Coverable headlessly now that the output geometry is passed in
        // rather than looked up here — this arm used to need a live socket
        // and was exempt from coverage accordingly.
        let node = node_with_geometry(400, 300, 0, 25);
        assert_eq!(
            expected_position(&Position::Center, &node, Some(rect(0, 0, 1920, 1080))),
            Some(((1920 - 400) / 2, (1080 - 325) / 2))
        );
    }

    #[test]
    fn expected_position_center_accounts_for_a_non_origin_output() {
        let node = node_with_geometry(400, 300, 0, 0);
        assert_eq!(
            expected_position(&Position::Center, &node, Some(rect(1920, 0, 1920, 1080))),
            Some((1920 + (1920 - 400) / 2, (1080 - 300) / 2))
        );
    }

    fn workspace_node_tree_with_layout(
        container_id: i64,
        name: &str,
        layout: &str,
        nodes: Vec<serde_json::Value>,
    ) -> Node {
        let mut value = container_node_value(container_id, nodes, vec![]);
        value["type"] = serde_json::json!("workspace");
        value["name"] = serde_json::json!(name);
        value["layout"] = serde_json::json!(layout);
        serde_json::from_value(value).expect("valid Node test fixture")
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_true_for_a_solo_window() {
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![leaf]);
        assert!(is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_true_for_the_last_of_several_siblings() {
        // The case the old solo-window-only check missed: container_id has
        // a sibling, but is still the trailing (rightmost) child of a
        // workspace whose own layout already matches the move axis.
        let leaf1 = leaf_node_value(10, Some("foot"), None);
        let leaf2 = leaf_node_value(11, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![leaf1, leaf2]);
        assert!(is_at_the_trailing_workspace_edge(
            &workspace,
            11,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_false_for_a_leading_sibling() {
        let leaf1 = leaf_node_value(10, Some("foot"), None);
        let leaf2 = leaf_node_value(11, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![leaf1, leaf2]);
        assert!(!is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_false_when_the_workspace_layout_does_not_match_the_axis() {
        // Confirmed live: a solo window stacked via splitv, then moved
        // right, restructures in place rather than escalating — the
        // workspace's own layout has to match the move's axis too, not
        // just "container_id is the trailing child".
        let leaf = leaf_node_value(10, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splitv", vec![leaf]);
        assert!(!is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Right
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_checks_the_down_axis_for_new_row() {
        let leaf1 = leaf_node_value(10, Some("foot"), None);
        let leaf2 = leaf_node_value(11, Some("foot"), None);
        let workspace = workspace_node_tree_with_layout(2, "main", "splitv", vec![leaf1, leaf2]);
        assert!(is_at_the_trailing_workspace_edge(
            &workspace,
            11,
            MoveDirection::Down
        ));
        assert!(!is_at_the_trailing_workspace_edge(
            &workspace,
            10,
            MoveDirection::Down
        ));
    }

    #[test]
    fn is_at_the_trailing_workspace_edge_false_when_id_not_found() {
        let tree = node_tree(1, vec![], vec![]);
        assert!(!is_at_the_trailing_workspace_edge(
            &tree,
            999,
            MoveDirection::Right
        ));
    }
}
