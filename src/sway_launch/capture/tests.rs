use super::*;
use crate::sway_launch::test_support::{
    container_node_value, leaf_node_value, workspace_node_tree,
};

fn window(id: i64, app_id: &str, percent: f64) -> serde_json::Value {
    let mut value = leaf_node_value(id, Some(app_id), None);
    value["percent"] = serde_json::json!(percent);
    value["name"] = serde_json::json!(format!("{} {}", app_id, id));
    value
}

fn split(id: i64, layout: &str, percent: f64, nodes: Vec<serde_json::Value>) -> serde_json::Value {
    let mut value = container_node_value(id, nodes, vec![]);
    value["layout"] = serde_json::json!(layout);
    value["percent"] = serde_json::json!(percent);
    value
}

fn workspace(
    layout: &str,
    nodes: Vec<serde_json::Value>,
    floating: Vec<serde_json::Value>,
) -> Node {
    let mut value = serde_json::to_value(workspace_node_tree(1, "dev", nodes, floating))
        .expect("a Node serializes");
    value["layout"] = serde_json::json!(layout);
    serde_json::from_value(value).expect("valid workspace fixture")
}

/// The captured file's steps, without comments, as one string per step.
fn steps(captured: &str) -> Vec<String> {
    captured
        .split("[[step]]\n")
        .skip(1)
        .map(|step| {
            step.lines()
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .collect()
}

#[test]
fn capture_builds_each_level_before_going_into_a_nested_split() {
    // splith[ editor 65%, splitv[ top, bottom ] 35% ]: the stack's first
    // window opens beside the editor, and only then is it split and focused.
    let workspace = workspace(
        "splith",
        vec![
            window(10, "code", 0.65),
            split(
                11,
                "splitv",
                0.35,
                vec![window(12, "foot", 0.5), window(13, "foot", 0.5)],
            ),
        ],
        vec![],
    );
    let captured = capture(&workspace).expect("a split workspace captures");
    assert_eq!(
        steps(&captured),
        [
            r#"id = "code"; command = "code"; app_id = "code"; split = "h""#,
            r#"id = "foot"; command = "foot"; app_id = "foot""#,
            r#"target_id = "foot"; split = "v"; focus = true"#,
            r#"id = "foot-2"; command = "foot"; app_id = "foot""#,
            r#"target_id = "foot"; width = "35ppt""#,
        ]
    );
    assert!(captured.contains("# code 10\n[[step]]"), "{captured}");
    assert!(captured.starts_with("# Captured from workspace \"dev\""));
}

#[test]
fn capture_wraps_a_split_that_is_its_parents_first_child() {
    // splitv[ splith[a, b], c ]: a is the first window of both splits, so it
    // is split again once c has opened beside it.
    let workspace = workspace(
        "splitv",
        vec![
            split(
                11,
                "splith",
                0.5,
                vec![window(12, "a", 0.5), window(13, "b", 0.5)],
            ),
            window(14, "c", 0.5),
        ],
        vec![],
    );
    assert_eq!(
        steps(&capture(&workspace).expect("captures")),
        [
            r#"id = "a"; command = "a"; app_id = "a"; split = "v""#,
            r#"id = "c"; command = "c"; app_id = "c""#,
            r#"target_id = "a"; split = "h"; focus = true"#,
            r#"id = "b"; command = "b"; app_id = "b""#,
        ]
    );
}

#[test]
fn capture_sizes_three_children_through_the_first_and_last_only() {
    let workspace = workspace(
        "splith",
        vec![
            window(10, "a", 0.2),
            window(11, "b", 0.3),
            window(12, "c", 0.5),
        ],
        vec![],
    );
    let captured = capture(&workspace).expect("captures");
    assert_eq!(
        steps(&captured)[3..],
        [
            r#"target_id = "a"; width = "20ppt""#,
            r#"target_id = "c"; width = "50ppt""#,
        ]
    );
    assert!(
        captured.contains("is sized exactly on Sway 1.9 to 1.11 and approximately on newer ones"),
        "{captured}"
    );
}

#[test]
fn capture_sizes_a_split_with_equal_outer_children_through_the_middle_one() {
    // Every Sway version splits the middle one's change evenly between two
    // equal sides, so this is exact on all of them.
    let workspace = workspace(
        "splith",
        vec![
            window(10, "a", 0.25),
            window(11, "b", 0.5),
            window(12, "c", 0.25),
        ],
        vec![],
    );
    let captured = capture(&workspace).expect("captures");
    assert_eq!(
        steps(&captured)[3..],
        [r#"target_id = "b"; width = "50ppt""#]
    );
    assert!(!captured.contains("# Note"), "{captured}");
}

#[test]
fn capture_notes_that_four_uneven_children_are_approximate() {
    let workspace = workspace(
        "splitv",
        vec![
            window(10, "a", 0.1),
            window(11, "b", 0.2),
            window(12, "c", 0.3),
            window(13, "d", 0.4),
        ],
        vec![],
    );
    let captured = capture(&workspace).expect("captures");
    assert!(
        captured.contains("# Note: the 4-window v split holding a is sized approximately"),
        "{captured}"
    );
    assert_eq!(
        steps(&captured)[4..],
        [
            r#"target_id = "a"; height = "10ppt""#,
            r#"target_id = "b"; height = "20ppt""#,
            r#"target_id = "c"; height = "30ppt""#,
        ]
    );
}

#[test]
fn capture_leaves_even_splits_unsized() {
    let workspace = workspace(
        "splith",
        vec![
            window(10, "a", 1.0 / 3.0),
            window(11, "b", 1.0 / 3.0),
            window(12, "c", 1.0 / 3.0),
        ],
        vec![],
    );
    assert_eq!(steps(&capture(&workspace).expect("captures")).len(), 3);
}

#[test]
fn capture_keeps_floating_windows_geometry_marks_and_state() {
    let mut floating = leaf_node_value(20, None, Some("Code"));
    floating["type"] = serde_json::json!("floating_con");
    floating["rect"] = serde_json::json!({"x": 150, "y": 230, "width": 800, "height": 570});
    floating["deco_rect"] = serde_json::json!({"x": 100, "y": 200, "width": 800, "height": 30});
    floating["marks"] = serde_json::json!(["editor", "main"]);
    floating["sticky"] = serde_json::json!(true);
    let workspace = workspace("splith", vec![window(10, "foot", 1.0)], vec![floating]);
    let captured = capture(&workspace).expect("captures");
    assert_eq!(
        steps(&captured),
        [
            r#"id = "foot"; command = "foot"; app_id = "foot""#,
            r#"id = "Code"; command = "code"; class = "Code"; mark = "editor"; floating = true; sticky = true; width = "800px"; height = "600px"; position = "100,200""#,
        ]
    );
    assert!(
        captured.contains("# Note: Code also has the marks main; a step sets only one."),
        "{captured}"
    );
}

#[test]
fn capture_refuses_a_tabbed_container_and_an_empty_workspace() {
    let tabbed = workspace(
        "splith",
        vec![
            window(10, "a", 0.5),
            split(
                11,
                "tabbed",
                0.5,
                vec![window(12, "b", 0.5), window(13, "c", 0.5)],
            ),
        ],
        vec![],
    );
    let error = capture(&tabbed).expect_err("tabbed can't be rebuilt");
    assert_eq!(
        error,
        "workspace \"dev\": it holds a tabbed container, and only splith/splitv layouts can be \
         rebuilt"
    );
    let empty = workspace("splith", vec![], vec![]);
    assert_eq!(
        capture(&empty).expect_err("nothing to capture"),
        "workspace \"dev\": it has no windows to capture"
    );
}

#[test]
fn capture_refuses_a_window_with_nothing_to_name_a_command_after() {
    let workspace = workspace("splith", vec![leaf_node_value(10, None, None)], vec![]);
    assert_eq!(
        capture(&workspace).expect_err("no identity"),
        "workspace \"dev\": window 10 has no app_id or class to guess a command from"
    );
}

#[test]
fn capture_output_parses_as_a_layout_file() {
    let workspace = workspace(
        "splith",
        vec![
            window(10, "say \"hi\"", 0.7),
            split(
                11,
                "splitv",
                0.3,
                vec![window(12, "foot", 0.5), window(13, "foot", 0.5)],
            ),
        ],
        vec![],
    );
    let captured = capture(&workspace).expect("captures");
    let layout = crate::layout::parse(&captured).expect("the capture parses");
    assert_eq!(layout.step.len(), 5);
}
