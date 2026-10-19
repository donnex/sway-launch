//! A fake Sway IPC server, so the binary's confirmation drivers run headlessly
//! (#103).
//!
//! It sits on the far side of the socket rather than behind a trait: the
//! binary is pointed at it through `SWAYSOCK`, exactly as it would be at a
//! real compositor, and nothing in `src/` knows it exists. It speaks the
//! i3-ipc framing (`i3-ipc`, a native-endian `u32` payload length, a `u32`
//! message type, then the JSON payload) and answers the four requests the
//! binary makes — `RUN_COMMAND`, `SUBSCRIBE`, `GET_OUTPUTS`, `GET_TREE` — plus
//! `GET_VERSION`.
//!
//! The tree, outputs and command replies start from a recording of a real
//! compositor (`tests/fake-sway/sway-<version>/`, written by
//! `scripts/capture-fake-sway-fixtures`), so version differences such as
//! Sway 1.9 leaving `floating` unset are replayed rather than assumed. What
//! happens when a command arrives is up to each test: `FakeSway::start()`
//! takes a handler that sees every command and decides the reply, and can
//! change the tree and emit window events on the way, now or later. With no
//! handler a command succeeds and changes nothing, which is how "Sway accepted
//! it, then nothing happened" is tested.

use super::TestTempDir;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const SUBSCRIBE: u32 = 2;
const GET_OUTPUTS: u32 = 3;
const GET_TREE: u32 = 4;
const GET_VERSION: u32 = 7;
const WINDOW_EVENT: u32 = (1 << 31) | 3;

/// The fixtures' tiled window on workspace `1`.
pub const TILED: i64 = 5;
/// The fixtures' floating window on workspace `1`.
pub const FLOATING: i64 = 6;
/// The fixtures' window hidden in the scratchpad.
pub const SCRATCHPAD: i64 = 7;
/// The fixtures' one ordinary workspace, `1`, which has focus.
pub const WORKSPACE: i64 = 4;

/// The environment variable the binary tags a launched command with.
const PID_MARKER_VAR: &str = "SWAY_LAUNCH_PID_MARKER";

/// A Sway release a fixture directory was recorded from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwayVersion {
    V1_9,
    V1_11,
}

impl SwayVersion {
    /// Every recorded version, oldest first: the range the project supports.
    pub const ALL: [SwayVersion; 2] = [SwayVersion::V1_9, SwayVersion::V1_11];

    fn fixture_directory(self) -> PathBuf {
        let name = match self {
            SwayVersion::V1_9 => "sway-1.9",
            SwayVersion::V1_11 => "sway-1.11",
        };
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fake-sway")
            .join(name)
    }
}

type Handler = Box<dyn FnMut(&mut Sway, &str) -> Result<(), String> + Send>;

/// The fake compositor's state, as a command handler or a test sees it.
pub struct Sway {
    version: SwayVersion,
    tree: Value,
    outputs: Value,
    version_reply: Value,
    missing_container_error: String,
    window_template: Value,
    next_id: i64,
    commands: Vec<String>,
    subscribers: Vec<UnixStream>,
    handler: Option<Handler>,
    launched_pids: Vec<u32>,
    refusing_tree_reads: bool,
    tree_reads: usize,
    shared: Weak<Mutex<Sway>>,
}

/// A running fake server. Dropping it stops the server and kills every
/// command it launched.
pub struct FakeSway {
    shared: Arc<Mutex<Sway>>,
    socket: PathBuf,
    stopping: Arc<AtomicBool>,
    directory: TestTempDir,
}

impl FakeSway {
    /// Starts a server replaying `version`'s fixtures, answering every
    /// command through `handler`.
    pub fn start(
        version: SwayVersion,
        handler: impl FnMut(&mut Sway, &str) -> Result<(), String> + Send + 'static,
    ) -> Self {
        Self::start_with(version, Some(Box::new(handler)))
    }

