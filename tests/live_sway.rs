// Exercises the compiled `sway-launch` binary against a real, running Sway
// compositor (typically a headless one — see scripts/run-live-sway-tests),
// asserting on actual window state read back via `swayipc::Connection`
// rather than just exit codes. Gated behind the `live-sway-tests` feature so
// a normal `cargo test` (and the main CI job) never needs Sway installed.
//
// All tests here share one compositor, so they must run single-threaded
// (`cargo test ... -- --test-threads=1`), which scripts/run-live-sway-tests
// already does, and libtest then runs them in name order. So that what a test
// covers doesn't depend on that order, each one starts from the same state:
// it calls `fresh_workspace()` first (an empty workspace of its own on the
// compositor's original output), adds outputs only through `CreatedOutput`
// (removed again on drop), and cleans up the windows it launches via
// `KillOnDrop` — all three hold even when an assertion panics. Running one
// test on its own therefore exercises the same paths as the full run. A new
// test must follow the same pattern; where it is about one specific path,
// assert that path through `--json`'s per-action status as well, so taking
// a different one fails instead of passing quietly.
#![cfg(feature = "live-sway-tests")]

mod common;

use common::{TempToml, TestTempDir};
use std::io::BufRead;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use swayipc::{Connection, Node, NodeType};

/// The throwaway compositor's IPC socket, refusing to return anything unless
/// this run was demonstrably started by scripts/run-live-sway-tests.
///
/// This suite launches, moves and kills windows (`[app_id=foot] kill` closes
/// every foot terminal), switches workspaces and creates outputs, so pointing
/// it at a desktop session is destructive. Nothing about the environment
/// alone tells the two apart — inside a desktop session `SWAYSOCK` and
/// `I3SOCK` both name the real compositor — and a plain
/// `cargo test --all-features` compiles this file in. So the guard is
/// positive rather than inferred: the runner exports
/// `SWAY_LAUNCH_LIVE_TESTS_SOCKET`, `SWAYSOCK` must name the same socket, and
/// `I3SOCK` must be unset or name it too (swayipc's `Connection::new()` reads
/// `I3SOCK` first). As a last check, every output must be a headless one.
fn live_socket() -> &'static std::path::Path {
    static SOCKET: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    SOCKET.get_or_init(|| {
        const RUN_VIA_RUNNER: &str = "run the live suite via scripts/run-live-sway-tests, \
                                      which starts a throwaway headless Sway for it";
        let sentinel = std::env::var_os("SWAY_LAUNCH_LIVE_TESTS_SOCKET").unwrap_or_else(|| {
            panic!(
                "refusing to run: SWAY_LAUNCH_LIVE_TESTS_SOCKET is not set, so this may be a \
                 real Sway session; {RUN_VIA_RUNNER}"
            )
        });
        if std::env::var_os("SWAYSOCK").as_ref() != Some(&sentinel) {
            panic!(
                "refusing to run: SWAYSOCK does not name SWAY_LAUNCH_LIVE_TESTS_SOCKET's \
                 socket; {RUN_VIA_RUNNER}"
            );
        }
        if std::env::var_os("I3SOCK").is_some_and(|value| value != sentinel) {
            panic!(
                "refusing to run: I3SOCK names a different socket than \
                 SWAY_LAUNCH_LIVE_TESTS_SOCKET; {RUN_VIA_RUNNER}"
            );
        }
        let socket = std::path::PathBuf::from(sentinel);
        let outputs = try_connect(&socket)
            .and_then(|mut connection| connection.get_outputs().map_err(|e| e.to_string()))
            .unwrap_or_else(|error| {
                panic!("live-sway-tests could not reach {socket:?}: {error}; {RUN_VIA_RUNNER}")
            });
        if let Some(output) = outputs
            .iter()
            .find(|output| !output.name.starts_with("HEADLESS-"))
        {
            panic!(
                "refusing to run: output {:?} is not a headless one, so this is not a \
                 throwaway compositor; {RUN_VIA_RUNNER}",
                output.name
            );
        }
        socket
    })
}

fn try_connect(socket: &std::path::Path) -> Result<Connection, String> {
    std::os::unix::net::UnixStream::connect(socket)
        .map(Connection::from)
        .map_err(|error| error.to_string())
}

/// The binary under test, aimed explicitly at the throwaway compositor.
fn sway_launch_command() -> Command {
    let socket = live_socket();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
    command.env("SWAYSOCK", socket).env("I3SOCK", socket);
    command
}

fn connect() -> Connection {
    let socket = live_socket();
    try_connect(socket)
        .unwrap_or_else(|error| panic!("failed to connect to the live Sway at {socket:?}: {error}"))
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
/// `tree.rs`'s private `find_parent_layout()`, which
/// `SwayAction::poll_shows()` uses to confirm a `Split` action, since a
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

/// `con_id`'s direct parent node, tiling or floating.
fn parent_node(node: &Node, con_id: i64) -> Option<&Node> {
    if node
        .nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .any(|child| child.id == con_id)
    {
        return Some(node);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| parent_node(child, con_id))
}

/// Asserts `con_id` is the only child of a sub-container of `wrapper_layout`
/// sitting directly on its workspace, then that `--new-column` skips it
/// rather than letting Sway's singleton rule throw it onto another output.
fn assert_new_column_skips_a_lone_child(
    connection: &mut Connection,
    con_id: i64,
    wrapper_layout: swayipc::NodeLayout,
) {
    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let wrapper = parent_node(&tree_before, con_id).expect("window should have a parent");
    assert_eq!(
        wrapper.node_type,
        NodeType::Con,
        "expected a wrapping sub-container"
    );
    assert_eq!(wrapper.layout, wrapper_layout);
    assert_eq!(
        wrapper.nodes.len(),
        1,
        "expected the window to be a lone child"
    );
    assert_eq!(
        parent_node(&tree_before, wrapper.id).map(|node| node.node_type),
        Some(NodeType::Workspace),
        "expected the wrapper to sit directly on the workspace"
    );
    let output_before = output_containing(&tree_before, con_id, None)
        .expect("window should be found in the tree")
        .to_string();

    let output = sway_launch_command()
        .args(["--con-id", &con_id.to_string(), "--new-column", "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree_after = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree_after, con_id, None),
        Some(output_before.as_str()),
        "a lone child of a {wrapper_layout:?} sub-container should not be relocated to a \
         different output by --new-column"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    assert_eq!(
        json["actions"],
        serde_json::json!([{
            "kind": "new_column",
            "status": "skipped",
            "reason": "trailing_workspace_edge",
        }]),
        "expected the lone child's new_column to be skipped: {json:?}"
    );
}

fn get_node(connection: &mut Connection, con_id: i64) -> Node {
    let tree = connection.get_tree().expect("get_tree should succeed");
    find_node(&tree, con_id)
        .unwrap_or_else(|| panic!("container id {con_id} not found in the tree"))
        .clone()
}

/// `get_node()`, but first waits until Sway has applied `con_id`'s first
/// layout transaction, for a test asserting on `deco_rect`.
///
/// Sway serves a container's geometry (`rect`, `deco_rect`'s origin) from its
/// pending state but its `border` from the state it last applied, and zeroes
/// `deco_rect` unless that applied border is `normal`. A freshly mapped
/// window has applied nothing until the client acks the configure Sway sends
/// it on map (or Sway's transaction timeout passes), so until then it reads
/// as `border: none` with an all-zero `deco_rect` and a `rect` that includes
/// the title bar. sway-launch
/// can finish inside that window, since it confirms against the pending
/// state later commands act on — so asserting on `deco_rect` straight away
/// intermittently reads `0`. Every window this is used on has Sway's default
/// `normal` border, so `NodeBorder::None` means only "not applied yet".
fn get_settled_node(connection: &mut Connection, con_id: i64) -> Node {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let node = get_node(connection, con_id);
        if node.border != swayipc::NodeBorder::None {
            return node;
        }
        assert!(
            Instant::now() < deadline,
            "container id {con_id} never had a layout transaction applied"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `get_node()` once `con_id`'s applied border is `border` with `width`:
/// Sway reports the border it last applied, which lags a command until the
/// client acks the configure (see `get_settled_node()`).
fn get_node_with_border(
    connection: &mut Connection,
    con_id: i64,
    border: swayipc::NodeBorder,
    width: i32,
) -> Node {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let node = get_node(connection, con_id);
        if node.border == border && node.current_border_width == width {
            return node;
        }
        assert!(
            Instant::now() < deadline,
            "container id {con_id} never showed border {border:?} {width}: it has {:?} {}",
            node.border,
            node.current_border_width
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Mirrors `tree.rs`'s private `node_is_floating()`. Sway 1.9 never populates a
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

/// The name of the nearest `NodeType::Workspace` ancestor of `con_id` —
/// mirrors `tree.rs`'s `ContainerState::from_tree()` workspace lookup. Also used to
/// detect Sway's scratchpad (the fixed internal workspace named
/// `__i3_scratch`) in a way that's reliable on both Sway 1.9 and
/// 1.11, unlike the node's own
/// `scratchpad_state` field — that field stays `none` on Sway 1.9 even for a
/// genuinely scratchpadded window, and stays `fresh`/`changed` on 1.11 for
/// one merely shown via `scratchpad show` (see `tree.rs`'s
/// `is_scratchpad_workspace()` doc comment).
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

/// The `status` of the one action a single `--json` invocation reports.
fn single_action_status(stdout: &[u8]) -> String {
    let json: serde_json::Value =
        serde_json::from_slice(stdout).expect("stdout should be a JSON object");
    let actions = json["actions"]
        .as_array()
        .expect("--json output should carry an actions array");
    assert_eq!(actions.len(), 1, "expected exactly one action: {json}");
    actions[0]["status"]
        .as_str()
        .expect("an action should carry a status")
        .to_string()
}

/// Blocks until `con_id` is genuinely absent from Sway's tree, so a test
/// that deliberately closes a window before asserting on the
/// already-closed behaviour isn't racing the compositor.
///
/// `run_command("[con_id=N] kill")` returns as soon as Sway has *accepted*
/// the command, not once the window is gone — so a check made immediately
/// afterwards can still see the container and take a different code path
/// than the test intends. Panics rather than returning a bool: a window that
/// won't close within the deadline means the test's premise is broken, and
/// silently continuing would produce a confusing downstream assertion
/// instead.
fn wait_for_container_gone(connection: &mut Connection, con_id: i64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let tree = connection.get_tree().expect("get_tree should succeed");
        if find_node(&tree, con_id).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("container id {con_id} was still in the tree 5s after being killed");
}

/// Kills its container id when dropped, via a fresh IPC connection, so a
/// test's windows never leak into the next one — even on assertion panic.
struct KillOnDrop(i64);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(mut connection) = try_connect(live_socket()) {
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

/// Runs a Sway command and panics unless every part of it succeeded.
fn run_command_ok(connection: &mut Connection, command: &str) {
    connection
        .run_command(command)
        .unwrap_or_else(|error| panic!("`{command}` should be accepted: {error}"))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|error| panic!("`{command}` should succeed: {error}"));
}

/// The output the throwaway compositor started with. Every output a test adds
/// goes through `CreatedOutput`, which removes it again, so this is the only
/// output present whenever a test begins.
fn original_output() -> &'static str {
    static OUTPUT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OUTPUT.get_or_init(|| {
        connect()
            .get_outputs()
            .expect("get_outputs should succeed")
            .into_iter()
            .map(|output| output.name)
            .min()
            .expect("the compositor should have at least one output")
    })
}

/// Focuses the original output and switches to a workspace named after the
/// running test, returning its name — called first thing in every test, so
/// a test's windows start on an empty workspace whatever the tests sorted
/// before it left focused. libtest names each test's thread after the test,
/// which is what makes the name unique.
fn fresh_workspace() -> String {
    let test_name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .replace("::", "-");
    let workspace = format!("live-{test_name}");
    let mut connection = connect();
    run_command_ok(
        &mut connection,
        &format!("focus output {}", original_output()),
    );
    run_command_ok(&mut connection, &format!("workspace {workspace}"));
    workspace
}

/// A headless output added with `create_output` and removed again with
/// `output <name> unplug` when dropped, even on assertion panic — so every
/// test begins with the compositor's one original output, rather than with
/// however many the tests sorted before it added. Declare it before the
/// test's `KillOnDrop` guards: locals drop in reverse order, so the windows
/// are killed before their output goes away.
struct CreatedOutput(String);

impl CreatedOutput {
    fn new(connection: &mut Connection) -> Self {
        let outputs_before: Vec<String> = connection
            .get_outputs()
            .expect("get_outputs should succeed")
            .into_iter()
            .map(|output| output.name)
            .collect();
        run_command_ok(connection, "create_output");
        let name = connection
            .get_outputs()
            .expect("get_outputs should succeed")
            .into_iter()
            .map(|output| output.name)
            .find(|name| !outputs_before.contains(name))
            .expect("create_output should have added a new output");
        CreatedOutput(name)
    }

    fn name(&self) -> &str {
        &self.0
    }
}

impl Drop for CreatedOutput {
    fn drop(&mut self) {
        if let Ok(mut connection) = try_connect(live_socket()) {
            let _ = connection.run_command(format!("output {} unplug", self.0));
        }
    }
}

/// The number of threads and open sockets `pid` currently holds, read from
/// `/proc`. Both are `0` once the process has exited, which the sampling loop
/// in `event_reader_threads_stay_bounded_across_a_long_layout` relies on to
/// stop rather than treating as a real measurement.
fn thread_and_socket_count(pid: u32) -> (usize, usize) {
    let threads = std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    let sockets = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    std::fs::read_link(entry.path())
                        .map(|target| target.to_string_lossy().starts_with("socket:"))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    (threads, sockets)
}

#[test]
fn a_stalled_sway_socket_fails_instead_of_hanging_forever() {
    // Deliberately does NOT use the live compositor: it points SWAYSOCK at a
    // socket that accepts a connection and then never answers, which is the
    // one thing a real Sway session can't be made to do on demand. It lives in
    // this suite anyway because it costs ~10s of wall clock -- the headless
    // `cargo test` run is a fraction of a second and should stay that way,
    // while this tier is already the slow, environment-dependent one.
    //
    // Regression test for a confirmed hang: --timeout bounded only the wait
    // for a confirmation *event*, never the IPC round trips around it, so
    // every get_tree()/get_outputs()/run_command() was an unbounded blocking
    // read. `--con-id 42 --floating --timeout 2` against this socket ran until
    // killed externally at 15s, because reading the tree is the first thing it
    // does. IPC_ROUND_TRIP_TIMEOUT now bounds the socket itself.
    //
    // A plain launch and --debug-events go through a different first call:
    // both subscribe to the event stream before anything else, and that
    // handshake used to run unbounded, so `--timeout 2 foot` here never
    // returned even after the request path was fixed (#51). All three are run
    // concurrently so the test still costs one IPC_ROUND_TRIP_TIMEOUT, not
    // three.
    let temp_dir = TestTempDir::for_sockets();
    let socket_path = temp_dir.join("stalled-sway.sock");
    let listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("failed to bind test socket");

    // Accept and hold, never replying. Keeping the accepted streams alive is
    // the point: closing them would give the client a clean EOF instead of the
    // silence being reproduced here.
    let accepted = std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => break,
            }
        }
    });

    let invocations: [&[&str]; 3] = [
        &["--con-id", "42", "--floating", "--timeout", "2"],
        &["--timeout", "2", "foot"],
        &["--debug-events"],
    ];
    let started = std::time::Instant::now();
    let children: Vec<_> = invocations
        .iter()
        .map(|arguments| {
            let child = sway_launch_command()
                .env("SWAYSOCK", &socket_path)
                .env_remove("I3SOCK")
                .args(*arguments)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("failed to run sway-launch binary");
            (arguments, KillChildOnDrop(child))
        })
        .collect();

    // The bound is IPC_ROUND_TRIP_TIMEOUT (10s), deliberately not --timeout;
    // see that constant's doc comment for why the two are separate. The
    // ceiling here only has to be low enough to prove each one terminates on
    // its own -- one that doesn't is killed rather than left to hang the suite.
    let deadline = started + std::time::Duration::from_secs(25);
    for (arguments, mut child) in children {
        let status = loop {
            if let Some(status) = child.0.try_wait().expect("failed to poll sway-launch") {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{arguments:?} should fail on its own rather than hang; still running after \
                 {:?}",
                started.elapsed()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let mut stderr = String::new();
        std::io::Read::read_to_string(
            child.0.stderr.as_mut().expect("stderr was piped"),
            &mut stderr,
        )
        .expect("failed to read sway-launch stderr");

        assert!(
            !status.success(),
            "{arguments:?}: a compositor that never answers must be a failure, not a success"
        );
        assert!(
            stderr.contains("did not respond"),
            "{arguments:?}: the error should explain that the compositor stopped answering, \
             got {stderr:?}"
        );
    }

    drop(accepted);
    let _ = std::fs::remove_file(&socket_path);
}

/// The id of the container carrying `mark`, anywhere in the tree.
fn container_with_mark(node: &Node, mark: &str) -> Option<i64> {
    if node.marks.iter().any(|existing| existing == mark) {
        return Some(node.id);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| container_with_mark(child, mark))
}

/// Copies `from` into `to` until `from` closes, then closes `to`'s write half
/// so the far side sees the same end of stream.
fn forward_unix_stream(
    mut from: std::os::unix::net::UnixStream,
    mut to: std::os::unix::net::UnixStream,
) {
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from, &mut to);
        let _ = to.shutdown(std::net::Shutdown::Write);
    });
}

#[test]
fn rollback_on_error_stops_trying_once_the_compositor_stops_answering() {
    fresh_workspace();
    // Regression test for #94: when a step failed because the compositor
    // stopped answering, --rollback-on-error went on to send every launched
    // window its own kill, each on a fresh connection to the same silent
    // compositor and each waiting out the full IPC_ROUND_TRIP_TIMEOUT (10s)
    // before giving up -- 10s x (launched windows + 1) before the failure was
    // reported at all. Now the first unanswered kill ends the rollback and the
    // rest go straight into rollback_failed.
    //
    // The real compositor can't be stalled without stalling the whole suite,
    // so the binary is pointed at a proxy socket instead: it forwards every
    // connection to the live compositor until `stalled` is set, then accepts
    // and holds new ones without ever answering -- the same silence
    // a_stalled_sway_socket_fails_instead_of_hanging_forever reproduces, but
    // only after three real windows have launched through it.
    let temp_dir = TestTempDir::for_sockets();
    let socket_path = temp_dir.join("stalling-proxy.sock");
    let listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("failed to bind proxy socket");
    let stalled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let proxy_stalled = std::sync::Arc::clone(&stalled);
    let upstream_socket = live_socket().to_path_buf();
    let _proxy = std::thread::spawn(move || {
        let mut held = Vec::new();
        for client in listener.incoming() {
            let Ok(client) = client else { break };
            if proxy_stalled.load(std::sync::atomic::Ordering::SeqCst) {
                held.push(client);
                continue;
            }
            let Ok(upstream) = std::os::unix::net::UnixStream::connect(&upstream_socket) else {
                continue;
            };
            let (Ok(client_reader), Ok(upstream_reader)) =
                (client.try_clone(), upstream.try_clone())
            else {
                continue;
            };
            forward_unix_stream(client_reader, upstream);
            forward_unix_stream(upstream_reader, client);
        }
    });

    // Three launched windows, then a fourth step whose wait_time leaves time
    // to stall the proxy before its next IPC request: from then on every
    // request gets no reply, so the step fails on the IPC timeout and the
    // rollback that follows meets the same silence.
    const LAUNCHED: usize = 3;
    let marks: Vec<String> = (0..LAUNCHED)
        .map(|step| format!("live-sway-test-stalled-rollback-{step}"))
        .collect();
    let mut layout: String = marks
        .iter()
        .enumerate()
        .map(|(step, mark)| {
            format!(
                "[[step]]\nid = \"app{step}\"\ncommand = \"foot\"\napp_id = \"foot\"\n\
                 mark = \"{mark}\"\n\n"
            )
        })
        .collect();
    layout.push_str("[[step]]\ntarget_id = \"app0\"\nsplit = \"v\"\nwait_time = 2000\n");
    let path = TempToml::write("stalled-rollback", &layout);

    let started = Instant::now();
    let mut child = KillChildOnDrop(
        Command::new(env!("CARGO_BIN_EXE_sway-launch"))
            .env("SWAYSOCK", &socket_path)
            .env_remove("I3SOCK")
            .args([
                "--layout",
                path.to_str().unwrap(),
                "--rollback-on-error",
                "--json",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to run sway-launch binary"),
    );

    // Stall as soon as every launched window carries its mark: the last
    // step's 2s wait_time has only just begun then.
    let mut connection = connect();
    let launch_deadline = Instant::now() + Duration::from_secs(20);
    let ids: Vec<i64> = loop {
        let tree = connection.get_tree().expect("get_tree should succeed");
        let found: Vec<Option<i64>> = marks
            .iter()
            .map(|mark| container_with_mark(&tree, mark))
            .collect();
        if found.iter().all(Option::is_some) {
            stalled.store(true, std::sync::atomic::Ordering::SeqCst);
            break found.into_iter().flatten().collect();
        }
        assert!(
            Instant::now() < launch_deadline,
            "the three launch steps should have finished, found {found:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let _guards: Vec<KillOnDrop> = ids.iter().map(|&id| KillOnDrop(id)).collect();

    // One timeout for the failing step and one for the first kill, plus the
    // wait_time and the launches, is about 23s. Unfixed, each of the other two
    // kills adds another 10s on top.
    let deadline = started + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.0.try_wait().expect("failed to poll sway-launch") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "sway-launch should give up on its own; still running after {:?}",
            started.elapsed()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let elapsed = started.elapsed();
    let mut stderr = String::new();
    std::io::Read::read_to_string(
        child.0.stderr.as_mut().expect("stderr was piped"),
        &mut stderr,
    )
    .expect("failed to read sway-launch stderr");

    assert!(
        !status.success(),
        "a step that got no reply must fail: {stderr:?}"
    );
    assert!(
        stderr.contains("skipped closing 2 more window(s)"),
        "rollback should stop after the first unanswered kill: {stderr:?}"
    );
    let error_line = stderr.lines().last().expect("stderr should not be empty");
    let error: serde_json::Value =
        serde_json::from_str(error_line).expect("the last stderr line should be the JSON error");
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|message| message.contains("did not respond")),
        "the step should fail on the IPC timeout: {error}"
    );
    let mut rollback_failed: Vec<i64> = serde_json::from_value(error["rollback_failed"].clone())
        .expect("rollback_failed should list every launched window");
    rollback_failed.sort_unstable();
    let mut expected = ids.clone();
    expected.sort_unstable();
    assert_eq!(
        rollback_failed, expected,
        "every launched window should be reported as unconfirmed: {error}"
    );
    assert!(
        elapsed < Duration::from_secs(33),
        "a stalled compositor should cost one kill's timeout, not one per window; took \
         {elapsed:?}"
    );

    let _ = std::fs::remove_file(&socket_path);
}

#[test]
fn event_reader_threads_stay_bounded_across_a_long_layout() {
    fresh_workspace();
    // Every event-confirmed action spawns a thread owning a Sway IPC event
    // stream, and that thread outlives the action whenever it's still blocked
    // on the socket -- there's no way to interrupt a blocking EventStream read
    // from outside. The question this pins is whether those readers accumulate
    // with step count or drain themselves.
    //
    // They drain: Sway broadcasts every window event to every subscription, so
    // step N's own confirming event is what wakes step N-1's orphaned reader,
    // whose receiver is gone, so it breaks out and releases both thread and
    // socket. The cost is therefore a small constant, not O(steps).
    //
    // The bound is easy to doubt from reading the code alone (a reader
    // thread per action looks like it accumulates), so it's enforced here
    // rather than only described.
    //
    // Mark is used for every step because it is the one event-confirmed action
    // is_skipped_when_already_in_effect() excludes: re-applying a mark still
    // fires WindowChange::Mark, so all 120 steps genuinely wait on an event
    // and spawn a reader, rather than short-circuiting into a no-op.
    let (container_id, _guard) = launch_foot(&[]);

    const STEPS: usize = 120;
    let mut layout = String::new();
    for step in 0..STEPS {
        layout.push_str(&format!(
            "[[step]]\ncon_id = {container_id}\nmark = \"bounded-{step}\"\n\n"
        ));
    }
    let path = TempToml::write("event-reader-bound", &layout);

    let mut child = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch");
    let pid = child.id();

    let mut max_threads = 0;
    let mut max_sockets = 0;
    let mut samples = 0;
    while child.try_wait().expect("failed to poll child").is_none() {
        let (threads, sockets) = thread_and_socket_count(pid);
        // A zero reading means the process exited between try_wait() and the
        // /proc read; don't fold that into the maximum.
        if threads > 0 {
            max_threads = max_threads.max(threads);
            max_sockets = max_sockets.max(sockets);
            samples += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let status = child.wait().expect("failed to wait for child");
    assert!(status.success(), "the layout run itself should succeed");

    assert!(
        samples >= 5,
        "too few samples ({samples}) to conclude anything about resource growth"
    );

    // Generous ceilings relative to the ~3/~3 actually measured: the point is
    // to catch a return to O(steps) growth, not to pin an exact number that a
    // scheduling difference could jitter past.
    assert!(
        max_threads <= 12,
        "event-reader threads should stay bounded regardless of step count, but a \
         {STEPS}-step layout peaked at {max_threads} threads (over {samples} samples) -- \
         if this grew with STEPS, the readers are accumulating again"
    );
    assert!(
        max_sockets <= 12,
        "Sway IPC sockets should stay bounded regardless of step count, but a {STEPS}-step \
         layout peaked at {max_sockets} open sockets (over {samples} samples)"
    );
}

#[test]
fn a_layout_finishes_every_step_when_its_output_pipes_are_already_closed() {
    fresh_workspace();
    // `sway-launch --layout ... | head -n1` is an ordinary way to capture the
    // first id. Once the reader went away, the next stdout write used to end
    // the process with exit 0 between steps -- a layout cut short and reported
    // as a success -- and with --verbose, the next diagnostic on a closed
    // stderr panicked (exit 101) between two actions. Both pipes here are
    // closed before the process starts, so the very first write of each kind
    // fails, which makes this deterministic rather than dependent on how much
    // the pipe buffer happens to absorb.
    //
    // Each step carries its own mark, since stdout can't report the ids.
    const STEPS: usize = 3;
    let marks: Vec<String> = (0..STEPS)
        .map(|step| format!("live-sway-test-closed-pipes-{step}"))
        .collect();
    let layout: String = marks
        .iter()
        .map(|mark| {
            format!("[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\nmark = \"{mark}\"\n\n")
        })
        .collect();
    let path = TempToml::write("closed-output-pipes", &layout);

    let (stdout_reader, stdout_writer) = std::io::pipe().expect("failed to create a pipe");
    let (stderr_reader, stderr_writer) = std::io::pipe().expect("failed to create a pipe");
    drop(stdout_reader);
    drop(stderr_reader);

    let status = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--verbose"])
        .stdout(stdout_writer)
        .stderr(stderr_writer)
        .status()
        .expect("failed to run sway-launch binary");

    let tree = connect().get_tree().expect("failed to read the tree");
    let found: Vec<Option<i64>> = marks
        .iter()
        .map(|mark| container_with_mark(&tree, mark))
        .collect();
    let _guards: Vec<KillOnDrop> = found.iter().flatten().map(|&id| KillOnDrop(id)).collect();

    assert!(
        found.iter().all(Option::is_some),
        "every step should have run despite the closed pipes, but found {found:?}"
    );
    assert!(
        status.success(),
        "a run whose every step succeeded should exit 0, got {status}"
    );
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
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let node = get_node(&mut connection, container_id);
    assert_eq!(node.app_id.as_deref(), Some("foot"));
}

#[test]
fn the_live_compositor_is_recognized_as_a_supported_sway() {
    fresh_workspace();
    // The version check (#147) reads GET_VERSION's `variant` itself, which
    // swayipc's own Version drops, so this checks real Sway sets it the way
    // the check expects: no warning, and --verbose names the version that
    // `sway --version` reports.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--focus",
            "--verbose",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");
    assert!(!stderr.contains("warning:"), "{stderr}");

    let version = connection
        .get_version()
        .expect("get_version should succeed");
    let expected = format!(
        "Sway version: {}.{}.{}",
        version.major, version.minor, version.patch
    );
    assert!(
        stderr.contains(&expected),
        "expected {expected:?} in {stderr}"
    );
}

#[test]
fn exec_runs_a_command_that_starts_with_a_shell_builtin() {
    fresh_workspace();
    // The PID marker used to be prepended as `env VAR=value <command>`, and
    // `env` can only run an executable: `cd` failed, the `&&` short-circuited,
    // nothing launched, and the wait ran out the full --timeout. The `exec`
    // after `&&` also checks the marker reaches a later command in the list,
    // not only the first one -- the window must still be PID-marker-confirmed.
    let mut connection = connect();
    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            "--verbose",
            "cd / && exec foot",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");

    let container_id: i64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("stdout should be a container id");
    let _guard = KillOnDrop(container_id);

    assert!(
        stderr.contains("(PID-marker-confirmed)"),
        "the window should have been confirmed by its marker, not a fallback: {stderr}"
    );
    let node = get_node(&mut connection, container_id);
    assert_eq!(node.app_id.as_deref(), Some("foot"));
}

#[test]
fn cwd_and_env_reach_the_launched_command() {
    fresh_workspace();
    // --cwd/--env (#113) set what Sway's exec otherwise leaves as Sway's own:
    // the command starts in the given directory with the given variables. A
    // relative --cwd is relative to the caller's directory.
    let temp_dir = TestTempDir::new();
    let caller_directory = temp_dir.join("caller");
    let project_directory = caller_directory.join("project");
    std::fs::create_dir_all(&project_directory).expect("failed to create the directories");
    let working_directory_file = temp_dir.join("cwd-working-directory");
    let environment_file = temp_dir.join("cwd-environment");

    let output = sway_launch_command()
        .current_dir(&caller_directory)
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            "--cwd",
            "project",
            "--env",
            "SWAY_LAUNCH_LIVE_TEST_ENV=a value, with; syntax 'quoted' back\\slash",
            &format!(
                "pwd > '{}' && env > '{}' && exec foot",
                working_directory_file.display(),
                environment_file.display()
            ),
        ])
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

    let working_directory = std::fs::read_to_string(&working_directory_file)
        .expect("the launched command should have recorded its working directory");
    let environment = std::fs::read_to_string(&environment_file)
        .expect("the launched command should have recorded its environment");
    assert_eq!(
        std::path::Path::new(working_directory.trim()),
        project_directory.canonicalize().unwrap()
    );
    assert!(
        environment
            .lines()
            .any(|line| line
                == "SWAY_LAUNCH_LIVE_TEST_ENV=a value, with; syntax 'quoted' back\\slash"),
        "the variable should reach the command unchanged: {environment}"
    );

    // Neither the value nor the directory is on the launch shell's command
    // line, which every local user can read while foot runs: the shell
    // names only the setup file, and has removed it (#167).
    let pid = get_node(&mut connect(), container_id)
        .pid
        .expect("foot's window should have a pid");
    let ancestor_command_lines = ancestor_command_lines(pid as u32);
    assert!(
        ancestor_command_lines
            .iter()
            .any(|line| line.contains("SWAY_LAUNCH_PID_MARKER")),
        "the launch shell should be among foot's ancestors: {ancestor_command_lines:?}"
    );
    for line in &ancestor_command_lines {
        assert!(!line.contains("a value, with"), "{line}");
        assert!(!line.contains("caller/project"), "{line}");
    }
    let setup_directory =
        std::path::Path::new(&std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join("sway-launch");
    let leftovers: Vec<_> = std::fs::read_dir(setup_directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .filter(|name| name.to_string_lossy().starts_with("launch-"))
                .collect()
        })
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// The command lines of `pid`'s parent and the few processes above it,
/// arguments joined with spaces.
fn ancestor_command_lines(pid: u32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = pid;
    for _ in 0..4 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{current}/stat")) else {
            break;
        };
        let Some(parent) = stat
            .rsplit_once(") ")
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .and_then(|field| field.parse::<u32>().ok())
        else {
            break;
        };
        if parent <= 1 {
            break;
        }
        let command_line = std::fs::read(format!("/proc/{parent}/cmdline")).unwrap_or_default();
        lines.push(String::from_utf8_lossy(&command_line).replace('\0', " "));
        current = parent;
    }
    lines
}

#[test]
fn a_binding_s_cwd_and_env_reach_its_slot_s_launched_command() {
    fresh_workspace();
    // A bindings file's cwd/env (#113) go through the same launch setup as
    // --cwd/--env, checked against the bindings file in resolve() (#157).
    let temp_dir = TestTempDir::new();
    let project_directory = temp_dir.join("binding-project");
    std::fs::create_dir_all(&project_directory).expect("failed to create the directory");
    let working_directory_file = temp_dir.join("binding-working-directory");
    let environment_file = temp_dir.join("binding-environment");
    let launched_command = format!(
        "pwd > '{}' && env > '{}' && exec foot",
        working_directory_file.display(),
        environment_file.display()
    );
    let bindings = TempToml::write(
        "binding-cwd-env-bindings",
        &format!(
            "[[binding]]\nslot = \"top\"\ncommand = {launched_command:?}\ncwd = {:?}\n\
             env = {{ SWAY_LAUNCH_LIVE_TEST_ENV = \"a value, with; syntax\" }}\n\n\
             [[binding]]\nslot = \"bottom\"\ncommand = \"foot\"\n",
            project_directory.to_str().unwrap()
        ),
    );

    let output = sway_launch_command()
        .args([
            "--template",
            "dual-column",
            "--bindings",
            bindings.to_str().unwrap(),
            "--timeout",
            "10",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let guards: Vec<KillOnDrop> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| KillOnDrop(line.parse().expect("each line should be a container id")))
        .collect();
    assert_eq!(guards.len(), 2);

    let working_directory = std::fs::read_to_string(&working_directory_file)
        .expect("the launched command should have recorded its working directory");
    let environment = std::fs::read_to_string(&environment_file)
        .expect("the launched command should have recorded its environment");
    assert_eq!(
        std::path::Path::new(working_directory.trim()),
        project_directory.canonicalize().unwrap()
    );
    assert!(
        environment
            .lines()
            .any(|line| line == "SWAY_LAUNCH_LIVE_TEST_ENV=a value, with; syntax"),
        "the variable should reach the command unchanged: {environment}"
    );
}

#[test]
fn exec_runs_in_sway_s_directory_and_environment_not_the_caller_s() {
    fresh_workspace();
    // Sway's own `exec` forks the command from the compositor, so it inherits
    // Sway's working directory and environment rather than sway-launch's.
    // README.md's "Launching a command" documents this; pinned here so a future
    // change to how the command is launched can't alter it unnoticed.
    let temp_dir = TestTempDir::new();
    let caller_directory = temp_dir.join("exec-caller-directory");
    std::fs::create_dir(&caller_directory).expect("failed to create the caller directory");
    let working_directory_file = temp_dir.join("exec-working-directory");
    let environment_file = temp_dir.join("exec-environment");
    let variable = "SWAY_LAUNCH_LIVE_TEST_CALLER_ONLY";

    let output = sway_launch_command()
        .current_dir(&caller_directory)
        .env(variable, "set-by-caller")
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            &format!(
                "pwd > '{}' && env > '{}' && exec foot",
                working_directory_file.display(),
                environment_file.display()
            ),
        ])
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

    let working_directory = std::fs::read_to_string(&working_directory_file)
        .expect("the launched command should have recorded its working directory");
    let environment = std::fs::read_to_string(&environment_file)
        .expect("the launched command should have recorded its environment");

    assert_ne!(
        std::path::Path::new(working_directory.trim()),
        caller_directory.canonicalize().unwrap(),
        "the command should run in Sway's directory, not the caller's"
    );
    assert!(
        !environment.contains(variable),
        "a variable set only in the caller's environment should not reach the command"
    );
}

#[test]
fn exec_passes_a_quoted_separator_through_to_the_shell_intact() {
    fresh_workspace();
    // Sway splits an exec on an unquoted `,`/`;`, but leaves a quoted one --
    // quotes included -- for the shell, so the title must arrive verbatim.
    let mut connection = connect();
    let title = "live-sway,quoted;title";
    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            &format!("foot -T '{title}'"),
        ])
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

    // foot can map its window a moment before it sets the title.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let name = get_node(&mut connection, container_id).name;
        if name.as_deref() == Some(title) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the title should arrive intact, got {name:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn exec_rejects_an_unquoted_separator_before_sway_can_split_it() {
    fresh_workspace();
    // `foot -T x; mark ...` used to launch foot and then run the mark against
    // whatever window was focused -- exiting 0. Now nothing reaches Sway.
    let mut connection = connect();
    let (_container_id, _guard) = launch_foot(&[]);
    let injected_mark = "live-sway-test-exec-split-injection";

    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            &format!("foot -T x; mark {injected_mark}"),
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "a command Sway would split must be rejected"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing should have been launched or reported"
    );

    fn has_mark(node: &Node, mark: &str) -> bool {
        node.marks.iter().any(|existing| existing == mark)
            || node.nodes.iter().any(|child| has_mark(child, mark))
            || node
                .floating_nodes
                .iter()
                .any(|child| has_mark(child, mark))
    }
    let tree = connection.get_tree().expect("failed to read the tree");
    assert!(
        !has_mark(&tree, injected_mark),
        "the injected Sway command must never run"
    );
}

