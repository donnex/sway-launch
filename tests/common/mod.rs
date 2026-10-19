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
///
/// `PATH` starts with `path_with_swaynag_stub()`'s stand-in, which records
/// each call under the runtime directory for `swaynag_calls()` to read.
pub fn sway_launch() -> SwayLaunchCommand {
    let runtime_directory = TestTempDir::new();
    let swaynag_calls = runtime_directory.join("swaynag-calls");
    fs::create_dir(&swaynag_calls).expect("failed to create the swaynag call directory");
    let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
    command
        .env("SWAYSOCK", "/nonexistent/sway-launch-test-socket")
        .env_remove("I3SOCK")
        .env("XDG_RUNTIME_DIR", &*runtime_directory)
        .env("PATH", path_with_swaynag_stub())
        .env("SWAYNAG_STUB_CALLS", &swaynag_calls);
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

impl SwayLaunchCommand {
    /// The argument lists the stand-in `swaynag` was run with, one string per
    /// call, arguments joined by spaces. swaynag is spawned and not waited
    /// for, so this waits up to `wait` for a first call to land; pass a short
    /// `wait` to check that none does.
    pub fn swaynag_calls(&self, wait: std::time::Duration) -> Vec<String> {
        let directory = self._runtime_directory.join("swaynag-calls");
        let deadline = std::time::Instant::now() + wait;
        loop {
            let mut calls: Vec<String> = fs::read_dir(&directory)
                .expect("the swaynag call directory should be readable")
                .map(|entry| entry.expect("directory entry should be readable").path())
                .filter(|path| {
                    !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                })
                .map(|path| fs::read_to_string(path).expect("a swaynag call should be readable"))
                .collect();
            if !calls.is_empty() || std::time::Instant::now() >= deadline {
                calls.sort();
                return calls;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// `PATH` with a stand-in `swaynag` ahead of everything else (#148).
///
/// A failed run whose stderr isn't a terminal shows its error with swaynag,
/// and every test runs the binary with stderr piped, so without this a test
/// inside a Sway session would put a real swaynag bar on the developer's
/// screen, and the live suite one on its compositor. The stand-in records
/// its arguments in a file under `$SWAYNAG_STUB_CALLS` when that's set (as
/// `sway_launch()` sets it) and otherwise does nothing. It's written once per
/// test process, into Cargo's per-target scratch directory, and renamed into
/// place so a concurrent writer never leaves it half-written.
pub fn path_with_swaynag_stub() -> std::ffi::OsString {
    static STUB_DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let directory = STUB_DIRECTORY.get_or_init(|| {
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("swaynag-stub");
        fs::create_dir_all(&directory).expect("failed to create the swaynag stub directory");
        let temporary = directory.join(format!(".swaynag.{}", process::id()));
        fs::write(
            &temporary,
            "#!/bin/sh\n\
             [ -n \"${SWAYNAG_STUB_CALLS:-}\" ] || exit 0\n\
             printf '%s' \"$*\" >\"$SWAYNAG_STUB_CALLS/.$$\"\n\
             mv \"$SWAYNAG_STUB_CALLS/.$$\" \"$SWAYNAG_STUB_CALLS/$$\"\n",
        )
        .expect("failed to write the swaynag stub");
        let mut permissions = fs::metadata(&temporary)
            .expect("the swaynag stub should exist")
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&temporary, permissions).expect("failed to chmod the swaynag stub");
        fs::rename(&temporary, directory.join("swaynag"))
            .expect("failed to move the swaynag stub into place");
        directory
    });
    let mut path = directory.as_os_str().to_os_string();
    if let Some(inherited) = std::env::var_os("PATH") {
        path.push(":");
        path.push(inherited);
    }
    path
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
    // A run that refuses its arguments exits without reading stdin, so the
    // write can lose the race with that exit: a broken pipe isn't a failure.
    let written = child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(stdin.as_bytes());
    if let Err(error) = written {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "writing stdin failed: {error}"
        );
    }
    child
        .wait_with_output()
        .expect("failed to wait for sway-launch")
}
