// Smoke test only — clap_complete's own generation logic isn't ours to
// unit-test. This lives here rather than as a unit test in src/main.rs
// because CARGO_BIN_EXE_* (needed to invoke the compiled binary as a
// subprocess) is only set for integration tests, not for the bin crate's
// own unit test harness.

use std::process::Command;

#[test]
fn completions_bash_prints_something_and_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--completions", "bash"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
}