    /// Starts a server on which every command succeeds and changes nothing.
    pub fn accepting_everything(version: SwayVersion) -> Self {
        Self::start_with(version, None)
    }

    fn start_with(version: SwayVersion, handler: Option<Handler>) -> Self {
        let directory = TestTempDir::for_sockets();
        // Named as Sway names its own, so the binary's search for a running
        // Sway in `$XDG_RUNTIME_DIR` (#162) finds this one.
        let uid = std::fs::metadata(&*directory)
            .expect("the fake Sway's directory should exist")
            .uid();
        let socket = directory.join(format!("sway-ipc.{}.{}.sock", uid, std::process::id()));
        let listener = UnixListener::bind(&socket).expect("failed to bind the fake Sway socket");
        let shared = Arc::new_cyclic(|shared| Mutex::new(Sway::load(version, handler, shared)));
        let stopping = Arc::new(AtomicBool::new(false));

        let accepting = Arc::downgrade(&shared);
        let accept_stopping = Arc::clone(&stopping);
        thread::spawn(move || {
            for stream in listener.incoming() {
                if accept_stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { break };
                let connection = accepting.clone();
                thread::spawn(move || serve(stream, connection));
            }
        });

        Self {
            shared,
            socket,
            stopping,
            directory,
        }
    }

    /// The binary, pointed at this server and nothing else.
    ///
    /// `I3SOCK` is removed for the same reason `common::sway_launch()`
    /// removes it, and `XDG_RUNTIME_DIR` is this server's own directory, so a
    /// layout run's lock file never meets another test's.
    pub fn sway_launch(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
        command
            .env("SWAYSOCK", &self.socket)
            .env_remove("I3SOCK")
            .env("XDG_RUNTIME_DIR", &*self.directory);
        command
    }

    /// The server's socket, which `sway_launch()` names in `SWAYSOCK`.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The server's state, locked: for a test to inspect, or to change the
    /// tree from outside a command, as another client would.
    pub fn sway(&self) -> MutexGuard<'_, Sway> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Every command the binary sent, in order.
    pub fn commands(&self) -> Vec<String> {
        self.sway().commands.clone()
    }

    /// How many times the binary has read the tree so far.
    pub fn tree_reads(&self) -> usize {
        self.sway().tree_reads
    }
}

