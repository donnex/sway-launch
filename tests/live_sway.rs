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
    // Regression test: Sway treats a [con_id=N] criteria matching zero
    // containers as success, not an error, so a wait-time action
    // (Split/NewColumn/NewRow/Height/Width/Position) used to silently no-op
    // instead of erroring if the container closed between an earlier action
    // resolving it and this one running. run_wait_time()'s container_exists()
    // check is what's under test here.
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

#[test]
fn every_shipped_template_resolves_and_launches_successfully() {
    // Unlike template_apps_resolve_to_real_windows above (a hand-written,
    // minimal template), this drives the actual files under
    // examples/templates/ — nothing else in the test suite would catch a
    // shipped template that's silently broken. dual-output.toml is excluded
    // here since it needs a second output and real (non-placeholder) output
    // names to run at all; see dual_output_template_moves_windows_to_separate_outputs.
    let mut connection = connect();
    let mut paths: Vec<_> = std::fs::read_dir(examples_dir("templates"))
        .expect("examples/templates should be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("dual-output.toml"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 15,
        "expected at least 15 non-dual-output template files, found {}: {:?}",
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

    let contents = std::fs::read_to_string(examples_dir("templates").join("dual-output.toml"))
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
    // Drives the actual examples/layouts/quad-terminals.toml file (kitty
    // substituted for foot, which can't launch headlessly — see CLAUDE.md's
    // Testing section), rather than a hand-written stand-in.
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

    let contents = std::fs::read_to_string(examples_dir("layouts").join("quad-terminals.toml"))
        .expect("quad-terminals.toml should be readable");
    let contents = contents.replace("kitty", "foot");
    let path = TempToml::write("quad-terminals", &contents);

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

    let contents = std::fs::read_to_string(examples_dir("layouts").join("retarget-by-id.toml"))
        .expect("retarget-by-id.toml should be readable");
    let contents = contents.replace("kitty", "foot");
    let path = TempToml::write("retarget-by-id", &contents);

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
        matches!(
            node.floating,
            Some(swayipc::Floating::UserOn) | Some(swayipc::Floating::AutoOn)
        ),
        "the first step's window should end up floating, got {:?}",
        node.floating
    );
    // "resize set width" targets the border-inclusive outer width, same as
    // floating_with_width_and_height_applies_all_three's height adjustment —
    // rect.width alone is 2*current_border_width short of the requested
    // value (2px on each side with the default border style).
    assert_eq!(node.rect.width + 2 * node.current_border_width, 800);

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
    // occupant (see examples/templates/master-dual-stack.toml's header
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
