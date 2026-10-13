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

// descendants / ancestors_of

fn ids<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<i64> {
    nodes.into_iter().map(|node| node.id).collect()
}

#[test]
fn descendants_visits_self_then_tiling_then_floating_depth_first() {
    let inner = container_node_value(2, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    let tree = node_tree(
        1,
        vec![inner, leaf_node_value(11, Some("foot"), None)],
        vec![leaf_node_value(20, Some("foot"), None)],
    );
    assert_eq!(ids(descendants(&tree)), vec![1, 2, 10, 11, 20]);
}

#[test]
fn ancestors_of_returns_the_root_to_target_path() {
    let inner = container_node_value(2, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    let tree = node_tree(1, vec![inner], vec![]);
    let path = ancestors_of(&tree, 10).expect("the container is in the tree");
    assert_eq!(ids(path), vec![1, 2, 10]);
}

#[test]
fn ancestors_of_reaches_floating_children() {
    let floating = container_node_value(3, vec![leaf_node_value(20, Some("foot"), None)], vec![]);
    let tree = node_tree(
        1,
        vec![leaf_node_value(10, Some("foot"), None)],
        vec![floating],
    );
    let path = ancestors_of(&tree, 20).expect("the container is in the tree");
    assert_eq!(ids(path), vec![1, 3, 20]);
}

#[test]
fn ancestors_of_the_root_is_just_the_root() {
    let tree = node_tree(1, vec![], vec![]);
    assert_eq!(
        ids(ancestors_of(&tree, 1).expect("the root is in the tree")),
        vec![1]
    );
}

#[test]
fn ancestors_of_is_none_when_id_not_found() {
    let tree = node_tree(1, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    assert!(ancestors_of(&tree, 999).is_none());
}

// find_workspace_node / is_at_the_trailing_workspace_edge

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
fn present_ids_keeps_only_ids_still_in_the_tree_in_the_given_order() {
    let tiled = leaf_node_value(10, Some("foot"), None);
    let floating = leaf_node_value(11, Some("foot"), None);
    let tree = node_tree(1, vec![tiled], vec![floating]);
    assert_eq!(present_ids(&tree, &[11, 42, 10]), vec![11, 10]);
}

#[test]
fn present_ids_is_empty_when_every_id_is_gone() {
    let tree = node_tree(1, vec![], vec![]);
    assert!(present_ids(&tree, &[10, 11]).is_empty());
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
    value["deco_rect"] = serde_json::json!({"x": 0, "y": 0, "width": width, "height": deco_height});
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

// ContainerState::from_tree's in_scratchpad

fn in_scratchpad(tree: &Node, container_id: i64) -> bool {
    ContainerState::from_tree(tree, container_id)
        .expect("the container is in the tree")
        .in_scratchpad
}

#[test]
fn in_scratchpad_true_via_ancestor_workspace_name() {
    // Regression test for a CI failure against Sway 1.9 (still what
    // `apt` installs on Ubuntu 24.04/CI): scratchpad_state stays
    // Some(ScratchpadState::None) there even for a genuinely
    // scratchpadded window, so this must detect the scratchpad via the
    // ancestor workspace name alone, with no scratchpad_state set at
    // all on the leaf node.
    let leaf = leaf_node_value(10, Some("foot"), None);
    let tree = workspace_node_tree(2, "__i3_scratch", vec![leaf], vec![]);
    assert!(in_scratchpad(&tree, 10));
}

#[test]
fn in_scratchpad_false_for_a_shown_scratchpad_window() {
    // A scratchpad window brought onto an ordinary workspace via
    // `scratchpad show` keeps scratchpad_state "fresh"/"changed" on Sway
    // 1.11, but isn't hidden in the scratchpad — --scratchpad must still
    // move it back rather than short-circuit as already satisfied.
    for state in ["fresh", "changed"] {
        let mut leaf = leaf_node_value(10, Some("foot"), None);
        leaf["scratchpad_state"] = serde_json::json!(state);
        let tree = workspace_node_tree(2, "1", vec![leaf], vec![]);
        assert!(
            !in_scratchpad(&tree, 10),
            "scratchpad_state {state:?} on an ordinary workspace"
        );
    }
}

#[test]
fn in_scratchpad_false_for_an_ordinary_window() {
    let leaf = leaf_node_value(10, Some("foot"), None);
    let tree = workspace_node_tree(2, "1", vec![leaf], vec![]);
    assert!(!in_scratchpad(&tree, 10));
}

// ContainerState::from_tree

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
fn compute_center_position_centers_within_a_workspace_at_the_origin() {
    let workspace_rect = rect(0, 0, 1280, 720);
    assert_eq!(
        compute_center_position(workspace_rect, 400, 300),
        (440, 210)
    );
}

#[test]
fn compute_center_position_is_relative_to_the_workspace_origin() {
    // The result is workspace-relative, the space `move position` and
    // deco_rect both use, so the workspace's own origin isn't added in.
    let workspace_rect = rect(1920, 100, 1280, 720);
    assert_eq!(
        compute_center_position(workspace_rect, 400, 300),
        (440, 210)
    );
}

// node_position

/// A 1200x640 workspace at (40, 40): what `gaps outer 40` on a 1280x720
/// output gives, and the same shape a bar produces on one edge.
fn offset_workspace_rect() -> swayipc::Rect {
    rect(40, 40, 1200, 640)
}

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
    assert_eq!(node_position(&node, rect(0, 0, 1280, 720)), (100, 200));
}

#[test]
fn node_position_reads_the_decoration_frame_as_already_workspace_relative() {
    // deco_rect is relative to the workspace for a floating window, so an
    // offset workspace changes rect (absolute) but not deco_rect.
    let node = node_with_rects(140, 265, 100, 200, true);
    assert_eq!(node_position(&node, offset_workspace_rect()), (100, 200));
}

#[test]
fn node_position_falls_back_to_rect_when_decoration_geometry_is_unset() {
    // A fullscreen window, or one with a pixel/no border: Sway reports an
    // all-zero deco_rect for every window drawing no title bar, while `move
    // position` still lands rect.x/rect.y on the target. Comparing deco_rect
    // alone would make such a position unconfirmable.
    let node = node_with_rects(100, 200, 0, 0, false);
    assert_eq!(node_position(&node, rect(0, 0, 1280, 720)), (100, 200));
}

#[test]
fn node_position_makes_the_rect_fallback_workspace_relative() {
    // Reproduced against Sway 1.11 with `gaps outer 40` and `border pixel 2`:
    // `move position 100 200` left rect at (140, 240) and deco_rect all zero.
    // rect is absolute, so the workspace origin has to come off it.
    let node = node_with_rects(140, 240, 0, 0, false);
    assert_eq!(node_position(&node, offset_workspace_rect()), (100, 200));
}

#[test]
fn node_position_makes_the_rect_fallback_relative_to_a_negative_origin() {
    // A workspace on an output placed left of the primary one.
    let node = node_with_rects(-1180, 200, 0, 0, false);
    assert_eq!(node_position(&node, rect(-1280, 0, 1280, 720)), (100, 200));
}

#[test]
fn node_position_does_not_treat_a_zero_origin_decoration_as_unset() {
    // Only zero *width and height* means "no decoration geometry". A
    // window legitimately positioned at the origin still has a real
    // deco_rect, and must not be misread as the fallback case.
    let node = node_with_rects(40, 65, 0, 0, true);
    assert_eq!(node_position(&node, offset_workspace_rect()), (0, 0));
}

// expected_position

#[test]
fn expected_position_parses_explicit_coordinates() {
    let node = node_with_geometry(400, 300, 0, 0);
    assert_eq!(
        expected_position(
            &Position::Coordinates { x: 100, y: 200 },
            &node,
            offset_workspace_rect()
        ),
        (100, 200)
    );
}

#[test]
fn expected_position_centers_against_the_given_workspace_rect() {
    let node = node_with_geometry(400, 300, 0, 25);
    assert_eq!(
        expected_position(&Position::Center, &node, rect(0, 0, 1920, 1080)),
        ((1920 - 400) / 2, (1080 - 325) / 2)
    );
}

#[test]
fn expected_position_centers_on_the_workspace_not_the_output() {
    // A bar or outer gaps shrink the workspace, and Sway centers on what's
    // left: centering on the 1280x720 output instead was what reported
    // "unconfirmed" for a correctly centered window.
    let node = node_with_geometry(400, 300, 0, 0);
    assert_eq!(
        expected_position(&Position::Center, &node, offset_workspace_rect()),
        ((1200 - 400) / 2, (640 - 300) / 2)
    );
}

#[test]
fn expected_position_center_ignores_a_non_origin_workspace_s_offset() {
    let node = node_with_geometry(400, 300, 0, 0);
    assert_eq!(
        expected_position(&Position::Center, &node, rect(-1280, 23, 1280, 697)),
        ((1280 - 400) / 2, (697 - 300) / 2)
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

fn sub_container_value(
    container_id: i64,
    layout: &str,
    nodes: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut value = container_node_value(container_id, nodes, vec![]);
    value["layout"] = serde_json::json!(layout);
    value
}

#[test]
fn is_at_the_trailing_workspace_edge_true_for_a_lone_child_of_a_tabbed_wrapper() {
    // `layout tabbed` on a solo window, and every first window under
    // `workspace_layout tabbed`: Sway treats the singleton like a window
    // at workspace level and escalates (confirmed live).
    let wrapper = sub_container_value(5, "tabbed", vec![leaf_node_value(10, None, None)]);
    let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![wrapper]);
    assert!(is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Right
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_true_for_a_lone_child_of_a_cross_axis_wrapper() {
    // A window left alone in a `splitv` container after its sibling
    // closed: the cross-axis wrapper doesn't absorb a `move right`.
    let wrapper = sub_container_value(5, "splitv", vec![leaf_node_value(11, None, None)]);
    let workspace = workspace_node_tree_with_layout(
        2,
        "main",
        "splith",
        vec![leaf_node_value(10, None, None), wrapper],
    );
    assert!(is_at_the_trailing_workspace_edge(
        &workspace,
        11,
        MoveDirection::Right
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_true_for_a_lone_child_of_a_stacked_wrapper_moving_down() {
    let wrapper = sub_container_value(5, "stacked", vec![leaf_node_value(10, None, None)]);
    let workspace = workspace_node_tree_with_layout(2, "main", "splitv", vec![wrapper]);
    assert!(is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Down
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_false_for_a_lone_child_whose_wrapper_has_a_following_sibling()
{
    let wrapper = sub_container_value(5, "tabbed", vec![leaf_node_value(10, None, None)]);
    let workspace = workspace_node_tree_with_layout(
        2,
        "main",
        "splith",
        vec![wrapper, leaf_node_value(11, None, None)],
    );
    assert!(!is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Right
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_false_for_a_trailing_child_of_a_sub_container_with_siblings() {
    // Confirmed live: Sway promotes the window out of the sub-container
    // onto the same output instead of escalating.
    let wrapper = sub_container_value(
        5,
        "splith",
        vec![
            leaf_node_value(10, None, None),
            leaf_node_value(11, None, None),
        ],
    );
    let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![wrapper]);
    assert!(!is_at_the_trailing_workspace_edge(
        &workspace,
        11,
        MoveDirection::Right
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_false_for_a_lone_child_two_wrappers_deep() {
    // Sway's singleton rule only applies when the container's parent
    // sits directly on the workspace; one level deeper it promotes.
    let inner = sub_container_value(6, "splith", vec![leaf_node_value(10, None, None)]);
    let outer = sub_container_value(5, "splitv", vec![inner]);
    let workspace = workspace_node_tree_with_layout(2, "main", "splith", vec![outer]);
    assert!(!is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Right
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_treats_a_tabbed_workspace_as_horizontal() {
    let workspace =
        workspace_node_tree_with_layout(2, "main", "tabbed", vec![leaf_node_value(10, None, None)]);
    assert!(is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Right
    ));
    assert!(!is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Down
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_false_for_a_floating_window() {
    let mut value = container_node_value(2, vec![], vec![leaf_node_value(10, None, None)]);
    value["type"] = serde_json::json!("workspace");
    value["name"] = serde_json::json!("main");
    value["layout"] = serde_json::json!("splith");
    let workspace: Node = serde_json::from_value(value).expect("valid Node test fixture");
    assert!(!is_at_the_trailing_workspace_edge(
        &workspace,
        10,
        MoveDirection::Right
    ));
}

#[test]
fn is_at_the_trailing_workspace_edge_false_for_the_workspace_itself() {
    let workspace =
        workspace_node_tree_with_layout(2, "main", "splith", vec![leaf_node_value(10, None, None)]);
    assert!(!is_at_the_trailing_workspace_edge(
        &workspace,
        2,
        MoveDirection::Right
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
