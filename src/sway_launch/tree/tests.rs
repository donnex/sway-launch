use super::super::target::WindowMatch;
use super::super::test_support::*;
use super::*;

/// Criteria with just `matcher`, the shape `--existing` had before `--title`.
fn by(matcher: WindowMatch) -> WindowCriteria {
    WindowCriteria {
        matcher: Some(matcher),
        title: None,
    }
}

// WindowMatch::matches

#[test]
fn window_app_id_match_true_when_equal() {
    let event = window_event("new", 1, Some("foot"), None);
    assert!(WindowMatch::AppId("foot").matches(&event.container));
}

#[test]
fn window_app_id_match_false_when_different() {
    let event = window_event("new", 1, Some("foot"), None);
    assert!(!WindowMatch::AppId("alacritty").matches(&event.container));
}

#[test]
fn window_app_id_match_false_when_absent() {
    let event = window_event("new", 1, None, None);
    assert!(!WindowMatch::AppId("foot").matches(&event.container));
}

#[test]
fn window_class_match_true_when_equal() {
    let event = window_event("new", 1, None, Some("Firefox"));
    assert!(WindowMatch::Class("Firefox").matches(&event.container));
}

#[test]
fn window_class_match_false_when_different() {
    let event = window_event("new", 1, None, Some("Firefox"));
    assert!(!WindowMatch::Class("Chromium").matches(&event.container));
}

#[test]
fn window_class_match_false_when_window_properties_absent() {
    let event = window_event("new", 1, None, None);
    assert!(!WindowMatch::Class("Firefox").matches(&event.container));
}

#[test]
fn window_class_match_false_when_class_absent_but_window_properties_present() {
    let mut value = leaf_node_value(1, None, None);
    value["window_properties"] = serde_json::json!({});
    let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
    assert!(!WindowMatch::Class("Firefox").matches(&node));
}

#[test]
fn window_mark_match_true_when_present() {
    let mut value = leaf_node_value(1, None, None);
    value["marks"] = serde_json::json!(["dropdown-term"]);
    let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
    assert!(WindowMatch::Mark("dropdown-term").matches(&node));
}

#[test]
fn window_mark_match_false_when_different() {
    let mut value = leaf_node_value(1, None, None);
    value["marks"] = serde_json::json!(["other-mark"]);
    let node: Node = serde_json::from_value(value).expect("valid Node test fixture");
    assert!(!WindowMatch::Mark("dropdown-term").matches(&node));
}

#[test]
fn window_mark_match_false_when_absent() {
    let event = window_event("new", 1, None, None);
    assert!(!WindowMatch::Mark("dropdown-term").matches(&event.container));
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
    let mut ids = matching_container_ids(&tree, by(WindowMatch::AppId("foot")));
    ids.sort();
    assert_eq!(ids, vec![10, 20]);
}

