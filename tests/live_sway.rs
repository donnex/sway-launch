// Exercises the compiled `sway-launch` binary against a real, running Sway
// compositor (typically a headless one — see scripts/run-live-sway-tests),
// asserting on actual window state read back via `swayipc::Connection`
// rather than just exit codes. Gated behind the `live-sway-tests` feature so
// a normal `cargo test` (and the main CI job) never needs Sway installed.
//
// All tests here share one compositor's tree and focused workspace, so they
// must run single-threaded (`cargo test ... -- --test-threads=1`), which
// scripts/run-live-sway-tests already does. Each test cleans up the windows
// it launches via `KillOnDrop`, so a failing assertion still leaves the tree
// clean for the next test.
#![cfg(feature = "live-sway-tests")]

use std::process::Command;
use std::time::{Duration, Instant};
use swayipc::{Connection, Node, NodeType};

fn sway_launch_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sway-launch"))
}

fn connect() -> Connection {
    Connection::new().unwrap_or_else(|error| {
        panic!(
            "live-sway-tests requires a reachable Sway IPC socket \
             (SWAYSOCK/WAYLAND_DISPLAY) — run via scripts/run-live-sway-tests \
             rather than `cargo test` directly: {error}"
        )
    })
}

fn find_node(node: &Node, con_id: i64) -> Option<&Node> {
    if node.id == con_id {
        return Some(node);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| find_node(child, con_id))
}

fn get_node(connection: &mut Connection, con_id: i64) -> Node {
    let tree = connection.get_tree().expect("get_tree should succeed");
    find_node(&tree, con_id)
        .unwrap_or_else(|| panic!("container id {con_id} not found in the tree"))
        .clone()
}

fn workspace_containing<'a>(
    node: &'a Node,
    con_id: i64,
    current: Option<&'a str>,
) -> Option<&'a str> {
    let current = if node.node_type == NodeType::Workspace {
        node.name.as_deref()
    } else {
        current
    };
    if node.id == con_id {
        return current;
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| workspace_containing(child, con_id, current))
}

fn output_containing<'a>(node: &'a Node, con_id: i64, current: Option<&'a str>) -> Option<&'a str> {
    let current = if node.node_type == NodeType::Output {
        node.name.as_deref()
    } else {
        current
    };
    if node.id == con_id {
        return current;
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| output_containing(child, con_id, current))
}

/// Kills its container id when dropped, via a fresh IPC connection, so a
/// test's windows never leak into the next one — even on assertion panic.
struct KillOnDrop(i64);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(mut connection) = Connection::new() {
            let _ = connection.run_command(format!("[con_id={}] kill", self.0));
        }
    }
}

/// A layout file fixture written to the OS temp directory, removed again
/// when it goes out of scope (even if an assertion panics mid-test) —
/// mirrors tests/layout.rs's TempToml.
struct TempToml(std::path::PathBuf);

impl TempToml {
    fn write(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sway-launch-live-test-{}.toml", name));
        std::fs::write(&path, contents).expect("failed to write temp layout file");
        Self(path)
    }
}

impl std::ops::Deref for TempToml {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempToml {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn launch_foot(extra_args: &[&str]) -> (i64, KillOnDrop) {
    // A higher --wait-time than the 20ms CLI default: the headless/pixman
    // compositor these tests run against is otherwise flaky on multi-action
    // invocations (e.g. --floating followed by --position), occasionally
    // querying tree state before the second wait-time-based action's Sway
    // command has actually taken effect. Later args win on conflict, so a
    // test can still override this via extra_args if it needs to.
    let mut args = vec!["--app-id", "foot", "--timeout", "10", "--wait-time", "100"];
    args.extend_from_slice(extra_args);
    args.push("foot");

    let output = sway_launch_command()
        .args(&args)
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let container_id: i64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("stdout should be a container id, got {:?}", stdout));
    (container_id, KillOnDrop(container_id))
}

#[test]
fn exec_matches_by_app_id_and_returns_its_container_id() {
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let node = get_node(&mut connection, container_id);
    assert_eq!(node.app_id.as_deref(), Some("foot"));
}

#[test]
fn floating_with_width_and_height_applies_all_three() {
    let mut connection = connect();
    let (container_id, _guard) =
        launch_foot(&["--floating", "--width", "400px", "--height", "300px"]);

    let node = get_node(&mut connection, container_id);
    assert!(
        matches!(
            node.floating,
            Some(swayipc::Floating::UserOn) | Some(swayipc::Floating::AutoOn)
        ),
        "expected the window to be floating, got {:?}",
        node.floating
    );
    // "resize set height" targets the decoration-inclusive frame, same as
    // "move position" (see position_moves_a_floating_window_to_given_coordinates)
    // — rect.height alone is 25px short of the requested value with the
    // default border style, since it excludes the title bar.
    assert_eq!(node.rect.width, 400);
    assert_eq!(node.rect.height + node.deco_rect.height, 300);
}

#[test]
fn mark_applies_the_given_mark() {
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--mark", "live-sway-test-mark"]);

    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&"live-sway-test-mark".to_string()));
}

#[test]
fn workspace_moves_window_to_named_workspace() {
    // Proves the WindowChange::Move assumption documented for --workspace:
    // if Move didn't fire reliably, this would hang until --timeout instead
    // of returning promptly with the window actually moved.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--workspace", "live-sway-test-workspace"]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    let workspace = workspace_containing(&tree, container_id, None);
    assert_eq!(workspace, Some("live-sway-test-workspace"));
}

