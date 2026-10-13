// Helpers shared by the integration test files under tests/. Each of those
// files is compiled as its own crate and pulls this in with `mod common;`,
// so a helper only one of them uses is dead code in the others -- hence the
// crate-wide allow below rather than one per item.
#![allow(dead_code)]

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};

/// A per-process scratch directory for fixtures and marker files, so nothing
/// depends on a fixed path in a shared temp directory.
///
/// Two test files once independently used the fixture name "malformed", so
/// both wrote the same path -- harmless while cargo runs test binaries one at
/// a time, but a real collision under a parallel runner (cargo nextest),
/// under two concurrent `cargo test --test ...` runs, or with two users on
/// one machine. A predictable path in a shared /tmp is also writable through
/// a symlink planted by another user, and tests/live_sway.rs's injection
/// tests assert a marker file does *not* exist, which a stray file at a
/// predictable path would fail for the wrong reason.
pub fn test_temp_dir() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("sway-launch-test-{}", process::id()));
    fs::create_dir_all(&directory).expect("failed to create the test temp directory");
    directory
}

/// A TOML file fixture (layout, template, or bindings) written to
/// `test_temp_dir()`, removed again when it goes out of scope (even if an
/// assertion panics mid-test).
pub struct TempToml(PathBuf);

impl TempToml {
    pub fn write(name: &str, contents: &str) -> Self {
        let path = test_temp_dir().join(format!("{}.toml", name));
        fs::write(&path, contents).expect("failed to write temp fixture file");
        Self(path)
    }
}

impl Deref for TempToml {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempToml {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The binary, pointed at a socket path that cannot exist.
///
/// Some tests assert that an IPC-touching invocation fails. Left to the
/// ambient environment, that assertion holds on a bare CI runner and breaks in
/// a terminal inside a Sway session — which is where this project is developed,
/// and where CLAUDE.md's own "run `cargo test` before committing" gate runs. It
/// was worse than a false failure for the `--con-id 42` cases: with a session
/// reachable, whether the command failed depended on whether container id 42
/// happened to exist, and if it did, the test floated a real window.
///
/// `I3SOCK` is removed as well as `SWAYSOCK` set. `socket_path()` reads
/// `SWAYSOCK` first, so the override alone would do today, but both together
/// mean the connect attempt fails on a missing path rather than reaching a
/// compositor no matter what the environment holds or which order wins.
pub fn sway_launch_without_a_compositor() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
    command
        .env("SWAYSOCK", "/nonexistent/sway-launch-test-socket")
        .env_remove("I3SOCK");
    command
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