impl Drop for FakeSway {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag.
        let _ = UnixStream::connect(&self.socket);
        // Each launch leads its own process group, so signalling the group
        // takes the command the shell started with it, not just the shell.
        for pid in std::mem::take(&mut self.sway().launched_pids) {
            let _ = Command::new("kill")
                .args(["-TERM", "--", &format!("-{pid}")])
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Sway {
    fn load(version: SwayVersion, handler: Option<Handler>, shared: &Weak<Mutex<Sway>>) -> Self {
        let directory = version.fixture_directory();
        let read = |name: &str| -> Value {
            let path = directory.join(name);
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("failed to read fixture {path:?}: {error}"));
            serde_json::from_str(&contents)
                .unwrap_or_else(|error| panic!("fixture {path:?} is not JSON: {error}"))
        };
        let tree = read("tree.json");
        let missing_container_error = read("missing-container-reply.json")[0]["error"]
            .as_str()
            .expect("missing-container-reply.json has no error")
            .to_string();
        let window_template = find(&tree, TILED)
            .expect("the tree fixture has no tiled window")
            .clone();
        Self {
            version,
            outputs: read("outputs.json"),
            version_reply: read("version.json"),
            missing_container_error,
            window_template,
            next_id: 100,
            commands: Vec::new(),
            subscribers: Vec::new(),
            handler,
            launched_pids: Vec::new(),
            refusing_tree_reads: false,
            tree_reads: 0,
            shared: shared.clone(),
            tree,
        }
    }

    /// The node with `id`, as Sway would report it.
    pub fn node(&self, id: i64) -> &Value {
        find(&self.tree, id).unwrap_or_else(|| panic!("no node {id} in the fake tree"))
    }

    pub fn node_mut(&mut self, id: i64) -> &mut Value {
        find_mut(&mut self.tree, id).unwrap_or_else(|| panic!("no node {id} in the fake tree"))
    }

    /// Replaces the recorded `GET_VERSION` reply, standing in for a
    /// compositor the fixtures weren't recorded from: an older Sway, or i3.
    pub fn set_version_reply(&mut self, reply: Value) {
        self.version_reply = reply;
    }

    /// From now on, closes a connection that asks for the tree instead of
    /// answering, the way a broken connection or a compositor going away
    /// looks to the binary: the read fails.
    pub fn refuse_tree_reads(&mut self) {
        self.refusing_tree_reads = true;
    }

    pub fn contains(&self, id: i64) -> bool {
        find(&self.tree, id).is_some()
    }

    /// Floats tiled window `id`, the way this version reports it: moved to
    /// its workspace's `floating_nodes` as a `floating_con`, with `floating`
    /// set on Sway 1.11 and left unset on 1.9.
    pub fn float(&mut self, id: i64) {
        let workspace = self.workspace_of(id);
        let mut node = self.remove(id);
        node["type"] = json!("floating_con");
        node["floating"] = match self.version {
            SwayVersion::V1_9 => Value::Null,
            SwayVersion::V1_11 => json!("user_on"),
        };
        self.node_mut(workspace)["floating_nodes"]
            .as_array_mut()
            .expect("floating_nodes is an array")
            .push(node);
    }

    /// Removes node `id` from the tree and returns it.
    pub fn remove(&mut self, id: i64) -> Value {
        remove(&mut self.tree, id).unwrap_or_else(|| panic!("no node {id} in the fake tree"))
    }

    /// The id of the workspace holding `id`.
    pub fn workspace_of(&self, id: i64) -> i64 {
        workspace_of(&self.tree, id, None).unwrap_or_else(|| panic!("node {id} is on no workspace"))
    }

    /// Adds a new tiled window to workspace `1` and returns its id. Shaped
    /// like the fixture's own tiled window, with `app_id` and `pid` replaced.
    pub fn open_window(&mut self, app_id: Option<&str>, pid: Option<u32>) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut node = self.window_template.clone();
        node["id"] = json!(id);
        node["app_id"] = json!(app_id);
        node["pid"] = json!(pid);
        node["focused"] = json!(false);
        self.node_mut(WORKSPACE)["nodes"]
            .as_array_mut()
            .expect("nodes is an array")
            .push(node);
        id
    }

    /// Sends every subscriber a window event with `change` for node `id`,
    /// carrying the node as it is now.
    pub fn emit(&mut self, change: &str, id: i64) {
        let payload = json!({ "change": change, "container": self.node(id) }).to_string();
        let frame = frame(WINDOW_EVENT, &payload);
        self.subscribers
            .retain_mut(|subscriber| subscriber.write_all(&frame).is_ok());
    }

    /// Runs `change` against the state after `delay`, on its own thread, as
    /// Sway does work that finishes after its reply.
    pub fn after(&self, delay: Duration, change: impl FnOnce(&mut Sway) + Send + 'static) {
        let shared = self.shared.clone();
        thread::spawn(move || {
            thread::sleep(delay);
            if let Some(shared) = shared.upgrade() {
                change(
                    &mut shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                );
            }
        });
    }

    /// Runs an `exec` command the way Sway does, through `sh -c`, and
    /// returns the marker token the binary tagged it with. The shell leads
    /// its own process group, so it and everything it started are killed
    /// when the server is dropped.
    pub fn launch(&mut self, command: &str) -> String {
        let shell_command = command
            .strip_prefix("exec ")
            .unwrap_or_else(|| panic!("not an exec command: {command}"));
        let token = marker_token(shell_command)
            .unwrap_or_else(|| panic!("exec command carries no marker: {command}"));
        let mut child = Command::new("sh")
            .args(["-c", shell_command])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to run the launched command");
        self.launched_pids.push(child.id());
        // Reaped here so an exited command leaves no zombie behind for the
        // binary's /proc scan to read.
        thread::spawn(move || child.wait());
        token
    }

