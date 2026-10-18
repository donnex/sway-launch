// Smoke test only — clap_complete's own generation logic isn't ours to
// unit-test. This lives here rather than as a unit test in src/main.rs
// because CARGO_BIN_EXE_* (needed to invoke the compiled binary as a
// subprocess) is only set for integration tests, not for the bin crate's
// own unit test harness.

mod common;

use common::{assert_conflict_error, sway_launch};

#[test]
fn completions_bash_prints_something_and_exits_zero() {
    let output = sway_launch()
        .args(["--completions", "bash"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
}

#[test]
fn completions_rejects_json_rather_than_ignoring_it() {
    // Regression test: --completions' conflict list already covers every
    // other argument it could only discard (a command, the per-window flags,
    // --bindings/--apps, --dry-run/--validate), but --json was missing, so
    // `--completions bash --json` printed the ordinary shell script and threw
    // the flag away. Unlike --list-templates/--show-template, a completion
    // script has no JSON shape to take.
    let output = sway_launch()
        .args(["--completions", "bash", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert_conflict_error(&output, "--completions", "--json");
}

#[test]
fn completions_into_a_full_stdout_fail_cleanly_instead_of_panicking() {
    // Regression test: clap_complete::generate() wrote straight to stdout and
    // panicked on a failed write, so a closed pipe or a full disk ended in
    // exit 101. /dev/full fails every write deterministically, unlike a
    // closed pipe, which a short write can beat.
    let Ok(dev_full) = std::fs::OpenOptions::new().write(true).open("/dev/full") else {
        return;
    };
    let output = sway_launch()
        .args(["--completions", "bash"])
        .stdout(dev_full)
        .output()
        .expect("failed to run sway-launch binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "should not panic: {stderr}");
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("failed writing to stdout"),
        "stderr: {stderr}"
    );
}

#[test]
fn completions_into_a_closed_stdout_exit_cleanly_instead_of_panicking() {
    // Best-effort, like json_output.rs's --list-templates case: the script
    // may fit the pipe buffer before the read end closes, in which case this
    // passes trivially. It can only fail for the right reason.
    let mut child = sway_launch()
        .args(["--completions", "bash"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sway-launch binary");
    drop(child.stdout.take());

    let output = child.wait_with_output().expect("sway-launch should exit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "should not panic: {stderr}");
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
}
