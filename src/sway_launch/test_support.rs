//! Node/event fixtures shared by more than one module's unit tests.
//!
//! `swayipc`'s `Node` and `WindowEvent` have far more required fields than any
//! single test cares about, so they're built by deserializing a minimal JSON
//! value rather than by hand. Kept here, rather than duplicated per module,
//! so every module's tests agree on what a "plain tiled window" looks like.

use swayipc::{Node, WindowEvent};

pub(super) fn window_event(
    change: &str,
    container_id: i64,
    app_id: Option<&str>,
    class: Option<&str>,
) -> WindowEvent {
    let value = serde_json::json!({
        "change": change,
        "container": {
            "id": container_id,
            "type": "con",
            "border": "normal",
            "current_border_width": 0,
            "layout": "none",
            "orientation": "none",
            "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
            "urgent": false,
            "focused": false,
            "focus": [],
            "floating_nodes": [],
            "sticky": false,
            "app_id": app_id,
            "window_properties": class.map(|class| serde_json::json!({"class": class})),
        }
    });

    serde_json::from_value(value).expect("valid WindowEvent test fixture")
}

pub(super) fn leaf_node_value(
    container_id: i64,
    app_id: Option<&str>,
    class: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": container_id,
        "type": "con",
        "border": "normal",
        "current_border_width": 0,
        "layout": "none",
        "orientation": "none",
        "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
        "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
        "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
        "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
        "urgent": false,
        "focused": false,
        "focus": [],
        "floating_nodes": [],
        "sticky": false,
        "app_id": app_id,
        "window_properties": class.map(|class| serde_json::json!({"class": class})),
    })
}

pub(super) fn container_node_value(
    container_id: i64,
    nodes: Vec<serde_json::Value>,
    floating_nodes: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "id": container_id,
        "type": "con",
        "border": "normal",
        "current_border_width": 0,
        "layout": "none",
        "orientation": "none",
        "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
        "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
        "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
        "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
        "urgent": false,
        "focused": false,
        "focus": [],
        "nodes": nodes,
        "floating_nodes": floating_nodes,
        "sticky": false,
    })
}

pub(super) fn node_tree(
    container_id: i64,
    nodes: Vec<serde_json::Value>,
    floating_nodes: Vec<serde_json::Value>,
) -> Node {
    serde_json::from_value(container_node_value(container_id, nodes, floating_nodes))
        .expect("valid Node test fixture")
}

pub(super) fn workspace_node_tree(
    container_id: i64,
    name: &str,
    nodes: Vec<serde_json::Value>,
    floating_nodes: Vec<serde_json::Value>,
) -> Node {
    let mut value = container_node_value(container_id, nodes, floating_nodes);
    value["type"] = serde_json::json!("workspace");
    value["name"] = serde_json::json!(name);
    serde_json::from_value(value).expect("valid Node test fixture")
}

pub(super) fn named_node_value(
    container_id: i64,
    kind: &str,
    name: &str,
    nodes: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut value = container_node_value(container_id, nodes, vec![]);
    value["type"] = serde_json::json!(kind);
    value["name"] = serde_json::json!(name);
    value
}

pub(super) fn state_tree(workspace_name: &str, leaf: serde_json::Value) -> Node {
    let workspace = named_node_value(2, "workspace", workspace_name, vec![leaf]);
    let output = named_node_value(1, "output", "HEADLESS-1", vec![workspace]);
    serde_json::from_value(named_node_value(0, "root", "root", vec![output]))
        .expect("valid Node test fixture")
}

pub(super) fn rect(x: i32, y: i32, width: i32, height: i32) -> swayipc::Rect {
    serde_json::from_value(serde_json::json!({"x": x, "y": y, "width": width, "height": height}))
        .expect("valid Rect test fixture")
}