    /// Starts a process that has nothing to do with any launch (no marker,
    /// not descended from a launch's shell), standing in for one the user
    /// started themselves, and returns its pid. Killed with the fake.
    pub fn start_foreign_process(&mut self) -> u32 {
        let mut child = Command::new("sleep")
            .arg("30")
            .env_remove("SWAY_LAUNCH_PID_MARKER")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start a foreign process");
        let pid = child.id();
        self.launched_pids.push(pid);
        thread::spawn(move || child.wait());
        pid
    }

    /// Runs `change` with the pid of the process carrying `token`'s marker,
    /// once one exists — the process a real launched app's window would
    /// belong to. Gives up after five seconds, which fails the test through
    /// its own timeout.
    pub fn when_marked(&self, token: String, change: impl FnOnce(&mut Sway, u32) + Send + 'static) {
        let shared = self.shared.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Some(pid) = marked_process(&token) {
                    if let Some(shared) = shared.upgrade() {
                        change(
                            &mut shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                            pid,
                        );
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
    }

    /// Answers one `RUN_COMMAND`, with one outcome per `,`-separated part of
    /// a `[con_id=N] a, b` command, each handed to the handler as
    /// `[con_id=N] a` and so on, as Sway applies the criteria to every part.
    /// A part for a container not in the tree fails the way the recorded
    /// compositor's did, before the handler sees it. Every part runs even
    /// after one fails, as on real Sway. `commands()` records what was sent,
    /// unsplit.
    fn run_command(&mut self, command: &str) -> Value {
        self.commands.push(command.to_string());
        let outcomes: Vec<Value> = sub_commands(command)
            .into_iter()
            .map(|part| {
                let outcome = match target_con_id(&part) {
                    Some(id) if !self.contains(id) => Err(self.missing_container_error.clone()),
                    _ => match self.handler.take() {
                        Some(mut handler) => {
                            let outcome = handler(self, &part);
                            self.handler = Some(handler);
                            outcome
                        }
                        None => Ok(()),
                    },
                };
                match outcome {
                    Ok(()) => json!({ "success": true }),
                    Err(error) => json!({ "success": false, "parse_error": false, "error": error }),
                }
            })
            .collect();
        Value::Array(outcomes)
    }
}

/// `command` split the way Sway splits a criteria-led command: the
/// `[...]` prefix repeated in front of each part, parts separated by a `,`
/// outside double quotes. A command without a criteria prefix (a launch's
/// `exec`) is one part, whatever it contains.
fn sub_commands(command: &str) -> Vec<String> {
    let Some((criteria, rest)) = command
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("] "))
    else {
        return vec![command.to_string()];
    };
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in rest.chars() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| format!("[{criteria}] {}", part.trim()))
        .collect()
}

