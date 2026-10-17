use super::*;

// generate_pid_marker_token / process_has_env_var / read_process_marker /
// scan_marker_presence / classify_marker_reads

#[test]
fn generate_pid_marker_token_starts_with_this_processes_id() {
    let token = generate_pid_marker_token();
    let expected_prefix = format!("{}-", std::process::id());
    assert!(
        token.starts_with(&expected_prefix),
        "token {:?} should start with {:?}",
        token,
        expected_prefix
    );
}

#[test]
fn process_has_env_var_true_for_this_processes_own_environment() {
    let pid = std::process::id() as i32;
    let path = std::env::var("PATH").expect("PATH should be set in the test environment");
    assert!(process_has_env_var(pid, "PATH", &path));
}

#[test]
fn process_has_env_var_false_for_wrong_value() {
    let pid = std::process::id() as i32;
    assert!(!process_has_env_var(
        pid,
        "PATH",
        "definitely-not-the-real-path-value"
    ));
}

#[test]
fn process_has_env_var_false_for_nonexistent_pid() {
    assert!(!process_has_env_var(i32::MAX, "PATH", "anything"));
}

#[test]
fn read_process_marker_absent_for_nonexistent_pid() {
    assert_eq!(
        read_process_marker(i32::MAX, "PATH", "anything"),
        MarkerRead::Absent
    );
}

#[test]
fn scan_marker_presence_finds_this_process() {
    let path = std::env::var("PATH").expect("PATH should be set in the test environment");
    let own_pid = std::process::id() as i32;
    match scan_marker_presence("PATH", &path, &[]) {
        MarkerPresence::Present(pids) => assert!(pids.contains(&own_pid)),
        other => panic!("expected Present, got {:?}", other),
    }
}

#[test]
fn scan_marker_presence_absent_for_a_value_nothing_has() {
    assert_eq!(
        scan_marker_presence("SWAY_LAUNCH_DEFINITELY_UNUSED_TEST_VAR_XYZ", "nope", &[]),
        MarkerPresence::Absent
    );
}

#[test]
fn classify_marker_reads_present_wins_over_everything_else() {
    assert_eq!(
        classify_marker_reads(
            [
                (1, MarkerRead::Unreadable),
                (2, MarkerRead::Present),
                (3, MarkerRead::Absent),
            ],
            &[1],
        ),
        MarkerPresence::Present(vec![2])
    );
}

#[test]
fn classify_marker_reads_unrelated_unreadable_processes_still_allow_absent() {
    assert_eq!(
        classify_marker_reads([(1, MarkerRead::Unreadable), (2, MarkerRead::Absent)], &[7]),
        MarkerPresence::Absent
    );
}

#[test]
fn classify_marker_reads_a_previously_seen_unreadable_process_is_unknown() {
    assert_eq!(
        classify_marker_reads([(7, MarkerRead::Unreadable), (2, MarkerRead::Absent)], &[7]),
        MarkerPresence::Unknown
    );
}

#[test]
fn classify_marker_reads_a_previously_seen_process_that_exited_is_absent() {
    assert_eq!(
        classify_marker_reads([(2, MarkerRead::Absent)], &[7]),
        MarkerPresence::Absent
    );
}

#[test]
fn marker_tracker_never_reports_gone_for_a_marker_it_never_saw() {
    let mut tracker = MarkerTracker::default();
    tracker.observe(MarkerPresence::Absent);
    assert!(!tracker.is_gone());
    assert!(tracker.needs_scan(false));
    assert!(tracker.needs_scan(true));
}

#[test]
fn marker_tracker_reports_gone_once_a_seen_marker_disappears() {
    let mut tracker = MarkerTracker::default();
    tracker.observe(MarkerPresence::Present(vec![42]));
    assert!(!tracker.is_gone());
    assert_eq!(tracker.previously_seen(), &[42]);
    tracker.observe(MarkerPresence::Absent);
    assert!(tracker.is_gone());
    assert!(!tracker.needs_scan(true));
}

#[test]
fn marker_tracker_unknown_does_not_count_as_gone() {
    let mut tracker = MarkerTracker::default();
    tracker.observe(MarkerPresence::Present(vec![42]));
    tracker.observe(MarkerPresence::Unknown);
    assert!(!tracker.is_gone());
    assert!(tracker.needs_scan(true));
}

#[test]
fn marker_tracker_only_scans_after_seeing_the_marker_while_holding_a_fallback() {
    let mut tracker = MarkerTracker::default();
    tracker.observe(MarkerPresence::Present(vec![42, 43]));
    tracker.observe(MarkerPresence::Present(vec![43, 44]));
    assert_eq!(tracker.previously_seen(), &[42, 43, 44]);
    assert!(!tracker.needs_scan(false));
    assert!(tracker.needs_scan(true));
}

