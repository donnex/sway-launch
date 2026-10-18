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
fn generate_pid_marker_token_is_unique_across_threads_generating_at_once() {
    let tokens: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(generate_pid_marker_token))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect()
    });
    let unique: std::collections::HashSet<&String> = tokens.iter().collect();
    assert_eq!(unique.len(), tokens.len(), "{tokens:?}");
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

#[test]
fn marker_tracker_predates_launch_only_for_a_process_older_than_every_marked_one() {
    let mut tracker = MarkerTracker::default();
    assert!(!tracker.predates_launch(Some(1)));
    tracker.record_start(500);
    tracker.record_start(450);
    tracker.record_start(600);
    assert!(tracker.predates_launch(Some(449)));
    assert!(!tracker.predates_launch(Some(450)));
    assert!(!tracker.predates_launch(Some(700)));
    assert!(!tracker.predates_launch(None));
}

#[test]
fn start_ticks_from_stat_reads_the_22nd_field() {
    let stat = "42 (a) b) S 7 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 98765 1000 50";
    assert_eq!(start_ticks_from_stat(stat), Some(98765));
    assert_eq!(start_ticks_from_stat("42 (foot) S 7"), None);
    assert_eq!(start_ticks_from_stat("garbage"), None);
}

#[test]
fn process_start_ticks_reads_this_process() {
    let own = process_start_ticks(std::process::id() as i32);
    assert!(own.is_some());
    assert_eq!(process_start_ticks(-1), None);
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
fn launch_shell_token_reads_the_token_of_a_launch_shell_only() {
    let token = |cmdline: &[u8]| launch_shell_token(cmdline);
    assert_eq!(
        token(b"sh\0-c\0export SWAY_LAUNCH_PID_MARKER=12-345 && ( foot )\0").as_deref(),
        Some("12-345")
    );
    assert_eq!(
        token(b"sh\0-c\0export SWAY_LAUNCH_PID_MARKER=12-345 && cd '/' && ( foot )\0").as_deref(),
        Some("12-345")
    );
    assert_eq!(
        token(b"foot\0-T\0x export SWAY_LAUNCH_PID_MARKER=12-345 && \0"),
        None
    );
    assert_eq!(
        token(b"sh\0-c\0export SWAY_LAUNCH_PID_MARKER=12-345\0"),
        None
    );
}

#[test]
fn environ_marker_reads_the_markers_value() {
    assert_eq!(
        environ_marker(b"PATH=/bin\0SWAY_LAUNCH_PID_MARKER=12-345\0HOME=/h\0").as_deref(),
        Some("12-345")
    );
    assert_eq!(
        environ_marker(b"PATH=/bin\0SWAY_LAUNCH_PID_MARKER_X=1\0"),
        None
    );
}

#[test]
fn is_sibling_launchs_window_counts_only_its_groups_launches() {
    // The registry is process-wide, and this is the only test touching it.
    let token = format!("{}-sibling-test", std::process::id());
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .env(PID_MARKER_VAR, &token)
        .spawn()
        .expect("sleep should start");
    let pid = child.id() as i32;
    // Until the child has exec'd, its environ is still this process's.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !process_has_env_var(pid, PID_MARKER_VAR, &token) {
        assert!(std::time::Instant::now() < deadline, "sleep never started");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    register_launch(&token);
    assert!(
        !is_sibling_launchs_window(pid, "own-launch"),
        "outside a group, nothing is recorded"
    );
    let group = ConcurrentLaunches::begin();
    register_launch("own-launch");
    assert!(!is_sibling_launchs_window(pid, "own-launch"));
    register_launch(&token);
    assert!(is_sibling_launchs_window(pid, "own-launch"));
    assert!(!is_sibling_launchs_window(pid, &token));
    drop(group);
    assert!(!is_sibling_launchs_window(pid, "own-launch"));

    child.kill().ok();
    child.wait().ok();
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
