use super::*;

fn launch(step: usize, token: &str, already_reported: Vec<i64>) -> WatchedLaunch {
    WatchedLaunch {
        step,
        token: token.to_string(),
        already_reported,
    }
}

#[test]
fn watched_step_names_the_launch_whose_marker_the_window_carries() {
    let launches = [launch(1, "first", vec![]), launch(3, "second", vec![])];
    assert_eq!(
        watched_step(&launches, &[], 100, |token| token == "second"),
        Some(3)
    );
    assert_eq!(watched_step(&launches, &[], 100, |_| false), None);
}

#[test]
fn watched_step_skips_a_window_already_caught_or_already_reported() {
    let launches = [launch(1, "first", vec![100])];
    assert_eq!(watched_step(&launches, &[], 100, |_| true), None);
    let caught = [LateWindow {
        step: 1,
        container_id: 101,
        move_error: None,
    }];
    assert_eq!(watched_step(&launches, &caught, 101, |_| true), None);
    assert_eq!(watched_step(&launches, &caught, 102, |_| true), Some(1));
}

#[test]
fn take_new_reads_nothing_while_nothing_is_watched() {
    // No compositor here: a scan would fail, but none is attempted.
    assert!(LateWindowWatch::default().take_new().is_empty());
    assert!(LateWindowWatch::default().finish().is_empty());
}