#[test]
fn output_moves_window_to_named_output() {
    // Proves the WindowChange::Move assumption documented for --output:
    // unlike NewColumn/NewRow's "move right"/"move down", "move container
    // to output" has no "already there" no-op case (a window is always in
    // exactly one output's tree), confirmed by this actually completing
    // promptly with the window moved rather than hanging until --timeout.
    let mut connection = connect();
    let outputs_before: Vec<String> = connection
        .get_outputs()
        .expect("get_outputs should succeed")
        .into_iter()
        .map(|output| output.name)
        .collect();
    connection
        .run_command("create_output")
        .expect("create_output should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("create_output should succeed");
    let new_output = connection
        .get_outputs()
        .expect("get_outputs should succeed")
        .into_iter()
        .map(|output| output.name)
        .find(|name| !outputs_before.contains(name))
        .expect("create_output should have added a new output");

    let (container_id, _guard) = launch_foot(&["--output", &new_output]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    let output = output_containing(&tree, container_id, None);
    assert_eq!(output, Some(new_output.as_str()));
}

#[test]
fn fullscreen_enables_fullscreen_mode() {
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--fullscreen"]);

    let node = get_node(&mut connection, container_id);
    assert_ne!(node.fullscreen_mode, Some(0));
}

#[test]
fn focus_focuses_a_previously_unfocused_window() {
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&[]);
    // Launching a second window steals focus from the first, giving this
    // test an actual unfocused-to-focused transition to confirm — proves
    // WindowChange::Focus fires for real, rather than the --focus command
    // being a no-op on an already-focused window.
    let (_second_id, _second_guard) = launch_foot(&[]);
    assert!(!get_node(&mut connection, first_id).focused);

    let output = sway_launch_command()
        .args(["--con-id", &first_id.to_string(), "--focus"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --focus failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(get_node(&mut connection, first_id).focused);
}

#[test]
fn position_moves_a_floating_window_to_given_coordinates() {
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating", "--position", "100,200"]);

    // "move position" targets the decoration-inclusive frame (deco_rect),
    // not the content rect below the title bar (rect.y sits 25px lower than
    // the requested y with the default border style) — confirmed against
    // live Sway.
    let node = get_node(&mut connection, container_id);
    assert_eq!(node.deco_rect.x, 100);
    assert_eq!(node.deco_rect.y, 200);
}

#[test]
fn con_id_and_existing_target_an_already_open_window() {
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--mark",
            "live-sway-test-con-id",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(output.status.success());
    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&"live-sway-test-con-id".to_string()));

    let output = sway_launch_command()
        .args([
            "--existing",
            "--app-id",
            "foot",
            "--mark",
            "live-sway-test-existing",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&"live-sway-test-existing".to_string()));
}

#[test]
fn layout_target_id_references_an_earlier_steps_real_window() {
    let mut connection = connect();
    let path = TempToml::write(
        "target-id",
        "[[step]]\nid = \"first\"\napp_id = \"foot\"\ncommand = \"foot\"\n\n\
         [[step]]\ntarget_id = \"first\"\nmark = \"live-sway-test-target-id\"\n",
    );

    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --layout failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let container_id: i64 = stdout
        .lines()
        .next()
        .expect("stdout should have a first line")
        .parse()
        .expect("first line should be a container id");
    let _guard = KillOnDrop(container_id);

    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&"live-sway-test-target-id".to_string()));
}

#[test]
fn template_apps_resolve_to_real_windows() {
    let mut connection = connect();
    let path = TempToml::write(
        "template",
        "[[step]]\nslot = \"first\"\n\n\
         [[step]]\nslot = \"second\"\n\n\
         [[step]]\ntarget_id = \"first\"\nmark = \"live-sway-test-template\"\n",
    );

    let output = sway_launch_command()
        .args(["--template", path.to_str().unwrap(), "--apps", "foot,foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --template failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let mut lines = stdout.lines();
    let first_id: i64 = lines
        .next()
        .expect("stdout should have a first line")
        .parse()
        .expect("first line should be a container id");
    let second_id: i64 = lines
        .next()
        .expect("stdout should have a second line")
        .parse()
        .expect("second line should be a container id");
    let _first_guard = KillOnDrop(first_id);
    let _second_guard = KillOnDrop(second_id);

    let first_node = get_node(&mut connection, first_id);
    assert_eq!(first_node.app_id.as_deref(), Some("foot"));
    assert!(first_node
        .marks
        .contains(&"live-sway-test-template".to_string()));
    let second_node = get_node(&mut connection, second_id);
    assert_eq!(second_node.app_id.as_deref(), Some("foot"));
}

#[test]
fn new_column_and_new_row_complete_promptly_when_already_at_the_edge() {
    // Regression test for the bug this crate's own README/CLAUDE.md
    // describe: "move right"/"move down" don't fire WindowChange::Move when
    // the window is already at the tree's rightmost/bottommost position —
    // the ordinary two-window case — so the old event-confirmed dispatch
    // hung until --timeout (5s default) instead of completing. Asserting
    // "well under the default --timeout" is the proxy for "did not hang
    // waiting on an event that never fires".
    let (_first_id, _first_guard) = launch_foot(&[]);
    let (second_id, _second_guard) = launch_foot(&[]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &second_id.to_string(),
            "--new-column",
            "--wait-time",
            "50",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--new-column took {:?}, suggesting it hung waiting for an event",
        started.elapsed()
    );

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &second_id.to_string(),
            "--new-row",
            "--wait-time",
            "50",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-row failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--new-row took {:?}, suggesting it hung waiting for an event",
        started.elapsed()
    );
}