/// Serves one client connection until it closes.
fn serve(mut stream: UnixStream, shared: Weak<Mutex<Sway>>) {
    loop {
        let mut header = [0u8; 14];
        if stream.read_exact(&mut header).is_err() {
            return;
        }
        assert_eq!(
            &header[..6],
            MAGIC,
            "the binary sent a frame without the i3-ipc magic"
        );
        let length = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
        let message_type = u32::from_ne_bytes(header[10..14].try_into().unwrap());
        let mut payload = vec![0u8; length];
        if stream.read_exact(&mut payload).is_err() {
            return;
        }
        let payload = String::from_utf8(payload).expect("the binary sent a non-UTF-8 payload");

        let Some(shared) = shared.upgrade() else {
            return;
        };
        let mut sway = shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reply = match message_type {
            RUN_COMMAND => sway.run_command(&payload),
            SUBSCRIBE => {
                let events: Vec<String> =
                    serde_json::from_str(&payload).expect("SUBSCRIBE payload is a JSON list");
                if events.iter().any(|event| event == "window") {
                    if let Ok(subscriber) = stream.try_clone() {
                        sway.subscribers.push(subscriber);
                    }
                }
                json!({ "success": true })
            }
            GET_OUTPUTS => sway.outputs.clone(),
            GET_TREE if sway.refusing_tree_reads => return,
            GET_TREE => {
                sway.tree_reads += 1;
                sway.tree.clone()
            }
            GET_VERSION => sway.version_reply.clone(),
            other => panic!("the fake Sway doesn't answer message type {other}"),
        };
        // Written while the lock is held, so an event emitted by a later
        // command can't overtake this reply on a subscribed connection.
        if stream
            .write_all(&frame(message_type, &reply.to_string()))
            .is_err()
        {
            return;
        }
    }
}

fn frame(message_type: u32, payload: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    frame.extend_from_slice(&message_type.to_ne_bytes());
    frame.extend_from_slice(payload.as_bytes());
    frame
}

fn children(node: &Value) -> impl Iterator<Item = &Value> {
    ["nodes", "floating_nodes"]
        .into_iter()
        .filter_map(|key| node[key].as_array())
        .flatten()
}

fn find(node: &Value, id: i64) -> Option<&Value> {
    if node["id"].as_i64() == Some(id) {
        return Some(node);
    }
    children(node).find_map(|child| find(child, id))
}

fn find_mut(node: &mut Value, id: i64) -> Option<&mut Value> {
    if node["id"].as_i64() == Some(id) {
        return Some(node);
    }
    node.as_object_mut()?
        .iter_mut()
        .filter(|(key, _)| *key == "nodes" || *key == "floating_nodes")
        .filter_map(|(_, children)| children.as_array_mut())
        .flatten()
        .find_map(|child| find_mut(child, id))
}

fn remove(node: &mut Value, id: i64) -> Option<Value> {
    for key in ["nodes", "floating_nodes"] {
        let Some(nodes) = node[key].as_array_mut() else {
            continue;
        };
        if let Some(index) = nodes
            .iter()
            .position(|child| child["id"].as_i64() == Some(id))
        {
            return Some(nodes.remove(index));
        }
        if let Some(removed) = nodes.iter_mut().find_map(|child| remove(child, id)) {
            return Some(removed);
        }
    }
    None
}

fn workspace_of(node: &Value, id: i64, workspace: Option<i64>) -> Option<i64> {
    let workspace = match node["type"].as_str() {
        Some("workspace") => node["id"].as_i64(),
        _ => workspace,
    };
    if node["id"].as_i64() == Some(id) {
        return workspace;
    }
    children(node).find_map(|child| workspace_of(child, id, workspace))
}

/// The `N` of a command starting `[con_id=N]`.
fn target_con_id(command: &str) -> Option<i64> {
    command
        .strip_prefix("[con_id=")?
        .split_once(']')?
        .0
        .parse()
        .ok()
}

/// The token in a launch's `export SWAY_LAUNCH_PID_MARKER=<token>` prefix.
fn marker_token(shell_command: &str) -> Option<String> {
    let prefix = format!("export {PID_MARKER_VAR}=");
    let rest = shell_command.strip_prefix(&prefix)?;
    Some(rest.split_whitespace().next()?.to_string())
}

/// The pid of a running process whose environment carries `token`'s marker.
pub fn marked_process(token: &str) -> Option<u32> {
    let entry = format!("{PID_MARKER_VAR}={token}");
    std::fs::read_dir("/proc")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|process| process.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
                environ
                    .split(|byte| *byte == 0)
                    .any(|variable| variable == entry.as_bytes())
            })
        })
}
