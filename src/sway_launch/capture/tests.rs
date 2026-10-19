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
            r#"id = "Code"; command = "code"; class = "Code"; mark = "editor"; floating = true; sticky = true; width = "800px"; height = "600px"; position = "100,200"; settle = 500"#,
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

#[test]
fn capture_writes_a_command_guessed_from_a_hostile_app_id_as_one_quoted_word() {
    // foot takes its app_id from an escape sequence anything printed in the
    // terminal can send, so the guess can't be trusted to be a program name.
    let hostile = "foot && touch /tmp/example-file";
    let workspace = workspace("splith", vec![window(10, hostile, 1.0)], vec![]);
    let captured = capture(&workspace).expect("captures");
    assert_eq!(
        steps(&captured),
        [format!(
            r#"id = "{hostile}"; command = ["{hostile}"]; app_id = "{hostile}""#
        )]
    );
    assert!(
        captured.contains(&format!(
            "# Note: {hostile}'s app_id isn't a plain program name, so its command is written \
             as one quoted word; check it before running this.\n"
        )),
        "{captured}"
    );
    let layout = crate::layout::parse(&captured).expect("the capture parses");
    assert_eq!(
        layout.step[0].command.as_deref(),
        Some("'foot && touch /tmp/example-file'")
    );
}

#[test]
fn plain_program_names_are_letters_digits_and_a_few_punctuation_marks() {
    for plain in ["foot", "org.gnome.Nautilus", "code-oss", "g++", "app_2"] {
        assert!(is_plain_program_name(plain), "{plain}");
    }
    for unsafe_name in ["", "-x", "a b", "a;b", "a&&b", "$(x)", "a'b", "a/b"] {
        assert!(!is_plain_program_name(unsafe_name), "{unsafe_name}");
    }
}

#[test]
fn capture_makes_a_fullscreen_window_fullscreen_last_and_ignores_its_reported_percent() {
    // one | two | [three / four], even, with one fullscreen: Sway reports
    // one's percent as 1.0, which isn't its share of the split.
    let mut fullscreen = window(10, "one", 1.0);
    fullscreen["fullscreen_mode"] = serde_json::json!(1);
    let workspace = workspace(
        "splith",
        vec![
            fullscreen,
            window(11, "two", 1.0 / 3.0),
            split(
                12,
                "splitv",
                1.0 / 3.0,
                vec![window(13, "three", 0.5), window(14, "four", 0.5)],
            ),
        ],
        vec![],
    );
    let captured = capture(&workspace).expect("captures");
    assert_eq!(
        steps(&captured),
        [
            r#"id = "one"; command = "one"; app_id = "one"; split = "h""#,
            r#"id = "two"; command = "two"; app_id = "two""#,
            r#"id = "three"; command = "three"; app_id = "three""#,
            r#"target_id = "three"; split = "v"; focus = true"#,
            r#"id = "four"; command = "four"; app_id = "four""#,
            r#"target_id = "one"; fullscreen = true"#,
        ]
    );
    assert!(!captured.contains("# Note"), "{captured}");
}

#[test]
fn capture_gives_a_fullscreen_window_what_its_siblings_leave() {
    let mut fullscreen = window(11, "b", 1.0);
    fullscreen["fullscreen_mode"] = serde_json::json!(1);
    let workspace = workspace("splith", vec![window(10, "a", 0.3), fullscreen], vec![]);
    assert_eq!(
        steps(&capture(&workspace).expect("captures"))[2..],
        [
            r#"target_id = "b"; width = "70ppt""#,
            r#"target_id = "b"; fullscreen = true"#,
        ]
    );
}

#[test]
fn capture_makes_a_floating_fullscreen_window_fullscreen_after_its_geometry() {
    let mut floating = leaf_node_value(20, Some("foot"), None);
    floating["type"] = serde_json::json!("floating_con");
    floating["fullscreen_mode"] = serde_json::json!(1);
    floating["rect"] = serde_json::json!({"x": 0, "y": 0, "width": 400, "height": 300});
    let workspace = workspace("splith", vec![], vec![floating]);
    let captured = steps(&capture(&workspace).expect("captures"));
    assert_eq!(captured.len(), 2);
    assert!(!captured[0].contains("fullscreen"), "{captured:?}");
    assert_eq!(captured[1], r#"target_id = "foot"; fullscreen = true"#);
}

/// A split container Sway floated as a whole: a `floating_con` whose
/// children are ordinary `con` windows.
fn floating_container(nodes: Vec<serde_json::Value>) -> serde_json::Value {
    let mut container = split(30, "splith", 1.0, nodes);
    container["type"] = serde_json::json!("floating_con");
    container["rect"] = serde_json::json!({"x": 150, "y": 230, "width": 800, "height": 570});
    container["deco_rect"] = serde_json::json!({"x": 100, "y": 200, "width": 800, "height": 30});
    container["sticky"] = serde_json::json!(true);
    container
}

#[test]
fn capture_refuses_a_floating_container_of_several_windows() {
    let workspace = workspace(
        "splith",
        vec![window(10, "foot", 1.0)],
        vec![floating_container(vec![
            window(31, "a", 0.5),
            window(32, "b", 0.5),
        ])],
    );
    assert_eq!(
        capture(&workspace).expect_err("a floating split can't be rebuilt"),
        "workspace \"dev\": it holds a floating container of 2 windows, and only a floating \
         window on its own can be rebuilt"
    );
}

#[test]
fn capture_takes_a_floating_containers_geometry_for_the_one_window_in_it() {
    let workspace = workspace(
        "splith",
        vec![],
        vec![floating_container(vec![window(31, "a", 1.0)])],
    );
    assert_eq!(
        steps(&capture(&workspace).expect("captures")),
        [
            r#"id = "a"; command = "a"; app_id = "a"; floating = true; sticky = true; width = "800px"; height = "600px"; position = "100,200"; settle = 500"#,
        ]
    );
}
