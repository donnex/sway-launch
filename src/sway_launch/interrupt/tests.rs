use super::*;

// No signal is ever installed in unit tests, so these cover the waits as
// they behave without one; tests/fake_sway.rs sends real signals.

#[test]
fn recv_returns_a_value_sent_before_the_timeout() {
    let (sender, receiver) = mpsc::channel();
    sender.send(7).unwrap();
    assert_eq!(recv(&receiver, time::Duration::from_secs(1)), Ok(7));
}

#[test]
fn recv_times_out_after_the_whole_timeout_rather_than_one_check_interval() {
    let (_sender, receiver) = mpsc::channel::<()>();
    let started = time::Instant::now();
    assert_eq!(recv(&receiver, CHECK_INTERVAL * 3), Err(WaitEnd::Timeout));
    assert!(started.elapsed() >= CHECK_INTERVAL * 3);
}

#[test]
fn recv_reports_a_closed_channel() {
    let (sender, receiver) = mpsc::channel::<()>();
    drop(sender);
    assert_eq!(
        recv(&receiver, time::Duration::from_secs(1)),
        Err(WaitEnd::Disconnected)
    );
}

#[test]
fn sleep_waits_its_whole_duration_without_a_signal() {
    let started = time::Instant::now();
    assert_eq!(sleep(CHECK_INTERVAL * 2), Ok(()));
    assert!(started.elapsed() >= CHECK_INTERVAL * 2);
    assert_eq!(check(), Ok(()));
    assert_eq!(exit_status(), None);
}

#[test]
fn signal_names_name_every_caught_signal() {
    assert_eq!(signal_name(signal_hook::consts::SIGINT), "SIGINT");
    assert_eq!(signal_name(signal_hook::consts::SIGTERM), "SIGTERM");
    assert_eq!(signal_name(signal_hook::consts::SIGHUP), "SIGHUP");
    for signal in CAUGHT_SIGNALS {
        assert_ne!(signal_name(signal), "a signal");
    }
    assert_eq!(signal_name(9), "a signal");
}
