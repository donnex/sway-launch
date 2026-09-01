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

#[test]
fn completions_rejects_json_rather_than_ignoring_it() {
    // Regression test: --completions' conflict list already covers every
    // other argument it could only discard (a command, the per-window flags,
    // --bindings/--apps, --dry-run/--validate), but --json was missing, so
    // `--completions bash --json` printed the ordinary shell script and threw
    // the flag away. Unlike --list-templates/--show-template, a completion
    // script has no JSON shape to take.
    let output = Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--completions", "bash", "--json"])
        .output()
        .expect("failed to run sway-launch binary");

    assert!(
        !output.status.success(),
        "--completions --json should be rejected, not silently ignored"
    );
}