#[test]
fn matching_container_ids_empty_when_no_match() {
    let tree = node_tree(1, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    assert_eq!(
        matching_container_ids(&tree, by(WindowMatch::AppId("nonexistent"))),
        Vec::<i64>::new()
    );
}

#[test]
fn matching_container_ids_matches_by_class() {
    let tree = node_tree(1, vec![leaf_node_value(10, None, Some("Firefox"))], vec![]);
    assert_eq!(
        matching_container_ids(&tree, by(WindowMatch::Class("Firefox"))),
        vec![10]
    );
}

#[test]
fn matching_container_ids_matches_by_mark() {
    let mut value = leaf_node_value(10, None, None);
    value["marks"] = serde_json::json!(["dropdown-term"]);
    let tree = node_tree(1, vec![value], vec![]);
    assert_eq!(
        matching_container_ids(&tree, by(WindowMatch::Mark("dropdown-term"))),
        vec![10]
    );
}

#[test]
fn matching_container_ids_recurses_into_nested_containers() {
    let inner = container_node_value(2, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    let tree = node_tree(1, vec![inner], vec![]);
    assert_eq!(
        matching_container_ids(&tree, by(WindowMatch::AppId("foot"))),
        vec![10]
    );
}

// resolve_awaited_matches

#[test]
fn resolve_awaited_matches_keeps_waiting_on_none_and_fails_on_several() {
    assert_eq!(resolve_awaited_matches(vec![], "app_id \"x\""), Ok(None));
    assert_eq!(
        resolve_awaited_matches(vec![7], "app_id \"x\""),
        Ok(Some(7))
    );
    let error = resolve_awaited_matches(vec![7, 8], "app_id \"x\"").unwrap_err();
    assert!(
        error.starts_with("2 windows match app_id \"x\": 7, 8"),
        "{error}"
    );
}

// window_listing

/// root → HEADLESS-1 → workspace "1" (a tiled foot, and a marked split
/// container holding an unmarked Firefox, plus a floating Code) and the
/// scratchpad holding a marked foot.
fn listing_tree() -> Node {
    let mut foot = leaf_node_value(10, Some("foot"), None);
    foot["name"] = serde_json::json!("shell");
    foot["focused"] = serde_json::json!(true);
    let mut split =
        container_node_value(11, vec![leaf_node_value(12, None, Some("Firefox"))], vec![]);
    split["marks"] = serde_json::json!(["browsers"]);
    let mut code = leaf_node_value(13, None, Some("Code"));
    code["type"] = serde_json::json!("floating_con");
    let mut workspace = named_node_value(2, "workspace", "1", vec![foot, split]);
    workspace["floating_nodes"] = serde_json::json!([code]);
    let output = named_node_value(1, "output", "HEADLESS-1", vec![workspace]);

    let mut dropdown = leaf_node_value(20, Some("foot"), None);
    dropdown["type"] = serde_json::json!("floating_con");
    dropdown["marks"] = serde_json::json!(["dropdown"]);
    let mut scratch = named_node_value(4, "workspace", "__i3_scratch", vec![]);
    scratch["floating_nodes"] = serde_json::json!([dropdown]);
    let internal = named_node_value(3, "output", "__i3", vec![scratch]);

    serde_json::from_value(named_node_value(0, "root", "root", vec![internal, output]))
        .expect("valid Node test fixture")
}

#[test]
fn window_listing_lists_windows_and_marked_split_containers_in_tree_order() {
    let ids: Vec<i64> = window_listing(&listing_tree(), WindowCriteria::default())
        .iter()
        .map(|window| window.container_id)
        .collect();
    assert_eq!(ids, vec![20, 10, 11, 12, 13]);
}

#[test]
fn window_listing_describes_each_window() {
    let listing = window_listing(&listing_tree(), WindowCriteria::default());
    assert_eq!(
        listing[1],
        ListedWindow {
            container_id: 10,
            app_id: Some("foot".to_string()),
            class: None,
            title: Some("shell".to_string()),
            marks: vec![],
            workspace: Some("1".to_string()),
            output: Some("HEADLESS-1".to_string()),
            scratchpad: false,
            floating: false,
            focused: true,
            split_container: false,
        }
    );
    assert!(listing[2].split_container);
    assert_eq!(listing[2].marks, vec!["browsers".to_string()]);
    assert!(listing[4].floating);
    assert_eq!(listing[4].class.as_deref(), Some("Code"));
}

#[test]
fn window_listing_reports_a_scratchpad_window_without_a_workspace_or_output() {
    let dropdown = &window_listing(&listing_tree(), WindowCriteria::default())[0];
    assert!(dropdown.scratchpad);
    assert_eq!(dropdown.workspace, None);
    assert_eq!(dropdown.output, None);
}

#[test]
fn window_listing_with_a_filter_lists_what_existing_chooses_between() {
    let ids = |filter| -> Vec<i64> {
        window_listing(&listing_tree(), by(filter))
            .iter()
            .map(|window| window.container_id)
            .collect()
    };
    assert_eq!(ids(WindowMatch::AppId("foot")), vec![20, 10]);
    assert_eq!(ids(WindowMatch::Class("Firefox")), vec![12]);
    assert_eq!(ids(WindowMatch::Mark("browsers")), vec![11]);
    assert_eq!(ids(WindowMatch::AppId("nothing")), Vec::<i64>::new());
}

#[test]
fn a_title_alone_never_matches_a_workspace_output_root_or_the_scratchpad() {
    let pattern = super::super::values::parse_title_pattern("i3|1|HEADLESS|root|shell").unwrap();
    let criteria = WindowCriteria {
        matcher: None,
        title: Some(&pattern),
    };
    let tree = listing_tree();
    let listed: Vec<i64> = window_listing(&tree, criteria)
        .iter()
        .map(|window| window.container_id)
        .collect();
    assert_eq!(matching_container_ids(&tree, criteria), vec![10]);
    assert_eq!(listed, vec![10]);
}

// launch_filter_match / recheck_launched_windows

#[test]
fn launch_filter_match_checks_its_one_filter_and_accepts_anything_without_one() {
    let tree = node_tree(
        1,
        vec![leaf_node_value(10, Some("foot"), Some("Foot"))],
        vec![],
    );
    let node = find_node(&tree, 10).unwrap();
    assert!(launch_filter_match(node, Some(LaunchFilter::AppId("foot"))));
    assert!(!launch_filter_match(
        node,
        Some(LaunchFilter::AppId("other"))
    ));
    assert!(launch_filter_match(node, Some(LaunchFilter::Class("Foot"))));
    assert!(!launch_filter_match(
        node,
        Some(LaunchFilter::Class("Other"))
    ));
    assert!(launch_filter_match(node, None));
}

#[test]
fn recheck_launched_windows_matches_a_class_set_after_mapping() {
    // The window mapped with no class (what its New event carried); the tree
    // now shows the class it set afterwards, with no event to say so.
    let tree = node_tree(1, vec![leaf_node_value(12, None, Some("Spotify"))], vec![]);
    assert_eq!(
        recheck_launched_windows(&tree, &[12], filter("", "Spotify"), None),
        vec![RecheckedWindow {
            container_id: 12,
            app_id: None,
            class: Some("Spotify".to_string()),
            title: None,
            matches: true,
        }]
    );
}

#[test]
fn recheck_launched_windows_reports_a_still_unset_class_as_not_matching() {
    let tree = node_tree(1, vec![leaf_node_value(12, None, None)], vec![]);
    assert_eq!(
        recheck_launched_windows(&tree, &[12], filter("", "Spotify"), None),
        vec![RecheckedWindow {
            container_id: 12,
            app_id: None,
            class: None,
            title: None,
            matches: false,
        }]
    );
}

#[test]
fn recheck_launched_windows_drops_a_window_that_closed_and_keeps_the_order() {
    let tree = node_tree(
        1,
        vec![
            leaf_node_value(12, Some("early"), None),
            leaf_node_value(14, Some("late"), None),
        ],
        vec![],
    );
    let rechecked = recheck_launched_windows(&tree, &[14, 13, 12], filter("late", ""), None);
    assert_eq!(
        rechecked
            .iter()
            .map(|window| (window.container_id, window.matches))
            .collect::<Vec<_>>(),
        vec![(14, true), (12, false)]
    );
}

// resolve_matches

#[test]
fn resolve_matches_errors_on_zero_matches() {
    assert_eq!(
        resolve_matches(vec![], "app_id \"foot\""),
        Err(
            "No existing window matches app_id \"foot\" (--list-windows shows every open window)"
                .to_string()
        )
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
        Err(
            "2 windows match app_id \"foot\": 42, 91 — retarget with --con-id (--list-windows \
             shows them)"
                .to_string()
        )
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
    // Regression test for a CI failure against Sway 1.9: scratchpad_state stays
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
fn container_state_from_tree_reads_the_workspace_s_number() {
    let workspace = named_node_value(
        2,
        "workspace",
        "2: web",
        vec![leaf_node_value(10, None, None)],
    );
    let mut workspace = workspace;
    workspace["num"] = serde_json::json!(2);
    let output = named_node_value(1, "output", "HEADLESS-1", vec![workspace]);
    let tree: Node = serde_json::from_value(named_node_value(0, "root", "root", vec![output]))
        .expect("valid Node test fixture");

    let state = ContainerState::from_tree(&tree, 10).expect("the container is in the tree");
    assert_eq!(state.workspace_number, Some(2));
}

#[test]
fn container_state_from_tree_reports_whether_the_workspace_is_the_visible_one() {
    let visible = ContainerState::from_tree(
        &state_tree("3", leaf_node_value(10, Some("foot"), None)),
        10,
    )
    .expect("the container is in the tree");
    assert!(visible.workspace_visible);

    let hidden_workspace = named_node_value(
        2,
        "workspace",
        "3",
        vec![leaf_node_value(10, Some("foot"), None)],
    );
    let shown_workspace = named_node_value(3, "workspace", "4", vec![]);
    let mut output = named_node_value(
        1,
        "output",
        "HEADLESS-1",
        vec![hidden_workspace, shown_workspace],
    );
    output["focus"] = serde_json::json!([3, 2]);
    let tree: Node = serde_json::from_value(named_node_value(0, "root", "root", vec![output]))
        .expect("valid Node test fixture");
    let hidden = ContainerState::from_tree(&tree, 10).expect("the container is in the tree");
    assert_eq!(hidden.output.as_deref(), Some("HEADLESS-1"));
    assert!(!hidden.workspace_visible);
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

// move_skip_reason

fn workspace_with_a_floating_window(container_id: i64) -> Node {
    let mut floating = leaf_node_value(container_id, Some("foot"), None);
    floating["type"] = serde_json::json!("floating_con");
    let mut value = container_node_value(2, vec![leaf_node_value(9, None, None)], vec![floating]);
    value["type"] = serde_json::json!("workspace");
    value["name"] = serde_json::json!("main");
    value["layout"] = serde_json::json!("splith");
    serde_json::from_value(value).expect("valid Node test fixture")
}

/// `[A, W]` laid out `splith`, so W (10) is a leading sibling the tree walk
/// alone would let through — the fullscreen reason has to come from W itself.
fn leading_window_with_fullscreen_mode(fullscreen_mode: u8) -> Node {
    let mut window = leaf_node_value(10, Some("foot"), None);
    window["fullscreen_mode"] = serde_json::json!(fullscreen_mode);
    workspace_node_tree_with_layout(
        2,
        "main",
        "splith",
        vec![window, leaf_node_value(11, None, None)],
    )
}

#[test]
fn move_skip_reason_flags_a_floating_window_even_with_a_single_output() {
    let workspace = workspace_with_a_floating_window(10);
    for multiple_outputs in [false, true] {
        for direction in [MoveDirection::Right, MoveDirection::Down] {
            assert_eq!(
                move_skip_reason(&workspace, 10, direction, multiple_outputs),
                Some("floating")
            );
        }
    }
}

#[test]
fn move_skip_reason_flags_a_fullscreen_window_with_more_than_one_output() {
    for fullscreen_mode in [1, 2] {
        let workspace = leading_window_with_fullscreen_mode(fullscreen_mode);
        assert_eq!(
            move_skip_reason(&workspace, 10, MoveDirection::Right, true),
            Some("fullscreen")
        );
    }
}

#[test]
fn move_skip_reason_lets_a_fullscreen_window_through_with_a_single_output() {
    let workspace = leading_window_with_fullscreen_mode(1);
    assert_eq!(
        move_skip_reason(&workspace, 10, MoveDirection::Right, false),
        None
    );
}

#[test]
fn move_skip_reason_lets_a_leading_window_that_is_not_fullscreen_through() {
    let workspace = leading_window_with_fullscreen_mode(0);
    assert_eq!(
        move_skip_reason(&workspace, 10, MoveDirection::Right, true),
        None
    );
}

#[test]
fn move_skip_reason_flags_the_trailing_edge_only_with_more_than_one_output() {
    let workspace =
        workspace_node_tree_with_layout(2, "main", "splith", vec![leaf_node_value(10, None, None)]);
    assert_eq!(
        move_skip_reason(&workspace, 10, MoveDirection::Right, true),
        Some("trailing_workspace_edge")
    );
    assert_eq!(
        move_skip_reason(&workspace, 10, MoveDirection::Right, false),
        None
    );
}

#[test]
fn move_skip_reason_is_none_when_the_container_is_not_in_the_tree() {
    let workspace = workspace_with_a_floating_window(10);
    assert_eq!(
        move_skip_reason(&workspace, 999, MoveDirection::Right, true),
        None
    );
}

// is_window

#[test]
fn is_window_is_true_for_a_leaf_and_false_for_a_split_container() {
    let tree = node_tree(1, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    assert!(is_window(find_node(&tree, 10).unwrap()));
    assert!(!is_window(&tree), "a container holding a window is not one");
}

// con_id_target_problem

#[test]
fn con_id_target_problem_accepts_a_window_and_a_split_container() {
    let split = container_node_value(20, vec![leaf_node_value(10, Some("foot"), None)], vec![]);
    let tree = state_tree("1", split);
    assert_eq!(con_id_target_problem(&tree, 10), None);
    assert_eq!(con_id_target_problem(&tree, 20), None);
}

#[test]
fn con_id_target_problem_names_a_missing_id_as_never_a_window() {
    let tree = state_tree("1", leaf_node_value(10, Some("foot"), None));
    assert_eq!(
        con_id_target_problem(&tree, 999999).as_deref(),
        Some("no window with container id 999999")
    );
}

#[test]
fn con_id_target_problem_names_what_a_non_window_id_is() {
    let tree = state_tree("1", leaf_node_value(10, Some("foot"), None));
    assert_eq!(
        con_id_target_problem(&tree, 2).as_deref(),
        Some("container 2 is a workspace, not a window")
    );
    assert_eq!(
        con_id_target_problem(&tree, 1).as_deref(),
        Some("container 1 is an output, not a window")
    );
    assert_eq!(
        con_id_target_problem(&tree, 0).as_deref(),
        Some("container 0 is the root of the tree, not a window")
    );
}

#[test]
fn recheck_launched_windows_needs_the_title_to_match_too() {
    let title = super::super::values::parse_title_pattern("project-a").unwrap();
    let mut value = leaf_node_value(12, Some("code"), None);
    value["name"] = serde_json::json!("Welcome");
    let tree = node_tree(1, vec![value], vec![]);
    let rechecked = recheck_launched_windows(&tree, &[12], filter("code", ""), Some(&title));
    assert_eq!(rechecked[0].title.as_deref(), Some("Welcome"));
    assert!(!rechecked[0].matches);
}

#[test]
fn matching_container_ids_narrows_by_title() {
    let title = super::super::values::parse_title_pattern("right").unwrap();
    let mut left = leaf_node_value(10, Some("twin"), None);
    left["name"] = serde_json::json!("left");
    let mut right = leaf_node_value(11, Some("twin"), None);
    right["name"] = serde_json::json!("right");
    let tree = node_tree(1, vec![left, right], vec![]);
    let criteria = WindowCriteria {
        matcher: Some(WindowMatch::AppId("twin")),
        title: Some(&title),
    };
    assert_eq!(matching_container_ids(&tree, criteria), vec![11]);
}

// launch_placement_problems

/// Workspace "4" (focused, through its empty sibling being unfocused) holding
/// window 10, workspace "8" holding `elsewhere`, and the scratchpad holding
/// `in_scratchpad`, so each placement a config rule can cause is reachable.
fn placement_tree(
    on_build_workspace: Vec<serde_json::Value>,
    floating_on_build_workspace: Vec<serde_json::Value>,
    elsewhere: Vec<serde_json::Value>,
    in_scratchpad: Vec<serde_json::Value>,
) -> Node {
    let mut focused = leaf_node_value(9, Some("terminal"), None);
    focused["focused"] = serde_json::json!(true);
    let mut build_nodes = vec![focused];
    build_nodes.extend(on_build_workspace);
    let mut build = named_node_value(2, "workspace", "4", build_nodes);
    build["floating_nodes"] = serde_json::json!(floating_on_build_workspace);
    let other = named_node_value(3, "workspace", "8", elsewhere);
    let output = named_node_value(1, "output", "HEADLESS-1", vec![build, other]);
    let mut scratchpad = named_node_value(5, "workspace", "__i3_scratch", vec![]);
    scratchpad["floating_nodes"] = serde_json::json!(in_scratchpad);
    let hidden = named_node_value(4, "output", "__i3", vec![scratchpad]);
    serde_json::from_value(named_node_value(0, "root", "root", vec![hidden, output]))
        .expect("valid Node test fixture")
}

fn floating(container_id: i64) -> serde_json::Value {
    let mut value = leaf_node_value(container_id, Some("app"), None);
    value["type"] = serde_json::json!("floating_con");
    value
}

#[test]
fn a_window_opened_tiled_on_the_build_workspace_has_no_placement_problem() {
    let tree = placement_tree(
        vec![leaf_node_value(10, Some("app"), None)],
        vec![],
        vec![],
        vec![],
    );
    assert!(launch_placement_problems(&tree, 10, false, false).is_empty());
}

#[test]
fn a_window_a_rule_sent_to_another_workspace_names_both_workspaces() {
    let tree = placement_tree(
        vec![],
        vec![],
        vec![leaf_node_value(10, Some("app"), None)],
        vec![],
    );
    let problems = launch_placement_problems(&tree, 10, false, false);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("container 10 opened on workspace \"8\", not on workspace \"4\""),
        "{problems:?}"
    );
    assert!(problems[0].contains("assign or for_window"), "{problems:?}");
}

#[test]
fn a_window_a_rule_sent_to_the_scratchpad_says_so_once() {
    let tree = placement_tree(vec![], vec![], vec![], vec![floating(10)]);
    let problems = launch_placement_problems(&tree, 10, false, false);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("container 10 opened in the scratchpad, not on workspace \"4\""),
        "{problems:?}"
    );
}

#[test]
fn a_window_a_rule_floated_is_reported_unless_the_step_floats_it() {
    let tree = placement_tree(vec![], vec![floating(10)], vec![], vec![]);
    let problems = launch_placement_problems(&tree, 10, false, false);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("container 10 opened floating"),
        "{problems:?}"
    );
    assert!(launch_placement_problems(&tree, 10, false, true).is_empty());
}

#[test]
fn a_step_that_moves_its_window_anyway_skips_the_workspace_check_only() {
    let tree = placement_tree(vec![], vec![], vec![floating(10)], vec![]);
    let problems = launch_placement_problems(&tree, 10, true, false);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("opened floating"), "{problems:?}");
    assert!(launch_placement_problems(&tree, 10, true, true).is_empty());
}

#[test]
fn a_window_no_longer_in_the_tree_has_no_placement_problem() {
    let tree = placement_tree(vec![], vec![], vec![], vec![]);
    assert!(launch_placement_problems(&tree, 10, false, false).is_empty());
}
