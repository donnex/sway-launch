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
