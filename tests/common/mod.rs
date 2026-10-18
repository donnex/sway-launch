// Helpers shared by the integration test files under tests/. Each of those
// files is compiled as its own crate and pulls this in with `mod common;`,
// so a helper only one of them uses is dead code in the others -- hence the
// crate-wide allow below rather than one per item.
#![allow(dead_code)]

pub mod fake_sway;

use std::fs::{self, DirBuilder};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::ErrorKind;
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};

/// A private scratch directory for one test's fixtures and marker files,
/// removed again (with everything in it) when it goes out of scope, even if
/// an assertion panics mid-test.
///
/// The name carries a random suffix and the directory is created with a
/// plain `create()` at mode 0700, so an existing directory or symlink at that
/// path is an error rather than silently reused. That is what makes a shared
/// temp directory safe here: another local user can neither predict the name
/// in advance nor pre-create it, so they can't plant a symlink inside it to
/// redirect a fixture write, or swap a layout file between the test writing
/// it and `sway-launch` running it (several live-suite layouts have
/// `command` steps). A name built from the process id alone was neither --
/// pids are small and sequential, and `create_dir_all()` accepts a directory
/// someone else already made. It also keeps tests from colliding with each
/// other: each one gets its own directory, so two that pick the same fixture
/// name never share a path, and tests/live_sway.rs's injection tests, which
/// assert a marker file does *not* exist, can't fail because of a stray file.
pub struct TestTempDir(PathBuf);

impl TestTempDir {
    pub fn new() -> Self {
        Self::new_in(&std::env::temp_dir())
    }

    /// A `TestTempDir` for a Unix socket, under `/tmp` rather than `TMPDIR`
    /// (#130). A socket path is limited to about 108 bytes, so a long `TMPDIR`
    /// (a sandbox's or CI container's per-session directory) leaves no room
    /// for the socket's own name and `bind()` fails. `scripts/` does the same
    /// for the compositor's `XDG_RUNTIME_DIR`.
    pub fn for_sockets() -> Self {
        Self::new_in(Path::new("/tmp"))
    }

    fn new_in(parent: &Path) -> Self {
        for _ in 0..16 {
            let path = parent.join(format!(
                "sway-launch-test-{}-{:016x}",
                process::id(),
                RandomState::new().build_hasher().finish()
            ));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("failed to create test temp directory {path:?}: {error}"),
            }
        }
        panic!("failed to find an unused test temp directory name in {parent:?}");
    }
}

impl Deref for TestTempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A TOML file fixture (layout, template, or bindings) written into its own
/// `TestTempDir`, removed again when it goes out of scope.
pub struct TempToml {
    path: PathBuf,
    _directory: TestTempDir,
}

impl TempToml {
    pub fn write(name: &str, contents: &str) -> Self {
        let directory = TestTempDir::new();
        let path = directory.join(format!("{}.toml", name));
        fs::write(&path, contents).expect("failed to write temp fixture file");
        Self {
            path,
            _directory: directory,
        }
    }
}

impl Deref for TempToml {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

/// The binary, isolated from the session `cargo test` runs in: pointed at a
/// socket path that cannot exist, and at a private `XDG_RUNTIME_DIR`. Every
/// test under `tests/` but the live and fake-Sway suites (which aim it at
/// their own compositor) runs the binary through this, and
/// `integration_tests_run_the_binary_only_through_the_shared_helpers` in
/// `src/main/tests.rs` holds them to it (#137).
///
/// Left to the ambient environment, a test is only kept off the compositor by
/// details of the code under test: that it passes `--dry-run`, or that a run
/// of bare `con_id` steps happens to make no IPC call. In a terminal inside a
/// Sway session -- where this project is developed, and where CLAUDE.md's own
/// "run `cargo test` before committing" gate runs -- the first change that
/// reads the tree up front would have those tests act on the developer's real
/// windows. It was already worse than a false failure for the `--con-id 42`
/// cases: with a session reachable, whether the command failed depended on
/// whether container id 42 happened to exist, and if it did, the test floated
/// a real window.
///
/// `I3SOCK` is removed as well as `SWAYSOCK` set. `socket_path()` reads
/// `SWAYSOCK` first, so the override alone would do today, but both together
/// mean the connect attempt fails on a missing path rather than reaching a
/// compositor no matter what the environment holds or which order wins. The
/// runtime directory keeps a `--layout`/`--template` run's lock file out of
/// the developer's own `$XDG_RUNTIME_DIR/sway-launch/`; a test may still set
/// its own, which replaces it.
pub fn sway_launch() -> SwayLaunchCommand {
    let runtime_directory = TestTempDir::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
    command
        .env("SWAYSOCK", "/nonexistent/sway-launch-test-socket")
        .env_remove("I3SOCK")
        .env("XDG_RUNTIME_DIR", &*runtime_directory);
    SwayLaunchCommand {
        command,
        _runtime_directory: runtime_directory,
    }
}

/// `sway_launch()`'s `Command`, owning the runtime directory it points at so
/// the directory lives as long as the command does. It derefs to `Command`,
/// so a call chained onto `sway_launch()` reads like `Command`'s own API.
pub struct SwayLaunchCommand {
    command: Command,
    _runtime_directory: TestTempDir,
}

impl Deref for SwayLaunchCommand {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

impl DerefMut for SwayLaunchCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

/// Asserts that `output` is clap's usage error for `flag` conflicting with
/// `conflicting` -- not merely a failure.
///
/// A bare `!output.status.success()` passes for the wrong reason once the
/// conflict is gone: the binary then runs the combination and usually fails
/// later anyway (a layout parser rejecting a template's fields, an IPC call
/// with no socket), so the test keeps passing while the regression it pins
/// has already happened. clap's usage errors exit 2 and runtime failures
/// exit 1, so the exit code plus clap's own wording tells the two apart.
/// `flag` is matched as a prefix of clap's rendering, so `"--layout"` matches
/// `'--layout <LAYOUT>'`.
pub fn assert_conflict_error(output: &Output, flag: &str, conflicting: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{flag} with {conflicting} should be a usage error (exit 2): {stderr}"
    );
    assert!(
        stderr.contains(&format!("the argument '{flag}")) && stderr.contains("cannot be used with"),
        "expected clap's conflict error for {flag}: {stderr}"
    );
    assert!(
        stderr.contains(conflicting),
        "expected the conflict error to name {conflicting}: {stderr}"
    );
}

/// Runs the binary with `args`, writing `stdin` to it, for the inputs that
/// accept `-` (#50).
pub fn run_with_stdin(args: &[&str], stdin: &str) -> std::process::Output {
    use std::io::Write;
    let mut command = sway_launch();
    let mut child = command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run sway-launch binary");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(stdin.as_bytes())
        .expect("writing stdin should succeed");
    child
        .wait_with_output()
        .expect("failed to wait for sway-launch")
}