#[test]
fn floating_with_width_and_height_applies_all_three() {
    fresh_workspace();
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
fn a_wait_time_past_the_timeout_still_confirms_a_floating_and_width_batch() {
    // #143: --timeout counts from when the batch's command is sent, so the
    // --wait-time sleep before it doesn't use the floating event's time up.
    fresh_workspace();
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--floating",
            "--width",
            "400px",
            "--timeout",
            "1",
            "--wait-time",
            "1200",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    let statuses: Vec<(&str, &str)> = result["actions"]
        .as_array()
        .expect("actions is an array")
        .iter()
        .map(|action| {
            (
                action["kind"].as_str().unwrap(),
                action["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        vec![("floating", "changed"), ("width", "changed")]
    );
}

#[test]
fn follow_up_actions_reach_sway_as_one_command_and_are_each_confirmed() {
    // #32: everything after the window is found goes to Sway in one command,
    // and each action is still confirmed on its own: floating and the mark by
    // their events, the geometry by polling.
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--floating",
            "--height",
            "300px",
            "--width",
            "400px",
            "--position",
            "50,60",
            "--mark",
            "batched",
            "--json",
            "--verbose",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");

    let sent: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("Sway command: "))
        .collect();
    assert_eq!(
        sent,
        vec![format!(
            "Sway command: [con_id={container_id}] floating enable, resize set height 300px, \
             resize set width 400px, move position 50 60, mark \"batched\""
        )],
        "{stderr}"
    );

    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    let statuses: Vec<(&str, &str)> = result["actions"]
        .as_array()
        .expect("actions is an array")
        .iter()
        .map(|action| {
            (
                action["kind"].as_str().unwrap(),
                action["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("floating", "changed"),
            ("height", "changed"),
            ("width", "changed"),
            ("position", "changed"),
            ("mark", "changed"),
        ]
    );

    let node = get_node(&mut connection, container_id);
    assert!(node_is_floating(&node));
    assert_eq!(node.rect.width, 400);
    assert_eq!(node.rect.height + node.deco_rect.height, 300);
    assert_eq!(node.marks, vec!["batched".to_string()]);
}

#[test]
fn sway_commands_are_sent_with_the_batch_and_take_effect() {
    // #90: each --sway-command joins the batch after the mark, as given, and
    // is reported unconfirmed, since nothing can say what it was meant to do.
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--mark",
            "raw",
            "--sway-command",
            "border pixel 3",
            "--sway-command",
            "title_format \"%title, raw\"",
            "--json",
            "--verbose",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");
    assert!(
        stderr.contains(&format!(
            "Sway command: [con_id={container_id}] mark \"raw\", border pixel 3, title_format \
             \"%title, raw\""
        )),
        "{stderr}"
    );

    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    let statuses: Vec<(&str, &str)> = result["actions"]
        .as_array()
        .expect("actions is an array")
        .iter()
        .map(|action| {
            (
                action["kind"].as_str().unwrap(),
                action["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("mark", "changed"),
            ("sway_command", "unconfirmed"),
            ("sway_command", "unconfirmed"),
        ]
    );

    let node = get_node_with_border(&mut connection, container_id, swayipc::NodeBorder::Pixel, 3);
    assert_eq!(node.marks, vec!["raw".to_string()]);
}

#[test]
fn a_layout_steps_sway_commands_take_effect_and_session_commands_are_refused() {
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let path = TempToml::write(
        "sway-commands",
        &format!("[[step]]\ncon_id = {container_id}\nsway_commands = [\"border pixel 4\"]\n"),
    );
    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    get_node_with_border(&mut connection, container_id, swayipc::NodeBorder::Pixel, 4);

    // Refused before anything is sent, so the compositor stays up and the
    // window keeps its border.
    let refused = TempToml::write(
        "sway-commands-refused",
        &format!(
            "[[step]]\ncon_id = {container_id}\nsway_commands = [\"border normal\", \"exit\"]\n"
        ),
    );
    let output = sway_launch_command()
        .args(["--layout", refused.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("acts on one window"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(200));
    get_node_with_border(&mut connection, container_id, swayipc::NodeBorder::Pixel, 4);

    // #123: a quote split across two entries, which joined would run the
    // `exec` behind [con_id=N]. Refused before anything is sent.
    let touched =
        std::env::temp_dir().join(format!("sway-launch-split-quote-{}", std::process::id()));
    let _ = std::fs::remove_file(&touched);
    let split_quote = TempToml::write(
        "sway-commands-split-quote",
        &format!(
            "[[step]]\ncon_id = {container_id}\nsway_commands = ['title_format \"a', \
             'border \", exec touch {}, border \"']\n",
            touched.display()
        ),
    );
    let output = sway_launch_command()
        .args(["--layout", split_quote.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unclosed '\"' quote"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(!touched.exists(), "the split-off exec ran");
    get_node_with_border(&mut connection, container_id, swayipc::NodeBorder::Pixel, 4);
}

#[test]
fn settle_applies_the_geometry_again_after_something_else_changes_it() {
    // #17: a browser sets its own size just after mapping. No browser is
    // available here, so the test plays the client, resizing the window over
    // IPC while --settle is holding it.
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating"]);

    let child = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--width",
            "400px",
            "--height",
            "300px",
            "--settle",
            "1000",
            "--json",
            "--verbose",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    std::thread::sleep(Duration::from_millis(400));
    for outcome in connection
        .run_command(format!("[con_id={container_id}] resize set width 600px"))
        .expect("the competing resize should be sent")
    {
        outcome.expect("the competing resize should succeed");
    }
    let output = child.wait_with_output().expect("sway-launch should finish");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");
    assert!(stderr.contains("re-applying (1 of 3)"), "{stderr}");
    assert!(stderr.contains("Geometry held for 1000 ms"), "{stderr}");

    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    for action in result["actions"].as_array().expect("actions is an array") {
        assert_eq!(action["status"], "changed", "{result}");
    }
    let node = get_node(&mut connection, container_id);
    assert_eq!(node.rect.width, 400);
    assert_eq!(node.rect.height + node.deco_rect.height, 300);
}

#[test]
fn settle_fails_without_re_sending_when_the_window_closes_mid_hold() {
    // #159: a closed window used to read as the client undoing the size, so
    // the geometry was sent again and the run failed with Sway's own
    // "No matching node." under the wrong action's name. The run may still
    // re-apply the width before the window goes (seen live: twice), so only
    // how it ends is asserted.
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating"]);

    let child = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--width",
            "400px",
            "--settle",
            "3000",
            "--json",
            "--verbose",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    std::thread::sleep(Duration::from_millis(600));
    for outcome in connection
        .run_command(format!("[con_id={container_id}] kill"))
        .expect("the kill should be sent")
    {
        outcome.expect("the kill should succeed");
    }
    let output = child.wait_with_output().expect("sway-launch should finish");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "sway-launch succeeded: {stderr}");
    assert!(
        stderr.contains(&format!(
            "resize set width 400px: container id {container_id} no longer exists"
        )),
        "{stderr}"
    );
    assert!(!stderr.contains("No matching node"), "{stderr}");
}

#[test]
fn swaysock_wins_over_a_stale_i3sock() {
    fresh_workspace();
    // Sway exports both SWAYSOCK and I3SOCK to everything it spawns, so the way
    // to aim a Sway tool at a different compositor is to override SWAYSOCK
    // alone — which swaymsg honors. sway-launch used to read I3SOCK first, so
    // that override was silently ignored (and the live runner, which sets
    // SWAYSOCK, would have driven the desktop session instead). A stale I3SOCK
    // here must not matter; --floating also exercises the event connection,
    // which has to resolve the socket the same way as the request connection.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .env("I3SOCK", "/nonexistent/sway-launch-stale-i3sock")
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
        "sway-launch with a stale I3SOCK should still use SWAYSOCK: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(node_is_floating(&get_node(&mut connection, container_id)));
}

#[test]
fn a_stale_swaysock_names_the_running_sway_beside_it() {
    // A tmux session started under an earlier Sway keeps its SWAYSOCK after
    // a restart (#162). Run here so the name real Sway gives its socket
    // (`sway-ipc.<uid>.<pid>.sock`) is what the search is checked against.
    let socket = live_socket();
    let runtime_directory = socket.parent().expect("the socket has a directory");
    let stale = runtime_directory.join("sway-ipc.stale.sock");
    let output = sway_launch_command()
        .env("SWAYSOCK", &stale)
        .env_remove("I3SOCK")
        .env("XDG_RUNTIME_DIR", runtime_directory)
        .args(["--existing", "--app-id", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("SWAYSOCK names a socket no Sway is listening on any more"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("export SWAYSOCK={}", socket.display())),
        "{stderr}"
    );
}

#[test]
fn sticky_floating_window_follows_a_workspace_switch() {
    let workspace = fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating", "--sticky"]);

    let elsewhere = format!("{workspace}-elsewhere");
    run_command_ok(&mut connection, &format!("workspace {elsewhere}"));

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some(elsewhere.as_str()),
        "a floating sticky window should follow the switch to another workspace"
    );
}

#[test]
fn sticky_on_a_tiled_window_only_takes_effect_once_floated() {
    // Sway accepts `sticky enable` against a tiled window and sets the flag,
    // but only carries a *floating* sticky window across workspace switches.
    // README.md's "Sticky" section documents exactly this, so this pins all
    // three halves: the flag is set, the tiled window stays put, and the flag
    // applies once the window is floated later.
    let workspace = fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--sticky"]);

    let node = get_node(&mut connection, container_id);
    assert!(!node_is_floating(&node), "window should still be tiled");
    assert!(node.sticky, "window should be marked sticky");

    let elsewhere = format!("{workspace}-elsewhere");
    run_command_ok(&mut connection, &format!("workspace {elsewhere}"));
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some(workspace.as_str()),
        "a tiled sticky window should stay on its own workspace"
    );

    run_command_ok(&mut connection, &format!("workspace {workspace}"));
    run_command_ok(
        &mut connection,
        &format!("[con_id={container_id}] floating enable"),
    );
    let later = format!("{workspace}-later");
    run_command_ok(&mut connection, &format!("workspace {later}"));
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some(later.as_str()),
        "the sticky flag should take effect once the window is floated"
    );
}

#[test]
fn sticky_confirms_via_poll_well_under_a_large_wait_time() {
    fresh_workspace();
    // Mirrors split_confirms_via_poll_well_under_a_large_wait_time: Sticky
    // has no dedicated WindowChange event (confirmed live — see
    // matching_window_change_event()'s Sticky arm), so it's a wait-time
    // action confirmed via SwayAction::poll_shows() polling the
    // container's own `sticky` field. A --wait-time well above the poll
    // grace period, with an assertion comfortably under 2 * --wait-time, is
    // what makes "confirmed via poll" observable rather than falling back
    // to a second full sleep.
    let (container_id, _guard) = launch_foot(&[]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--sticky",
            "--wait-time",
            "2000",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --sticky failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "--sticky took {:?} against a 2000ms --wait-time (fallback would take ~4000ms), \
         suggesting it fell back to sleeping instead of confirming via poll",
        started.elapsed()
    );

    let mut connection = connect();
    let node = get_node(&mut connection, container_id);
    assert!(node.sticky, "window should be marked sticky");
}

#[test]
fn mark_applies_the_given_mark() {
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--mark", "live-sway-test-mark"]);

    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&"live-sway-test-mark".to_string()));
}

#[test]
fn mark_replaces_existing_marks_and_takes_the_mark_from_another_window() {
    // Pins the replace semantics README.md's "Mark" section documents:
    // `--mark` sends a plain `mark`, so it drops the window's other marks,
    // and a mark another window holds moves rather than being shared.
    fresh_workspace();
    let mut connection = connect();
    let first_mark = "live-sway-test-mark-replace-first";
    let second_mark = "live-sway-test-mark-replace-second";
    let (first_id, _first_guard) = launch_foot(&["--mark", first_mark]);
    let (second_id, _second_guard) = launch_foot(&[]);

    let mark = |container_id: i64, mark: &str| {
        let output = sway_launch_command()
            .args(["--con-id", &container_id.to_string(), "--mark", mark])
            .output()
            .expect("failed to run sway-launch binary");
        assert!(
            output.status.success(),
            "sway-launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    mark(first_id, second_mark);
    assert_eq!(
        get_node(&mut connection, first_id).marks,
        vec![second_mark.to_string()],
        "a second --mark should replace the window's earlier mark, not add to it"
    );

    mark(second_id, second_mark);
    assert_eq!(
        get_node(&mut connection, second_id).marks,
        vec![second_mark.to_string()]
    );
    assert!(
        get_node(&mut connection, first_id).marks.is_empty(),
        "marking another window should take the mark away from the first one"
    );
}

#[test]
fn workspace_moves_window_to_named_workspace() {
    fresh_workspace();
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
fn workspace_and_output_names_match_regardless_of_case() {
    let home = fresh_workspace();
    // Sway matches workspace and output names case-insensitively, so
    // `--workspace live-case-target` lands on an existing `Live-Case-Target`.
    // The confirmation used to wait for the exact spelling, which never
    // appeared, and failed at --timeout after a move that worked.
    let mut connection = connect();
    run_command_ok(&mut connection, "workspace Live-Case-Target");
    let (_occupant, _occupant_guard) = launch_foot(&[]);
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));

    let started = Instant::now();
    let (container_id, _guard) = launch_foot(&["--workspace", "live-case-target"]);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?} -- the case-mismatched name should confirm at once, not at --timeout",
        started.elapsed()
    );
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some("Live-Case-Target")
    );

    let output = output_containing(&tree, container_id, None)
        .expect("the window should be on an output")
        .to_lowercase();
    run_command_ok(&mut connection, "workspace Live-Case-Target");
    let started = Instant::now();
    let result = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--output",
            &output,
            "--timeout",
            "3",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        result.status.success(),
        "a lower-cased output name should be accepted: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("\"status\":\"already_satisfied\""),
        "the window is already on that output under another spelling: {}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
fn workspace_and_output_together_put_the_window_on_that_workspace_on_that_output() {
    let home = fresh_workspace();
    // A container move to an output lands on the workspace that output is
    // showing, so running --output after --workspace used to drop the
    // workspace while both reported "changed". The workspace itself is now
    // moved to the output -- taking any window already on it along.
    let mut connection = connect();
    let created_output = CreatedOutput::new(&mut connection);
    let new_output = created_output.name().to_string();
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));
    run_command_ok(&mut connection, "workspace live-pinned-target");
    let (occupant, _occupant_guard) = launch_foot(&[]);
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));

    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            "--wait-time",
            "100",
            "--workspace",
            "live-pinned-target",
            "--output",
            &new_output,
            "--json",
            "foot",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    let container_id = json["container_id"]
        .as_i64()
        .expect("--json output should carry the container id");
    let _guard = KillOnDrop(container_id);
    assert!(
        stdout.contains("\"command\":\"move workspace to output"),
        "the workspace, not the container, should be moved to the output: {stdout}"
    );
    assert!(
        !stdout.contains("\"status\":\"unconfirmed\""),
        "both moves should be confirmed: {stdout}"
    );

    let tree = connection.get_tree().expect("get_tree should succeed");
    for id in [container_id, occupant] {
        assert_eq!(
            workspace_containing(&tree, id, None),
            Some("live-pinned-target")
        );
        assert_eq!(
            output_containing(&tree, id, None),
            Some(new_output.as_str())
        );
    }
}

#[test]
fn workspace_refuses_a_marked_split_container_instead_of_timing_out() {
    let home = fresh_workspace();
    // Sway sends window events only for views, so --workspace on a marked
    // split container applied the move, waited out --timeout for an event
    // that never came, and failed. It's now refused before anything is sent.
    let mut connection = connect();
    // Splitting a window that has a sibling wraps it in a new container,
    // which the next window then joins.
    let (_beside, _beside_guard) = launch_foot(&[]);
    let (first, _first_guard) = launch_foot(&["--split", "v"]);
    let (_second, _second_guard) = launch_foot(&[]);
    let tree = connection.get_tree().expect("get_tree should succeed");
    fn parent_of(node: &Node, child: i64) -> Option<i64> {
        if node.nodes.iter().any(|candidate| candidate.id == child) {
            return Some(node.id);
        }
        node.nodes
            .iter()
            .chain(&node.floating_nodes)
            .find_map(|candidate| parent_of(candidate, child))
    }
    let split = parent_of(&tree, first).expect("the window should have a parent");
    assert_ne!(
        find_node(&tree, split).map(|node| node.node_type),
        Some(NodeType::Workspace),
        "the two windows should share a split container below the workspace"
    );
    run_command_ok(
        &mut connection,
        &format!("[con_id={split}] mark live-split-group"),
    );

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--existing",
            "--mark-match",
            "live-split-group",
            "--workspace",
            "live-split-target",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "the container target should be refused"
    );
    assert!(
        stderr.contains("is a split container, not a window"),
        "{stderr}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?} -- should be refused up front, not at --timeout",
        started.elapsed()
    );
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, first, None),
        Some(home.as_str()),
        "nothing should have been moved"
    );
}

#[test]
fn output_moves_window_to_named_output() {
    fresh_workspace();
    // Proves the WindowChange::Move assumption documented for --output:
    // moving to a *different* output reliably fires the event, confirmed by
    // this actually completing promptly with the window moved rather than
    // hanging until --timeout. Moving to the output the window is already
    // on is a separate, already-there no-op case — see
    // output_is_a_no_op_when_already_on_the_target_output below.
    let mut connection = connect();
    let created_output = CreatedOutput::new(&mut connection);
    let new_output = created_output.name().to_string();

    let (container_id, _guard) = launch_foot(&["--output", &new_output]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    let output = output_containing(&tree, container_id, None);
    assert_eq!(output, Some(new_output.as_str()));

    // Combination coverage on the same output: --output+--split and
    // --output+--fullscreen, each combined in one invocation.
    //
    // output+split: --split v on a window that's solo on the new output
    // (build_actions() runs Output before Split, so by the time Split runs
    // the window is already there) should configure *that* output's
    // workspace layout, not wherever the window transiently existed before
    // the move -- confirmed by a second window landing stacked below it.
    let (split_id, _split_guard) = launch_foot(&["--output", &new_output, "--split", "v"]);
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree, split_id, None),
        Some(new_output.as_str()),
        "the --split-combined window should still land on the requested output"
    );
    let (stacked_id, _stacked_guard) = launch_foot(&["--output", &new_output]);
    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let split_rect = find_node(&tree, split_id)
        .expect("split window should be in the tree")
        .rect;
    let stacked_rect = find_node(&tree, stacked_id)
        .expect("stacked window should be in the tree")
        .rect;
    assert_eq!(
        split_rect.x, stacked_rect.x,
        "stacked windows on the new output should share the same x"
    );
    assert_ne!(
        split_rect.y, stacked_rect.y,
        "stacked windows on the new output should have different y"
    );

    // output+fullscreen: combined in one invocation, the window should end
    // up both on the requested output and fullscreen.
    let (fullscreen_id, _fullscreen_guard) =
        launch_foot(&["--output", &new_output, "--fullscreen"]);
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree, fullscreen_id, None),
        Some(new_output.as_str()),
        "the --fullscreen-combined window should still land on the requested output"
    );
    let fullscreen_node = get_node(&mut connection, fullscreen_id);
    assert_ne!(
        fullscreen_node.fullscreen_mode,
        Some(0),
        "the --output-combined window should also be fullscreen"
    );
}