#[test]
fn marker_tracker_stops_scanning_and_reports_once_when_proc_is_unavailable() {
    let mut tracker = MarkerTracker::default();
    assert!(tracker.observe(MarkerPresence::Unavailable));
    assert!(!tracker.observe(MarkerPresence::Unavailable));
    assert!(!tracker.is_gone());
    assert!(!tracker.needs_scan(true));
}

// launch_evidence / marked_ancestor / parent_pid_from_stat /
// cmdline_is_launch_shell

#[test]
fn parent_pid_from_stat_counts_fields_from_the_last_parenthesis() {
    assert_eq!(parent_pid_from_stat("42 (foot) S 7 42 42 0"), Some(7));
    assert_eq!(parent_pid_from_stat("42 (a) b (c) S 9 42 42 0"), Some(9));
    assert_eq!(parent_pid_from_stat("42 (foot)"), None);
    assert_eq!(parent_pid_from_stat("garbage"), None);
}

#[test]
fn cmdline_is_launch_shell_needs_this_launchs_exact_token() {
    let cmdline = b"sh\0-c\0export SWAY_LAUNCH_PID_MARKER=12-345 && ( foot )\0";
    assert!(cmdline_is_launch_shell(cmdline, "12-345"));
    assert!(!cmdline_is_launch_shell(cmdline, "12-34"));
    assert!(!cmdline_is_launch_shell(cmdline, "12-3456"));
    assert!(!cmdline_is_launch_shell(
        b"foot\0-T\0x export SWAY_LAUNCH_PID_MARKER=12-345 && \0",
        "12-345"
    ));
}

/// A process table for `marked_ancestor()`: pid -> (marked, parent).
fn ancestry(table: &[(i32, bool, Option<i32>)]) -> impl FnMut(i32) -> (bool, Option<i32>) + '_ {
    move |pid| {
        table
            .iter()
            .find(|(candidate, _, _)| *candidate == pid)
            .map_or((false, None), |&(_, marked, parent)| (marked, parent))
    }
}

#[test]
fn marked_ancestor_finds_the_nearest_marked_ancestor() {
    let table = [
        (40, true, Some(30)),
        (30, false, Some(20)),
        (20, true, Some(10)),
        (10, false, Some(1)),
    ];
    assert_eq!(marked_ancestor(40, 8, ancestry(&table)), Some(20));
}

#[test]
fn marked_ancestor_stops_at_init_the_depth_limit_and_a_loop() {
    let unmarked = [(40, false, Some(30)), (30, false, Some(1)), (1, true, None)];
    assert_eq!(marked_ancestor(40, 8, ancestry(&unmarked)), None);

    let deep = [
        (40, false, Some(30)),
        (30, false, Some(20)),
        (20, true, None),
    ];
    assert_eq!(marked_ancestor(40, 1, ancestry(&deep)), None);
    assert_eq!(marked_ancestor(40, 2, ancestry(&deep)), Some(20));

    let looping = [(40, false, Some(30)), (30, false, Some(40))];
    assert_eq!(marked_ancestor(40, 8, ancestry(&looping)), None);
}

#[test]
fn launch_evidence_finds_the_launch_shell_above_a_program_without_the_marker() {
    // The shape Sway produces for `sh -c "exec env -u MARKER program"`: the
    // program never sees the marker, but the shell Sway started is its
    // ancestor, and carries the token in its command line.
    let token = format!("{}-ancestry-test", std::process::id());
    let mut shell = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "export {}={} && ( exec env -u {} sleep 30 )",
            PID_MARKER_VAR, token, PID_MARKER_VAR
        ))
        .spawn()
        .expect("sh should start");
    let shell_pid = shell.id() as i32;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let program = loop {
        let child = std::fs::read_dir("/proc")
            .expect("/proc should be readable")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
            .find(|&pid| {
                parent_pid(pid) == Some(shell_pid)
                    && std::fs::read(format!("/proc/{}/cmdline", pid))
                        .is_ok_and(|cmdline| cmdline.starts_with(b"sleep\0"))
            });
        if let Some(child) = child {
            break child;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the program never started"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };

    let evidence = launch_evidence(program, &token);
    let other = launch_evidence(program, "another-launch");
    shell.kill().ok();
    std::process::Command::new("kill")
        .arg(program.to_string())
        .status()
        .ok();
    shell.wait().ok();

    assert_eq!(evidence, Some(LaunchEvidence::Ancestor(shell_pid)));
    assert_eq!(other, None);
}
