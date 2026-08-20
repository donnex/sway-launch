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

use std::io::BufRead;
use std::process::Command;
use std::sync::mpsc;
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

/// The `layout` of `con_id`'s direct parent node — mirrors
/// `sway_launch.rs`'s private `find_parent_layout()`, which
/// `SwayAction::poll_matches()` uses to confirm a `Split` action, since a
/// window's own `layout` field never carries its split direction (see that
/// function's doc comment).
fn parent_layout(node: &Node, con_id: i64) -> Option<swayipc::NodeLayout> {
    if node
        .nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .any(|child| child.id == con_id)
    {
        return Some(node.layout);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| parent_layout(child, con_id))
}

fn get_node(connection: &mut Connection, con_id: i64) -> Node {
    let tree = connection.get_tree().expect("get_tree should succeed");
    find_node(&tree, con_id)
        .unwrap_or_else(|| panic!("container id {con_id} not found in the tree"))
        .clone()
}

/// Mirrors `sway_launch.rs`'s private `node_is_floating()`. Sway 1.9 (still
/// what `apt` installs on the `ubuntu-latest` CI runner) never populates a
/// floating container's own `floating` field — confirmed live against a
/// headless compositor, `floating enable` correctly changes `node_type` to
/// `FloatingCon` but leaves `floating` at `None` — while Sway 1.11
/// populates both, so `node_type` is the version-portable check.
fn node_is_floating(node: &Node) -> bool {
    node.node_type == NodeType::FloatingCon
        || matches!(
            node.floating,
            Some(swayipc::Floating::UserOn) | Some(swayipc::Floating::AutoOn)
        )
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

/// Kills and reaps a long-running child process (e.g. `--debug-events`,
/// which runs until killed) when dropped, even on assertion panic.
struct KillChildOnDrop(std::process::Child);

impl Drop for KillChildOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
        node_is_floating(&node),
        "expected the window to be floating, got node_type {:?}, floating {:?}",
        node.node_type,
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
    // moving to a *different* output reliably fires the event, confirmed by
    // this actually completing promptly with the window moved rather than
    // hanging until --timeout. Moving to the output the window is already
    // on is a separate, already-there no-op case — see
    // output_is_a_no_op_when_already_on_the_target_output below.
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
fn workspace_is_a_no_op_when_already_on_the_target_workspace() {
    // Regression test: Sway doesn't fire WindowChange::Move for a "move
    // workspace" that doesn't actually change anything, so without
    // SwayAction::already_at_target()'s short-circuit this would hang until
    // --timeout instead of completing immediately.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);
    let tree = connection.get_tree().expect("get_tree should succeed");
    let workspace = workspace_containing(&tree, container_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--workspace",
            &workspace,
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --workspace failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--workspace to the already-current workspace took {:?}, suggesting it \
         hung waiting for an event Sway doesn't fire for a no-op move",
        started.elapsed()
    );
}

#[test]
fn output_is_a_no_op_when_already_on_the_target_output() {
    // Same as workspace_is_a_no_op_when_already_on_the_target_workspace,
    // for --output.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);
    let tree = connection.get_tree().expect("get_tree should succeed");
    let output_name = output_containing(&tree, container_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--output",
            &output_name,
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --output failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--output to the already-current output took {:?}, suggesting it hung \
         waiting for an event Sway doesn't fire for a no-op move",
        started.elapsed()
    );
}

#[test]
fn floating_is_a_no_op_when_already_floating() {
    // Regression test found during a bug hunt prompted by this crate's own
    // pattern: Workspace/Output/NewColumn/NewRow all turned out to have a
    // no-op case Sway doesn't fire an event for, so the same was checked
    // for Floating/Fullscreen/Focus too — confirmed live (manual testing
    // during development: re-running --floating on an already-floating
    // window hung the full 5s --timeout and then errored, before
    // SwayAction::already_at_target()'s Floating arm was added). Without
    // that short-circuit, this would fail this assertion or the process
    // would exit non-zero.
    let (container_id, _guard) = launch_foot(&["--floating"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--floating",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --floating (already floating) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--floating on an already-floating window took {:?}, suggesting it hung \
         waiting for an event Sway doesn't fire for a no-op",
        started.elapsed()
    );
}

#[test]
fn fullscreen_is_a_no_op_when_already_fullscreen() {
    // Same as floating_is_a_no_op_when_already_floating, for --fullscreen.
    let (container_id, _guard) = launch_foot(&["--fullscreen"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--fullscreen",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --fullscreen (already fullscreen) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--fullscreen on an already-fullscreen window took {:?}, suggesting it hung \
         waiting for an event Sway doesn't fire for a no-op",
        started.elapsed()
    );
}

#[test]
fn focus_is_a_no_op_when_already_focused() {
    // Same as floating_is_a_no_op_when_already_floating, for --focus.
    // Explicitly focuses first rather than relying on a freshly-launched
    // window already being focused by default, to guarantee the
    // "already there" state regardless of ambient focus left behind by
    // other tests in this shared compositor.
    let (container_id, _guard) = launch_foot(&["--focus"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--focus",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --focus (already focused) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--focus on an already-focused window took {:?}, suggesting it hung \
         waiting for an event Sway doesn't fire for a no-op",
        started.elapsed()
    );
}

#[test]
fn new_column_does_not_relocate_a_solo_window_to_a_different_output() {
    // Regression test for the multi-monitor NewColumn/NewRow escalation
    // documented in SwayAction::matching_window_change_events()'s reasoning
    // comment: "move right" on a window with no sibling to move past within
    // its workspace can otherwise relocate the whole workspace to the next
    // output in that direction, rather than a same-workspace no-op.
    // SwayLaunch::run() guards against this via relocates_to_another_output();
    // this proves the guard actually prevents the relocation.
    let mut connection = connect();
    connection
        .run_command("create_output")
        .expect("create_output should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("create_output should succeed");

    let (container_id, _guard) = launch_foot(&[]);
    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let output_before = output_containing(&tree_before, container_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let output = sway_launch_command()
        .args(["--con-id", &container_id.to_string(), "--new-column"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree_after = connection.get_tree().expect("get_tree should succeed");
    let output_after = output_containing(&tree_after, container_id, None);
    assert_eq!(
        output_after,
        Some(output_before.as_str()),
        "a solo window's --new-column should not relocate it to a different output"
    );
}

#[test]
fn new_column_does_not_relocate_a_non_solo_window_at_the_trailing_edge() {
    // Regression test for a gap found in relocates_to_another_output()
    // while developing docs/plan-poll-based-wait-time-actions.md's
    // NewColumn/NewRow poll matcher: the original guard only checked "is
    // container_id the *only* window in its workspace" — manual swaymsg
    // probing against a live multi-output compositor found that a
    // *non-solo* workspace can escalate too, whenever container_id is
    // already the trailing child of a workspace already laid out along
    // that axis (two windows side by side, "move right" on the rightmost
    // relocated it to a different output despite having a sibling to its
    // left). is_at_the_trailing_workspace_edge() closes this by checking
    // the workspace's own layout plus trailing-child position, not just
    // child count.
    let mut connection = connect();
    connection
        .run_command("create_output")
        .expect("create_output should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("create_output should succeed");

    let (_first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-non-solo-edge"]);
    let (second_id, _second_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-non-solo-edge"]);

    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let output_before = output_containing(&tree_before, second_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let output = sway_launch_command()
        .args(["--con-id", &second_id.to_string(), "--new-column"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree_after = connection.get_tree().expect("get_tree should succeed");
    let output_after = output_containing(&tree_after, second_id, None);
    assert_eq!(
        output_after,
        Some(output_before.as_str()),
        "the trailing window of a non-solo workspace should not be relocated to a different \
         output by --new-column either"
    );
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
fn position_confirms_via_poll_for_a_floating_window() {
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism reaching Position: confirms via
    // SwayAction::poll_matches()'s position_matches() (a get_tree() +
    // get_outputs() poll for deco_rect.x/deco_rect.y) rather than always
    // sleeping the full --wait-time after the command too.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--position",
            "300,400",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --position failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--position took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let node = get_node(&mut connection, container_id);
    assert_eq!(node.deco_rect.x, 300);
    assert_eq!(node.deco_rect.y, 400);
}

#[test]
fn position_confirms_via_poll_for_a_fullscreen_window() {
    // Regression test: confirmed live that a fullscreen window's deco_rect
    // stays {0, 0, 0, 0} permanently (stable across a multi-second sweep,
    // not a transient race), since Sway never computes decoration geometry
    // for a window with no border/titlebar to draw. position_matches()
    // comparing only deco_rect.x/y meant --position against a fullscreen
    // container could never be confirmed via poll -- move position actually
    // succeeds immediately (rect.x/y land on the requested target right
    // away), but every invocation still burned the full poll grace period
    // before falling back to sleeping --wait-time. Falling back to
    // rect.x/y when deco_rect is unset fixes this, mirroring
    // width_matches()'s existing dual-formula tolerance.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating", "--fullscreen"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--position",
            "100,100",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --position failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--position took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let node = get_node(&mut connection, container_id);
    assert_eq!(
        node.deco_rect.width, 0,
        "expected deco_rect to stay unset while fullscreen"
    );
    assert_eq!(
        node.deco_rect.height, 0,
        "expected deco_rect to stay unset while fullscreen"
    );
    assert_eq!(node.rect.x, 100);
    assert_eq!(node.rect.y, 100);
}

#[test]
fn position_errors_clearly_for_a_tiled_window() {
    // Corrects an assumption docs/plan-poll-based-wait-time-actions.md
    // carried over from earlier conversation exploration: a tiled window
    // isn't a silent "move position" no-op the way Height/Width's
    // solo-window clamp is — Sway rejects the command outright ("Only
    // floating containers can be moved to an absolute position"), which
    // run_sway_command()'s `?` propagates as an error *before*
    // poll_matches()/poll_baseline() are ever reached. So there's no
    // "poll can never confirm this, fall back gracefully" case to test
    // here after all — this instead confirms that existing, pre-poll
    // error path is unaffected by this feature: it still fails fast and
    // clearly, not silently, and not by hanging through the poll grace
    // period first.
    let (container_id, _guard) = launch_foot(&[]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--position",
            "300,400",
            "--wait-time",
            "300",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        !output.status.success(),
        "sway-launch --position on a tiled window should fail, not silently succeed"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--position on a tiled window took {:?}, suggesting it hung instead of erroring \
         immediately",
        started.elapsed()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Only floating containers can be moved to an absolute position"),
        "unexpected stderr: {stderr:?}"
    );
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

#[test]
fn new_column_confirms_via_poll_when_swapping_past_a_sibling() {
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism reaching NewColumn: confirms via
    // SwayAction::poll_matches()'s rect-snapshot comparison (see
    // poll_baseline()'s doc comment) rather than always sleeping the full
    // --wait-time after the command too. Needs a third window so the first
    // one has a real sibling to swap past, unlike the ordinary two-window
    // edge case in new_column_and_new_row_complete_promptly_when_already_at_the_edge.
    // Isolated via --workspace (needed on every launch, not just the
    // first — --workspace doesn't switch the current focused workspace,
    // confirmed during this feature's development) so this test's "just
    // these three windows" assumption holds regardless of ambient state
    // left behind by other tests in this shared compositor.
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&["--workspace", "live-sway-test-new-column-swap"]);
    let (_second_id, _second_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-swap"]);
    let (_third_id, _third_guard) = launch_foot(&["--workspace", "live-sway-test-new-column-swap"]);

    let before = get_node(&mut connection, first_id).rect;

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &first_id.to_string(),
            "--new-column",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--new-column took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let after = get_node(&mut connection, first_id).rect;
    assert_ne!(
        before, after,
        "the leftmost window should have swapped position with its sibling"
    );
}

#[test]
fn new_row_confirms_via_poll_when_swapping_past_a_sibling() {
    // Same as new_column_confirms_via_poll_when_swapping_past_a_sibling,
    // for NewRow — needs a vertically-arranged sibling to swap past, same
    // as new_row_places_window_below_the_first. Isolated via --workspace
    // for the same reason as that test.
    let mut connection = connect();
    let (first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-row-swap", "--split", "v"]);
    let (_second_id, _second_guard) = launch_foot(&["--workspace", "live-sway-test-new-row-swap"]);
    let (_third_id, _third_guard) = launch_foot(&["--workspace", "live-sway-test-new-row-swap"]);

    let before = get_node(&mut connection, first_id).rect;

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &first_id.to_string(),
            "--new-row",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-row failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--new-row took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let after = get_node(&mut connection, first_id).rect;
    assert_ne!(
        before, after,
        "the topmost window should have swapped position with its sibling"
    );
}

#[test]
fn new_column_falls_back_gracefully_at_the_edge_with_a_large_wait_time() {
    // The documented "already at the edge" no-op (see
    // matching_window_change_events()'s comment in src/sway_launch.rs) —
    // confirmed during this feature's development that Sway can still
    // incidentally restructure *other* siblings in this case (wrapping one
    // in a new split container) even though this window's own rect never
    // changes, which is exactly why poll_matches() compares the target
    // window's own rect rather than its parent's children list. Uses a
    // --wait-time much larger than
    // new_column_and_new_row_complete_promptly_when_already_at_the_edge's
    // 50ms, so a false-positive "confirmed" would be caught by the rect
    // assertion below even if the timing alone looked fine. Isolated via
    // --workspace so this window is genuinely at the tree's edge,
    // regardless of ambient state left behind by other tests in this
    // shared compositor — this is exactly the kind of test where ambient
    // extra windows/outputs (e.g. from tests that call create_output)
    // would otherwise produce a misleading rect delta unrelated to
    // --new-column itself, confirmed while chasing down this test's own
    // first failed run during development.
    let mut connection = connect();
    let (_first_id, _first_guard) = launch_foot(&["--workspace", "live-sway-test-new-column-edge"]);
    let (second_id, _second_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-edge"]);
    // See quad_terminals_layout_launches_four_windows_in_a_grid's comment —
    // the second window's geometry (specifically its decoration) can still
    // be settling briefly right after launch_foot's own process exits, so
    // capturing "before" too early can itself look like a spurious change.
    std::thread::sleep(Duration::from_millis(300));

    let before = get_node(&mut connection, second_id).rect;

    let output = sway_launch_command()
        .args([
            "--con-id",
            &second_id.to_string(),
            "--new-column",
            "--wait-time",
            "300",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column (at the edge) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = get_node(&mut connection, second_id).rect;
    assert_eq!(
        before, after,
        "the rightmost window's own rect should be unaffected by a no-op move"
    );
}

#[test]
fn concurrent_exec_invocations_do_not_collide_on_the_same_container_id() {
    // Regression test for the critical concurrent-invocation bug this
    // crate's README warns about: two sway-launch processes launching
    // matching windows at the same time used to be able to both match the
    // same New event and return the identical container id, silently
    // orphaning the other process's real window.
    // run_wait_matching_exec_event()'s PID-marker correlation is what
    // closes this for the common case (an app that isn't single-instance),
    // and this test is exactly that case. Inherently timing-based, but
    // reliable in practice — 0 collisions across 90 manual trials during
    // development, versus every trial colliding before the fix.
    let child_a = sway_launch_command()
        .args(["--app-id", "foot", "--timeout", "10", "foot"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch (process A)");
    let child_b = sway_launch_command()
        .args(["--app-id", "foot", "--timeout", "10", "foot"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch (process B)");

    let output_a = child_a.wait_with_output().expect("process A should exit");
    let output_b = child_b.wait_with_output().expect("process B should exit");

    assert!(
        output_a.status.success(),
        "process A failed: {}",
        String::from_utf8_lossy(&output_a.stderr)
    );
    assert!(
        output_b.status.success(),
        "process B failed: {}",
        String::from_utf8_lossy(&output_b.stderr)
    );

    let id_a: i64 = String::from_utf8_lossy(&output_a.stdout)
        .trim()
        .parse()
        .expect("process A's stdout should be a container id");
    let id_b: i64 = String::from_utf8_lossy(&output_b.stdout)
        .trim()
        .parse()
        .expect("process B's stdout should be a container id");
    let _guard_a = KillOnDrop(id_a);
    let _guard_b = KillOnDrop(id_b);

    assert_ne!(
        id_a, id_b,
        "two concurrent invocations returned the same container id — the other \
         process's real window was silently orphaned"
    );
}

#[test]
fn exec_falls_back_to_a_content_match_when_its_own_process_already_exited() {
    // Regression test for the other half of run_wait_matching_exec_event():
    // some applications (browsers, editors) are single-instance and forward
    // a second invocation's request to an already-running process before
    // exiting, so the window that eventually appears is legitimately the
    // right one, but owned by a PID that was never given sway-launch's PID
    // marker. Simulated deterministically here — no need for a real
    // single-instance app — via a delayed, unmarked "foot" launched
    // directly (standing in for the pre-existing instance's new window)
    // while sway-launch's own command is `true`, which exits immediately
    // without ever creating a window, exactly like a forwarding process
    // would.
    let mut connection = connect();
    connection
        .run_command("exec sh -c 'sleep 1; exec foot'")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let started = Instant::now();
    let output = sway_launch_command()
        .args(["--app-id", "foot", "--timeout", "10", "true"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let container_id: i64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("stdout should be a container id");
    let _guard = KillOnDrop(container_id);

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?} — should have fallen back once its own process (already \
         exited, since `true` exits immediately) was found not running, well \
         before --timeout",
        started.elapsed()
    );

    let node = get_node(&mut connection, container_id);
    assert_eq!(node.app_id.as_deref(), Some("foot"));
}

#[test]
fn wait_time_action_errors_clearly_when_its_container_already_closed() {
    // Regression test: on Sway 1.9, a [con_id=N] criteria matching zero
    // containers is treated as success, not an error, so a wait-time action
    // (Split/NewColumn/NewRow/Height/Width/Position) used to silently no-op
    // instead of erroring if the container closed between an earlier action
    // resolving it and this one running. run_wait_time()'s container_exists()
    // check is what's under test here — still required on 1.9, though
    // confirmed live to be redundant on Sway 1.11, which already errors
    // clearly ("No matching node.") on its own; see container_exists()'s doc
    // comment for that version split.
    let mut connection = connect();
    let (container_id, guard) = launch_foot(&[]);
    connection
        .run_command(format!("[con_id={container_id}] kill"))
        .expect("kill should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("kill should succeed");
    std::mem::forget(guard); // already closed — nothing left for KillOnDrop to clean up

    let output = sway_launch_command()
        .args(["--con-id", &container_id.to_string(), "--split", "h"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "--split against an already-closed container should fail, not silently no-op"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&container_id.to_string()),
        "error should name the container id: {stderr:?}"
    );
}

/// The repo-root-relative directory a shipped example file lives under,
/// resolved via CARGO_MANIFEST_DIR rather than a relative path, so these
/// tests work regardless of the working directory `cargo test` is invoked
/// from.
fn examples_dir(subdir: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(subdir)
}

/// `templates/` lives at the repo root, not under `examples/`, since its
/// contents are embedded into the binary as built-ins rather than being
/// purely illustrative — see "Built-in templates" in CLAUDE.md.
fn templates_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")
}

#[test]
fn every_shipped_template_resolves_and_launches_successfully() {
    // Unlike template_apps_resolve_to_real_windows above (a hand-written,
    // minimal template), this drives the actual files under
    // templates/ — nothing else in the test suite would catch a
    // shipped template that's silently broken. dual-output.toml is excluded
    // here since it needs a second output and real (non-placeholder) output
    // names to run at all; see dual_output_template_moves_windows_to_separate_outputs.
    let mut connection = connect();
    let mut paths: Vec<_> = std::fs::read_dir(templates_dir())
        .expect("templates/ should be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("dual-output.toml"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 18,
        "expected at least 18 non-dual-output template files, found {}: {:?}",
        paths.len(),
        paths
    );

    for path in paths {
        connection
            .run_command("[app_id=foot] kill")
            .expect("kill should succeed");

        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"));
        let slot_count = contents
            .lines()
            .filter(|line| line.starts_with("slot = "))
            .count();
        assert!(slot_count > 0, "{path:?} should declare at least one slot");
        let apps = vec!["foot"; slot_count].join(",");
        // Total step count (slot steps + target_id steps, e.g.
        // retarget-by-slot.toml's retarget step) is how many container id
        // lines --apps actually prints — --apps only sizes the *slot* count.
        let step_count = contents.matches("[[step]]").count();

        let output = sway_launch_command()
            .args([
                "--template",
                path.to_str().unwrap(),
                "--apps",
                &apps,
                "--timeout",
                "10",
            ])
            .output()
            .unwrap_or_else(|error| panic!("failed to run sway-launch for {path:?}: {error}"));
        assert!(
            output.status.success(),
            "{path:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let ids: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            ids.len(),
            step_count,
            "{path:?} should print one container id per step, got {stdout:?}"
        );
        for id in ids {
            id.parse::<i64>()
                .unwrap_or_else(|_| panic!("{path:?} printed a non-id line: {id:?}"));
        }
    }

    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
}

#[test]
fn builtin_template_name_resolves_and_launches_without_a_toml_extension() {
    // --template <name> (no .toml extension, no path) is a separate
    // dispatch path from --template <file>.toml (main.rs's
    // resolve_template_contents()), driving the same embedded content
    // every_shipped_template_resolves_and_launches_successfully above
    // drives from disk — proves the bare-name lookup finds the real
    // embedded quad-grid template and genuinely launches real windows via
    // it, not just that resolve_template_contents() parses successfully.
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");

    let output = sway_launch_command()
        .args([
            "--template",
            "quad-grid",
            "--apps",
            "foot,foot,foot,foot",
            "--timeout",
            "10",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ids: Vec<i64> = stdout
        .lines()
        .map(|line| line.parse().expect("each line should be a container id"))
        .collect();
    assert_eq!(ids.len(), 4, "quad-grid has 4 slots");

    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
}

fn count_app_id_windows(node: &Node, app_id: &str) -> usize {
    let mut count = usize::from(node.app_id.as_deref() == Some(app_id));
    for child in node.nodes.iter().chain(node.floating_nodes.iter()) {
        count += count_app_id_windows(child, app_id);
    }
    count
}

#[test]
fn every_basic_example_script_launches_successfully() {
    // Drives the actual shipped shell scripts under examples/scripts/,
    // which use only foot — CLAUDE.md's live-Sway coverage rule names "an
    // example script" alongside --layout/--template files, which already
    // got this treatment; scripts never did until this test. Each script invokes
    // `sway-launch` by bare name (relying on PATH, since that's how a user
    // actually runs these), so a temporary directory holding a copy of the
    // compiled test binary under that exact name is prepended to PATH for
    // the duration of each script run. The five "advanced" scripts
    // (browser-comparison, dev-workspace, editor-with-floating-terminal,
    // floating-file-manager, quad-mixed-apps) need Firefox/Chromium/Thunar/
    // VS Code, none of which are installed in this project's
    // live-sway-tests CI job — scoped out here rather than silently
    // covering none of the 11.
    let mut connection = connect();

    let bin_dir = std::env::temp_dir().join("sway-launch-live-test-script-bin");
    std::fs::create_dir_all(&bin_dir).expect("failed to create temp PATH dir for scripts");
    let fake_bin_path = bin_dir.join("sway-launch");
    std::fs::copy(env!("CARGO_BIN_EXE_sway-launch"), &fake_bin_path)
        .expect("failed to copy the compiled binary into the temp PATH dir");
    let mut permissions = std::fs::metadata(&fake_bin_path)
        .expect("copied binary should exist")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&fake_bin_path, permissions)
        .expect("failed to chmod +x the copied binary");
    let path_with_fake_bin_first = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let scripts_with_expected_window_counts = [
        ("dual-terminals", 2),
        ("triple-row", 3),
        ("column-split", 2),
        ("quad-terminals", 4),
        ("workspace-and-position", 1),
        ("retarget-floating", 1),
    ];

    for (name, expected_window_count) in scripts_with_expected_window_counts {
        connection
            .run_command("[app_id=foot] kill")
            .expect("kill should succeed");

        let path = examples_dir("scripts").join(name);

        let output = Command::new(&path)
            .env("PATH", &path_with_fake_bin_first)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {name}: {error}"));
        assert!(
            output.status.success(),
            "examples/scripts/{name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        std::thread::sleep(Duration::from_millis(300));
        let tree = connection.get_tree().expect("get_tree should succeed");
        let window_count = count_app_id_windows(&tree, "foot");
        assert_eq!(
            window_count, expected_window_count,
            "examples/scripts/{name} should have launched {expected_window_count} foot \
             window(s), found {window_count}"
        );
    }

    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
}

#[test]
fn dual_output_template_moves_windows_to_separate_outputs() {
    // dual-output.toml ships with HDMI-A-1/DP-1 as placeholder output names
    // (documented in the file's own header comment) — a real user swaps
    // those for their own setup's names, so this test does the equivalent:
    // create a second headless output and substitute its real name in.
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
    let first_output = outputs_before
        .first()
        .expect("at least one output should already exist")
        .clone();

    let contents = std::fs::read_to_string(templates_dir().join("dual-output.toml"))
        .expect("dual-output.toml should be readable");
    let contents = contents
        .replace("HDMI-A-1", &first_output)
        .replace("DP-1", &new_output);
    let path = TempToml::write("dual-output", &contents);

    let output = sway_launch_command()
        .args([
            "--template",
            path.to_str().unwrap(),
            "--apps",
            "foot,foot",
            "--timeout",
            "10",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "dual-output.toml failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
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

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree, first_id, None),
        Some(first_output.as_str())
    );
    assert_eq!(
        output_containing(&tree, second_id, None),
        Some(new_output.as_str())
    );
}

#[test]
fn quad_terminals_layout_launches_four_windows_in_a_grid() {
    // Drives the actual examples/layouts/quad-terminals.toml file directly,
    // rather than a hand-written stand-in.
    //
    // Unlike every other test in this file, this one asserts on *relative*
    // multi-window grid geometry rather than a single freshly launched
    // window's own state, so it switches to a workspace of its own rather
    // than relying on killing leftover windows in the shared one. It also
    // sleeps briefly after the layout command completes, before querying
    // tree state: under this suite's cumulative load (several earlier tests
    // create extra outputs via create_output, never removed), the last
    // window's surface can still be settling into its final geometry for a
    // short while after sway-launch's own process has already exited —
    // --wait-time alone (which only sleeps *between* sway-launch's own
    // actions) isn't enough to cover that.
    let mut connection = connect();
    connection
        .run_command("workspace live-sway-test-quad-terminals")
        .expect("workspace switch should succeed");

    let path = examples_dir("layouts").join("quad-terminals.toml");

    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--wait-time", "100"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "quad-terminals.toml failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(300));

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let ids: Vec<i64> = stdout
        .lines()
        .map(|line| line.parse().expect("each line should be a container id"))
        .collect();
    assert_eq!(ids.len(), 4, "should launch exactly 4 windows");
    let guards: Vec<_> = ids.iter().map(|&id| KillOnDrop(id)).collect();

    let tree = connection.get_tree().expect("get_tree should succeed");
    let rects: Vec<_> = ids
        .iter()
        .map(|&id| {
            find_node(&tree, id)
                .expect("container should be in the tree")
                .rect
        })
        .collect();
    // A 2x2 grid should have exactly 2 distinct x positions and 2 distinct
    // y positions among the four windows.
    let mut xs: Vec<i32> = rects.iter().map(|rect| rect.x).collect();
    let mut ys: Vec<i32> = rects.iter().map(|rect| rect.y).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    assert_eq!(xs.len(), 2, "expected 2 distinct columns, got {:?}", xs);
    assert_eq!(ys.len(), 2, "expected 2 distinct rows, got {:?}", ys);

    drop(guards);
}

#[test]
fn retarget_by_id_layout_floats_the_first_step_by_name() {
    // Drives the actual examples/layouts/retarget-by-id.toml file, proving
    // the shipped file (not just the concept, already covered by
    // layout_target_id_references_an_earlier_steps_real_window's inline
    // fixture) resolves an earlier step's id and applies floating+size to
    // the right window.
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");

    let path = examples_dir("layouts").join("retarget-by-id.toml");

    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--wait-time", "100"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "retarget-by-id.toml failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let ids: Vec<i64> = stdout
        .lines()
        .map(|line| line.parse().expect("each line should be a container id"))
        .collect();
    assert_eq!(
        ids.len(),
        3,
        "should print one id per step, including the retarget step"
    );
    assert_eq!(
        ids[0], ids[2],
        "the retarget step's id should be the first step's container id"
    );
    let guards: Vec<_> = [ids[0], ids[1]].iter().map(|&id| KillOnDrop(id)).collect();

    let node = get_node(&mut connection, ids[0]);
    assert!(
        node_is_floating(&node),
        "the first step's window should end up floating, got node_type {:?}, floating {:?}",
        node.node_type,
        node.floating
    );
    // Mirrors src/sway_launch.rs's width_matches(): a window that had
    // already been tiled with a sibling for a while before being floated
    // and resized can come out short by 2*current_border_width on
    // rect.width alone (confirmed live on Sway 1.11) — but confirmed live
    // on Sway 1.9 (still what `apt` installs on the `ubuntu-latest` CI
    // runner) that the exact rect.width match applies instead in this same
    // scenario, so — same as width_matches() itself — accept either rather
    // than hardcoding one specific formula.
    assert!(
        node.rect.width == 800 || node.rect.width + 2 * node.current_border_width == 800,
        "expected rect.width 800 (or {} short of it accounting for the border), got {}",
        2 * node.current_border_width,
        node.rect.width
    );

    drop(guards);
}

#[test]
fn split_h_places_windows_side_by_side() {
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&["--split", "h"]);
    let (second_id, _second_guard) = launch_foot(&[]);
    // See quad_terminals_layout_launches_four_windows_in_a_grid's comment —
    // the last window's geometry can still be settling briefly after
    // sway-launch's own process exits.
    std::thread::sleep(Duration::from_millis(300));

    let tree = connection.get_tree().expect("get_tree should succeed");
    let first_rect = find_node(&tree, first_id)
        .expect("first window should be in the tree")
        .rect;
    let second_rect = find_node(&tree, second_id)
        .expect("second window should be in the tree")
        .rect;

    assert_eq!(
        first_rect.y, second_rect.y,
        "side-by-side windows should share the same y"
    );
    assert_ne!(
        first_rect.x, second_rect.x,
        "side-by-side windows should have different x"
    );
}

#[test]
fn split_v_stacks_windows() {
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&["--split", "v"]);
    let (second_id, _second_guard) = launch_foot(&[]);
    std::thread::sleep(Duration::from_millis(300));

    let tree = connection.get_tree().expect("get_tree should succeed");
    let first_rect = find_node(&tree, first_id)
        .expect("first window should be in the tree")
        .rect;
    let second_rect = find_node(&tree, second_id)
        .expect("second window should be in the tree")
        .rect;

    assert_eq!(
        first_rect.x, second_rect.x,
        "stacked windows should share the same x"
    );
    assert_ne!(
        first_rect.y, second_rect.y,
        "stacked windows should have different y"
    );
}

#[test]
fn split_confirms_via_poll_well_under_a_large_wait_time() {
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism: Split now confirms via
    // SwayAction::poll_matches() (a get_tree() poll of the container's
    // *parent* layout field — see parent_node_layout()'s doc comment for
    // why it's the parent, not the container's own node) rather than always
    // sleeping the full --wait-time after the command too. run_wait_time()
    // still sleeps the full --wait-time *before* sending the command
    // unconditionally (unrelated to this feature), so the total can never
    // go below that — a --wait-time well above the poll grace period, with
    // an assertion comfortably under 2 * --wait-time, is what makes "did it
    // actually confirm via poll rather than falling back to a second full
    // sleep" observable. --split v (the workspace's non-default direction,
    // confirmed via split_v_stacks_windows/manual testing) forces a real
    // parent-layout change, unlike --split h against a freshly-launched
    // window, whose workspace is already splith by default.
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&[]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &first_id.to_string(),
            "--split",
            "v",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --split v failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--split v took {:?} against a 2000ms --wait-time (fallback would take ~4000ms: \
         --wait-time before the command, plus --wait-time again after), suggesting it fell \
         back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        parent_layout(&tree, first_id),
        Some(swayipc::NodeLayout::SplitV),
        "the window's parent (its workspace, since it's solo) should now be splitv"
    );
}

#[test]
fn split_is_idempotent_and_still_confirms_promptly_when_already_set() {
    // Split has no genuine no-op ambiguity (unlike Height/Width's
    // solo-window clamp or NewColumn/NewRow's edge-of-tree case, per
    // docs/plan-poll-based-wait-time-actions.md): re-applying the split
    // direction a container's parent already has still matches on the very
    // first poll, so this should complete just as promptly as the
    // fresh-change case above rather than needing the grace-period
    // fallback. --split h here matches the workspace's own default layout
    // even before launch_foot's own --split h runs, so this is confirmed
    // true from the very first poll of the very first command.
    let (first_id, _first_guard) = launch_foot(&["--split", "h"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &first_id.to_string(),
            "--split",
            "h",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --split h (already splith) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "re-applying an already-set split took {:?} against a 2000ms --wait-time (fallback \
         would take ~4000ms), suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );
}

#[test]
fn new_row_places_window_below_the_first() {
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&[]);
    let (second_id, _second_guard) = launch_foot(&["--new-row"]);
    std::thread::sleep(Duration::from_millis(300));

    let tree = connection.get_tree().expect("get_tree should succeed");
    let first_rect = find_node(&tree, first_id)
        .expect("first window should be in the tree")
        .rect;
    let second_rect = find_node(&tree, second_id)
        .expect("second window should be in the tree")
        .rect;

    assert_eq!(
        first_rect.x, second_rect.x,
        "new-row should keep the same column"
    );
    assert!(
        second_rect.y > first_rect.y,
        "new-row window ({:?}) should be below the first ({:?})",
        second_rect,
        first_rect
    );
}

#[test]
fn height_alone_resizes_a_non_solo_window() {
    // Resizing a window is a no-op while it's still the workspace's only
    // occupant (see templates/master-dual-stack.toml's header
    // comment) — this needs a sibling present for --height to take effect
    // at all, which also makes it a live check that --height works on its
    // own, not just combined with --floating/--width like
    // floating_with_width_and_height_applies_all_three.
    let mut connection = connect();
    let (_first_id, _first_guard) = launch_foot(&["--split", "v"]);
    let (second_id, _second_guard) = launch_foot(&["--height", "200px"]);
    std::thread::sleep(Duration::from_millis(300));

    let node = get_node(&mut connection, second_id);
    // "resize set height" targets the decoration-inclusive frame, same as
    // floating_with_width_and_height_applies_all_three.
    assert_eq!(node.rect.height + node.deco_rect.height, 200);
}

#[test]
fn height_confirms_via_poll_when_resized_with_a_sibling() {
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism reaching Height: confirms via
    // SwayAction::poll_matches()'s height_matches() (a get_tree() poll for
    // the decoration-inclusive rect.height + deco_rect.height) rather than
    // always sleeping the full --wait-time after the command too. Needs a
    // vertically-arranged sibling for the resize to take effect at all —
    // see height_alone_resizes_a_non_solo_window. Both windows use an
    // explicit --workspace (rather than whatever's currently focused) so
    // this test's "just these two windows" assumption holds regardless of
    // ambient state left behind by other tests in this shared compositor
    // (e.g. accumulated outputs from tests that call create_output) —
    // confirmed during this feature's development that --workspace doesn't
    // switch the *current* focused workspace, so every launch in an
    // isolated test needs it, not just the first.
    let mut connection = connect();
    let (_first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-height-poll", "--split", "v"]);
    let (second_id, _second_guard) = launch_foot(&["--workspace", "live-sway-test-height-poll"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &second_id.to_string(),
            "--height",
            "200px",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --height failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--height took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let node = get_node(&mut connection, second_id);
    assert_eq!(node.rect.height + node.deco_rect.height, 200);
}

#[test]
fn width_confirms_via_poll_when_resized_with_a_sibling() {
    // Same as height_confirms_via_poll_when_resized_with_a_sibling, for
    // Width — a freshly-tiled window's width_matches() formula is an exact
    // rect.width match (no border adjustment), confirmed live earlier in
    // this suite by plain swaymsg probing during development. Isolated via
    // --workspace for the same reason as that test.
    let mut connection = connect();
    let (_first_id, _first_guard) = launch_foot(&["--workspace", "live-sway-test-width-poll"]);
    let (second_id, _second_guard) = launch_foot(&["--workspace", "live-sway-test-width-poll"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &second_id.to_string(),
            "--width",
            "300px",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --width failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--width took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let node = get_node(&mut connection, second_id);
    assert_eq!(node.rect.width, 300);
}

#[test]
fn height_and_width_fall_back_gracefully_when_solo_window_clamps_the_resize() {
    // Resizing a window that's the sole occupant of its workspace is
    // silently clamped by Sway (see templates/master-dual-stack.toml's
    // header comment) — height_matches()/width_matches() can never confirm
    // this, so the grace period must elapse and fall back to the original
    // wait-time behavior (succeed, don't hang or error) rather than the
    // poll turning a legitimate no-op into an indefinite wait. Isolated via
    // --workspace so this window is genuinely solo, regardless of ambient
    // state left behind by other tests in this shared compositor.
    let mut connection = connect();
    let (first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-height-width-clamp"]);

    let height_output = sway_launch_command()
        .args([
            "--con-id",
            &first_id.to_string(),
            "--height",
            "200px",
            "--wait-time",
            "300",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        height_output.status.success(),
        "sway-launch --height (solo window) failed: {}",
        String::from_utf8_lossy(&height_output.stderr)
    );

    let width_output = sway_launch_command()
        .args([
            "--con-id",
            &first_id.to_string(),
            "--width",
            "200px",
            "--wait-time",
            "300",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        width_output.status.success(),
        "sway-launch --width (solo window) failed: {}",
        String::from_utf8_lossy(&width_output.stderr)
    );

    let node = get_node(&mut connection, first_id);
    assert_ne!(
        node.rect.height + node.deco_rect.height,
        200,
        "a solo window's height should stay clamped to 100%, not the requested value"
    );
    assert_ne!(
        node.rect.width, 200,
        "a solo window's width should stay clamped to 100%, not the requested value"
    );
}

#[test]
fn fallback_path_stays_bounded_at_the_default_wait_time() {
    // Regression test found during a code-review bug hunt: WAIT_TIME_POLL_GRACE
    // (200ms) is 10x the CLI's own --wait-time default (20ms), so before
    // run_poll_then_fallback() capped its grace period at
    // WAIT_TIME_POLL_GRACE.min(wait_time), any fallback case at default
    // settings (confirmed live: this exact scenario) took ~220ms instead of
    // the ~40ms (2 * wait_time) it cost before polling existed at all —
    // this asserts that regression stays fixed. Omits --wait-time entirely
    // to exercise the actual CLI default, not just a manually-chosen small
    // value.
    let mut connection = connect();
    let (first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-fallback-default-wait-time"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args(["--con-id", &first_id.to_string(), "--height", "200px"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --height (solo window, default --wait-time) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "--height on a solo window at the default --wait-time took {:?} — the \
         uncapped grace period regression took ~220ms, so this bound catches it \
         reappearing without being so tight it flakes on ordinary system load",
        started.elapsed()
    );

    let node = get_node(&mut connection, first_id);
    assert_ne!(
        node.rect.height + node.deco_rect.height,
        200,
        "a solo window's height should stay clamped to 100%, confirming this actually \
         exercised the fallback path rather than a lucky fast-path match"
    );
}

#[test]
fn position_center_centers_a_floating_window() {
    // Determines which output the window actually landed on (rather than
    // assuming get_outputs()'s first entry, which isn't necessarily where
    // the current workspace lives — several earlier tests in this suite
    // call create_output, and cumulative extra outputs can shift which one
    // a new workspace defaults to) and centers against that output's own
    // dimensions.
    let mut connection = connect();

    let (container_id, _guard) = launch_foot(&[
        "--floating",
        "--width",
        "400px",
        "--height",
        "300px",
        "--position",
        "center",
    ]);
    std::thread::sleep(Duration::from_millis(300));

    let tree = connection.get_tree().expect("get_tree should succeed");
    let output_name = output_containing(&tree, container_id, None)
        .expect("window should be found in the tree")
        .to_string();
    let outputs = connection
        .get_outputs()
        .expect("get_outputs should succeed");
    let output = outputs
        .iter()
        .find(|output| output.name == output_name)
        .expect("the window's own output should be in get_outputs()");
    // rect/deco_rect coordinates are global, not output-relative, so the
    // expected center must account for the output's own position too — only
    // matters once a second output exists to its left/above, but do it
    // unconditionally rather than assuming this is the primary output.
    let expected_x = output.rect.x + (output.rect.width - 400) / 2;
    let expected_y = output.rect.y + (output.rect.height - 300) / 2;

    let node = get_node(&mut connection, container_id);
    assert_eq!(node.rect.width, 400);
    assert_eq!(node.rect.x, expected_x);
    // deco_rect.y (not rect.y) is the decoration-inclusive top edge "move
    // position"/centering actually targets — see
    // position_moves_a_floating_window_to_given_coordinates.
    assert_eq!(node.deco_rect.y, expected_y);
}

#[test]
fn json_output_for_a_real_exec_is_a_clean_container_id_object() {
    // con_id_json_output_is_a_clean_json_object in tests/json_output.rs
    // covers --json's formatting for --con-id, which never touches the
    // socket; this covers the same formatting for a real exec+match.
    let output = sway_launch_command()
        .args(["--app-id", "foot", "--timeout", "10", "--json", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let trimmed = stdout.trim();
    let container_id: i64 = trimmed
        .strip_prefix("{\"container_id\":")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("unexpected --json output: {trimmed:?}"))
        .parse()
        .unwrap_or_else(|_| panic!("--json container_id should be an integer: {trimmed:?}"));
    let _guard = KillOnDrop(container_id);
}

#[test]
fn existing_errors_with_zero_matches() {
    let output = sway_launch_command()
        .args([
            "--existing",
            "--app-id",
            "live-sway-test-nonexistent-app-id",
            "--floating",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No existing window matches"),
        "unexpected stderr: {stderr:?}"
    );
}

#[test]
fn existing_errors_listing_ids_with_multiple_matches() {
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");

    let (first_id, _first_guard) = launch_foot(&[]);
    let (second_id, _second_guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args(["--existing", "--app-id", "foot", "--floating"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&first_id.to_string()) && stderr.contains(&second_id.to_string()),
        "error should list both matching ids: {stderr:?}"
    );
}

#[test]
fn mark_with_special_characters_is_stored_literally_not_executed() {
    // Live version of quote_sway_string's unit tests (sway_launch.rs):
    // proves the injection-style mark actually ends up stored as one
    // literal mark against a real compositor, rather than being split into
    // multiple Sway commands.
    let mut connection = connect();
    let malicious_mark =
        "live-sway-test-injection, exec touch /tmp/sway-launch-live-test-pwned; echo bar";
    let _ = std::fs::remove_file("/tmp/sway-launch-live-test-pwned");

    let (container_id, _guard) = launch_foot(&["--mark", malicious_mark]);

    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&malicious_mark.to_string()));
    assert!(
        !std::path::Path::new("/tmp/sway-launch-live-test-pwned").exists(),
        "mark should be stored literally, not executed as a separate command"
    );
}

#[test]
fn verbose_prints_real_diagnostics_to_stderr_for_a_live_action() {
    // con_id_verbose_diagnostics_go_to_stderr_not_stdout in
    // tests/json_output.rs covers --verbose's stream separation via
    // --con-id, which never touches the socket — this covers the same
    // separation for a real exec+match against a live compositor, and
    // checks the diagnostic content itself, not just which stream it's on.
    let output = sway_launch_command()
        .args(["--app-id", "foot", "--timeout", "10", "--verbose", "foot"])
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
    let _guard = KillOnDrop(container_id);

    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    assert!(
        stderr.contains("Sway action: Exec"),
        "stderr should describe the action: {stderr:?}"
    );
    assert!(
        stderr.contains("Sway command: exec"),
        "stderr should show the actual Sway command: {stderr:?}"
    );
    assert!(
        stderr.contains("Event match:"),
        "stderr should confirm the matched event: {stderr:?}"
    );
    assert!(
        stderr.contains(&format!("Target container id: {container_id}")),
        "stderr should name the same container id printed to stdout: {stderr:?}"
    );
}

#[test]
fn bindings_file_resolves_existing_and_command_slots_to_real_windows() {
    // tests/template.rs exercises --bindings headlessly with con_id-only
    // bindings; this drives the same flag against real windows, covering
    // both a Binding's `existing = true` and `command` forms (mirroring
    // README.md's own Templates example) in one template.
    let mut connection = connect();
    let (existing_id, _existing_guard) = launch_foot(&[]);

    let template_path = TempToml::write(
        "bindings-template",
        "[[step]]\nslot = \"existing_window\"\nmark = \"live-sway-test-bindings-existing\"\n\n\
         [[step]]\nslot = \"primary\"\nmark = \"live-sway-test-bindings-primary\"\n",
    );
    let bindings_path = TempToml::write(
        "bindings-bindings",
        "[[binding]]\nslot = \"existing_window\"\nexisting = true\napp_id = \"foot\"\n\n\
         [[binding]]\nslot = \"primary\"\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--template",
            template_path.to_str().unwrap(),
            "--bindings",
            bindings_path.to_str().unwrap(),
            "--wait-time",
            "100",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --template --bindings failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let ids: Vec<i64> = stdout
        .lines()
        .map(|line| line.parse().expect("each line should be a container id"))
        .collect();
    assert_eq!(ids.len(), 2, "should print one id per step");
    assert_eq!(
        ids[0], existing_id,
        "the existing_window slot should resolve to the already-open window"
    );
    let _primary_guard = KillOnDrop(ids[1]);

    let existing_node = get_node(&mut connection, ids[0]);
    assert!(existing_node
        .marks
        .contains(&"live-sway-test-bindings-existing".to_string()));
    let primary_node = get_node(&mut connection, ids[1]);
    assert_eq!(primary_node.app_id.as_deref(), Some("foot"));
    assert!(primary_node
        .marks
        .contains(&"live-sway-test-bindings-primary".to_string()));
}

#[test]
fn debug_events_prints_a_real_window_event() {
    // --debug-events runs until killed, so it doesn't fit this file's usual
    // spawn-and-wait-for-exit pattern — spawns it as a background child with
    // its stdout read on a separate thread and forwarded through a channel,
    // the same shape sway_launch.rs's own event loop uses internally.
    let mut child = sway_launch_command()
        .arg("--debug-events")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch --debug-events");
    let stdout = child.stdout.take().expect("child should have piped stdout");
    let _child_guard = KillChildOnDrop(child);

    let (line_sender, line_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if line_sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Give the event subscription time to establish before triggering the
    // window it needs to see — otherwise the New event could fire before
    // --debug-events is actually subscribed.
    std::thread::sleep(Duration::from_millis(300));
    let (_container_id, _guard) = launch_foot(&[]);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found = false;
    while !found {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match line_receiver.recv_timeout(remaining) {
            Ok(line) => {
                found = line.contains("WindowEvent") && line.contains("change: New");
            }
            Err(_) => break,
        }
    }

    assert!(
        found,
        "expected --debug-events to print a New WindowEvent within 10s"
    );
}