#[test]
fn workspace_is_a_no_op_when_already_on_the_target_workspace() {
    fresh_workspace();
    // Regression test: Sway doesn't fire WindowChange::Move for a "move
    // workspace" that doesn't actually change anything, so without
    // run_batch()'s already-in-effect short-circuit this would hang until
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
    fresh_workspace();
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
fn output_brings_a_window_off_a_hidden_workspace_of_that_output() {
    let home = fresh_workspace();
    // Sway's `move container to output` puts a window on the output's
    // visible workspace, even when it already sits on a hidden workspace of
    // the same output. --output used to count "anywhere on the output" as
    // already satisfied, sent nothing and left the window hidden.
    let mut connection = connect();
    run_command_ok(&mut connection, "workspace live-output-hidden");
    let (container_id, _guard) = launch_foot(&[]);
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let output_name = output_containing(&tree, container_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--output",
            &output_name,
            "--timeout",
            "5",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --output failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("\"status\":\"changed\""),
        "the move should have been sent: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some(home.as_str()),
        "the window should now be on the output's visible workspace"
    );
}

#[test]
fn floating_is_a_no_op_when_already_floating() {
    fresh_workspace();
    // Regression test found during a bug hunt prompted by this crate's own
    // pattern: Workspace/Output/NewColumn/NewRow all turned out to have a
    // no-op case Sway doesn't fire an event for, so the same was checked
    // for Floating/Fullscreen/Focus too — confirmed live (manual testing
    // during development: re-running --floating on an already-floating
    // window hung the full 5s --timeout and then errored, before
    // the already-in-effect check covered Floating). Without
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
fn floating_still_succeeds_when_another_client_wins_the_race() {
    fresh_workspace();
    // Regression test for a TOCTOU race between the already-in-effect no-op
    // check and the subscription that waits for the confirming event. When the
    // check ran first, another client satisfying the state in between left this
    // invocation's own command a no-op — which Sway fires no window event for
    // (the very reason the check exists) — so it waited out the full
    // --timeout and then errored, having asked for a state that was in effect
    // the whole time.
    //
    // Subscribing before the check closes the window rather than narrowing it,
    // so this passes for every offset rather than most of them: a change
    // landing before the check is seen by the check, and one landing after it
    // is seen by the subscription.
    //
    // The offsets sweep the window rather than targeting it, since where it
    // falls is a function of process startup on the machine running this.
    // Measured against the pre-fix binary on a headless compositor, a 0-20ms
    // sweep hung 5 of 40 trials, clustered 2.5-4ms after spawn; the fixed
    // binary hung 0 of 160 over the same sweep.
    const TRIALS: u64 = 24;

    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    for trial in 0..TRIALS {
        connection
            .run_command(format!("[con_id={}] floating disable", container_id))
            .expect("failed to reset the window to tiled");
        wait_until_not_floating(&mut connection, container_id);

        let started = Instant::now();
        let child = sway_launch_command()
            .args([
                "--con-id",
                &container_id.to_string(),
                "--floating",
                "--timeout",
                "5",
            ])
            .spawn()
            .expect("failed to run sway-launch binary");

        std::thread::sleep(Duration::from_micros(500 * trial));
        connection
            .run_command(format!("[con_id={}] floating enable", container_id))
            .expect("failed to float the window from the competing client");

        let output = child
            .wait_with_output()
            .expect("failed to wait for sway-launch");
        assert!(
            output.status.success(),
            "--floating failed when another client floated the window {}us after spawn: {}",
            500 * trial,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "--floating took {:?} when another client floated the window {}us after spawn, \
             suggesting it waited on an event Sway never fired for the resulting no-op",
            started.elapsed(),
            500 * trial
        );
    }
}

fn wait_until_not_floating(connection: &mut Connection, con_id: i64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !node_is_floating(&get_node(connection, con_id)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("container {} never returned to a tiled state", con_id);
}

#[test]
fn fullscreen_is_a_no_op_when_already_fullscreen() {
    fresh_workspace();
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
    fresh_workspace();
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
fn scratchpad_moves_a_tiled_window_to_the_scratchpad() {
    fresh_workspace();
    // Also proves the WindowChange::Move confirmation actually fires despite
    // Sway auto-floating a still-tiled window as part of the move (see
    // SwayAction::matching_window_change_event()'s Scratchpad doc comment)
    // — a WindowChange::Floating event fires first, and this only passes if
    // that's correctly skipped rather than mistaken for the confirmation.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args(["--con-id", &container_id.to_string(), "--scratchpad"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --scratchpad failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = connection.get_tree().expect("get_tree should succeed");
    let workspace = workspace_containing(&tree, container_id, None);
    assert_eq!(
        workspace,
        Some("__i3_scratch"),
        "expected the window to be in Sway's scratchpad workspace, got ancestor workspace {:?}",
        workspace
    );
}

#[test]
fn scratchpad_is_a_no_op_when_already_in_the_scratchpad() {
    fresh_workspace();
    // Same as floating_is_a_no_op_when_already_floating, for --scratchpad:
    // re-running "move scratchpad" on a window already there fires no event
    // at all, confirmed live, so run_batch()'s already-in-effect check for
    // Scratchpad must short-circuit rather than hang the full --timeout.
    let (container_id, _guard) = launch_foot(&["--scratchpad"]);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--scratchpad",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --scratchpad (already in the scratchpad) failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--scratchpad on an already-scratchpadded window took {:?}, suggesting it hung \
         waiting for an event Sway doesn't fire for a no-op",
        started.elapsed()
    );
}

#[test]
fn scratchpad_hides_a_window_currently_shown_from_the_scratchpad() {
    fresh_workspace();
    // Regression test: a window brought out with `scratchpad show` keeps
    // scratchpad_state "fresh" on Sway 1.11 while sitting on an ordinary
    // workspace, which used to make --scratchpad short-circuit as
    // already_satisfied and leave it visible — the README's dropdown-terminal
    // example did nothing in exactly the case it exists for.
    let mut connection = connect();
    let mark = "live-sway-test-scratchpad-shown";
    let (container_id, _guard) = launch_foot(&["--mark", mark, "--scratchpad"]);

    for outcome in connection
        .run_command(format!("[con_id={container_id}] scratchpad show"))
        .expect("scratchpad show should succeed")
    {
        outcome.expect("scratchpad show should apply");
    }
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_ne!(
        workspace_containing(&tree, container_id, None),
        Some("__i3_scratch"),
        "scratchpad show should have brought the window onto a normal workspace"
    );

    let output = sway_launch_command()
        .args([
            "--json",
            "--existing",
            "--mark-match",
            mark,
            "--scratchpad",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --scratchpad on a shown scratchpad window failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#"{"command":"move scratchpad","kind":"scratchpad","status":"changed"}"#),
        "expected move scratchpad to report changed, got: {stdout}"
    );

    let tree = connection.get_tree().expect("get_tree should succeed");
    let workspace = workspace_containing(&tree, container_id, None);
    assert_eq!(
        workspace,
        Some("__i3_scratch"),
        "expected the window back in Sway's scratchpad workspace, got {:?}",
        workspace
    );
}

#[test]
fn existing_matches_a_window_in_the_scratchpad() {
    fresh_workspace();
    // README.md/CLAUDE.md document that --existing's tree walk reaches
    // Sway's __i3_scratch scratchpad — a hidden window is just as eligible a
    // match as a visible one.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--scratchpad"]);

    let output = sway_launch_command()
        .args([
            "--existing",
            "--app-id",
            "foot",
            "--mark",
            "live-sway-test-scratchpad-existing",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --existing failed to match a scratchpad window: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let node = get_node(&mut connection, container_id);
    assert!(node
        .marks
        .contains(&"live-sway-test-scratchpad-existing".to_string()));
}

#[test]
fn new_column_does_not_relocate_a_solo_window_to_a_different_output() {
    fresh_workspace();
    // Regression test for the multi-monitor NewColumn/NewRow escalation
    // documented in SwayAction::matching_window_change_event()'s reasoning
    // comment: "move right" on a window with no sibling to move past within
    // its workspace can otherwise relocate the whole workspace to the next
    // output in that direction, rather than a same-workspace no-op.
    // SwayLaunch::run() guards against this via current_move_skip_reason();
    // this proves the guard actually prevents the relocation. Also asserts
    // on --json's unified "actions" array reporting the skip (RunOutcome)
    // rather than adding a second, near-identical test. The output is a
    // dedicated, freshly created one rather than one an earlier test left
    // behind: the guard's escalation is directional (whether "move right"
    // actually crosses into another output depends on Sway's own output
    // arrangement and which output is focused), so only a known arrangement
    // reliably reproduces the scenario.
    let mut connection = connect();
    let _created_output = CreatedOutput::new(&mut connection);

    let (container_id, _guard) = launch_foot(&[]);
    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let output_before = output_containing(&tree_before, container_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--new-column",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree_after = connection.get_tree().expect("get_tree should succeed");
    let output_after = output_containing(&tree_after, container_id, None);
    assert_eq!(
        output_after,
        Some(output_before.as_str()),
        "a solo window's --new-column should not relocate it to a different output"
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    assert_eq!(
        json["actions"],
        serde_json::json!([{
            "kind": "new_column",
            "status": "skipped",
            "reason": "trailing_workspace_edge",
        }]),
        "expected the skipped new_column action reported, and nothing else: {json:?}"
    );
}

#[test]
fn new_column_does_not_relocate_a_non_solo_window_at_the_trailing_edge() {
    fresh_workspace();
    // Regression test for a gap found in the multi-output move guard
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
    let _created_output = CreatedOutput::new(&mut connection);

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

/// Runs `sway-launch --con-id <con_id> <flags> --json`, asserts it succeeded,
/// and returns its `"actions"` array.
fn json_actions_for(con_id: i64, flags: &[&str]) -> serde_json::Value {
    let con_id = con_id.to_string();
    let mut args = vec!["--con-id", con_id.as_str()];
    args.extend_from_slice(flags);
    args.push("--json");
    let output = sway_launch_command()
        .args(&args)
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    json["actions"].clone()
}

#[test]
fn new_column_and_new_row_skip_a_floating_window_instead_of_nudging_it() {
    fresh_workspace();
    // Sway's "move right"/"move down" on a floating window nudges it 10 px
    // and returns -- never a new column or row. The move guard used to let it
    // through (it only modelled tiled windows), and the rect change then read
    // as a confirmed "changed". Needs no second output: this skip applies on
    // any setup.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating"]);
    // Settled first: until the window's first layout transaction applies, its
    // rect includes the title bar, which would read as a move.
    let rect_before = get_settled_node(&mut connection, container_id).rect;

    let actions = json_actions_for(container_id, &["--new-column", "--new-row"]);

    assert_eq!(
        actions,
        serde_json::json!([
            { "kind": "new_column", "status": "skipped", "reason": "floating" },
            { "kind": "new_row", "status": "skipped", "reason": "floating" },
        ]),
        "a floating window's moves should be skipped: {actions:?}"
    );
    let rect_after = get_settled_node(&mut connection, container_id).rect;
    assert_eq!(
        (rect_after.x, rect_after.y),
        (rect_before.x, rect_before.y),
        "the floating window should not have been nudged"
    );
}

#[test]
fn new_column_skips_a_fullscreen_window_on_a_multi_output_setup() {
    fresh_workspace();
    // Sway sends a fullscreen window's "move right" straight to the next
    // output, whatever its tree position. The target here is a *leading*
    // sibling, which the trailing-edge walk alone lets through, so the skip
    // has to come from the fullscreen check -- the case a layout step that
    // retargets an earlier step's fullscreen window would hit.
    let mut connection = connect();
    let _created_output = CreatedOutput::new(&mut connection);

    let (first_id, _first_guard) = launch_foot(&[]);
    let (_second_id, _second_guard) = launch_foot(&[]);
    run_command_ok(
        &mut connection,
        &format!("[con_id={first_id}] fullscreen enable"),
    );
    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let output_before = output_containing(&tree_before, first_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let actions = json_actions_for(first_id, &["--new-column"]);

    assert_eq!(
        actions,
        serde_json::json!([
            { "kind": "new_column", "status": "skipped", "reason": "fullscreen" },
        ]),
        "a fullscreen window's --new-column should be skipped: {actions:?}"
    );
    let tree_after = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree_after, first_id, None),
        Some(output_before.as_str()),
        "a fullscreen window's --new-column should not move it to a different output"
    );
}

#[test]
fn new_row_is_judged_after_new_column_has_run() {
    fresh_workspace();
    // On a splitv workspace [A, W], W is the trailing child of a vertical
    // workspace -- so judged up front, --new-row looks like it would escalate
    // to another output. But --new-column runs first, and its "move right"
    // re-orients the workspace horizontally with W at the end, where "move
    // down" is a safe perpendicular move. The guard used to decide both moves
    // while planning, so it skipped --new-row against a tree that no longer
    // existed by the time it would have run.
    let mut connection = connect();
    let _created_output = CreatedOutput::new(&mut connection);

    let (first_id, _first_guard) = launch_foot(&[]);
    run_command_ok(&mut connection, &format!("[con_id={first_id}] splitv"));
    let (second_id, _second_guard) = launch_foot(&[]);
    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let workspace_node =
        parent_node(&tree_before, second_id).expect("the target should have a parent");
    assert_eq!(workspace_node.node_type, NodeType::Workspace);
    assert_eq!(workspace_node.layout, swayipc::NodeLayout::SplitV);
    assert_eq!(
        workspace_node.nodes.last().map(|node| node.id),
        Some(second_id),
        "expected the target to be the vertical workspace's trailing child"
    );
    let output_before = output_containing(&tree_before, second_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let actions = json_actions_for(second_id, &["--new-column", "--new-row"]);

    let statuses: Vec<&str> = actions
        .as_array()
        .expect("actions should be an array")
        .iter()
        .map(|action| action["status"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        statuses.len(),
        2,
        "expected one record per move: {actions:?}"
    );
    assert!(
        !statuses.contains(&"skipped"),
        "neither move should be skipped once each is judged against the tree it runs on: \
         {actions:?}"
    );
    let tree_after = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree_after, second_id, None),
        Some(output_before.as_str()),
        "neither move should have relocated the window to a different output"
    );
}

#[test]
fn new_column_does_not_relocate_a_nested_window_to_a_different_output() {
    fresh_workspace();
    // Covers both nested shapes, against the one output this test creates.
    //
    // A window that is the trailing child of an axis-matched sub-container
    // *with* a sibling: probed live, "move right" pops it out of the
    // sub-container onto the same output, so the guard must let it run.
    //
    // A window that is the *only* child of a sub-container sitting directly
    // on the workspace: Sway treats that singleton like a window at
    // workspace level (mimicking i3) and escalates to the next output, so
    // the guard must skip it. Checked for a `layout tabbed` wrapper and for
    // a `splitv` wrapper left with one child after its sibling closed.
    let mut connection = connect();
    let _created_output = CreatedOutput::new(&mut connection);

    let (_first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-nested"]);
    let (second_id, _second_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-nested"]);
    // Splitting a window that already has a sibling wraps it in a new,
    // nested split container one level up (see find_parent_layout()'s doc) --
    // splith here matches the workspace's own layout, nesting second_id in
    // the axis-matched worst case rather than the safer mismatched one.
    connection
        .run_command(format!("[con_id={second_id}] splith"))
        .expect("splith should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("splith should succeed");
    let (third_id, _third_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-nested"]);

    let tree_before = connection.get_tree().expect("get_tree should succeed");
    let output_before = output_containing(&tree_before, third_id, None)
        .expect("launched window should be found in the tree")
        .to_string();

    let output = sway_launch_command()
        .args(["--con-id", &third_id.to_string(), "--new-column"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --new-column failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree_after = connection.get_tree().expect("get_tree should succeed");
    let output_after = output_containing(&tree_after, third_id, None);
    assert_eq!(
        output_after,
        Some(output_before.as_str()),
        "a window nested one level deep, even as the trailing child of an axis-matched \
         sub-container, should not be relocated to a different output by --new-column"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    assert_eq!(
        stdout.trim(),
        third_id.to_string(),
        "the nested window's --new-column should have run, not been skipped"
    );

    let (tabbed_id, _tabbed_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-lone-tabbed"]);
    connection
        .run_command(format!("[con_id={tabbed_id}] layout tabbed"))
        .expect("layout tabbed should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("layout tabbed should succeed");
    assert_new_column_skips_a_lone_child(&mut connection, tabbed_id, swayipc::NodeLayout::Tabbed);

    let (closing_id, closing_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-lone-splitv"]);
    let (survivor_id, _survivor_guard) = launch_foot(&[
        "--workspace",
        "live-sway-test-new-column-lone-splitv",
        "--split",
        "v",
    ]);
    drop(closing_guard);
    wait_for_container_gone(&mut connection, closing_id);
    assert_new_column_skips_a_lone_child(&mut connection, survivor_id, swayipc::NodeLayout::SplitV);
}

#[test]
fn new_column_and_new_row_output_guard_still_applies_when_combined_with_output() {
    fresh_workspace();
    // Companion to the workspace case above: when --new-column/--new-row and
    // --output are combined in one invocation and the window is at the
    // trailing edge of an axis-matched, multi-output-eligible workspace,
    // the multi-output move guard should still skip the (redundant, and
    // in a single-output world dangerous) "move right"/"move down" exactly
    // as it does when --new-column/--new-row runs alone -- confirmed live
    // this combination doesn't bypass or double up on the existing guard.
    // Covers --new-row+--output in the same test, against the same output.
    let mut connection = connect();
    let created_output = CreatedOutput::new(&mut connection);
    let new_output = created_output.name().to_string();

    let (first_id, _first_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-column-output-combo"]);
    connection
        .run_command("workspace live-sway-test-new-column-output-combo")
        .expect("workspace switch should succeed");
    let tree_before_first = connection.get_tree().expect("get_tree should succeed");
    let origin_output = output_containing(&tree_before_first, first_id, None)
        .expect("first window should be found in the tree")
        .to_string();

    let (second_id, _second_guard) = launch_foot(&["--new-column", "--output", &new_output]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree, second_id, None),
        Some(new_output.as_str()),
        "the window should end up on the requested output, same as --output alone"
    );
    assert_eq!(
        output_containing(&tree, first_id, None),
        Some(origin_output.as_str()),
        "the first window should stay on its original output, undisturbed"
    );

    // Same combination, for --new-row, on a fresh origin workspace.
    let (third_id, _third_guard) =
        launch_foot(&["--workspace", "live-sway-test-new-row-output-combo"]);
    connection
        .run_command("workspace live-sway-test-new-row-output-combo")
        .expect("workspace switch should succeed");
    let tree_before_third = connection.get_tree().expect("get_tree should succeed");
    let third_origin_output = output_containing(&tree_before_third, third_id, None)
        .expect("third window should be found in the tree")
        .to_string();

    let (fourth_id, _fourth_guard) = launch_foot(&["--new-row", "--output", &new_output]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree, fourth_id, None),
        Some(new_output.as_str()),
        "the --new-row-combined window should end up on the requested output, same as --output alone"
    );
    assert_eq!(
        output_containing(&tree, third_id, None),
        Some(third_origin_output.as_str()),
        "the third window should stay on its original output, undisturbed"
    );
}

#[test]
fn fullscreen_enables_fullscreen_mode() {
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--fullscreen"]);

    let node = get_node(&mut connection, container_id);
    assert_ne!(node.fullscreen_mode, Some(0));
}

#[test]
fn fullscreen_combined_with_workspace_lands_on_the_target_workspace_fullscreen() {
    fresh_workspace();
    // Combination coverage: --workspace runs before --fullscreen in
    // build_actions()'s fixed order, so a step combining both should end up
    // both on the target workspace and fullscreen, not one or the other.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[
        "--workspace",
        "live-sway-test-fullscreen-workspace-combo",
        "--fullscreen",
    ]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some("live-sway-test-fullscreen-workspace-combo"),
        "the window should have moved to the target workspace"
    );
    let node = get_node(&mut connection, container_id);
    assert_ne!(
        node.fullscreen_mode,
        Some(0),
        "the workspace-combined window should also be fullscreen"
    );
}

#[test]
fn split_combined_with_workspace_configures_the_target_workspace_not_the_origin() {
    fresh_workspace();
    // Combination coverage: --workspace runs before --split in
    // build_actions()'s fixed order, so a step combining both should
    // configure the *target* workspace's own layout (via the solo-window
    // "sets the workspace's layout directly" rule — see find_parent_layout's
    // doc comment), not whatever origin workspace the window transiently
    // existed on before the move.
    let mut connection = connect();
    let (first_id, _first_guard) = launch_foot(&[
        "--workspace",
        "live-sway-test-workspace-split-combo",
        "--split",
        "v",
    ]);
    let (second_id, _second_guard) =
        launch_foot(&["--workspace", "live-sway-test-workspace-split-combo"]);
    std::thread::sleep(Duration::from_millis(300));

    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, second_id, None),
        Some("live-sway-test-workspace-split-combo"),
        "second window should land on the target workspace"
    );
    let first_rect = find_node(&tree, first_id)
        .expect("first window should be in the tree")
        .rect;
    let second_rect = find_node(&tree, second_id)
        .expect("second window should be in the tree")
        .rect;
    assert_eq!(
        first_rect.x, second_rect.x,
        "combining --workspace with --split v should stack the windows vertically \
         on the target workspace"
    );
    assert_ne!(
        first_rect.y, second_rect.y,
        "combining --workspace with --split v should stack the windows vertically \
         on the target workspace"
    );
}

#[test]
fn floating_centered_and_scratchpadded_together_lands_hidden_but_correctly_configured() {
    fresh_workspace();
    // Combination coverage: --scratchpad always runs last (after Floating/
    // Position/etc.), specifically so earlier actions like centering still
    // apply to a visible window before it's hidden away -- the classic
    // "dropdown terminal" pattern. This confirms centering and
    // scratchpad-ing don't race (e.g. --position center silently failing
    // once the window is already hidden).
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[
        "--floating",
        "--width",
        "400px",
        "--height",
        "300px",
        "--position",
        "center",
        "--scratchpad",
    ]);

    let tree = connection.get_tree().expect("get_tree should succeed");
    let workspace = workspace_containing(&tree, container_id, None);
    assert_eq!(
        workspace,
        Some("__i3_scratch"),
        "expected the window to be in Sway's scratchpad workspace, got ancestor workspace {:?}",
        workspace
    );
    let node = find_node(&tree, container_id).expect("window should be in the tree");
    assert!(
        node_is_floating(node),
        "the scratchpadded window should still be floating"
    );
}

#[test]
fn focus_focuses_a_previously_unfocused_window() {
    fresh_workspace();
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
    fresh_workspace();
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--floating", "--position", "100,200"]);

    // "move position" targets the decoration-inclusive frame (deco_rect),
    // not the content rect below the title bar (rect.y sits 25px lower than
    // the requested y with the default border style) — confirmed against
    // live Sway.
    let node = get_settled_node(&mut connection, container_id);
    assert_eq!(node.deco_rect.x, 100);
    assert_eq!(node.deco_rect.y, 200);
}

#[test]
fn position_places_negative_coordinates_relative_to_the_workspace() {
    let workspace = fresh_workspace();
    // --position is workspace-relative, as Sway's own `move position` is, so
    // a negative coordinate lands past the workspace's left/top edge rather
    // than on an output left of or above this one (#64). Outer gaps move the
    // workspace origin off the output corner, so the two readings differ.
    // Also covers clap's `allow_hyphen_values`, without which a value
    // starting with `-` is rejected as an unknown option.
    let mut connection = connect();
    let _gaps = OuterGaps::set(&mut connection, 40);
    let workspace_rect = workspace_rect(&mut connection, &workspace);
    let (container_id, _guard) =
        launch_foot(&["--floating", "--width", "400px", "--height", "300px"]);
    // No title bar, so rect's absolute origin is the frame's origin.
    run_command_ok(
        &mut connection,
        &format!("[con_id={container_id}] border pixel 2"),
    );

    let stdout = run_position_json(container_id, &["--position", "-30,-20"]);
    assert!(
        stdout
            .contains(r#""command":"move position -30 -20","kind":"position","status":"changed""#),
        "a negative position should report changed, got {stdout:?}"
    );
    let node = get_node(&mut connection, container_id);
    assert_eq!(
        (node.rect.x, node.rect.y),
        (workspace_rect.x - 30, workspace_rect.y - 20)
    );
}

#[test]
fn position_confirms_via_poll_for_a_floating_window() {
    fresh_workspace();
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism reaching Position: confirms via
    // SwayAction::poll_shows()'s position_shows() (a get_tree() poll
    // for deco_rect.x/deco_rect.y) rather than always
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

    let node = get_settled_node(&mut connection, container_id);
    assert_eq!(node.deco_rect.x, 300);
    assert_eq!(node.deco_rect.y, 400);
}

#[test]
fn position_confirms_via_poll_for_a_fullscreen_window() {
    fresh_workspace();
    // Regression test: confirmed live that a fullscreen window's deco_rect
    // stays {0, 0, 0, 0} permanently (stable across a multi-second sweep,
    // not a transient race), since Sway never computes decoration geometry
    // for a window with no border/titlebar to draw. position_shows()
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
    fresh_workspace();
    // Corrects an assumption docs/plan-poll-based-wait-time-actions.md
    // carried over from earlier conversation exploration: a tiled window
    // isn't a silent "move position" no-op the way Height/Width's
    // solo-window clamp is — Sway rejects the command outright ("Only
    // floating containers can be moved to an absolute position"), which
    // run_sway_command()'s `?` propagates as an error *before*
    // poll_shows()/poll_baseline() are ever reached. So there's no
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
    assert!(
        stderr
            .lines()
            .next_back()
            .unwrap_or_default()
            .starts_with("error: move position 300 400: "),
        "a failing action's error should lead with its own command: {stderr:?}"
    );
}

#[test]
fn con_id_with_actions_names_a_missing_id_and_a_non_window_before_acting() {
    let workspace = fresh_workspace();
    // A wrong --con-id used to fail inside its first action as "no longer
    // exists — window may have closed", sending the user after a window that
    // never existed, and a workspace id got Sway's bare "No matching node."
    // (#107). A bare --con-id with no actions stays unchecked, by design.
    let mut connection = connect();
    let workspace_id = connection
        .get_tree()
        .expect("get_tree should succeed")
        .iter()
        .find(|node| node.name.as_deref() == Some(workspace.as_str()))
        .expect("the test's workspace should be in the tree")
        .id;
    let missing_id = 2_000_000_000;

    for (target, expected) in [
        (
            missing_id,
            format!("error: no window with container id {missing_id}"),
        ),
        (
            workspace_id,
            format!("error: container {workspace_id} is a workspace, not a window"),
        ),
    ] {
        let output = sway_launch_command()
            .args(["--con-id", &target.to_string(), "--floating"])
            .output()
            .expect("failed to run sway-launch binary");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.lines().next_back().unwrap_or_default(),
            expected,
            "unexpected stderr: {stderr:?}"
        );
    }

    let output = sway_launch_command()
        .args(["--con-id", &missing_id.to_string()])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(output.status.success(), "a bare --con-id is not looked up");
}

#[test]
fn con_id_and_existing_target_an_already_open_window() {
    fresh_workspace();
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
fn rollback_on_error_kills_earlier_launched_windows_when_a_later_step_fails() {
    fresh_workspace();
    // Regression test for --rollback-on-error: without it, an earlier
    // step's real window stays open when a later step fails; with it,
    // run_steps() kills every window this invocation itself launched
    // before reporting the failure. The second step targets a con_id that
    // was never opened, so --mark against it fails reliably and immediately
    // -- via run_batch()'s own existence check, before any subscription is
    // opened (Sway's own "No matching node." would fail it too, on every
    // supported version) -- without launching anything of its own to roll
    // back.
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");

    let path = TempToml::write(
        "rollback",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("\"rolled_back\"") && !stderr.contains("\"rolled_back\":[]"),
        "expected a non-empty rolled_back list reported in the JSON error: {stderr:?}"
    );

    // No settle sleep: a kill is only a close request, so rollback waits for
    // each window to leave the tree before listing it under rolled_back. A
    // window reported rolled back must therefore already be gone the moment
    // the process exits.
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        count_app_id_windows(&tree, "foot"),
        0,
        "a window listed under rolled_back should already be gone: {stderr:?}"
    );
}

#[test]
fn rollback_on_error_closes_the_window_of_a_launch_a_signal_interrupted() {
    fresh_workspace();
    // #127: SIGINT while step 2's application is still starting. Rollback
    // used to close step 1's window and exit, and step 2's window then
    // mapped and stayed open; now the run waits briefly for it and closes it
    // too.
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");

    let path = TempToml::write(
        "rollback-interrupted",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[step]]\ncommand = \"sleep 1 && foot\"\napp_id = \"foot\"\n",
    );
    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    let started = std::time::Instant::now();
    while count_app_id_windows(
        &connection.get_tree().expect("get_tree should succeed"),
        "foot",
    ) < 1
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "step 1 never opened its window"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    let killed = Command::new("kill")
        .args(["-INT".to_string(), child.id().to_string()])
        .status()
        .expect("failed to run kill");
    assert!(killed.success());

    let output = child.wait_with_output().expect("sway-launch should exit");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(output.status.code(), Some(130), "{stderr:?}");
    let error: serde_json::Value =
        serde_json::from_str(stderr.lines().next_back().unwrap_or_default())
            .unwrap_or_else(|error| panic!("no JSON error on stderr ({error}): {stderr:?}"));
    assert_eq!(
        error["rolled_back"].as_array().map(Vec::len),
        Some(2),
        "both windows should be rolled back: {stderr:?}"
    );
    // Past the point the interrupted application would have mapped anyway.
    std::thread::sleep(Duration::from_millis(1500));
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        count_app_id_windows(&tree, "foot"),
        0,
        "no window should be left open: {stderr:?}"
    );
}

#[test]
fn rollback_on_error_handles_a_launched_window_that_already_closed_itself() {
    fresh_workspace();
    // Regression test: if step 1's launched window closes on its own before
    // a later step's failure triggers --rollback-on-error, rollback()'s own
    // kill_container() call against that already-gone container must not be
    // treated as fatal -- run.rs's rollback() doc comment already says a
    // kill failure (e.g. the window already closed) is logged and skipped,
    // not fatal, but this exact scenario had no dedicated test.
    //
    // Step 1 launches a foot window running a shell that exits after a
    // short sleep, closing the window on its own. Step 2 retargets that
    // same window (via target_id) with a NewColumn action and a large
    // wait_time. run_batch() checks the container exists twice, on either
    // side of that wait: step 2's window is still alive for the first,
    // fast-fail check, and reliably gone by the *second*, which is what fails
    // step 2 deterministically -- no race against an external kill needed.
    let path = TempToml::write(
        "rollback-exit",
        "[[step]]\n\
         id = \"app\"\n\
         command = \"foot -e sh -c 'sleep 0.3'\"\n\
         app_id = \"foot\"\n\n\
         [[step]]\n\
         target_id = \"app\"\n\
         new_column = true\n\
         wait_time = 1200\n",
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "sway-launch should fail once step 2 finds its target already closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("no longer exists"),
        "expected step 2's error to name the already-closed container: {stderr:?}"
    );
    // Rollback attempted to close the already-gone window as part of
    // --rollback-on-error, but there's nothing left to kill -- confirming
    // that attempt was logged and skipped (not fatal, didn't hang) rather
    // than counted as a successful rollback.
    assert!(
        stderr.contains("\"rolled_back\":[]"),
        "the already-closed window's own kill can't have succeeded: {stderr:?}"
    );
    // ...and is reported under its own field rather than vanishing. This is
    // the only way to reach rollback_failed at all -- it needs a step to have
    // genuinely launched a window (so it's eligible for rollback) whose kill
    // then fails, which can't happen without a real compositor.
    assert!(
        stderr.contains("\"rollback_failed\":["),
        "a kill that failed should be reported, not silently dropped: {stderr:?}"
    );
    // Neither open nor closed, so it must not be reported as still-open
    // partial progress either: with step 1's id moved into rollback_failed,
    // nothing is left for container_ids/containers to report and both are
    // omitted entirely.
    assert!(
        !stderr.contains("\"container_ids\":"),
        "an id whose rollback failed must not also be reported as open: {stderr:?}"
    );
}

#[test]
fn rollback_on_error_undoes_launches_not_mutations_to_existing_windows() {
    fresh_workspace();
    // Pins the public semantic README.md states: --rollback-on-error is
    // *window-launch* rollback, not transactional rollback. A layout that
    // mutates an already-open window and then fails leaves that mutation in
    // place -- there's no mechanism tracking or reversing individual
    // mutations, only whether this invocation created the window.
    //
    // A user could read "rollback" the wrong way. The behavior is
    // intentional and documented; this test keeps a future change from
    // quietly widening or narrowing it without saying so.
    let mut connection = connect();
    let (existing_id, _existing_guard) = launch_foot(&[]);

    let path = TempToml::write(
        "rollback-scope",
        &format!(
            "[[step]]\ncon_id = {existing_id}\nmark = \"survives-rollback\"\n\n\
             [[step]]\ncommand = \"foot --app-id=rollback-scope-probe\"\n\
             app_id = \"rollback-scope-probe\"\n\n\
             [[step]]\ncon_id = 999999999\nmark = \"x\"\n"
        ),
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad third step"
    );

    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");

    // Step 2's window was launched by this run, so rollback closed it.
    let launched = first_app_id_window(&tree, "rollback-scope-probe");
    let _launched_guard = launched.map(KillOnDrop);
    assert_eq!(
        launched, None,
        "the window this run launched should have been rolled back"
    );

    // Step 1's window was only retargeted, so it's still open -- and step 1's
    // mark is still on it. That mark is the mutation rollback deliberately
    // does not reverse.
    let existing = get_node(&mut connection, existing_id);
    assert!(
        existing
            .marks
            .iter()
            .any(|mark| mark == "survives-rollback"),
        "a mutation applied to an already-open window is not rolled back, but its mark is \
         gone: {:?}",
        existing.marks
    );
}

#[test]
fn rollback_on_error_kills_the_failing_step_s_own_launched_window() {
    fresh_workspace();
    // A step can launch its window and then fail on a later action of its
    // own -- here `position` on a still-tiled window, which Sway rejects. That
    // window used to be neither reported nor rolled back, since the error
    // path dropped its container id. Run twice: without the flag the window
    // stays open and is reported (as progress and as container_id), and with
    // it the window is closed and reported as rolled back instead.
    let mut connection = connect();
    let path = TempToml::write(
        "rollback-failing-step",
        "[[step]]\ncommand = \"foot --app-id=rollback-failing-step-probe\"\n\
         app_id = \"rollback-failing-step-probe\"\nposition = \"10,10\"\n",
    );

    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success(), "a tiled position should fail");
    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let left_open = first_app_id_window(&tree, "rollback-failing-step-probe")
        .expect("without --rollback-on-error the failing step's window stays open");
    let left_open_guard = KillOnDrop(left_open);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let error: serde_json::Value =
        serde_json::from_str(stderr.lines().next_back().unwrap_or_default())
            .unwrap_or_else(|_| panic!("the last stderr line should be JSON, got {stderr:?}"));
    assert_eq!(error["container_id"], serde_json::json!(left_open));
    assert_eq!(error["container_ids"], serde_json::json!([left_open]));
    drop(left_open_guard);
    std::thread::sleep(Duration::from_millis(300));

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success(), "a tiled position should fail");
    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let launched = first_app_id_window(&tree, "rollback-failing-step-probe");
    let _launched_guard = launched.map(KillOnDrop);
    assert_eq!(
        launched, None,
        "the failing step's own launched window should have been rolled back"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid utf8");
    let error: serde_json::Value =
        serde_json::from_str(stderr.lines().next_back().unwrap_or_default())
            .unwrap_or_else(|_| panic!("the last stderr line should be JSON, got {stderr:?}"));
    assert_eq!(
        error["rolled_back"].as_array().map(Vec::len),
        Some(1),
        "exactly the failing step's window should be rolled back: {error:?}"
    );
    assert!(
        error.get("container_id").is_none(),
        "a rolled-back window isn't reported as the open target: {error:?}"
    );
}

#[test]
fn rollback_on_error_leaves_a_window_it_only_adopted_open() {
    fresh_workspace();
    // --rollback-on-error kills windows, and a kill can't be undone, so it
    // must only ever kill a window this run can prove it launched.
    //
    // run_wait_matching_exec_event() deliberately falls back to a content
    // match (app_id/class) when no PID-marker-confirmed window appears, and
    // that fallback covers two different situations. Here it's the ambiguous
    // one: this invocation's own marked process (`sleep 30`) is still running,
    // and the matching window came from somewhere else entirely -- launched
    // directly below, standing in for another launcher, another sway-launch,
    // or the user opening something at the wrong moment. sway-launch adopts it
    // once PID_MARKER_FALLBACK_GRACE elapses, because it matches the requested
    // app_id, but it never confirmed the window was its own.
    //
    // Step 2 then fails (a con_id that was never opened), triggering rollback.
    // The adopted window must survive it. The other branch of the same
    // fallback -- the marked process being *gone*, the single-instance
    // browser/editor case -- is treated as launched and does get rolled back:
    // rollback_on_error_still_kills_a_window_matched_after_its_launcher_exited
    // is the same scenario with that one difference, and asserts the opposite.
    let mut connection = connect();
    let path = TempToml::write(
        "rollback-adopted",
        "[[step]]\ncommand = \"sleep 30\"\napp_id = \"adopt-probe\"\ntimeout = 20\n\n\
         [[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
            "--verbose",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");

    std::thread::sleep(Duration::from_millis(500));
    connection
        .run_command("exec foot --app-id=adopt-probe")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");

    let tree = connection.get_tree().expect("get_tree should succeed");
    let adopted_id = first_app_id_window(&tree, "adopt-probe");
    let _guard = adopted_id.map(KillOnDrop);

    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        adopted_id.is_some(),
        "--rollback-on-error killed a window this run only adopted: {stderr:?}"
    );
    assert!(
        stderr.contains("\"rolled_back\":[]"),
        "an adopted window must not be reported as rolled back: {stderr:?}"
    );
    // Not rolled back, so it's still open -- and still reported as partial
    // progress, so a caller that does want it closed knows its id.
    assert!(
        stderr.contains("\"container_ids\":["),
        "the adopted window should be reported as still-open progress: {stderr:?}"
    );
    // Being spared silently would leave a user with a window they didn't ask
    // to keep and no way to know why -- --verbose is the only signal, so it's
    // asserted rather than assumed.
    assert!(
        stderr.contains("adopted rather than confirmed as launched"),
        "--verbose should say why the window was left open: {stderr:?}"
    );
}

#[test]
fn rollback_on_error_still_kills_a_window_matched_after_its_launcher_exited() {
    fresh_workspace();
    // The other branch of the same fallback, and the reason ownership isn't
    // simply "was the PID marker confirmed": a single-instance application
    // hands the request to an already-running process and exits, so the window
    // that appears carries no marker of ours — but nothing marker-confirmed can
    // still be coming, and the window appeared because we asked for it. That's
    // this run's own window, and --rollback-on-error must still close it.
    //
    // Simulated the same way exec_falls_back_to_a_content_match_when_its_own_
    // process_already_exited does: step 1's command is `sleep 0.3` (runs long
    // enough to be seen carrying the marker, then exits without creating a
    // window), while an unmarked window appears shortly after, standing in for
    // the forwarding instance's. It has to be *seen* first: a marker that's
    // never observed is no evidence the process exited (see
    // rollback_on_error_leaves_a_window_opened_while_an_env_dropping_wrapper_waits).
    let mut connection = connect();
    let path = TempToml::write(
        "rollback-forwarded",
        "[[step]]\ncommand = \"sleep 0.3\"\napp_id = \"forward-probe\"\ntimeout = 20\n\n\
         [[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    connection
        .run_command("exec sh -c 'sleep 1; exec foot --app-id=forward-probe'")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");
    // Start times are compared in clock ticks (10ms), and a tie reads as a
    // process started after the launch, so the stand-in instance must be
    // running a few ticks before step 1's command starts.
    std::thread::sleep(Duration::from_millis(50));

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("\"rolled_back\":[") && !stderr.contains("\"rolled_back\":[]"),
        "a window matched after its launcher exited is this run's own and should have been \
         rolled back: {stderr:?}"
    );

    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let survivor = first_app_id_window(&tree, "forward-probe");
    let _guard = survivor.map(KillOnDrop);
    assert_eq!(
        survivor, None,
        "the window should have been closed by --rollback-on-error"
    );
}

#[test]
fn rollback_on_error_leaves_a_window_the_user_opened_while_a_failed_launch_exited() {
    fresh_workspace();
    // #124: the same "seen, then gone" as the test above, but here the
    // launched command fails (foot refuses an unknown flag) and the matching
    // window is one the user opens a second in, from a process started after
    // the launch. It can't be the instance a hand-off went to, so it is only
    // adopted, and --rollback-on-error must leave it open.
    let mut connection = connect();
    let path = TempToml::write(
        "rollback-failed-launch",
        "[[step]]\ncommand = \"sleep 0.3 && foot --no-such-flag\"\napp_id = \"failed-probe\"\n\
         timeout = 20\n\n[[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
            "--verbose",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");

    std::thread::sleep(Duration::from_millis(1000));
    connection
        .run_command("exec foot --app-id=failed-probe")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");

    let tree = connection.get_tree().expect("get_tree should succeed");
    let users_window = first_app_id_window(&tree, "failed-probe");
    let _guard = users_window.map(KillOnDrop);

    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        users_window.is_some(),
        "--rollback-on-error closed a window the user opened: {stderr:?}"
    );
    assert!(
        stderr.contains("\"rolled_back\":[]"),
        "nothing this run can prove it launched should be rolled back: {stderr:?}"
    );
    assert!(
        stderr.contains("its process started after the launch"),
        "--verbose should say why the window was only adopted: {stderr:?}"
    );
}

#[test]
fn a_hand_off_whose_launcher_exits_after_the_window_maps_is_launched_promptly() {
    fresh_workspace();
    // The single-instance hand-off where the forwarding process outlives the
    // window's arrival: the window maps without our marker while the marked
    // launcher is still running, and the launcher exits 0.6s later. The
    // marker is re-scanned on a timer, not only when another window event
    // arrives, so the run ends once the launcher is gone -- well before the
    // 2s fallback grace -- and the window counts as this run's own, which
    // --rollback-on-error proves by closing it.
    let mut connection = connect();
    let path = TempToml::write(
        "handoff-launcher-outlives-window",
        "[[step]]\ncommand = 'sh -c \"env -u SWAY_LAUNCH_PID_MARKER foot \
         --app-id=handoff-probe & sleep 0.6\"'\napp_id = \"handoff-probe\"\ntimeout = 20\n\n\
         [[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let elapsed = started.elapsed();

    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("\"rolled_back\":[") && !stderr.contains("\"rolled_back\":[]"),
        "the handed-off window should count as launched and be rolled back: {stderr:?}"
    );
    assert!(
        elapsed < Duration::from_millis(1800),
        "took {elapsed:?} -- should have ended once the launcher exited (0.6s), not after \
         the 2s fallback grace"
    );

    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let survivor = first_app_id_window(&tree, "handoff-probe");
    let _guard = survivor.map(KillOnDrop);
    assert_eq!(
        survivor, None,
        "the window should have been closed by --rollback-on-error"
    );
}

#[test]
fn rollback_on_error_leaves_a_window_opened_while_an_env_dropping_wrapper_waits() {
    fresh_workspace();
    // A launcher that drops the environment (systemd-run, an app launcher, a
    // wrapper doing `env -u`) means the marker is never observed at all, while
    // the real program is still seconds away. "No process carries the marker"
    // used to be read as "the marked process exited", so the first matching
    // window from anywhere -- here a foreign one opened 0.3s in -- was claimed
    // as this run's own and killed by --rollback-on-error. A marker that was
    // never seen now counts for nothing: the fallback ends through the grace
    // period, as adopted, and the foreign window survives rollback.
    //
    // The marker is dropped with `unset` in the launching shell itself, not
    // `env -u`: an `env` process carries the marker until it execs, and a
    // scan catching that moment rightly reads "seen, then gone", which made
    // this test fail now and then. With `unset`, no process ever carries it.
    //
    // The program's own window would be recognised through its ancestry
    // (#96), so it starts 4s in, after the grace has already settled on the
    // foreign one: this is the case where nothing better arrives in time.
    // rollback_on_error_kills_a_wrappers_window_found_through_its_ancestry is
    // the same run with the program in time.
    let mut connection = connect();
    let path = TempToml::write(
        "rollback-env-dropping-wrapper",
        "[[step]]\ncommand = 'unset SWAY_LAUNCH_PID_MARKER && sh -c \"sleep 4 && exec foot \
         --app-id=wrap-probe -T mine sleep 60\"'\napp_id = \"wrap-probe\"\ntimeout = 20\n\n\
         [[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
            "--verbose",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");

    std::thread::sleep(Duration::from_millis(300));
    connection
        // `sleep` rather than a shell: a shell's prompt can retitle the
        // window (Arch's bash does), and windows are told apart by title.
        .run_command("exec foot --app-id=wrap-probe -T FOREIGN sleep 60")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");

    // Both windows (the foreign one and the wrapper's own) end up open; wait
    // for both so neither leaks into a later test.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tree = connection.get_tree().expect("get_tree should succeed");
    while count_app_id_windows(&tree, "wrap-probe") < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        tree = connection.get_tree().expect("get_tree should succeed");
    }
    let foreign_id = window_titled(&tree, "FOREIGN");
    let _foreign_guard = foreign_id.map(KillOnDrop);
    let _own_guard = window_titled(&tree, "mine").map(KillOnDrop);

    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let foreign_id = foreign_id.unwrap_or_else(|| {
        panic!("--rollback-on-error killed a foreign window it never saw launch: {stderr:?}")
    });
    assert!(
        stderr.contains("\"rolled_back\":[]"),
        "nothing this run can prove it launched should be rolled back: {stderr:?}"
    );
    assert!(
        stderr.contains(&format!("\"container_ids\":[{foreign_id}]")),
        "the foreign window arrived first, so it is the adopted fallback: {stderr:?}"
    );
    assert!(
        !stderr.contains("Marked process no longer running"),
        "a marker that was never seen must not be read as exited: {stderr:?}"
    );
}

#[test]
fn rollback_on_error_kills_a_wrappers_window_found_through_its_ancestry() {
    fresh_workspace();
    // #96: the same wrapper as the test above, which no process lets see the
    // marker, but with the program's window arriving before the grace runs
    // out. It descends from the shell Sway started for this launch, so it's
    // taken as launched over the foreign window held as the fallback, and
    // --rollback-on-error closes it. The foreign window is never this run's.
    let mut connection = connect();
    let path = TempToml::write(
        "rollback-ancestry",
        "[[step]]\ncommand = 'unset SWAY_LAUNCH_PID_MARKER && sh -c \"sleep 0.8 && exec foot \
         --app-id=ancestry-probe -T mine sleep 60\"'\napp_id = \"ancestry-probe\"\n\
         timeout = 20\n\n[[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );

    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
            "--verbose",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");

    std::thread::sleep(Duration::from_millis(300));
    connection
        .run_command("exec foot --app-id=ancestry-probe -T FOREIGN sleep 60")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");

    let tree = connection.get_tree().expect("get_tree should succeed");
    let foreign_id = window_titled(&tree, "FOREIGN");
    let _foreign_guard = foreign_id.map(KillOnDrop);
    let own_left_open = window_titled(&tree, "mine");
    let _own_guard = own_left_open.map(KillOnDrop);

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "sway-launch should fail on the bad second step"
    );
    assert!(
        stderr.contains("(PID-marker-confirmed through its ancestor process"),
        "the wrapper's window should be recognised through its ancestry: {stderr:?}"
    );
    assert!(
        foreign_id.is_some(),
        "--rollback-on-error killed a foreign window: {stderr:?}"
    );
    assert_eq!(
        own_left_open, None,
        "the launched window should have been rolled back: {stderr:?}"
    );
    assert!(
        !stderr.contains("\"rolled_back\":[]"),
        "the launched window should be listed as rolled back: {stderr:?}"
    );
}

#[test]
fn a_launch_through_an_exec_env_wrapper_is_recognised_at_once() {
    fresh_workspace();
    // #96: `exec env -u` leaves no process carrying the marker in its
    // environment at all. The window is found through the shell Sway started
    // for the launch, by its command line, instead of waiting out the 2s
    // grace and being adopted.
    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--verbose",
            "sh -c 'exec env -u SWAY_LAUNCH_PID_MARKER foot'",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");
    let container_id: i64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("stdout should be a container id");
    let _guard = KillOnDrop(container_id);

    assert!(
        stderr.contains("(PID-marker-confirmed through its ancestor process"),
        "{stderr}"
    );
    assert!(
        elapsed < Duration::from_millis(1800),
        "took {elapsed:?}, as if waiting out the fallback grace: {stderr}"
    );
}

fn window_titled(node: &Node, title: &str) -> Option<i64> {
    if node.pid.is_some() && node.name.as_deref() == Some(title) {
        return Some(node.id);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| window_titled(child, title))
}

#[test]
fn concurrent_template_runs_report_disjoint_container_ids() {
    fresh_workspace();
    // Two multi-step runs started together give each one many chances to see
    // the other's `New` window in the gap between sending `exec` and its own
    // marked process starting. With nothing yet carrying its marker, a run used
    // to read that as "my process already exited" and claim the other run's
    // window -- measured as a shared container id in 4 of 7 trials, with the
    // window each run actually launched for that slot reported by neither.
    // Each run's own marker-confirmed window now wins, so the two id lists
    // must be disjoint. Timing-dependent, so not a deterministic reproduction:
    // reinstating the old "absent means gone" rule passed all three trials
    // here once. It pins the invariant (no window claimed twice, none
    // unreported); the deterministic regression test for the rule itself is
    // rollback_on_error_leaves_a_window_opened_while_an_env_dropping_wrapper_waits.
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    connection
        .run_command("workspace live-sway-test-concurrent-templates")
        .expect("workspace switch should succeed");

    let run = || {
        sway_launch_command()
            .args([
                "--template",
                "quad-grid",
                "--apps",
                "foot,foot,foot,foot",
                "--timeout",
                "10",
                // Overlapping on purpose: the run lock (#55) would otherwise
                // refuse the second run before it launches anything.
                "--no-lock",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn sway-launch")
    };
    let ids_of = |output: std::process::Output| -> Vec<i64> {
        assert!(
            output.status.success(),
            "sway-launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.parse().expect("each line should be a container id"))
            .collect()
    };

    for trial in 1..=3 {
        let child_a = run();
        std::thread::sleep(Duration::from_millis(50));
        let child_b = run();
        let ids_a = ids_of(child_a.wait_with_output().expect("run A should exit"));
        let ids_b = ids_of(child_b.wait_with_output().expect("run B should exit"));
        let _guards: Vec<_> = ids_a
            .iter()
            .chain(&ids_b)
            .copied()
            .map(KillOnDrop)
            .collect();

        assert_eq!(ids_a.len(), 4, "trial {trial}: quad-grid has 4 slots");
        assert_eq!(ids_b.len(), 4, "trial {trial}: quad-grid has 4 slots");
        let shared: Vec<_> = ids_a.iter().filter(|id| ids_b.contains(id)).collect();
        assert!(
            shared.is_empty(),
            "trial {trial}: both runs claimed {shared:?} (A {ids_a:?}, B {ids_b:?})"
        );

        let tree = connection.get_tree().expect("get_tree should succeed");
        assert_eq!(
            count_app_id_windows(&tree, "foot"),
            8,
            "trial {trial}: every window opened should be reported by exactly one run"
        );
        drop(_guards);
        connection
            .run_command("[app_id=foot] kill")
            .expect("kill should succeed");
        std::thread::sleep(Duration::from_millis(300));
    }

    connection
        .run_command("workspace back_and_forth")
        .expect("workspace switch should succeed");
}

#[test]
fn two_templates_started_together_each_build_on_their_own_workspace() {
    fresh_workspace();
    // #156: focus is one value for the whole session, so two runs started
    // together (several `exec sway-launch --template ... --on-workspace N`
    // lines at login) used to take it from each other: all four windows
    // ended up interleaved on one workspace, the other left empty, and both
    // runs exited 0 (4 of 4 trials). A launching run now waits for the other
    // to finish, so each shape lands on its own workspace, the one its
    // recorded shape names.
    let mut connection = connect();
    let spawn = |template: &str| {
        sway_launch_command()
            .args([
                "--template",
                template,
                "--apps",
                "foot,foot",
                "--on-workspace",
                &format!("template-shape-{template}"),
                "--timeout",
                "20",
                "--json",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn sway-launch")
    };
    for trial in 1..=2 {
        let children = [
            ("dual-column", spawn("dual-column")),
            ("dual-row", spawn("dual-row")),
        ];
        let runs: Vec<(&str, std::process::Output)> = children
            .into_iter()
            .map(|(template, child)| {
                (
                    template,
                    child.wait_with_output().expect("the run should exit"),
                )
            })
            .collect();
        let parsed: Vec<(&str, serde_json::Value)> = runs
            .iter()
            .map(|(template, output)| {
                let parsed =
                    serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
                (*template, parsed)
            })
            .collect();
        let _guards: Vec<KillOnDrop> = parsed
            .iter()
            .flat_map(|(_, parsed)| {
                parsed["container_ids"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|id| id.as_i64())
            .map(KillOnDrop)
            .collect();
        for ((template, output), (_, parsed)) in runs.iter().zip(&parsed) {
            assert!(
                output.status.success(),
                "trial {trial}: {template} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let shape = rendered_shape_of(&mut connection, parsed);
            assert_eq!(
                Some(shape),
                recorded_template_shape(template),
                "trial {trial}: {template} should build its own shape on its own workspace"
            );
        }
    }
}

fn first_app_id_window(node: &Node, app_id: &str) -> Option<i64> {
    if node.app_id.as_deref() == Some(app_id) {
        return Some(node.id);
    }
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| first_app_id_window(child, app_id))
}

#[test]
fn layout_target_id_references_an_earlier_steps_real_window() {
    fresh_workspace();
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
    fresh_workspace();
    let mut connection = connect();
    let path = TempToml::write(
        "template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"first\"\n\n\
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
fn template_layout_context_moves_a_step_without_its_own_workspace_to_the_template_s_workspace() {
    fresh_workspace();
    // Proves the [layout] table's workspace reaches a real window: the run
    // switches there before the first step, so a step with no workspace of
    // its own opens on it.
    let mut connection = connect();
    let path = TempToml::write(
        "layout-context",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [layout]\nworkspace = \"live-sway-test-template-layout-context\"\n\n\
         [[step]]\nslot = \"window\"\n",
    );

    let output = sway_launch_command()
        .args(["--template", path.to_str().unwrap(), "--apps", "foot"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --template failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let container_id: i64 = stdout
        .trim()
        .parse()
        .expect("stdout should be a container id");
    let _guard = KillOnDrop(container_id);

    let tree = connection.get_tree().expect("get_tree should succeed");
    let workspace = workspace_containing(&tree, container_id, None);
    assert_eq!(workspace, Some("live-sway-test-template-layout-context"));
}

/// Runs `template` with one foot per slot (four for these tests) and returns
/// the `--json` output, asserting the run succeeded.
fn run_four_slot_template(template: &str, extra_args: &[&str]) -> serde_json::Value {
    let mut args = vec![
        "--template",
        template,
        "--apps",
        "foot,foot,foot,foot",
        "--timeout",
        "10",
        "--json",
    ];
    args.extend_from_slice(extra_args);
    let output = sway_launch_command()
        .args(&args)
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{template} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("--json output should be valid JSON")
}

/// The shape a `--json` run built, rendered the way the recorded
/// `tests/template-shapes/` files are, after killing its windows.
fn rendered_shape_of(connection: &mut Connection, parsed: &serde_json::Value) -> String {
    let slot_by_container: std::collections::HashMap<i64, String> = parsed["containers"]
        .as_object()
        .expect("--json output should name each slot's container")
        .iter()
        .map(|(slot, id)| (id.as_i64().expect("a container id"), slot.clone()))
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let shape = render_template_shape(&tree, &slot_by_container);
    for id in slot_by_container.keys() {
        let _ = connection.run_command(format!("[con_id={id}] kill"));
    }
    shape
}

#[test]
fn template_layout_workspace_builds_the_shape_in_place_on_that_workspace() {
    fresh_workspace();
    // A [layout] workspace used to be copied into every step as a move after
    // launch, so the shape was built on the old workspace and each window
    // then landed beside whatever the target had focused: quad-grid arrived
    // as a 1+1+2 shape while every action reported "changed". The run now
    // switches there first. Named after the shape test's own workspace, so
    // the result compares byte for byte with the recorded quad-grid shape.
    let mut connection = connect();
    let contents = std::fs::read_to_string(templates_dir().join("quad-grid.toml"))
        .expect("quad-grid.toml should be readable");
    let path = TempToml::write(
        "quad-grid-with-layout",
        &format!("{contents}\n[layout]\nworkspace = \"template-shape-quad-grid\"\n"),
    );

    let parsed = run_four_slot_template(path.to_str().unwrap(), &[]);
    assert!(parsed.get("warnings").is_none(), "{parsed}");
    let shape = rendered_shape_of(&mut connection, &parsed);
    assert_eq!(
        Some(shape),
        recorded_template_shape("quad-grid"),
        "quad-grid with a [layout] workspace should build the recorded grid there"
    );
}

#[test]
fn a_template_warns_about_an_occupied_workspace_and_on_workspace_builds_cleanly() {
    let home = fresh_workspace();
    // Run from a terminal, the Quickstart grid was built around that
    // terminal: a three-window top row. The run now says so, and
    // --on-workspace builds the shape the template describes elsewhere.
    let mut connection = connect();
    let (_terminal, _terminal_guard) = launch_foot(&[]);

    let crowded = run_four_slot_template("quad-grid", &[]);
    let warnings = crowded["warnings"].as_array().cloned().unwrap_or_default();
    assert_eq!(warnings.len(), 1, "{crowded}");
    assert!(
        warnings[0]
            .as_str()
            .is_some_and(|warning| warning.contains("already has 1 tiled window")),
        "{crowded}"
    );
    rendered_shape_of(&mut connection, &crowded);

    run_command_ok(&mut connection, &format!("workspace \"{home}\""));
    let clean =
        run_four_slot_template("quad-grid", &["--on-workspace", "template-shape-quad-grid"]);
    assert!(clean.get("warnings").is_none(), "{clean}");
    assert_eq!(
        Some(rendered_shape_of(&mut connection, &clean)),
        recorded_template_shape("quad-grid")
    );
}

#[test]
fn a_launch_whose_window_opens_on_another_workspace_fails_its_step() {
    let home = fresh_workspace();
    // A workspace switch while a step's application starts used to split
    // the layout across two workspaces with nothing reported: Sway still
    // puts that step's window where its exec ran, but every later step is
    // launched on the new workspace. Here step 2's own command switches
    // workspace before its window maps, the same race made deterministic.
    // The step fails, naming both workspaces, and --rollback-on-error closes
    // both windows.
    let mut connection = connect();
    let path = TempToml::write(
        "focus-moved-mid-run",
        "[[step]]\ncommand = \"foot --app-id=focus-moved-first\"\napp_id = \"focus-moved-first\"\n\n\
         [[step]]\ncommand = 'sh -c \"swaymsg -q workspace live-focus-moved-elsewhere && exec foot \
         --app-id=focus-moved-second\"'\napp_id = \"focus-moved-second\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));

    assert!(!output.status.success(), "the second step should fail");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let error_line = stderr.lines().last().unwrap_or_default();
    let error: serde_json::Value =
        serde_json::from_str(error_line).expect("the last stderr line should be the JSON error");
    let message = error["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("step 2:")
            && message.contains(&format!(
                "focus moved from workspace {home:?} to \"live-focus-moved-elsewhere\""
            )),
        "{message}"
    );
    assert_eq!(
        error["rolled_back"].as_array().map(Vec::len),
        Some(2),
        "both launched windows should be rolled back: {error}"
    );
}

#[test]
fn a_layout_warns_when_config_rules_place_its_windows_elsewhere() {
    let home = fresh_workspace();
    // #160: a user's `assign`/`for_window` rules apply to the windows a
    // layout opens. The run used to report success with nothing said while
    // one window sat on another workspace and another floated; each is now
    // named in a warning. The app_ids are this test's own, so the rules
    // (which stay in the compositor) match nothing else.
    let mut connection = connect();
    run_command_ok(
        &mut connection,
        "assign [app_id=\"live-placement-assigned\"] workspace live-placement-assigned",
    );
    run_command_ok(
        &mut connection,
        "for_window [app_id=\"live-placement-floated\"] floating enable",
    );
    let path = TempToml::write(
        "placement-rules",
        "[[step]]\nid = \"assigned\"\ncommand = \"foot --app-id=live-placement-assigned\"\n\
         app_id = \"live-placement-assigned\"\n\n\
         [[step]]\nid = \"floated\"\ncommand = \"foot --app-id=live-placement-floated\"\n\
         app_id = \"live-placement-floated\"\n\n\
         [[step]]\nid = \"floated-on-purpose\"\n\
         command = \"foot --app-id=live-placement-floated\"\n\
         app_id = \"live-placement-floated\"\nfloating = true\n\n\
         [[step]]\nid = \"plain\"\ncommand = \"foot --app-id=live-placement-plain\"\n\
         app_id = \"live-placement-plain\"\n",
    );

    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "--json output should be valid JSON: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let _guards: Vec<KillOnDrop> = parsed["container_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_i64)
        .map(KillOnDrop)
        .collect();
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let id = |slot: &str| parsed["containers"][slot].as_i64().expect("a container id");
    let warnings: Vec<&str> = parsed["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("no warnings in {parsed}"))
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(warnings.len(), 2, "{parsed}");
    assert!(
        warnings[0].contains(&format!(
            "step 1: container {} opened on workspace \"live-placement-assigned\", not on \
             workspace {home:?}",
            id("assigned")
        )),
        "{warnings:?}"
    );
    assert!(
        warnings[1].contains(&format!(
            "step 2: container {} opened floating",
            id("floated")
        )),
        "{warnings:?}"
    );
}

#[test]
fn new_column_and_new_row_complete_promptly_when_already_at_the_edge() {
    fresh_workspace();
    // Regression test for the bug this crate's own README/CLAUDE.md
    // describe: "move right"/"move down" don't fire WindowChange::Move when
    // the window is already at the tree's rightmost/bottommost position —
    // the ordinary two-window case — so the old event-confirmed dispatch
    // hung until --timeout (5s default) instead of completing. Asserting
    // "well under the default --timeout" is the proxy for "did not hang
    // waiting on an event that never fires". Asserting on --json's status
    // as well is what keeps it testing that path: with a second output to
    // the right, the relocation guard skips "move right" outright, which
    // finishes just as quickly without ever sending the command.
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
            "--json",
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
    assert_ne!(
        single_action_status(&output.stdout),
        "skipped",
        "--new-column should have sent its move, not been skipped by the relocation guard"
    );

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &second_id.to_string(),
            "--new-row",
            "--wait-time",
            "50",
            "--json",
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
    assert_ne!(
        single_action_status(&output.stdout),
        "skipped",
        "--new-row should have sent its move, not been skipped by the relocation guard"
    );
}

#[test]
fn new_column_confirms_via_poll_when_swapping_past_a_sibling() {
    fresh_workspace();
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism reaching NewColumn: confirms via
    // SwayAction::poll_shows()'s rect-snapshot comparison (see
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
            "--json",
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
    // Asserted directly rather than inferred from the timing above, because
    // this is what guards MOVE_POLL_GRACE against being tightened too far: a
    // real move has to still confirm inside that much shorter window, and
    // "unconfirmed" here is exactly what a too-small bound would produce.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\":\"changed\""),
        "a real sibling swap should confirm via poll, not fall back: {stdout:?}"
    );

    let after = get_node(&mut connection, first_id).rect;
    assert_ne!(
        before, after,
        "the leftmost window should have swapped position with its sibling"
    );
}

#[test]
fn new_row_confirms_via_poll_when_swapping_past_a_sibling() {
    fresh_workspace();
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
            "--json",
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
    // Same reasoning as the --new-column test above: this is what keeps
    // MOVE_POLL_GRACE honest against a real move.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\":\"changed\""),
        "a real sibling swap should confirm via poll, not fall back: {stdout:?}"
    );

    let after = get_node(&mut connection, first_id).rect;
    assert_ne!(
        before, after,
        "the topmost window should have swapped position with its sibling"
    );
}

#[test]
fn new_column_falls_back_gracefully_at_the_edge_with_a_large_wait_time() {
    fresh_workspace();
    // The documented "already at the edge" no-op (see
    // matching_window_change_event()'s comment in confirmation.rs) —
    // confirmed during this feature's development that Sway can still
    // incidentally restructure *other* siblings in this case (wrapping one
    // in a new split container) even though this window's own rect never
    // changes, which is exactly why poll_shows() compares the target
    // window's own rect rather than its parent's children list. Uses a
    // --wait-time much larger than
    // new_column_and_new_row_complete_promptly_when_already_at_the_edge's
    // 50ms, so a false-positive "confirmed" would be caught by the rect
    // assertion below even if the timing alone looked fine. Isolated via
    // --workspace so this window is genuinely at the tree's edge,
    // with no other window on its workspace to produce a misleading rect
    // delta unrelated to --new-column itself, confirmed while chasing down
    // this test's own first failed run during development.
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
fn new_column_does_not_credit_a_geometry_change_that_arrives_after_its_poll_window() {
    fresh_workspace();
    // The direction MOVE_POLL_GRACE (25ms) exists for, and the one the two
    // fast-path tests can't check: NewColumn/NewRow confirm on "this window's
    // rect differs from the snapshot", which cannot tell this action's own
    // move from another client's change to the same window. Narrowing the poll
    // window from 200ms to 25ms is what bounds how long that confusion is
    // possible, and nothing asserted the bound actually holds.
    //
    // The setup has to satisfy three things at once, which took some finding:
    // the move must be a geometric no-op (so any rect change is provably not
    // ours), the multi-output relocation guard must *not* skip the action (or
    // no poll happens at all -- it would whenever a second output sits to the
    // right, so this must keep holding if one is ever added here), and the
    // competing change must actually move the window. A solo window on a splitv workspace does it:
    // "move right" there restructures in place without changing the rect
    // (Sway re-orients the workspace; see is_at_the_trailing_workspace_edge()'s
    // doc), and the workspace's axis doesn't match the move, so
    // is_at_the_trailing_workspace_edge() returns false and the action runs.
    //
    // run_batch() sleeps --wait-time (2000ms) before sending the command,
    // so the poll window runs from ~2000ms to ~2025ms after spawn. The
    // competing change lands at ~2100ms: after the poll has stopped looking,
    // but comfortably inside the old 200ms window -- so this reports
    // "unconfirmed" where the pre-narrowing code would have reported "changed"
    // for a change it did not cause.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&[
        "--workspace",
        "live-sway-test-new-column-late-change",
        "--split",
        "v",
    ]);
    std::thread::sleep(Duration::from_millis(300));

    let before = get_node(&mut connection, container_id).rect;

    let child = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--new-column",
            "--wait-time",
            "2000",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");

    std::thread::sleep(Duration::from_millis(2100));
    // A gap change rather than a resize: a solo window's own resize is the
    // clamped no-op height_and_width_fall_back_gracefully_when_solo_window
    // _clamps_the_resize covers, so it would leave the rect untouched and make
    // this test vacuous. Reset below, since it applies compositor-wide and
    // every later test in this shared session would otherwise inherit it.
    connection
        .run_command("gaps inner all set 40")
        .expect("the competing gap change should be accepted")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("the competing gap change should succeed");

    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");

    // Read while the gap change is still in effect — the reset below restores
    // the original geometry, so reading after it would compare `before`
    // against itself.
    let after = get_node(&mut connection, container_id).rect;
    connection
        .run_command("gaps inner all set 0")
        .expect("resetting gaps should be accepted")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("resetting gaps should succeed");

    assert!(
        output.status.success(),
        "sway-launch --new-column failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The competing change has to have actually landed, or this test would
    // pass for the wrong reason -- "unconfirmed" only means something if there
    // was a change the poll could have wrongly credited.
    assert_ne!(
        before, after,
        "the competing gap change should have moved the window"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\":\"unconfirmed\""),
        "a change arriving after the poll window must not be credited to the move: {stdout:?}"
    );
}

#[test]
fn concurrent_exec_invocations_do_not_collide_on_the_same_container_id() {
    fresh_workspace();
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
    fresh_workspace();
    // Regression test for the other half of run_wait_matching_exec_event():
    // some applications (browsers, editors) are single-instance and forward
    // a second invocation's request to an already-running process before
    // exiting, so the window that eventually appears is legitimately the
    // right one, but owned by a PID that was never given sway-launch's PID
    // marker. Simulated deterministically here — no need for a real
    // single-instance app — via a delayed, unmarked "foot" launched
    // directly (standing in for the pre-existing instance's new window)
    // while sway-launch's own command is `sleep 0.3`, which is seen carrying
    // the marker and then exits without ever creating a window, exactly like
    // a forwarding process would.
    let mut connection = connect();
    connection
        .run_command("exec sh -c 'sleep 1; exec foot'")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let started = Instant::now();
    let output = sway_launch_command()
        .args(["--app-id", "foot", "--timeout", "10", "sleep 0.3"])
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
        "took {:?} — should have fallen back once its own process (seen, \
         then exited after 0.3s) was found not running, well before --timeout",
        started.elapsed()
    );

    let node = get_node(&mut connection, container_id);
    assert_eq!(node.app_id.as_deref(), Some("foot"));
}

#[test]
fn an_unfiltered_launch_matched_by_fallback_says_to_pass_a_filter() {
    fresh_workspace();
    // Without -a/-c any new window qualifies for the fallback, so an
    // unrelated one can be picked up; --verbose says so when it happens (#18).
    // Same setup as the filtered test above, minus the filter.
    let mut connection = connect();
    connection
        .run_command("exec sh -c 'sleep 1; exec foot'")
        .expect("exec should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("exec should succeed");

    let output = sway_launch_command()
        .args(["--verbose", "--timeout", "10", "sleep 0.3"])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "sway-launch failed: {stderr}");
    let container_id: i64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("stdout should be a container id");
    let _guard = KillOnDrop(container_id);
    assert!(
        stderr.contains("pass -a/-c"),
        "an unfiltered fallback match should suggest a filter: {stderr}"
    );
}

#[test]
fn wait_time_action_errors_clearly_when_its_container_already_closed() {
    fresh_workspace();
    // A wait-time action (Split/NewColumn/NewRow/Height/Width/Position)
    // against a container that closed after it was resolved must fail and
    // name the container. Sway alone answers "No matching node." (on 1.9 and
    // 1.11 alike, confirmed live), which says neither; run_batch()'s
    // existence check is what's under test here, see
    // ensure_container_exists()'s doc comment.
    let mut connection = connect();
    let (container_id, guard) = launch_foot(&[]);
    connection
        .run_command(format!("[con_id={container_id}] kill"))
        .expect("kill should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("kill should succeed");
    std::mem::forget(guard); // already closed — nothing left for KillOnDrop to clean up
                             // Deterministic rather than incidental: this test only ever passed
                             // because run_batch() sleeps --wait-time *before* its own
                             // ensure_container_exists() check, which happened to give Sway long enough to
                             // process the kill. Waiting explicitly removes that dependency.
    wait_for_container_gone(&mut connection, container_id);

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

#[test]
fn wait_time_action_does_not_sleep_out_the_wait_before_noticing_its_container_is_gone() {
    fresh_workspace();
    // run_batch() sleeps --wait-time before sending its command, to let
    // other IPC clients finish theirs. That sleep used to come before the
    // ensure_container_exists() check, so an action whose target had already closed
    // waited out the entire wait before reporting a foregone answer -- with a
    // large --wait-time (the caller's own knob, deliberately unbounded) that
    // reads as a hang. The check now runs before the sleep too; the one after
    // it stays, for the different case of a window closing *during* the wait.
    let mut connection = connect();
    let (container_id, guard) = launch_foot(&[]);
    connection
        .run_command(format!("[con_id={container_id}] kill"))
        .expect("kill should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("kill should succeed");
    std::mem::forget(guard); // already closed — nothing left for KillOnDrop to clean up
    wait_for_container_gone(&mut connection, container_id);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--split",
            "h",
            "--wait-time",
            "8000",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "--split against an already-closed container should fail"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?} against an already-closed container with --wait-time 8000, so the wait was \
         slept out before the container was checked",
        started.elapsed()
    );
}

#[test]
fn event_confirmed_action_errors_clearly_when_its_container_already_closed() {
    fresh_workspace();
    // Companion to wait_time_action_errors_clearly_when_its_container_already_closed
    // above, for the other dispatch half. Until this check was added, only
    // the wait-time actions pre-checked that their container still exists;
    // an event-confirmed one (here --mark, which has no already-in-effect
    // arm) sent its command and then waited out the full --timeout for a
    // confirmation event that could never arrive, reporting "N sec timeout
    // reached" instead of naming the closed window. Sway's own reply ("No
    // matching node.", on every supported version) already fails the
    // command, so the assertion below is on the *message* and the time taken
    // rather than just a non-zero exit.
    let mut connection = connect();
    let (container_id, guard) = launch_foot(&[]);
    connection
        .run_command(format!("[con_id={container_id}] kill"))
        .expect("kill should succeed")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("kill should succeed");
    std::mem::forget(guard); // already closed — nothing left for KillOnDrop to clean up
                             // Without this the test races the compositor: if the container is still
                             // in the tree when sway-launch reads it, the guard under test passes and
                             // the command instead fails via Sway 1.11's own "No matching node."
                             // (which doesn't name the id) -- a correct failure, but not the one this
                             // test exists to pin.
    wait_for_container_gone(&mut connection, container_id);

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--mark",
            "live-sway-test-closed-container",
            "--timeout",
            "5",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "--mark against an already-closed container should fail, not silently no-op"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "--mark against a closed container took {:?}, suggesting it waited out --timeout for an \
         event that can never arrive rather than failing fast",
        started.elapsed()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&container_id.to_string()),
        "error should name the container id: {stderr:?}"
    );
}

#[test]
fn debug_events_exits_cleanly_when_its_reader_goes_away() {
    fresh_workspace();
    // The deterministic half of tests/json_output.rs's
    // a_closed_stdout_exits_cleanly_instead_of_panicking. --debug-events is
    // the one mode that writes until killed, so `sway-launch --debug-events
    // | head -5` is an ordinary way to use it -- and Rust ignores SIGPIPE,
    // so before this every such invocation ended in a panic and exit 101
    // once the reader left. Deterministic here (unlike the headless version)
    // because closing the read end while events keep arriving guarantees a
    // failed write rather than depending on the pipe buffer.
    let mut child = sway_launch_command()
        .arg("--debug-events")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch --debug-events");

    // Let the subscription establish, then close the read end and keep
    // generating events for it to fail writing.
    std::thread::sleep(Duration::from_millis(300));
    drop(child.stdout.take());
    let (_container_id, _guard) = launch_foot(&[]);

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().expect("try_wait should succeed") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("--debug-events did not exit within 10s of its reader going away");
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };

    let mut stderr = String::new();
    if let Some(mut handle) = child.stderr.take() {
        use std::io::Read;
        let _ = handle.read_to_string(&mut stderr);
    }
    assert!(
        !stderr.contains("panicked"),
        "--debug-events should treat a closed stdout as end-of-output, not panic: {stderr}"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "--debug-events should exit cleanly once its reader is gone: {stderr}"
    );
}

#[test]
fn debug_events_json_names_each_event_as_sway_does() {
    fresh_workspace();
    // --debug-events --json names each event by Sway's own type name (#191),
    // the name a script filters on. Checked here on a tick, which carries a
    // payload this test chooses, and a window opening. (barconfig_update,
    // the one name the variant's own spelling got wrong, needs a bar in the
    // config; the unit tests cover it.)
    let mut child = KillChildOnDrop(
        sway_launch_command()
            .args(["--debug-events", "--json"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn sway-launch --debug-events --json"),
    );
    let stdout = child.0.stdout.take().expect("stdout should be piped");
    let (lines_sender, lines) = mpsc::channel::<serde_json::Value>();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            let value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("not one JSON object per line: {error}: {line}"));
            if lines_sender.send(value).is_err() {
                break;
            }
        }
    });

    std::thread::sleep(Duration::from_millis(300));
    connect()
        .send_tick("debug-events-probe")
        .expect("send_tick should succeed");
    let (container_id, _guard) = launch_foot(&[]);

    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut saw_tick, mut saw_window) = (false, false);
    while !(saw_tick && saw_window) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = lines.recv_timeout(remaining).unwrap_or_else(|_| {
            panic!("no tick and window event seen (tick: {saw_tick}, window: {saw_window})")
        });
        saw_tick |= event["event"] == "tick" && event["payload"] == "debug-events-probe";
        saw_window |= event["event"] == "window" && event["container"]["id"] == container_id;
    }
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

/// Where each built-in template's recorded shape lives: one
/// `<template-name>.txt` per file under `templates/` (dual-output.toml
/// aside), compared against what the template actually builds by
/// every_shipped_template_resolves_and_launches_successfully.
fn template_shapes_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("template-shapes")
}

/// Setting this rewrites the recorded shapes from what the templates build
/// now, so an intended shape change lands as a reviewed diff to
/// tests/template-shapes/ instead of either failing or passing silently.
const UPDATE_TEMPLATE_SHAPES: &str = "UPDATE_TEMPLATE_SHAPES";

/// A tiled subtree reduced to what decides how it looks: each split's
/// layout and its children in order, each child's share of its parent as a
/// whole percentage, and each window as its template slot name. Container
/// ids and pixel sizes are left out, so the result is the same on any
/// output size and across runs.
enum TemplateShape {
    Window(String),
    Split(String, Vec<(i64, TemplateShape)>),
}

fn tiled_template_shape(
    node: &Node,
    slot_by_container: &std::collections::HashMap<i64, String>,
) -> Option<TemplateShape> {
    if node.nodes.is_empty() {
        if node.node_type == NodeType::Workspace {
            return None;
        }
        return Some(TemplateShape::Window(
            slot_by_container
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| format!("unexpected window {:?}", node.app_id)),
        ));
    }
    let mut children: Vec<(i64, TemplateShape)> = node
        .nodes
        .iter()
        .filter_map(|child| {
            let share = (child.percent.unwrap_or(1.0) * 100.0).round() as i64;
            tiled_template_shape(child, slot_by_container).map(|shape| (share, shape))
        })
        .collect();
    // A split holding a single child looks exactly like that child, and
    // whether Sway wraps a window in one is an implementation detail that
    // can differ between Sway releases, so it's collapsed away here.
    if children.len() == 1 {
        return children.pop().map(|(_, shape)| shape);
    }
    Some(TemplateShape::Split(
        format!("{:?}", node.layout).to_lowercase(),
        children,
    ))
}

fn render_tiled_template_shape(
    shape: &TemplateShape,
    share: Option<i64>,
    depth: usize,
    rendered: &mut String,
) {
    use std::fmt::Write;
    let indent = "  ".repeat(depth);
    let prefix = share.map(|share| format!("{share}% ")).unwrap_or_default();
    match shape {
        TemplateShape::Window(slot) => {
            let _ = writeln!(rendered, "{indent}{prefix}{slot}");
        }
        TemplateShape::Split(layout, children) => {
            let _ = writeln!(rendered, "{indent}{prefix}{layout}");
            for (child_share, child) in children {
                render_tiled_template_shape(child, Some(*child_share), depth + 1, rendered);
            }
        }
    }
}

fn subtree_contains_any(node: &Node, ids: &std::collections::HashMap<i64, String>) -> bool {
    ids.contains_key(&node.id)
        || node
            .nodes
            .iter()
            .chain(node.floating_nodes.iter())
            .any(|child| subtree_contains_any(child, ids))
}

fn collect_workspaces<'a>(node: &'a Node, workspaces: &mut Vec<&'a Node>) {
    if node.node_type == NodeType::Workspace {
        workspaces.push(node);
        return;
    }
    for child in &node.nodes {
        collect_workspaces(child, workspaces);
    }
}

/// Renders every workspace holding one of a template's windows (usually
/// just the one it ran on; workspace-spread.toml uses three) as an indented
/// outline: the tiled tree per `TemplateShape`, then each floating window by
/// slot name alone. A floating window's geometry is in pixels and depends on
/// the output's size and the titlebar font, both of which differ between
/// environments, so it's left to the dedicated floating/width/height/position
/// tests rather than recorded here.
fn render_template_shape(
    tree: &Node,
    slot_by_container: &std::collections::HashMap<i64, String>,
) -> String {
    use std::fmt::Write;
    let mut workspaces = Vec::new();
    collect_workspaces(tree, &mut workspaces);
    workspaces.retain(|workspace| subtree_contains_any(workspace, slot_by_container));
    workspaces.sort_by(|left, right| left.name.cmp(&right.name));

    let mut rendered = String::new();
    for workspace in workspaces {
        let _ = writeln!(
            rendered,
            "workspace {}",
            workspace.name.as_deref().unwrap_or("?")
        );
        if let Some(shape) = tiled_template_shape(workspace, slot_by_container) {
            render_tiled_template_shape(&shape, None, 1, &mut rendered);
        }
        for floating in &workspace.floating_nodes {
            let slot = slot_by_container
                .get(&floating.id)
                .cloned()
                .unwrap_or_else(|| format!("unexpected window {:?}", floating.app_id));
            let _ = writeln!(rendered, "  floating {slot}");
        }
    }
    rendered
}

/// Runs a template (a built-in name or a `.toml` path) with one `foot` per
/// slot, plus `extra_args`, on a fresh workspace of its own and returns the
/// shape it built, per
/// `render_template_shape()`. Asserts the run succeeded and printed every
/// step's container id, as the plain launch check this replaced did.
fn launch_template_and_render_shape(
    connection: &mut Connection,
    template: &str,
    name: &str,
    slot_count: usize,
    step_count: usize,
    extra_args: &[&str],
) -> String {
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    connection
        .run_command(format!("workspace template-shape-{name}"))
        .expect("workspace switch should succeed");

    let apps = vec!["foot"; slot_count].join(",");
    let output = sway_launch_command()
        .args([
            "--template",
            template,
            "--apps",
            &apps,
            "--timeout",
            "10",
            "--json",
        ])
        .args(extra_args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run sway-launch for {template}: {error}"));
    assert!(
        output.status.success(),
        "{template} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{template} printed invalid --json output: {error}"));
    // A relative move that doesn't change the window's geometry is a dead
    // step: it reports "unconfirmed" on one output, and leans on the
    // trailing-edge guard ("skipped") on several. A shape snapshot can't
    // catch one, since the tree is identical with or without it.
    let dead_moves: Vec<_> = parsed["actions"]
        .as_array()
        .unwrap_or_else(|| panic!("{template} --json output lacks actions: {parsed}"))
        .iter()
        .filter(|action| matches!(action["kind"].as_str(), Some("new_column" | "new_row")))
        .filter(|action| action["status"] != "changed")
        .collect();
    assert!(
        dead_moves.is_empty(),
        "{template} has new_column/new_row steps that moved nothing: {dead_moves:?}"
    );
    // Total step count (slot steps + target_id steps, e.g.
    // retarget-by-slot.toml's retarget step) is how many container ids
    // --apps reports — --apps only sizes the *slot* count.
    let container_ids = parsed["container_ids"]
        .as_array()
        .unwrap_or_else(|| panic!("{template} --json output lacks container_ids: {parsed}"));
    assert_eq!(
        container_ids.len(),
        step_count,
        "{template} should report one container id per step, got {parsed}"
    );
    let slot_by_container: std::collections::HashMap<i64, String> = parsed["containers"]
        .as_object()
        .unwrap_or_else(|| panic!("{template} --json output lacks containers: {parsed}"))
        .iter()
        .map(|(slot, id)| {
            let id = id
                .as_i64()
                .unwrap_or_else(|| panic!("{template} reported a non-id for {slot}: {id}"));
            (id, slot.clone())
        })
        .collect();
    assert_eq!(
        slot_by_container.len(),
        slot_count,
        "{template} should name one container per slot, got {parsed}"
    );

    // See quad_terminals_layout_launches_four_windows_in_a_grid's comment on
    // why geometry needs a moment to settle after sway-launch exits.
    std::thread::sleep(Duration::from_millis(300));
    let tree = connection.get_tree().expect("get_tree should succeed");
    let shape = render_template_shape(&tree, &slot_by_container);

    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    shape
}

fn recorded_template_shape(name: &str) -> Option<String> {
    std::fs::read_to_string(template_shapes_dir().join(format!("{name}.txt"))).ok()
}

/// Every built-in template's slot names, keyed by template name, as
/// `--list-templates --json` reports them — the same slot-step order
/// `--apps` binds against, taken from the binary itself rather than worked
/// out again here from the template text.
fn builtin_slot_names() -> std::collections::HashMap<String, Vec<String>> {
    let output = sway_launch_command()
        .args(["--list-templates", "--json"])
        .output()
        .expect("failed to run sway-launch --list-templates --json");
    assert!(
        output.status.success(),
        "--list-templates --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("--list-templates --json should print valid JSON");
    parsed["templates"]
        .as_array()
        .expect("--list-templates --json should carry a templates array")
        .iter()
        .map(|template| {
            let name = template["name"]
                .as_str()
                .expect("every listed template should have a name")
                .to_string();
            let slots = template["slot_names"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} should list its slot_names"))
                .iter()
                .map(|slot| slot.as_str().expect("slot names are strings").to_string())
                .collect();
            (name, slots)
        })
        .collect()
}

/// How many `[[step]]` entries a template file declares, read by parsing
/// it as TOML rather than by matching its text.
fn template_step_count(path: &std::path::Path) -> usize {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"));
    let document: toml::Table = contents
        .parse()
        .unwrap_or_else(|error| panic!("{path:?} should be valid TOML: {error}"));
    document
        .get("step")
        .and_then(toml::Value::as_array)
        .map_or(0, Vec::len)
}

fn focused_workspace_name(connection: &mut Connection) -> String {
    connection
        .get_workspaces()
        .expect("get_workspaces should succeed")
        .into_iter()
        .find(|workspace| workspace.focused)
        .map(|workspace| workspace.name)
        .expect("some workspace should be focused")
}

#[test]
fn every_shipped_template_resolves_and_launches_successfully() {
    assert_every_shipped_template_builds_its_recorded_shape(&[]);
}

#[test]
fn every_shipped_template_builds_the_same_shape_with_concurrent() {
    // #73: launched all at once on a staging workspace, then moved into
    // place in step order, every template must build exactly the shape it
    // builds one launch at a time. Never rewrites the recordings: the
    // sequential run above owns them.
    assert_every_shipped_template_builds_its_recorded_shape(&["--concurrent"]);
}

/// The body of every_shipped_template_resolves_and_launches_successfully,
/// run with `extra_args` added to every template run.
fn assert_every_shipped_template_builds_its_recorded_shape(extra_args: &[&str]) {
    fresh_workspace();
    // Unlike template_apps_resolve_to_real_windows above (a hand-written,
    // minimal template), this drives the actual files under templates/ and
    // compares the shape each one builds — layout, child order, relative
    // sizes, which slot sits where — against tests/template-shapes/. Exiting
    // 0 alone says nothing about a template's shape: a reordered action, a
    // step that silently no-ops, or an edited split all still exit 0.
    // Rerun with UPDATE_TEMPLATE_SHAPES=1 to record an intended change.
    // dual-output.toml is excluded since it needs a second output and real
    // (non-placeholder) output names to run at all; see
    // dual_output_template_moves_windows_to_separate_outputs.
    let mut connection = connect();
    let original_workspace = focused_workspace_name(&mut connection);
    let updating = extra_args.is_empty() && std::env::var_os(UPDATE_TEMPLATE_SHAPES).is_some();
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

    let slot_names = builtin_slot_names();
    let mut checked_names = std::collections::HashSet::new();
    let mut mismatches = Vec::new();
    for path in &paths {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("template file names should be UTF-8")
            .to_string();
        let slot_count = slot_names
            .get(&name)
            .unwrap_or_else(|| panic!("{name} should be listed by --list-templates"))
            .len();
        assert!(slot_count > 0, "{path:?} should declare at least one slot");
        let step_count = template_step_count(path);

        let shape = launch_template_and_render_shape(
            &mut connection,
            path.to_str().unwrap(),
            &name,
            slot_count,
            step_count,
            extra_args,
        );
        if updating {
            std::fs::create_dir_all(template_shapes_dir())
                .expect("tests/template-shapes/ should be creatable");
            std::fs::write(template_shapes_dir().join(format!("{name}.txt")), &shape)
                .unwrap_or_else(|error| panic!("failed to record {name}'s shape: {error}"));
        } else {
            match recorded_template_shape(&name) {
                Some(recorded) if recorded == shape => {}
                Some(recorded) => mismatches.push(format!(
                    "{name} built a different shape than recorded.\n\
                     recorded:\n{recorded}built:\n{shape}"
                )),
                None => mismatches.push(format!(
                    "{name} has no recorded shape in tests/template-shapes/.\n\
                     built:\n{shape}"
                )),
            }
        }
        checked_names.insert(name);
    }

    connection
        .run_command(format!("workspace {original_workspace}"))
        .expect("workspace switch should succeed");

    let stale: Vec<_> = std::fs::read_dir(template_shapes_dir())
        .expect("tests/template-shapes/ should be readable")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            (!checked_names.contains(&stem)).then_some(path)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "recorded shapes with no matching template under templates/: {stale:?}"
    );
    assert!(
        mismatches.is_empty(),
        "{}\nIf a change is intended, rerun with {UPDATE_TEMPLATE_SHAPES}=1 and review the \
         diff to tests/template-shapes/.",
        mismatches.join("\n")
    );
}

/// Every window on `workspace` (tiled and floating) labelled "window", for
/// comparing two shapes whose windows have different names.
fn anonymous_shape(connection: &mut Connection, workspace: &str) -> String {
    let tree = connection.get_tree().expect("get_tree should succeed");
    let mut workspaces = Vec::new();
    collect_workspaces(&tree, &mut workspaces);
    let node = workspaces
        .into_iter()
        .find(|node| node.name.as_deref() == Some(workspace))
        .unwrap_or_else(|| panic!("no workspace {workspace:?}"));
    let windows = node
        .nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .flat_map(|child| {
            let mut ids = Vec::new();
            collect_window_ids(child, &mut ids);
            ids
        })
        .map(|id| (id, "window".to_string()))
        .collect();
    render_template_shape(&tree, &windows).replace(&format!("workspace {workspace}"), "workspace")
}

fn collect_window_ids(node: &Node, ids: &mut Vec<i64>) {
    if node.nodes.is_empty() && node.floating_nodes.is_empty() {
        ids.push(node.id);
    }
    for child in node.nodes.iter().chain(node.floating_nodes.iter()) {
        collect_window_ids(child, ids);
    }
}

/// Runs `sway-launch --capture <workspace>` and writes what it printed to a
/// temporary layout file.
fn capture_to_file(workspace: &str, name: &str) -> TempToml {
    let output = sway_launch_command()
        .args(["--capture", workspace])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "--capture {workspace} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    TempToml::write(name, &String::from_utf8_lossy(&output.stdout))
}

#[test]
fn every_shipped_template_round_trips_through_capture() {
    fresh_workspace();
    // #31: capturing what a template built and replaying the capture on a
    // fresh workspace must build the same shape: layouts, child order and
    // sizes. dual-output and workspace-spread span outputs or workspaces,
    // which a one-workspace capture can't hold.
    let mut connection = connect();
    let original_workspace = focused_workspace_name(&mut connection);
    let slot_names = builtin_slot_names();
    let mut paths: Vec<_> = std::fs::read_dir(templates_dir())
        .expect("templates/ should be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            !matches!(
                path.file_stem().and_then(|stem| stem.to_str()),
                Some("dual-output" | "workspace-spread")
            )
        })
        .collect();
    paths.sort();
    let mut mismatches = Vec::new();
    for path in &paths {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let slot_count = slot_names[&name].len();
        connection
            .run_command("[app_id=foot] kill")
            .expect("kill should succeed");
        let built = format!("capture-built-{name}");
        connection
            .run_command(format!("workspace {built}"))
            .expect("workspace switch should succeed");
        let output = sway_launch_command()
            .args([
                "--template",
                path.to_str().unwrap(),
                "--apps",
                &vec!["foot"; slot_count].join(","),
                "--timeout",
                "10",
            ])
            .output()
            .expect("failed to run sway-launch binary");
        assert!(output.status.success(), "{name} failed to build");
        let shape = anonymous_shape(&mut connection, &built);
        let file = capture_to_file(&built, &format!("capture-{name}"));

        connection
            .run_command("[app_id=foot] kill")
            .expect("kill should succeed");
        let replayed = format!("capture-replayed-{name}");
        connection
            .run_command(format!("workspace {replayed}"))
            .expect("workspace switch should succeed");
        let output = sway_launch_command()
            .args(["--layout", file.to_str().unwrap(), "--timeout", "10"])
            .output()
            .expect("failed to run sway-launch binary");
        assert!(
            output.status.success(),
            "{name}'s capture failed to replay: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            std::fs::read_to_string(file.to_str().unwrap()).unwrap_or_default()
        );
        let replayed_shape = anonymous_shape(&mut connection, &replayed);
        if replayed_shape != shape {
            mismatches.push(format!(
                "{name}: built\n{shape}replayed\n{replayed_shape}capture:\n{}",
                std::fs::read_to_string(file.to_str().unwrap()).unwrap_or_default()
            ));
        }
    }
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    connection
        .run_command(format!("workspace {original_workspace}"))
        .expect("workspace switch should succeed");
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn capture_keeps_a_floating_windows_geometry_and_mark() {
    fresh_workspace();
    let mut connection = connect();
    let built = focused_workspace_name(&mut connection);
    let (original, original_guard) = launch_foot(&[
        "--floating",
        "--width",
        "500px",
        "--height",
        "300px",
        "--position",
        "40,50",
        "--mark",
        "captured-float",
    ]);
    let before = get_settled_node(&mut connection, original);
    let file = capture_to_file(&built, "capture-floating");
    drop(original_guard);

    fresh_workspace();
    let output = sway_launch_command()
        .args(["--layout", file.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    let replayed = result["container_ids"][0].as_i64().expect("one window");
    let _guard = KillOnDrop(replayed);
    let after = get_settled_node(&mut connection, replayed);
    assert!(node_is_floating(&after));
    assert_eq!(after.marks, vec!["captured-float".to_string()]);
    assert_eq!(
        (
            after.rect.width,
            after.rect.height,
            after.deco_rect.x,
            after.deco_rect.y
        ),
        (
            before.rect.width,
            before.rect.height,
            before.deco_rect.x,
            before.deco_rect.y
        )
    );
}

/// Runs a layout file with `--json` on the focused workspace, asserting it
/// succeeded, and returns its windows' ids, each closed again on drop.
fn run_layout_for_ids(file: &TempToml) -> (Vec<i64>, Vec<KillOnDrop>) {
    let output = sway_launch_command()
        .args([
            "--layout",
            file.to_str().unwrap(),
            "--timeout",
            "10",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        std::fs::read_to_string(file.to_str().unwrap()).unwrap_or_default()
    );
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    let ids: Vec<i64> = result["container_ids"]
        .as_array()
        .expect("container_ids is a list")
        .iter()
        .map(|id| id.as_i64().expect("an id"))
        .collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    let guards = unique.into_iter().map(KillOnDrop).collect();
    (ids, guards)
}

#[test]
fn capture_rebuilds_a_workspace_holding_a_fullscreen_window() {
    let built = fresh_workspace();
    // #189: Sway leaves focus on a fullscreen window when another opens on
    // its workspace and places the new one next to it, and reports its
    // percent as 1.0. A capture that made the window fullscreen at launch, or
    // sized it from that percent, rebuilt one | [three / four] | two, with
    // nothing fullscreen at the end.
    let layout = TempToml::write(
        "capture-fullscreen-source",
        r#"
[[step]]
command = "foot"
app_id = "foot"
split = "h"

[[step]]
command = "foot"
app_id = "foot"

[[step]]
id = "three"
command = "foot"
app_id = "foot"

[[step]]
target_id = "three"
split = "v"
focus = true

[[step]]
command = "foot"
app_id = "foot"
"#,
    );
    let mut connection = connect();
    let (ids, built_guards) = run_layout_for_ids(&layout);
    let shape = anonymous_shape(&mut connection, &built);
    run_command_ok(
        &mut connection,
        &format!("[con_id={}] fullscreen enable", ids[0]),
    );
    let file = capture_to_file(&built, "capture-fullscreen");
    let captured = std::fs::read_to_string(file.to_str().unwrap()).unwrap_or_default();
    assert!(
        !captured.contains("ppt"),
        "an even split is left unsized:\n{captured}"
    );
    assert!(
        captured.trim_end().ends_with("fullscreen = true"),
        "fullscreen comes last:\n{captured}"
    );
    drop(built_guards);

    let replayed = fresh_workspace();
    let (replayed_ids, _replayed_guards) = run_layout_for_ids(&file);
    let first = get_settled_node(&mut connection, replayed_ids[0]);
    assert_eq!(first.fullscreen_mode, Some(1), "{captured}");
    run_command_ok(
        &mut connection,
        &format!("[con_id={}] fullscreen disable", replayed_ids[0]),
    );
    assert_eq!(
        anonymous_shape(&mut connection, &replayed),
        shape,
        "{captured}"
    );
}

#[test]
fn capture_refuses_a_split_container_floated_as_a_whole() {
    fresh_workspace();
    // #153: Sway floats a split container as one floating node holding
    // ordinary windows. A capture used to keep only its first window, tiled;
    // it must refuse instead, as it does a tabbed container.
    let mut connection = connect();
    let built = focused_workspace_name(&mut connection);
    let (_tiled, _tiled_guard) = launch_foot(&[]);
    let (_top, _top_guard) = launch_foot(&["--split", "v"]);
    let (bottom, _bottom_guard) = launch_foot(&[]);
    run_command_ok(&mut connection, &format!("[con_id={bottom}] focus"));
    run_command_ok(&mut connection, "focus parent");
    run_command_ok(&mut connection, "floating enable");
    let output = sway_launch_command()
        .args(["--capture", &built])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "captured a floating split:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("it holds a floating container of 2 windows"),
        "{stderr}"
    );
}

#[test]
fn capture_never_replays_an_app_id_as_shell_code() {
    fresh_workspace();
    // #125: an app_id is the client's to set (foot takes one from an escape
    // sequence anything printed in its terminal can send), so --capture must
    // not write one as a command string a replay hands to `sh`. Written as one
    // quoted word, it names a program that doesn't exist: the replay times out
    // and the payload never runs.
    let temp_dir = TestTempDir::new();
    let payload_file = temp_dir.join("capture-payload-ran");
    let hostile_app_id = format!("foot && touch {}", payload_file.display());
    let mut connection = connect();
    let built = focused_workspace_name(&mut connection);
    let output = sway_launch_command()
        .args([
            "--app-id",
            &hostile_app_id,
            "--timeout",
            "10",
            &format!("foot --app-id='{hostile_app_id}'"),
        ])
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
    let guard = KillOnDrop(container_id);
    let file = capture_to_file(&built, "capture-hostile-app-id");
    drop(guard);
    let captured = std::fs::read_to_string(file.to_str().unwrap()).unwrap_or_default();
    assert!(
        captured.contains(&format!("command = [\"{hostile_app_id}\"]")),
        "{captured}"
    );

    fresh_workspace();
    let output = sway_launch_command()
        .args(["--layout", file.to_str().unwrap(), "--timeout", "2"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        !output.status.success(),
        "no window can open for a program that doesn't exist"
    );
    assert!(
        !payload_file.exists(),
        "the captured app_id ran as shell code:\n{captured}"
    );
}

#[test]
fn template_params_build_the_shapes_of_the_ratio_variants_they_replaced() {
    fresh_workspace();
    // master-dual-stack-wide and dual-sidebars-wide were folded into a param
    // of their base template (#92); each override has to build exactly the
    // shape the deleted file did, recorded under tests/template-shapes/params/.
    let mut connection = connect();
    let original_workspace = focused_workspace_name(&mut connection);
    let updating = std::env::var_os(UPDATE_TEMPLATE_SHAPES).is_some();
    let shapes_dir = template_shapes_dir().join("params");
    let mut mismatches = Vec::new();
    for (template, param, slot_count, step_count) in [
        ("master-dual-stack", "stack-width=40ppt", 3, 3),
        ("dual-sidebars", "main-width=60ppt", 3, 4),
    ] {
        let name = format!("{template}--{}", param.replace('=', "-"));
        let shape = launch_template_and_render_shape(
            &mut connection,
            template,
            &name,
            slot_count,
            step_count,
            &["--param", param],
        );
        let recorded_path = shapes_dir.join(format!("{name}.txt"));
        if updating {
            std::fs::write(&recorded_path, &shape)
                .unwrap_or_else(|error| panic!("failed to record {name}'s shape: {error}"));
        } else if std::fs::read_to_string(&recorded_path).ok().as_deref() != Some(shape.as_str()) {
            mismatches.push(format!("{name} built:\n{shape}"));
        }
    }

    connection
        .run_command(format!("workspace {original_workspace}"))
        .expect("workspace switch should succeed");
    assert!(
        mismatches.is_empty(),
        "{}\nIf a change is intended, rerun with {UPDATE_TEMPLATE_SHAPES}=1 and review the \
         diff to tests/template-shapes/params/.",
        mismatches.join("\n")
    );
}

#[test]
fn builtin_template_name_resolves_and_launches_without_a_toml_extension() {
    fresh_workspace();
    // --template <name> (no .toml extension, no path) is a separate
    // dispatch path from --template <file>.toml (main.rs's
    // resolve_template_contents()), driving the same embedded content
    // every_shipped_template_resolves_and_launches_successfully above
    // drives from disk — proves the bare-name lookup finds the real
    // embedded quad-grid template and builds the same shape from it, not
    // just that resolve_template_contents() parses successfully.
    let mut connection = connect();
    let original_workspace = focused_workspace_name(&mut connection);

    let shape =
        launch_template_and_render_shape(&mut connection, "quad-grid", "quad-grid", 4, 4, &[]);

    connection
        .run_command(format!("workspace {original_workspace}"))
        .expect("workspace switch should succeed");
    assert_eq!(
        Some(shape),
        recorded_template_shape("quad-grid"),
        "the built-in quad-grid should build the shape recorded for templates/quad-grid.toml"
    );
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
    fresh_workspace();
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

    let bin_dir = TestTempDir::new();
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

    // (script, app_id its windows carry, expected window count). Only
    // retarget-floating gets its own app_id: its --existing step must match
    // exactly one window, so a decoy foot window is opened before running it,
    // standing in for the foot terminal a user normally runs the script from.
    let scripts = [
        ("dual-terminals", "foot", 2),
        ("triple-row", "foot", 3),
        ("column-split", "foot", 2),
        ("quad-terminals", "foot", 4),
        ("workspace-and-position", "foot", 1),
        ("retarget-floating", "sway-launch-example", 1),
    ];

    for (name, app_id, expected_window_count) in scripts {
        connection
            .run_command("[app_id=foot] kill")
            .expect("kill should succeed");
        connection
            .run_command("[app_id=sway-launch-example] kill")
            .expect("kill should succeed");

        let _decoy = (app_id != "foot").then(|| launch_foot(&[]).1);

        let path = examples_dir("scripts").join(name);

        let output = Command::new(&path)
            .env("PATH", &path_with_fake_bin_first)
            .env("SWAYSOCK", live_socket())
            .env("I3SOCK", live_socket())
            .output()
            .unwrap_or_else(|error| panic!("failed to run {name}: {error}"));
        assert!(
            output.status.success(),
            "examples/scripts/{name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        std::thread::sleep(Duration::from_millis(300));
        let tree = connection.get_tree().expect("get_tree should succeed");
        let window_count = count_app_id_windows(&tree, app_id);
        assert_eq!(
            window_count, expected_window_count,
            "examples/scripts/{name} should have launched {expected_window_count} \
             {app_id} window(s), found {window_count}"
        );
    }

    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    connection
        .run_command("[app_id=sway-launch-example] kill")
        .expect("kill should succeed");
}

#[test]
fn dual_output_template_moves_windows_to_separate_outputs() {
    fresh_workspace();
    // The built-in dual-output ships with HDMI-A-1/DP-1 as placeholder
    // output names; a bindings file supplies the real ones per slot, the way
    // the file's own header comment tells a user to (#30).
    let mut connection = connect();
    let created_output = CreatedOutput::new(&mut connection);
    let new_output = created_output.name().to_string();
    let first_output = original_output().to_string();

    let bindings = TempToml::write(
        "dual-output-bindings",
        &format!(
            "[[binding]]\nslot = \"first\"\ncommand = \"foot\"\noutput = \"{first_output}\"\n\n\
             [[binding]]\nslot = \"second\"\ncommand = \"foot\"\noutput = \"{new_output}\"\n"
        ),
    );

    let output = sway_launch_command()
        .args([
            "--template",
            "dual-output",
            "--bindings",
            bindings.to_str().unwrap(),
            "--timeout",
            "10",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "dual-output failed: {}",
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
    fresh_workspace();
    // Drives the actual examples/layouts/quad-terminals.toml file directly,
    // rather than a hand-written stand-in.
    //
    // Unlike every other test in this file, this one asserts on *relative*
    // multi-window grid geometry rather than a single freshly launched
    // window's own state, so it switches to a workspace of its own rather
    // than relying on killing leftover windows in the shared one. It also
    // sleeps briefly after the layout command completes, before querying
    // tree state: the last window's surface can still be settling into its
    // final geometry for a short while after sway-launch's own process has
    // already exited — --wait-time alone (which only sleeps *between*
    // sway-launch's own actions) isn't enough to cover that.
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
    fresh_workspace();
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
    // Mirrors tree.rs's width_matches(): a window that had
    // already been tiled with a sibling for a while before being floated
    // and resized can come out short by 2*current_border_width on
    // rect.width alone (confirmed live on Sway 1.11) — but confirmed live
    // on Sway 1.9 that the exact rect.width match applies instead in this same
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

fn no_staging_workspace_is_left(connection: &mut Connection) {
    let workspaces: Vec<String> = connection
        .get_workspaces()
        .expect("get_workspaces should succeed")
        .into_iter()
        .map(|workspace| workspace.name)
        .collect();
    assert!(
        !workspaces
            .iter()
            .any(|name| name.starts_with("sway-launch-staging-")),
        "a staging workspace was left behind: {workspaces:?}"
    );
}

fn workspace_of(connection: &mut Connection, con_id: i64) -> String {
    let tree = connection.get_tree().expect("get_tree should succeed");
    workspace_containing(&tree, con_id, None)
        .unwrap_or_else(|| panic!("container id {con_id} is on no workspace"))
        .to_string()
}

#[test]
fn concurrent_retarget_by_id_layout_resolves_a_staged_window_by_name() {
    fresh_workspace();
    // #73: the shipped retarget-by-id.toml with --concurrent. Its third step
    // has no command of its own, so it runs as usual, against the id the
    // first staged launch resolved to.
    let mut connection = connect();
    let build_workspace = focused_workspace_name(&mut connection);
    let path = examples_dir("layouts").join("retarget-by-id.toml");
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--concurrent",
            "--wait-time",
            "100",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "retarget-by-id.toml --concurrent failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ids: Vec<i64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.parse().expect("each line should be a container id"))
        .collect();
    let _guards: Vec<_> = [ids[0], ids[1]].iter().map(|&id| KillOnDrop(id)).collect();
    assert_eq!(ids.len(), 3);
    assert_eq!(ids[0], ids[2]);
    assert_ne!(ids[0], ids[1]);
    assert!(node_is_floating(&get_node(&mut connection, ids[0])));
    for &id in &ids[..2] {
        assert_eq!(workspace_of(&mut connection, id), build_workspace);
    }
    assert_eq!(focused_workspace_name(&mut connection), build_workspace);
    no_staging_workspace_is_left(&mut connection);
}

/// Every container in `node` (scratchpad included) whose app_id is `app_id`.
fn containers_with_app_id(node: &Node, app_id: &str) -> Vec<i64> {
    let own = (node.app_id.as_deref() == Some(app_id)).then_some(node.id);
    own.into_iter()
        .chain(
            node.nodes
                .iter()
                .chain(node.floating_nodes.iter())
                .flat_map(|child| containers_with_app_id(child, app_id)),
        )
        .collect()
}

/// The container ids a successful run printed, one per line.
fn run_ids(arguments: &[&str]) -> Vec<i64> {
    let output = sway_launch_command()
        .args(arguments)
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.parse().expect("each line should be a container id"))
        .collect()
}

#[test]
fn an_existing_step_ahead_of_a_launch_of_the_same_app_works_with_and_without_concurrent() {
    // #190: with --concurrent, step 2's window is already open on the
    // staging workspace when step 1 looks for the one window it names.
    let path = TempToml::write(
        "existing-before-launch",
        "[[step]]\nexisting = true\napp_id = \"live-existing-190\"\n\
         mark = \"live-existing-190\"\n\n\
         [[step]]\ncommand = \"foot --app-id=live-existing-190\"\napp_id = \"live-existing-190\"\n",
    );
    for concurrent in [false, true] {
        fresh_workspace();
        let mut connection = connect();
        let opened = run_ids(&[
            "-a",
            "live-existing-190",
            "--timeout",
            "10",
            "foot --app-id=live-existing-190",
        ]);
        let _opened_guard = KillOnDrop(opened[0]);
        let mut arguments = vec!["--layout", path.to_str().unwrap(), "--timeout", "10"];
        if concurrent {
            arguments.push("--concurrent");
        }
        let ids = run_ids(&arguments);
        let _launched_guard = KillOnDrop(ids[1]);
        assert_eq!(ids[0], opened[0], "concurrent {concurrent}");
        assert_ne!(ids[1], opened[0], "concurrent {concurrent}");
        assert_eq!(
            get_node(&mut connection, opened[0]).marks,
            vec!["live-existing-190".to_string()],
            "concurrent {concurrent}"
        );
        no_staging_workspace_is_left(&mut connection);
    }
}

#[test]
fn an_existing_wait_step_ahead_of_a_launch_waits_for_the_autostarted_window() {
    // #190: the window step 1 waits for is started by Sway's own exec 1.5s
    // in. With --concurrent, step 2's staged window matches it first, and
    // must not be taken in its place.
    let path = TempToml::write(
        "existing-wait-before-launch",
        "[[step]]\nexisting = true\nwait = true\napp_id = \"live-wait-190\"\n\
         mark = \"live-wait-190\"\n\n\
         [[step]]\ncommand = \"foot --app-id=live-wait-190\"\napp_id = \"live-wait-190\"\n",
    );
    for concurrent in [false, true] {
        fresh_workspace();
        let mut connection = connect();
        let mut command = sway_launch_command();
        command.args(["--layout", path.to_str().unwrap(), "--timeout", "10"]);
        if concurrent {
            command.arg("--concurrent");
        }
        let running = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to start sway-launch binary");
        std::thread::sleep(Duration::from_millis(1500));
        let tree = connection.get_tree().expect("get_tree should succeed");
        let before_autostart = containers_with_app_id(&tree, "live-wait-190");
        run_command_ok(&mut connection, "exec foot --app-id=live-wait-190");
        let output = running
            .wait_with_output()
            .expect("sway-launch should finish");
        assert!(
            output.status.success(),
            "concurrent {concurrent}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ids: Vec<i64> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.parse().expect("each line should be a container id"))
            .collect();
        let _guards: Vec<_> = ids.iter().map(|&id| KillOnDrop(id)).collect();
        assert_eq!(ids.len(), 2, "concurrent {concurrent}");
        assert_ne!(ids[0], ids[1], "concurrent {concurrent}");
        assert!(
            !before_autostart.contains(&ids[0]),
            "concurrent {concurrent}: step 1 took step 2's window {ids:?}"
        );
        assert_eq!(
            get_node(&mut connection, ids[0]).marks,
            vec!["live-wait-190".to_string()],
            "concurrent {concurrent}"
        );
        assert!(
            get_node(&mut connection, ids[1]).marks.is_empty(),
            "concurrent {concurrent}"
        );
        no_staging_workspace_is_left(&mut connection);
    }
}

#[test]
fn concurrent_rollback_closes_every_staged_window_a_later_step_fails_after() {
    fresh_workspace();
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    let path = TempToml::write(
        "concurrent-rollback",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[step]]\ncon_id = 999999999\nmark = \"x\"\n",
    );
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--concurrent",
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!output.status.success(), "the third step should fail");
    let error: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr is the --json error");
    assert_eq!(
        error["rolled_back"].as_array().map(Vec::len),
        Some(2),
        "{stderr}"
    );
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(count_app_id_windows(&tree, "foot"), 0, "{stderr}");
    no_staging_workspace_is_left(&mut connection);
}

/// Runs `args` with `--verbose --json` and checks the run staged its
/// launches (#121): the staging line is printed only by a concurrent run.
/// Returns the run's container ids, each closed on drop.
fn run_expecting_staged_launches(args: &[&str], expected_windows: usize) -> Vec<KillOnDrop> {
    let mut connection = connect();
    let build_workspace = focused_workspace_name(&mut connection);
    let output = sway_launch_command()
        .args(args)
        .args(["--verbose", "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let result: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("stdout should be the --json result: {stdout}\n{stderr}"));
    let guards: Vec<KillOnDrop> = result["container_ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_i64())
                .map(KillOnDrop)
                .collect()
        })
        .unwrap_or_default();
    assert!(output.status.success(), "{args:?} failed: {stderr}");
    assert!(
        stderr.contains(&format!("Launching {expected_windows} steps at once")),
        "{args:?} should have staged its launches: {stderr}"
    );
    assert_eq!(guards.len(), expected_windows, "{stdout}");
    for guard in &guards {
        assert_eq!(workspace_of(&mut connection, guard.0), build_workspace);
    }
    no_staging_workspace_is_left(&mut connection);
    guards
}

#[test]
fn concurrent_key_in_a_layout_file_stages_its_launches() {
    fresh_workspace();
    let path = TempToml::write(
        "concurrent-key-layout",
        "concurrent = true\n\n\
         [[step]]\ncommand = \"foot\"\napp_id = \"foot\"\nsplit = \"h\"\n\n\
         [[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );
    run_expecting_staged_launches(&["--layout", path.to_str().unwrap()], 2);
}

#[test]
fn concurrent_key_in_a_bindings_file_stages_the_template_launches() {
    fresh_workspace();
    let path = TempToml::write(
        "concurrent-key-bindings",
        "concurrent = true\n\n\
         [[binding]]\nslot = \"left\"\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[binding]]\nslot = \"right\"\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );
    run_expecting_staged_launches(
        &[
            "--template",
            "dual-row",
            "--bindings",
            path.to_str().unwrap(),
        ],
        2,
    );
}

#[test]
fn concurrent_launch_that_times_out_leaves_the_others_on_the_build_workspace() {
    fresh_workspace();
    // A staged launch failing must not strand the windows already staged
    // out of sight: they're moved to the build workspace and reported.
    let mut connection = connect();
    let build_workspace = focused_workspace_name(&mut connection);
    let path = TempToml::write(
        "concurrent-timeout",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[step]]\ncommand = \"sleep 30\"\napp_id = \"never-opens\"\ntimeout = 1\n",
    );
    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--concurrent", "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!output.status.success(), "the second step should time out");
    let error: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr is the --json error");
    assert!(
        error["error"]
            .as_str()
            .unwrap_or_default()
            .contains("step 2"),
        "{stderr}"
    );
    let foot = error["container_ids"][0]
        .as_i64()
        .unwrap_or_else(|| panic!("the staged foot should be reported: {stderr}"));
    let _guard = KillOnDrop(foot);
    assert_eq!(workspace_of(&mut connection, foot), build_workspace);
    no_staging_workspace_is_left(&mut connection);
    // #126: the launch that never opened a window may still open one, on
    // the staging workspace, so the run names it.
    assert_eq!(error["unresolved_launches"][0]["step"], 2, "{stderr}");
}

#[test]
fn concurrent_rollback_closes_a_staged_launch_s_late_window() {
    fresh_workspace();
    // #126: a staged launch whose window opens just after its timeout has
    // it closed by rollback, rather than left to open on the staging
    // workspace once the run has gone.
    let mut connection = connect();
    connection
        .run_command("[app_id=foot] kill")
        .expect("kill should succeed");
    let path = TempToml::write(
        "concurrent-late-window",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n\n\
         [[step]]\ncommand = \"sleep 1.5 && foot\"\napp_id = \"foot\"\ntimeout = 1\n",
    );
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--concurrent",
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!output.status.success(), "the second step should time out");
    let error: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr is the --json error");
    assert!(
        error["error"]
            .as_str()
            .unwrap_or_default()
            .contains("opened within 2 sec after that"),
        "{stderr}"
    );
    assert_eq!(
        error["rolled_back"].as_array().map(Vec::len),
        Some(2),
        "{stderr}"
    );
    assert!(error.get("unresolved_launches").is_none(), "{stderr}");
    std::thread::sleep(Duration::from_millis(500));
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(count_app_id_windows(&tree, "foot"), 0, "{stderr}");
    no_staging_workspace_is_left(&mut connection);
}

#[test]
fn concurrent_continue_on_timeout_arranges_the_windows_that_did_open() {
    fresh_workspace();
    let mut connection = connect();
    let build_workspace = focused_workspace_name(&mut connection);
    let path = TempToml::write(
        "concurrent-continue",
        "[[step]]\ncommand = \"sleep 30\"\napp_id = \"never-opens\"\ntimeout = 1\n\n\
         [[step]]\ncommand = \"foot\"\napp_id = \"foot\"\nfloating = true\n",
    );
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--concurrent",
            "--continue-on-timeout",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert_eq!(output.status.code(), Some(1));
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is the --json result");
    let foot = result["container_ids"][0]
        .as_i64()
        .unwrap_or_else(|| panic!("foot should be arranged: {result}"));
    let _guard = KillOnDrop(foot);
    assert_eq!(result["timed_out_steps"][0]["step"], 1, "{result}");
    assert_eq!(result["unresolved_launches"][0]["step"], 1, "{result}");
    assert!(node_is_floating(&get_node(&mut connection, foot)));
    assert_eq!(workspace_of(&mut connection, foot), build_workspace);
    no_staging_workspace_is_left(&mut connection);
}

#[test]
fn split_h_places_windows_side_by_side() {
    fresh_workspace();
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
    fresh_workspace();
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
    fresh_workspace();
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism: Split now confirms via
    // SwayAction::poll_shows() (a get_tree() poll of the container's
    // *parent* layout field — see find_parent_layout()'s doc comment for
    // why it's the parent, not the container's own node) rather than always
    // sleeping the full --wait-time after the command too. run_batch()
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
    fresh_workspace();
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
    fresh_workspace();
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
    fresh_workspace();
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
    fresh_workspace();
    // Regression test for docs/plan-poll-based-wait-time-actions.md's
    // poll-then-fallback mechanism reaching Height: confirms via
    // SwayAction::poll_shows()'s height_matches() (a get_tree() poll for
    // the decoration-inclusive rect.height + deco_rect.height) rather than
    // always sleeping the full --wait-time after the command too. Needs a
    // vertically-arranged sibling for the resize to take effect at all —
    // see height_alone_resizes_a_non_solo_window. Both windows use an
    // explicit --workspace (rather than whatever's currently focused) so
    // this test's "just these two windows" assumption holds regardless of
    // ambient state left behind by other tests in this shared compositor —
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
            "--json",
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

    // The other half of the "unconfirmed" distinction: this resize really did
    // take effect and was observed, so it reports "changed" -- paired with
    // height_and_width_fall_back_gracefully_when_solo_window_clamps_the_resize,
    // where the identical command on a solo window reports "unconfirmed".
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\":\"changed\""),
        "a confirmed resize should report changed, got {stdout:?}"
    );

    let node = get_node(&mut connection, second_id);
    assert_eq!(node.rect.height + node.deco_rect.height, 200);
}

#[test]
fn width_confirms_via_poll_when_resized_with_a_sibling() {
    fresh_workspace();
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
    fresh_workspace();
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
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        height_output.status.success(),
        "sway-launch --height (solo window) failed: {}",
        String::from_utf8_lossy(&height_output.stderr)
    );
    // Succeeding is not the same as having worked, and --json now says which
    // it was: the clamp means the requested height never appears, so this is
    // reported as "unconfirmed" rather than passing as "changed". That
    // distinction is the whole point of the status -- a caller chaining
    // further actions on the assumption this resize took effect can now see
    // that it didn't.
    let height_stdout = String::from_utf8_lossy(&height_output.stdout);
    assert!(
        height_stdout.contains("\"status\":\"unconfirmed\""),
        "a clamped resize should report unconfirmed, got {height_stdout:?}"
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
    fresh_workspace();
    // Regression test found during a code-review bug hunt: WAIT_TIME_POLL_GRACE
    // (200ms) is 10x the CLI's own --wait-time default (20ms), so before
    // the poll grace period was capped at
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
    let workspace = fresh_workspace();
    // Sway centers on the window's workspace, not its output: the two only
    // coincide when nothing (a bar, outer gaps) takes space at the output's
    // edge. position_confirms_center_and_coordinates_on_an_offset_workspace
    // covers the case where they differ.
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

    let workspace_rect = workspace_rect(&mut connection, &workspace);
    let node = get_node(&mut connection, container_id);
    assert_eq!(node.rect.width, 400);
    // deco_rect (not rect) is the decoration-inclusive top edge "move
    // position"/centering actually targets, and it's relative to the
    // workspace — see position_moves_a_floating_window_to_given_coordinates.
    assert_eq!(node.deco_rect.x, (workspace_rect.width - 400) / 2);
    assert_eq!(node.deco_rect.y, (workspace_rect.height - 300) / 2);
}

#[test]
fn position_center_with_a_requested_size_reports_the_final_placement() {
    // #145: foot asked for 800x500 commits a size of its own a moment later
    // (snapped to its cell grid, or the size it had tiled), while Sway keeps
    // the window where it centered the 800x500 frame it set. Judged on
    // whichever size the poll read, the same final placement reported
    // `changed` on one run and `unconfirmed` on the next. README's quick
    // start, so each run's status must agree with where the window is once
    // it settled.
    let workspace = fresh_workspace();
    let mut connection = connect();
    let workspace_rect = workspace_rect(&mut connection, &workspace);
    let expected = (
        (workspace_rect.width - 800) / 2,
        (workspace_rect.height - 500) / 2,
    );
    for _ in 0..4 {
        let output = sway_launch_command()
            .args([
                "--app-id",
                "foot",
                "--timeout",
                "10",
                "--json",
                "--floating",
                "--width",
                "800px",
                "--height",
                "500px",
                "--position",
                "center",
                "foot",
            ])
            .output()
            .expect("failed to run sway-launch binary");
        assert!(
            output.status.success(),
            "sway-launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let parsed: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("stdout should be one JSON object");
        let container_id = parsed["container_id"]
            .as_i64()
            .expect("container_id should be an integer");
        let _guard = KillOnDrop(container_id);
        let position = parsed["actions"]
            .as_array()
            .expect("actions should be an array")
            .iter()
            .find(|action| action["kind"] == "position")
            .expect("the run should report its position action");

        std::thread::sleep(Duration::from_millis(500));
        let node = get_settled_node(&mut connection, container_id);
        assert_eq!(
            (node.deco_rect.x, node.deco_rect.y),
            expected,
            "Sway should have centered the requested 800x500 frame, whatever size foot kept \
             ({}x{})",
            node.rect.width,
            node.rect.height + node.deco_rect.height
        );
        assert_eq!(
            position["status"], "changed",
            "a window settled where it was centered should report changed, got {parsed}"
        );
    }
}

/// Sets every workspace's outer gaps to `gaps` and resets them to zero when
/// dropped, even on assertion panic — the setting applies compositor-wide, so
/// every later test in this shared session would otherwise inherit it.
struct OuterGaps;

impl OuterGaps {
    fn set(connection: &mut Connection, gaps: i32) -> Self {
        run_command_ok(connection, &format!("gaps outer all set {gaps}"));
        OuterGaps
    }
}

impl Drop for OuterGaps {
    fn drop(&mut self) {
        let _ = connect().run_command("gaps outer all set 0");
    }
}

/// The geometry of the workspace named `workspace`.
fn workspace_rect(connection: &mut Connection, workspace: &str) -> swayipc::Rect {
    connection
        .get_workspaces()
        .expect("get_workspaces should succeed")
        .into_iter()
        .find(|candidate| candidate.name == workspace)
        .unwrap_or_else(|| panic!("workspace {workspace:?} should exist"))
        .rect
}

/// Runs sway-launch against `container_id` with `args` plus `--json` and a
/// `--wait-time` far longer than the poll grace, returning stdout.
fn run_position_json(container_id: i64, args: &[&str]) -> String {
    let output = sway_launch_command()
        .args(["--con-id", &container_id.to_string()])
        .args(args)
        .args(["--wait-time", "2000", "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout should be valid utf8")
}

#[test]
fn position_confirms_center_and_coordinates_on_an_offset_workspace() {
    let workspace = fresh_workspace();
    // Regression test: position_shows() used to center against the
    // *output* and compare an absolute rect with a workspace-relative target,
    // so it only confirmed when the workspace started at (0, 0) -- true of
    // this suite's `-c /dev/null` compositor, but not of a desktop with a bar
    // or outer gaps. There, a correctly placed window reported "unconfirmed"
    // after waiting out the poll grace. Outer gaps move the workspace origin
    // the same way a bar does, without needing a bar client.
    let mut connection = connect();
    let _gaps = OuterGaps::set(&mut connection, 40);
    let workspace_rect = workspace_rect(&mut connection, &workspace);
    assert_eq!(
        (workspace_rect.x, workspace_rect.y),
        (40, 40),
        "outer gaps should have moved the workspace origin off (0, 0)"
    );
    let (container_id, _guard) =
        launch_foot(&["--floating", "--width", "400px", "--height", "300px"]);

    // deco_rect is workspace-relative while the old expected center was
    // output-relative, so the two differ by exactly the 40px origin.
    let stdout = run_position_json(container_id, &["--position", "center"]);
    assert!(
        stdout.contains(r#""command":"move position center","kind":"position","status":"changed""#),
        "a centered window on an offset workspace should report changed, got {stdout:?}"
    );
    let node = get_settled_node(&mut connection, container_id);
    assert_eq!(
        node.deco_rect.x,
        (workspace_rect.width - node.rect.width) / 2
    );

    // A pixel border makes Sway report an all-zero deco_rect, so the check
    // falls back to rect, which is absolute rather than workspace-relative.
    run_command_ok(
        &mut connection,
        &format!("[con_id={container_id}] border pixel 2"),
    );
    let stdout = run_position_json(container_id, &["--position", "100,200"]);
    assert!(
        stdout
            .contains(r#""command":"move position 100 200","kind":"position","status":"changed""#),
        "explicit coordinates on a pixel-border window should report changed, got {stdout:?}"
    );
    let node = get_node(&mut connection, container_id);
    assert_eq!(
        (node.deco_rect.width, node.deco_rect.height),
        (0, 0),
        "a pixel-border window should report no decoration geometry"
    );
    assert_eq!(
        (node.rect.x, node.rect.y),
        (workspace_rect.x + 100, workspace_rect.y + 200)
    );
}

#[test]
fn json_output_for_a_real_exec_is_a_clean_container_id_object() {
    fresh_workspace();
    // con_id_json_output_is_a_clean_json_object in tests/json_output.rs
    // covers --json's formatting for --con-id, which never touches the
    // socket; this covers the same formatting for a real exec+match, with
    // no other action flags -- confirming "actions" is empty when nothing
    // else ran, same as the headless --con-id-alone case.
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
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    let container_id = json["container_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected an integer container_id, got {json:?}"));
    assert_eq!(
        json["actions"],
        serde_json::json!([]),
        "no action flags were passed, so actions should be empty: {json:?}"
    );
    let _guard = KillOnDrop(container_id);
}

#[test]
fn json_output_lists_the_actions_that_actually_ran() {
    fresh_workspace();
    // Companion to the test above: with real action flags, "actions" should
    // list each one's sway_command_verb() text with a "changed" status (a
    // freshly launched window is neither already floating nor already
    // marked), in the fixed order SwayLaunch::run() always applies them
    // (Floating before Mark here).
    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            "--wait-time",
            "100",
            "--floating",
            "--mark",
            "live-sway-test-json-actions",
            "--json",
            "foot",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    let container_id = json["container_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected an integer container_id, got {json:?}"));
    assert_eq!(
        json["actions"],
        serde_json::json!([
            { "kind": "floating", "command": "floating enable", "status": "changed" },
            {
                "kind": "mark",
                "command": "mark \"live-sway-test-json-actions\"",
                "status": "changed",
            },
        ]),
        "actions should list what actually ran, in order, with a changed status: {json:?}"
    );
    let _guard = KillOnDrop(container_id);
}

#[test]
fn json_output_reports_already_satisfied_when_re_applying_an_unchanged_state() {
    fresh_workspace();
    // Companion to the test above: re-running --floating against a window
    // that's already floating should report "already_satisfied", not
    // "changed" — run_batch()'s already-in-effect check computes this
    // distinction internally; this confirms it actually reaches --json.
    let (container_id, _guard) = launch_foot(&["--floating"]);

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--floating",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --floating (already floating) --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid utf8");
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("--json output should be valid JSON");
    assert_eq!(
        json["actions"],
        serde_json::json!([{
            "kind": "floating",
            "command": "floating enable",
            "status": "already_satisfied",
        }]),
        "re-applying an already-floating state should report already_satisfied: {json:?}"
    );
}

#[test]
fn existing_errors_with_zero_matches() {
    fresh_workspace();
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
fn existing_matches_a_window_by_mark() {
    fresh_workspace();
    // --mark-match: retargets a previously-marked window by its mark, the
    // classic "dropdown terminal" scripting pattern, instead of tracking a
    // container id or matching by app_id/class.
    let mut connection = connect();
    let (container_id, _guard) = launch_foot(&["--mark", "live-sway-test-dropdown-term"]);

    let output = sway_launch_command()
        .args([
            "--existing",
            "--mark-match",
            "live-sway-test-dropdown-term",
            "--floating",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --existing --mark-match failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let node = get_node(&mut connection, container_id);
    assert!(
        node_is_floating(&node),
        "the window matched by mark should have had --floating applied to it"
    );
}

#[test]
fn existing_mark_match_errors_with_zero_matches() {
    fresh_workspace();
    let output = sway_launch_command()
        .args([
            "--existing",
            "--mark-match",
            "live-sway-test-nonexistent-mark",
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
    fresh_workspace();
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
    fresh_workspace();
    // Live version of quote_sway_string's unit tests (ipc.rs):
    // proves the injection-style mark actually ends up stored as one
    // literal mark against a real compositor, rather than being split into
    // multiple Sway commands.
    let mut connection = connect();
    let temp_dir = TestTempDir::new();
    let marker = temp_dir.join("injection-pwned");
    let malicious_mark = format!(
        "live-sway-test-injection, exec touch {}; echo bar",
        marker.display()
    );

    let (container_id, _guard) = launch_foot(&["--mark", &malicious_mark]);

    let node = get_node(&mut connection, container_id);
    assert!(node.marks.contains(&malicious_mark));
    assert!(
        !marker.exists(),
        "mark should be stored literally, not executed as a separate command"
    );
}

#[test]
fn mark_containing_command_separators_still_round_trips_through_mark_match() {
    fresh_workspace();
    // The other half of the `"`/`\` rejection added alongside this test (see
    // values.rs's validate_sway_string_argument): proving the new
    // validation didn't over-reject. A `,`/`;` in a mark *is* genuinely
    // neutralized by quote_sway_string() and stored byte-for-byte, so
    // --mark followed by --mark-match on the same value must still find the
    // window. This is the live-only half -- the rejection itself is caught
    // by clap before any IPC and is covered headlessly.
    let mut connection = connect();
    let mark = "live-sway-test-roundtrip, exec true; echo";
    let (container_id, _guard) = launch_foot(&["--mark", mark]);

    let output = sway_launch_command()
        .args(["--existing", "--mark-match", mark, "--floating"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "a mark containing command separators should still be matchable by --mark-match: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let node = get_node(&mut connection, container_id);
    assert!(
        node.marks.contains(&mark.to_string()),
        "the mark should be stored byte-for-byte, got {:?}",
        node.marks
    );
    assert!(
        node_is_floating(&node),
        "the window matched by mark should have had --floating applied to it"
    );
}

#[test]
fn mark_with_an_embedded_newline_is_not_executed_as_a_separate_command() {
    fresh_workspace();
    // Companion to the comma/semicolon case above, for a character
    // quote_sway_string() doesn't escape at all (only `\`/`"` are).
    //
    // This used to assert that Sway stored such a mark as one literal mark
    // with the newline normalized to `;` -- true, and it proved nothing was
    // executed. What it missed is that the normalization means the value
    // never comes back out again: a mark set as "a<LF>b" is stored as "a;b",
    // so --mark-match could never find it. That's the same round-trip failure
    // `"`/`\` were rejected for, and it survived because this test asked what
    // Sway *stored* rather than whether the value survived.
    //
    // The value is now rejected before it reaches Sway, so this asserts the
    // rejection and, still, that nothing was executed as a side effect --
    // which is the part worth checking against a real compositor rather than
    // at the clap level.
    let temp_dir = TestTempDir::new();
    let marker = temp_dir.join("newline-injection-pwned");
    let malicious_mark = format!(
        "live-sway-test-newline-injection\nexec touch {}",
        marker.display()
    );

    let output = sway_launch_command()
        .args([
            "--app-id",
            "foot",
            "--timeout",
            "10",
            "--mark",
            &malicious_mark,
            "foot",
        ])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "a mark containing a newline should be rejected, not silently rewritten by Sway"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("newline"),
        "the error should name the offending character: {stderr:?}"
    );
    assert!(
        !marker.exists(),
        "nothing in the mark may be executed, whether it is accepted or rejected"
    );
}

#[test]
fn verbose_prints_real_diagnostics_to_stderr_for_a_live_action() {
    fresh_workspace();
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
    fresh_workspace();
    // tests/template.rs exercises --bindings headlessly with con_id-only
    // bindings; this drives the same flag against real windows, covering
    // both a Binding's `existing = true` and `command` forms (mirroring
    // README.md's own Templates example) in one template.
    let mut connection = connect();
    let (existing_id, _existing_guard) = launch_foot(&[]);

    let template_path = TempToml::write(
        "bindings-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"existing_window\"\nmark = \"live-sway-test-bindings-existing\"\n\n\
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

/// Spawns `sway-launch --debug-events` as a background child and forwards
/// its stdout line by line through a channel. It runs until killed, so it
/// doesn't fit this file's usual spawn-and-wait-for-exit pattern; the reader
/// thread is the same shape confirmation.rs's own event loop uses internally.
/// Returns once the subscription has had time to establish, so an event the
/// caller triggers next isn't sent before anything is listening.
fn spawn_debug_events(extra_args: &[&str]) -> (KillChildOnDrop, mpsc::Receiver<String>) {
    let mut child = sway_launch_command()
        .arg("--debug-events")
        .args(extra_args)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch --debug-events");
    let stdout = child.stdout.take().expect("child should have piped stdout");
    let child_guard = KillChildOnDrop(child);

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

    std::thread::sleep(Duration::from_millis(300));
    (child_guard, line_receiver)
}

/// Whether a line satisfying `matches` arrives on `lines` within 10s.
fn debug_events_line_arrives(
    lines: &mpsc::Receiver<String>,
    matches: impl Fn(&str) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match lines.recv_timeout(remaining) {
            Ok(line) if matches(&line) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

/// Starts a run of a layout whose one window takes about a second to map,
/// then a second run of the same layout with `second_args` while the first is
/// still going, and asserts the run lock refused the second one.
fn assert_second_run_is_refused(first_args: &[&str], second_args: &[&str]) {
    let path = TempToml::write(
        "run-lock",
        "[[step]]\ncommand = 'sh -c \"sleep 1 && exec foot --app-id=lock-probe\"'\n\
         app_id = \"lock-probe\"\n",
    );
    let first = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .args(first_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    std::thread::sleep(Duration::from_millis(300));

    let second = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .args(second_args)
        .output()
        .expect("failed to run sway-launch binary");
    let first = first
        .wait_with_output()
        .expect("failed to wait for the first run");
    let container_id: i64 = String::from_utf8_lossy(&first.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("the first run should print its id: {first:?}"));
    let _guard = KillOnDrop(container_id);

    assert_eq!(second.status.code(), Some(1), "{second:?}");
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("is still in progress"),
        "{second:?}"
    );
}

#[test]
fn a_second_run_of_a_layout_on_the_same_workspace_is_refused_while_the_first_runs() {
    let workspace = fresh_workspace();
    // A double-triggered layout used to interleave its windows with the first
    // run's (#55); the run lock now refuses the second run at once.
    assert_second_run_is_refused(
        &["--on-workspace", &workspace],
        &["--on-workspace", &workspace],
    );
}

#[test]
fn the_run_lock_holds_however_the_second_run_names_the_workspace() {
    // Every spelling of one workspace takes the same lock (#129): a name in
    // another case, the focused workspace with no --on-workspace at all, and
    // a workspace number against the numbered workspace it focuses, even once
    // a daemon has renamed it (#178).
    let workspace = fresh_workspace();
    assert_second_run_is_refused(
        &["--on-workspace", &workspace],
        &["--on-workspace", &workspace.to_uppercase()],
    );
    assert_second_run_is_refused(&["--on-workspace", &workspace], &[]);

    let mut connection = connect();
    let numbered = format!("47: {workspace}");
    run_command_ok(&mut connection, &format!("workspace \"{numbered}\""));
    assert_second_run_is_refused(&["--on-workspace-number", "47"], &[]);
    run_command_ok(
        &mut connection,
        &format!("rename workspace \"{numbered}\" to \"47: renamed-{workspace}\""),
    );
    assert_second_run_is_refused(&[], &["--on-workspace", "47"]);
}

#[test]
fn the_run_lock_with_only_on_output_names_the_workspace_that_output_shows() {
    // With only --on-output, the lock used to name the workspace focused
    // before the run moved to that output (#129).
    let workspace = fresh_workspace();
    let mut connection = connect();
    let created = CreatedOutput::new(&mut connection);
    let other = format!("{workspace}-other");
    run_command_ok(&mut connection, &format!("focus output {}", created.name()));
    run_command_ok(&mut connection, &format!("workspace \"{other}\""));
    run_command_ok(&mut connection, &format!("workspace \"{workspace}\""));
    assert_second_run_is_refused(
        &["--on-output", created.name()],
        &["--on-workspace", &other],
    );
}

#[test]
fn a_command_given_as_words_reaches_the_program_exactly() {
    fresh_workspace();
    // A word-list `command` (#77) is quoted by sway-launch, so a title full
    // of Sway and shell syntax arrives as one argument, unchanged.
    let title = "a, b; it's \\ \"q\"";
    let toml = format!(
        "[[step]]\nid = \"words\"\ncommand = [\"foot\", \"-T\", {}, \"sleep\", \"60\"]\n\
         app_id = \"foot\"\n",
        toml::Value::String(title.to_string())
    );
    let path = TempToml::write("words-command", &toml);
    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap(), "--json"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    let container_id = json["containers"]["words"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected the step's container id in {json}"));
    let _guard = KillOnDrop(container_id);
    let mut connection = connect();
    assert_eq!(
        get_node(&mut connection, container_id).name.as_deref(),
        Some(title)
    );
}

#[test]
fn a_layout_piped_through_stdin_launches_its_steps() {
    fresh_workspace();
    // `--layout -` reads the layout from stdin (#50), so a script can pipe a
    // generated one in; the run itself is an ordinary layout run.
    use std::io::Write;
    let mut child = sway_launch_command()
        .args(["--layout", "-", "--json"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(b"[[step]]\nid = \"piped\"\ncommand = \"foot\"\napp_id = \"foot\"\n")
        .expect("writing stdin should succeed");
    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");
    assert!(
        output.status.success(),
        "sway-launch --layout - failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    let container_id = json["containers"]["piped"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected the piped step's container id in {json}"));
    let _guard = KillOnDrop(container_id);
    let mut connection = connect();
    assert_eq!(
        get_node(&mut connection, container_id).app_id.as_deref(),
        Some("foot")
    );
}

#[test]
fn debug_events_json_prints_a_real_window_event_as_one_json_line() {
    fresh_workspace();
    // --debug-events --json writes Sway's payload plus an `event` type per
    // line (#85), so the lines parse and can be filtered like this.
    let (_child_guard, lines) = spawn_debug_events(&["--json"]);
    let (container_id, _guard) = launch_foot(&[]);

    assert!(
        debug_events_line_arrives(&lines, |line| {
            serde_json::from_str::<serde_json::Value>(line).is_ok_and(|event| {
                event["event"] == "window"
                    && event["change"] == "new"
                    && event["container"]["id"] == container_id
            })
        }),
        "expected a JSON line for the new window within 10s"
    );
}

#[test]
fn debug_events_prints_a_real_window_event() {
    fresh_workspace();
    let (_child_guard, lines) = spawn_debug_events(&[]);
    let (_container_id, _guard) = launch_foot(&[]);

    assert!(
        debug_events_line_arrives(&lines, |line| line.contains("WindowEvent")
            && line.contains("change: New")),
        "expected --debug-events to print a New WindowEvent within 10s"
    );
}

/// Sets an output's scale back to what it was when dropped, even on
/// assertion panic, so a test that changes it can't leave every later test
/// running against a rescaled output.
struct RestoreOutputScale {
    output: String,
    scale: f64,
}

impl Drop for RestoreOutputScale {
    fn drop(&mut self) {
        if let Ok(mut connection) = try_connect(live_socket()) {
            let _ = connection.run_command(format!("output {} scale {}", self.output, self.scale));
        }
    }
}

#[test]
fn debug_events_prints_a_real_output_event() {
    // Output events were once missing from --debug-events' subscription list,
    // so it printed nothing at all while outputs were added, removed or
    // reconfigured. Rescaling the original output is the cheapest way to make
    // Sway send one: it needs no create_output, so no CreatedOutput to set
    // up and tear down.
    fresh_workspace();
    let mut connection = connect();
    let output = original_output().to_string();
    let scale = connection
        .get_outputs()
        .expect("get_outputs should succeed")
        .into_iter()
        .find(|candidate| candidate.name == output)
        .and_then(|candidate| candidate.scale)
        .unwrap_or(1.0);
    let _restore_scale = RestoreOutputScale {
        output: output.clone(),
        scale,
    };

    let (_child_guard, lines) = spawn_debug_events(&[]);
    run_command_ok(
        &mut connection,
        &format!("output {output} scale {}", scale * 2.0),
    );

    assert!(
        debug_events_line_arrives(&lines, |line| line.contains("OutputEvent")),
        "expected --debug-events to print an OutputEvent within 10s"
    );
}

#[test]
fn launch_timeout_names_the_window_its_app_id_filter_rejected() {
    fresh_workspace();
    // A wrong --app-id used to end in the same bare timeout as a typo'd
    // command, with the window it rejected left open and never mentioned --
    // not even under --verbose. The error now names that window, says it came
    // from this command, and hands its id to a --json caller. It also pins
    // when the wait gives up: at --timeout, not early and not much later.
    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--timeout",
            "2",
            "--json",
            "--verbose",
            "--app-id",
            "reject-probe-wrong",
            "foot --app-id=reject-probe",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(
        output.status.code(),
        Some(1),
        "the filter matches nothing, so this must fail: {stderr}"
    );
    assert!(
        elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(4),
        "the launch should time out at --timeout 2, took {elapsed:?}"
    );

    let last_line = stderr.lines().next_back().unwrap_or_default();
    let error: serde_json::Value =
        serde_json::from_str(last_line).expect("the last stderr line should be a JSON error");
    let unmatched = error["unmatched_windows"]
        .as_array()
        .unwrap_or_else(|| panic!("expected unmatched_windows in {error}"));
    assert_eq!(unmatched.len(), 1, "{error}");
    let container_id = unmatched[0]["container_id"]
        .as_i64()
        .expect("container_id should be an integer");
    let _guard = KillOnDrop(container_id);
    assert_eq!(unmatched[0]["app_id"], "reject-probe", "{error}");
    assert_eq!(unmatched[0]["launched_by_this_run"], true, "{error}");

    let message = error["error"].as_str().expect("error should be a string");
    assert!(
        message.starts_with("launch \"foot --app-id=reject-probe\": "),
        "the error should say which launch failed: {message}"
    );
    assert!(
        message.contains(&format!("con_id {container_id}"))
            && message.contains("check --app-id/--class"),
        "the error should name the rejected window and the filter to check: {message}"
    );
    assert!(
        stderr.contains(&format!("Event mismatch: New con_id {container_id}")),
        "--verbose should log the rejected window: {stderr}"
    );
}

/// Whether a failed launch's only unmatched window still has `initial_app_id`
/// in the tree, meaning the client never changed it. Closes that window.
fn late_app_id_was_never_set(stderr: &str, initial_app_id: &str) -> bool {
    let Some(error) = stderr
        .lines()
        .next_back()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    else {
        return false;
    };
    let Some(container_id) = error["unmatched_windows"][0]["container_id"].as_i64() else {
        return false;
    };
    let _guard = KillOnDrop(container_id);
    let mut connection = connect();
    get_node(&mut connection, container_id).app_id.as_deref() == Some(initial_app_id)
}

#[test]
fn launch_matches_a_window_that_sets_its_app_id_after_mapping() {
    fresh_workspace();
    // Some apps set their app_id/class only after the window maps, and Sway
    // sends no window event when they change. A launch used to decide on the
    // New event alone, so it ignored its own window and timed out beside it.
    // foot's OSC 176 escape changes its app_id at runtime, standing in for
    // such an app: the window maps as "late-probe-early" and becomes
    // "late-probe" half a second later, with no event in between.
    let temp_dir = TestTempDir::new();
    let script = temp_dir.join("set-app-id-late");
    std::fs::write(
        &script,
        "sleep 0.5\nprintf '\\033]176;late-probe\\033\\\\'\nexec sleep 60\n",
    )
    .expect("failed to write the app_id script");

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--timeout",
            "10",
            "--json",
            "--verbose",
            "--app-id",
            "late-probe",
            &format!("foot --app-id=late-probe-early sh '{}'", script.display()),
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() && late_app_id_was_never_set(&stderr, "late-probe-early") {
        // OSC 176 is newer than some distributions' foot (Ubuntu 24.04's
        // 1.16.2 ignores it), so there was no late change to notice. A window
        // whose app_id did change still fails below.
        eprintln!("skipped: this foot ignores OSC 176, so its app_id never changed");
        return;
    }
    assert!(output.status.success(), "sway-launch failed: {stderr}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("stdout should be JSON: {:?}", output.stdout));
    let container_id = json["container_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected a container_id in {json}"));
    let _guard = KillOnDrop(container_id);

    assert!(
        elapsed < Duration::from_secs(5),
        "the late app_id should be noticed promptly, not at --timeout: took {elapsed:?}"
    );
    assert!(
        stderr.contains(&format!("Event mismatch: New con_id {container_id}"))
            && stderr.contains("Recheck match"),
        "the window should be rejected on its New event, then matched by a recheck: {stderr}"
    );
    let mut connection = connect();
    assert_eq!(
        get_node(&mut connection, container_id).app_id.as_deref(),
        Some("late-probe")
    );
}

#[test]
fn layout_launch_timeout_reports_its_step_and_rolls_back_earlier_steps() {
    fresh_workspace();
    // A wrong app_id in a layout step is the timeout a real user hits most,
    // and the one --rollback-on-error exists for: the error must name the
    // step, arrive at --timeout, and close what earlier steps launched --
    // and step 2's own window, which its filter rejected but whose process
    // carries the marker (#168).
    let path = TempToml::write(
        "timeout-rollback",
        "[[step]]\ncommand = \"foot --app-id=timeout-step-one\"\napp_id = \"timeout-step-one\"\n\n\
         [[step]]\ncommand = \"foot --app-id=timeout-step-two\"\napp_id = \"timeout-step-two-wrong\"\n",
    );

    let started = Instant::now();
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
            "--timeout",
            "2",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let elapsed = started.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(
        output.status.code(),
        Some(1),
        "step 2's filter matches nothing, so this must fail: {stderr}"
    );

    let last_line = stderr.lines().next_back().unwrap_or_default();
    let error: serde_json::Value =
        serde_json::from_str(last_line).expect("the last stderr line should be a JSON error");
    let _step_two_guards: Vec<KillOnDrop> = error["unmatched_windows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|window| window["container_id"].as_i64())
        .map(KillOnDrop)
        .collect();
    assert!(
        elapsed >= Duration::from_secs(2) && elapsed < Duration::from_secs(5),
        "step 2 should time out at --timeout 2, took {elapsed:?}"
    );

    let message = error["error"].as_str().expect("error should be a string");
    assert!(
        message.starts_with(&format!("{}: step 2: ", path.display()))
            && message.contains("appeared within 2 sec"),
        "the error should name the step that timed out: {message}"
    );
    let rolled_back = error["rolled_back"]
        .as_array()
        .unwrap_or_else(|| panic!("expected rolled_back in {error}"));
    let step_two_id = error["unmatched_windows"][0]["container_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected step 2's rejected window in {error}"));
    assert_eq!(
        error["unmatched_windows"][0]["launched_by_this_run"], true,
        "{error}"
    );
    assert_eq!(rolled_back.len(), 2, "{error}");
    assert_eq!(rolled_back[0], step_two_id, "most recent first: {error}");
    let step_one_id = rolled_back[1]
        .as_i64()
        .expect("rolled_back should hold container ids");
    let _step_one_guard = KillOnDrop(step_one_id);

    let mut connection = connect();
    wait_for_container_gone(&mut connection, step_one_id);
    wait_for_container_gone(&mut connection, step_two_id);
}

#[test]
fn launch_timeout_says_when_the_command_exited_without_a_window() {
    fresh_workspace();
    // A command that isn't on PATH exits at once, and Sway's exec reports
    // nothing -- this used to wait out --timeout and say only that it had.
    let output = sway_launch_command()
        .args(["--timeout", "1", "sway-launch-no-such-command-probe"])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "nothing can open a window: {stderr}"
    );
    assert!(
        stderr.contains("launch \"sway-launch-no-such-command-probe\": ")
            && stderr.contains("exited without opening a window"),
        "the error should say the command exited: {stderr}"
    );
}

#[test]
fn launch_timeout_says_a_command_that_ran_and_exited_may_have_raised_an_open_window() {
    fresh_workspace();
    // What a single-instance app does when a copy is already open: it runs,
    // raises that window and exits. That used to be blamed on PATH (#175).
    let output = sway_launch_command()
        .args(["--timeout", "2", "--app-id", "foot", "sleep 0.3"])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "nothing can open a window: {stderr}"
    );
    assert!(
        stderr.contains("ran and exited without opening a new window")
            && stderr.contains("--existing")
            && !stderr.contains("PATH"),
        "the error should point at --existing, not PATH: {stderr}"
    );
}

#[test]
fn launch_timeout_blames_the_command_not_the_filter_for_another_program_s_window() {
    fresh_workspace();
    // Another program opening a window during the wait used to replace the
    // explanation: a typo'd command was reported as a wrong --app-id (#161).
    let child = sway_launch_command()
        .args([
            "--timeout",
            "3",
            "--json",
            "--app-id",
            "foot",
            "sway-launch-no-such-command-probe",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    run_command_ok(
        &mut connect(),
        "exec sleep 0.5 && foot --app-id=foreign-window-probe",
    );
    let output = child
        .wait_with_output()
        .expect("failed to wait for sway-launch");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "nothing can open a matching window: {stderr}"
    );
    let last_line = stderr.lines().next_back().unwrap_or_default();
    let error: serde_json::Value =
        serde_json::from_str(last_line).expect("the last stderr line should be a JSON error");
    let unmatched = error["unmatched_windows"]
        .as_array()
        .unwrap_or_else(|| panic!("expected unmatched_windows in {error}"));
    assert_eq!(unmatched.len(), 1, "{error}");
    let container_id = unmatched[0]["container_id"]
        .as_i64()
        .expect("container_id should be an integer");
    let _guard = KillOnDrop(container_id);
    assert_eq!(unmatched[0]["app_id"], "foreign-window-probe", "{error}");
    assert_eq!(unmatched[0]["launched_by_this_run"], false, "{error}");

    let message = error["error"].as_str().expect("error should be a string");
    let exited = message
        .find("exited without opening a window")
        .unwrap_or_else(|| panic!("the error should say the command exited: {message}"));
    let foreign = message
        .find("not traced to this command")
        .unwrap_or_else(|| panic!("the error should list the other window: {message}"));
    assert!(
        exited < foreign && message.contains(&format!("con_id {container_id}")),
        "the command's exit should lead, the other window follow: {message}"
    );
}

#[test]
fn launch_timeout_says_a_command_that_cleared_its_environment_is_still_running() {
    fresh_workspace();
    // `env -i` drops the marker, so only Sway's own shell for the launch,
    // alive while the command runs, shows it hasn't exited (#152).
    let output = sway_launch_command()
        .args(["--timeout", "1", "env -i sleep 3"])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "nothing can open a window: {stderr}"
    );
    assert!(
        stderr.contains("the launched command is still running"),
        "the error should say the command is still running: {stderr}"
    );
}

/// `--list-windows --json`'s `windows` array, run with `extra_args`.
fn list_windows(extra_args: &[&str]) -> Vec<serde_json::Value> {
    let output = sway_launch_command()
        .args(["--list-windows", "--json"])
        .args(extra_args)
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "sway-launch --list-windows failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json output should be valid JSON");
    json["windows"].as_array().expect("a windows array").clone()
}

#[test]
fn list_windows_shows_where_each_window_is_and_what_existing_would_match() {
    let home = fresh_workspace();
    let (tiled, _tiled_guard) = launch_foot(&["--mark", "live-list-tiled"]);
    let (floating, _floating_guard) = launch_foot(&["--floating"]);
    let (hidden, _hidden_guard) = launch_foot(&["--mark", "live-list-hidden", "--scratchpad"]);

    let windows = list_windows(&[]);
    let row = |id: i64| {
        windows
            .iter()
            .find(|window| window["container_id"] == id)
            .unwrap_or_else(|| panic!("no row for {id}: {windows:?}"))
    };
    assert_eq!(row(tiled)["app_id"], "foot");
    assert_eq!(row(tiled)["workspace"], home.as_str());
    assert_eq!(row(tiled)["output"], original_output());
    assert_eq!(row(tiled)["marks"], serde_json::json!(["live-list-tiled"]));
    assert_eq!(row(tiled)["floating"], false);
    assert_eq!(row(floating)["floating"], true);
    assert_eq!(row(hidden)["scratchpad"], true);
    assert_eq!(row(hidden)["workspace"], serde_json::Value::Null);

    // The filter narrows the list to exactly what --existing resolves.
    let marked = list_windows(&["--mark-match", "live-list-hidden"]);
    assert_eq!(marked.len(), 1, "{marked:?}");
    let output = sway_launch_command()
        .args(["--existing", "--mark-match", "live-list-hidden"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        marked[0]["container_id"].to_string()
    );

    // The plain table names the scratchpad rather than __i3_scratch.
    let output = sway_launch_command()
        .args(["--list-windows", "--mark-match", "live-list-hidden"])
        .output()
        .expect("failed to run sway-launch binary");
    let table = String::from_utf8_lossy(&output.stdout);
    assert!(table.starts_with("ID "), "{table}");
    assert!(table.contains("(scratchpad)"), "{table}");
    assert!(!table.contains("__i3_scratch"), "{table}");
}

#[test]
fn list_windows_includes_a_marked_split_container() {
    fresh_workspace();
    let mut connection = connect();
    let (_beside, _beside_guard) = launch_foot(&[]);
    let (first, _first_guard) = launch_foot(&["--split", "v"]);
    let (_second, _second_guard) = launch_foot(&[]);
    run_command_ok(&mut connection, &format!("[con_id={first}] focus parent"));
    run_command_ok(&mut connection, "mark live-list-split");

    let rows = list_windows(&["--mark-match", "live-list-split"]);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["split_container"], true);
    assert_ne!(rows[0]["container_id"], first);
}

/// Every workspace name Sway currently has.
fn workspace_names(connection: &mut Connection) -> Vec<String> {
    connection
        .get_workspaces()
        .expect("get_workspaces should succeed")
        .into_iter()
        .map(|workspace| workspace.name)
        .collect()
}

#[test]
fn workspace_number_finds_a_numbered_and_labelled_workspace() {
    let home = fresh_workspace();
    let mut connection = connect();
    // A workspace exists only while it holds a window (or has focus), so the
    // labelled one gets a window of its own first.
    let (_occupant, _occupant_guard) = launch_foot(&["--workspace", "47: live-web"]);
    let (container_id, _guard) = launch_foot(&[]);

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--workspace-number",
            "47",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(single_action_status(&output.stdout), "changed");
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some("47: live-web")
    );
    assert!(
        !workspace_names(&mut connection).contains(&"47".to_string()),
        "no workspace literally named 47 should have been created"
    );
    assert_ne!(home, "47: live-web");

    let output = sway_launch_command()
        .args([
            "--con-id",
            &container_id.to_string(),
            "--workspace-number",
            "47",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(output.status.success());
    assert_eq!(single_action_status(&output.stdout), "already_satisfied");
}

#[test]
fn continue_on_timeout_carries_a_template_past_a_slot_whose_binding_timeout_ran_out() {
    fresh_workspace();
    let mut connection = connect();
    // The left slot's command exits without a window; its binding's own
    // one-second timeout, not the run's ten, decides when it gives up.
    let bindings = TempToml::write(
        "continue-on-timeout-bindings",
        "[[binding]]\nslot = \"left\"\ncommand = \"true\"\ntimeout = 1\n\n\
         [[binding]]\nslot = \"right\"\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--template",
            "dual-row",
            "--bindings",
            bindings.to_str().unwrap(),
            "--continue-on-timeout",
            "--timeout",
            "10",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout should be the run's JSON result ({error}); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let container_id = json["containers"]["right"]
        .as_i64()
        .unwrap_or_else(|| panic!("the right slot should have opened: {json}"));
    let _guard = KillOnDrop(container_id);

    assert_eq!(output.status.code(), Some(1), "{json}");
    assert_eq!(json["container_ids"], serde_json::json!([container_id]));
    assert_eq!(json["timed_out_steps"][0]["step"], 1, "{json}");
    let error = json["timed_out_steps"][0]["error"].as_str().unwrap();
    assert!(error.contains("appeared within 1 sec"), "{error}");
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert!(find_node(&tree, container_id).is_some());
}

#[test]
fn sigint_during_a_layout_rolls_back_what_it_launched() {
    fresh_workspace();
    let mut connection = connect();
    let path = TempToml::write(
        "interrupted-layout",
        "[[step]]\ncommand = \"foot --app-id=live-interrupted\"\napp_id = \"live-interrupted\"\n\n\
         [[step]]\ncommand = \"sleep 30\"\napp_id = \"never-opens\"\ntimeout = 30\n",
    );
    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start sway-launch binary");
    let started = std::time::Instant::now();
    while count_app_id_windows(
        &connection.get_tree().expect("get_tree should succeed"),
        "live-interrupted",
    ) == 0
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the first step's window never opened"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Long enough for the second step to be waiting on its window.
    std::thread::sleep(Duration::from_millis(500));
    let killed = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("failed to run kill");
    assert!(killed.success());

    let output = child.wait_with_output().expect("sway-launch should exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(130), "{stderr}");
    let error: serde_json::Value = serde_json::from_str(stderr.lines().next_back().unwrap())
        .unwrap_or_else(|error| panic!("no JSON error on stderr ({error}): {stderr}"));
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("interrupted by SIGINT"),
        "{error}"
    );
    let rolled_back = error["rolled_back"].as_array().unwrap();
    assert_eq!(rolled_back.len(), 1, "{error}");
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        count_app_id_windows(&tree, "live-interrupted"),
        0,
        "{error}"
    );
}

#[test]
fn sighup_during_a_concurrent_layout_rolls_back_and_leaves_no_staging_workspace() {
    let build_workspace = fresh_workspace();
    let mut connection = connect();
    // #128: the hangup a closed terminal sends, while both staged launches
    // are still starting. Left uncaught it killed the run with focus on the
    // staging workspace and both windows on it.
    let path = TempToml::write(
        "hangup-concurrent-layout",
        "concurrent = true\n\n\
         [[step]]\ncommand = \"sleep 1 && foot --app-id=live-hangup\"\napp_id = \"live-hangup\"\n\n\
         [[step]]\ncommand = \"sleep 1 && foot --app-id=live-hangup\"\napp_id = \"live-hangup\"\n",
    );
    let child = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start sway-launch binary");
    std::thread::sleep(Duration::from_millis(500));
    let killed = std::process::Command::new("kill")
        .args(["-HUP", &child.id().to_string()])
        .status()
        .expect("failed to run kill");
    assert!(killed.success());

    let output = child.wait_with_output().expect("sway-launch should exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(129), "{stderr}");
    let error: serde_json::Value = serde_json::from_str(stderr.lines().next_back().unwrap())
        .unwrap_or_else(|error| panic!("no JSON error on stderr ({error}): {stderr}"));
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("interrupted by SIGHUP"),
        "{error}"
    );
    no_staging_workspace_is_left(&mut connection);
    assert_eq!(focused_workspace_name(&mut connection), build_workspace);
    let tree = connection.get_tree().expect("get_tree should succeed");
    let left_open = containers_with_app_id(&tree, "live-hangup");
    let _guards: Vec<_> = left_open.iter().map(|&id| KillOnDrop(id)).collect();
    assert!(left_open.is_empty(), "{error}");
}

#[test]
fn rollback_closes_a_window_that_opens_just_after_its_launch_timed_out() {
    fresh_workspace();
    let mut connection = connect();
    // The command outlives its one-second timeout by half a second before
    // opening its window, which carries the launch's marker all the same.
    let path = TempToml::write(
        "late-window-layout",
        "[[step]]\ncommand = \"sleep 1.5 && foot --app-id=live-late-window\"\n\
         app_id = \"live-late-window\"\ntimeout = 1\n",
    );
    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--rollback-on-error",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_str(stderr.lines().next_back().unwrap())
        .unwrap_or_else(|error| panic!("no JSON error on stderr ({error}): {stderr}"));
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("opened within 2 sec after that"),
        "{error}"
    );
    assert_eq!(error["rolled_back"].as_array().unwrap().len(), 1, "{error}");
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        count_app_id_windows(&tree, "live-late-window"),
        0,
        "{error}"
    );
}

#[test]
fn on_output_builds_a_layout_on_that_output() {
    fresh_workspace();
    let mut connection = connect();
    let created_output = CreatedOutput::new(&mut connection);
    let new_output = created_output.name().to_string();
    let path = TempToml::write(
        "on-output",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--on-output",
            &new_output,
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json output should be valid JSON");
    let container_id = json["container_ids"][0]
        .as_i64()
        .unwrap_or_else(|| panic!("no container id in {json}"));
    let _guard = KillOnDrop(container_id);
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        output_containing(&tree, container_id, None),
        Some(new_output.as_str())
    );
}

#[test]
fn on_workspace_number_builds_a_layout_on_the_numbered_workspace() {
    fresh_workspace();
    let mut connection = connect();
    let (_occupant, _occupant_guard) = launch_foot(&["--workspace", "48: live-build"]);
    let path = TempToml::write(
        "on-workspace-number",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--on-workspace-number",
            "48",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json output should be valid JSON");
    let container_id = json["container_ids"][0]
        .as_i64()
        .unwrap_or_else(|| panic!("no container id in {json}"));
    let _guard = KillOnDrop(container_id);
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some("48: live-build")
    );
    assert!(!workspace_names(&mut connection).contains(&"48".to_string()));
}

/// Turns on `workspace_auto_back_and_forth` and turns it off again when
/// dropped, even on assertion panic, so no later test in this shared session
/// inherits it.
struct AutoBackAndForth;

impl AutoBackAndForth {
    fn enable(connection: &mut Connection) -> Self {
        run_command_ok(connection, "workspace_auto_back_and_forth yes");
        AutoBackAndForth
    }
}

impl Drop for AutoBackAndForth {
    fn drop(&mut self) {
        let _ = connect().run_command("workspace_auto_back_and_forth no");
    }
}

#[test]
fn on_workspace_builds_on_the_focused_workspace_under_auto_back_and_forth() {
    let home = fresh_workspace();
    let mut connection = connect();
    run_command_ok(
        &mut connection,
        "workspace live-auto-back-and-forth-previous",
    );
    run_command_ok(&mut connection, &format!("workspace \"{home}\""));
    let _auto_back_and_forth = AutoBackAndForth::enable(&mut connection);
    let path = TempToml::write(
        "on-workspace-auto-back-and-forth",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );

    let output = sway_launch_command()
        .args([
            "--layout",
            path.to_str().unwrap(),
            "--on-workspace",
            &home,
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json output should be valid JSON");
    let container_id = json["container_ids"][0]
        .as_i64()
        .unwrap_or_else(|| panic!("no container id in {json}"));
    let _guard = KillOnDrop(container_id);
    let tree = connection.get_tree().expect("get_tree should succeed");
    assert_eq!(
        workspace_containing(&tree, container_id, None),
        Some(home.as_str())
    );
    assert_eq!(focused_workspace_name(&mut connection), home);
}

#[test]
fn existing_wait_arranges_a_window_something_else_starts() {
    fresh_workspace();
    let mut connection = connect();
    // The window is started by Sway's own exec, not by sway-launch, after the
    // wait has begun: the autostart case --wait exists for.
    let waiting = sway_launch_command()
        .args([
            "--existing",
            "--wait",
            "-a",
            "live-existing-wait",
            "--floating",
            "--timeout",
            "10",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start sway-launch binary");
    std::thread::sleep(Duration::from_millis(500));
    run_command_ok(&mut connection, "exec foot --app-id=live-existing-wait");

    let output = waiting
        .wait_with_output()
        .expect("sway-launch should finish");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json output should be valid JSON");
    let container_id = json["container_id"]
        .as_i64()
        .unwrap_or_else(|| panic!("no container id in {json}"));
    let _guard = KillOnDrop(container_id);
    assert_eq!(single_action_status(&output.stdout), "changed");
    let node = get_node(&mut connection, container_id);
    assert_eq!(node.app_id.as_deref(), Some("live-existing-wait"));
    assert!(node_is_floating(&node));
}

#[test]
fn existing_wait_in_a_layout_step_times_out_naming_the_window() {
    fresh_workspace();
    let path = TempToml::write(
        "existing-wait-timeout",
        "[[step]]\nexisting = true\nwait = true\napp_id = \"live-never-opens\"\ntimeout = 1\n",
    );
    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no window with app_id \"live-never-opens\" appeared within 1 sec"),
        "{stderr}"
    );
}

/// Launches `foot -T <title>`, matched on app_id and `title_pattern`.
fn launch_titled_foot(title: &str, title_pattern: &str) -> (i64, KillOnDrop) {
    let output = sway_launch_command()
        .args([
            "-a",
            "foot",
            "--title",
            title_pattern,
            "--timeout",
            "10",
            "--",
            // `sleep` rather than a shell: Arch's bash prompt retitles the
            // window, and these windows are told apart by title.
            &format!("foot -T {title} sleep 60"),
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let container_id: i64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("stdout should be a container id");
    (container_id, KillOnDrop(container_id))
}

/// A title pattern with no other matcher is tested against windows only, as
/// `--list-windows` lists them: not the workspace whose name it also matches,
/// nor the scratchpad's `__i3_scratch` (#144).
#[test]
fn existing_title_alone_matches_only_windows() {
    let workspace = fresh_workspace();
    let (window, _guard) =
        launch_titled_foot("live-existing-title-alone", "^live-existing-title-alone$");
    let pattern = "existing.title.alone";
    assert!(
        regex_lite::Regex::new(pattern)
            .unwrap()
            .is_match(&workspace),
        "the pattern should match the workspace name {workspace:?} too"
    );

    let output = sway_launch_command()
        .args(["--existing", "--title", pattern])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        window.to_string()
    );

    let output = sway_launch_command()
        .args(["--existing", "--title", "__i3_scratch"])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No existing window matches"), "{stderr}");
}

#[test]
fn title_matches_a_launch_and_picks_one_of_several_existing_windows() {
    fresh_workspace();
    let mut connection = connect();
    let (left, _left_guard) = launch_titled_foot("live-title-left", "^live-title-left$");
    let (right, _right_guard) = launch_titled_foot("live-title-right", "title-right");
    assert_eq!(
        get_node(&mut connection, right).name.as_deref(),
        Some("live-title-right")
    );

    let output = sway_launch_command()
        .args([
            "--existing",
            "-a",
            "foot",
            "--title",
            "^live-title-left$",
            "--floating",
            "--json",
        ])
        .output()
        .expect("failed to run sway-launch binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(single_action_status(&output.stdout), "changed");
    assert!(node_is_floating(&get_node(&mut connection, left)));
    assert!(!node_is_floating(&get_node(&mut connection, right)));

    let rows = list_windows(&["--title", "^live-title-"]);
    let ids: Vec<i64> = rows
        .iter()
        .filter_map(|row| row["container_id"].as_i64())
        .collect();
    assert_eq!(ids.len(), 2, "{rows:?}");
    assert!(ids.contains(&left) && ids.contains(&right), "{rows:?}");
}

/// A stand-in for a workspace-icon daemon (sworkstyle, swaywsr and the
/// like): on every new or moved window, renames that window's workspace to
/// `<base>: <one "f" per window>`, which keeps its id. Only workspaces named
/// `base` or `base: ...` are touched, and it stops acting once dropped, so it
/// never reaches another test's workspace.
struct RenamingDaemon(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl RenamingDaemon {
    fn start(base: &str) -> Self {
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (ready, subscribed) = mpsc::channel();
        let base = base.to_string();
        let thread_stopped = stopped.clone();
        std::thread::spawn(move || {
            let events = connect()
                .subscribe([swayipc::EventType::Window])
                .expect("the renaming daemon should subscribe");
            let _ = ready.send(());
            let mut connection = connect();
            for event in events {
                if thread_stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let Ok(swayipc::Event::Window(event)) = event else {
                    continue;
                };
                if !matches!(
                    event.change,
                    swayipc::WindowChange::New | swayipc::WindowChange::Move
                ) {
                    continue;
                }
                let Ok(tree) = connection.get_tree() else {
                    continue;
                };
                let Some(workspace) = workspace_node_containing(&tree, event.container.id) else {
                    continue;
                };
                let name = workspace.name.clone().unwrap_or_default();
                if name != base && !name.starts_with(&format!("{base}: ")) {
                    continue;
                }
                let renamed = format!("{base}: {}", "f".repeat(view_count(workspace)));
                if renamed != name {
                    let _ = connection
                        .run_command(format!("rename workspace \"{name}\" to \"{renamed}\""));
                }
            }
        });
        subscribed
            .recv_timeout(Duration::from_secs(5))
            .expect("the renaming daemon should be subscribed");
        RenamingDaemon(stopped)
    }
}

impl Drop for RenamingDaemon {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn workspace_node_containing(node: &Node, con_id: i64) -> Option<&Node> {
    node.nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .find_map(|child| {
            if child.node_type == NodeType::Workspace {
                find_node(child, con_id).map(|_| child)
            } else {
                workspace_node_containing(child, con_id)
            }
        })
}

fn view_count(node: &Node) -> usize {
    let own = usize::from(node.pid.is_some());
    own + node
        .nodes
        .iter()
        .chain(node.floating_nodes.iter())
        .map(view_count)
        .sum::<usize>()
}

/// Runs a two-foot layout with a renaming daemon on its workspace, and
/// checks both windows end up together on that workspace, renamed (#178).
fn assert_a_renamed_workspace_holds_the_whole_layout(extra_arguments: &[&str]) {
    let base = fresh_workspace();
    let mut connection = connect();
    let _daemon = RenamingDaemon::start(&base);
    let path = TempToml::write(
        "renamed-workspace",
        "[[step]]\ncommand = \"foot\"\napp_id = \"foot\"\nsplit = \"h\"\n\n\
         [[step]]\ncommand = \"foot\"\napp_id = \"foot\"\n",
    );
    let output = sway_launch_command()
        .args(["--layout", path.to_str().unwrap()])
        .args(extra_arguments)
        .output()
        .expect("failed to run sway-launch binary");
    let ids: Vec<i64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.parse().ok())
        .collect();
    let _guards: Vec<_> = ids.iter().map(|&id| KillOnDrop(id)).collect();
    assert!(
        output.status.success(),
        "the layout failed under a renaming daemon: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(ids.len(), 2, "{ids:?}");
    let first = workspace_of(&mut connection, ids[0]);
    assert!(
        first.starts_with(&format!("{base}: ")),
        "the daemon never renamed {base:?}: the window is on {first:?}"
    );
    assert_eq!(
        workspace_of(&mut connection, ids[1]),
        first,
        "the layout was split across two workspaces"
    );
    no_staging_workspace_is_left(&mut connection);
}

#[test]
fn a_layout_survives_a_daemon_renaming_its_workspace() {
    // #178: a rename landing while a step's window opens used to fail the
    // step with "focus moved". Here it usually lands just after; the
    // fake-Sway suite pins the ordering where it lands during.
    assert_a_renamed_workspace_holds_the_whole_layout(&[]);
}

#[test]
fn a_concurrent_layout_survives_a_daemon_renaming_its_workspace() {
    // #178: moving the second staged window by the old name used to create
    // a second workspace under it, splitting the layout and exiting 0.
    assert_a_renamed_workspace_holds_the_whole_layout(&["--concurrent"]);
}
