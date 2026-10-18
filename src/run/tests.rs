use super::*;
use clap::Parser;

#[test]
fn classify_rollback_counts_only_closed_windows_as_rolled_back() {
    let outcome = classify_rollback(vec![30, 20, 10], vec![5], &[20]);
    assert_eq!(outcome.rolled_back, vec![30, 10]);
    assert_eq!(outcome.failed, vec![5, 20]);
}

#[test]
fn classify_rollback_with_nothing_still_open_rolls_back_every_accepted_kill() {
    let outcome = classify_rollback(vec![20, 10], vec![], &[]);
    assert_eq!(outcome.rolled_back, vec![20, 10]);
    assert!(outcome.failed.is_empty());
}

#[test]
fn classify_rollback_with_every_window_still_open_rolls_back_nothing() {
    let outcome = classify_rollback(vec![20, 10], vec![], &[10, 20]);
    assert!(outcome.rolled_back.is_empty());
    assert_eq!(outcome.failed, vec![20, 10]);
}

#[test]
fn send_rollback_kills_moves_on_past_a_kill_sway_refuses() {
    let mut attempted = Vec::new();
    let kills = send_rollback_kills(&[10, 20, 30], |container_id| {
        attempted.push(container_id);
        if container_id == 20 {
            Err(sway_launch::KillError::Refused(
                "no matching node".to_string(),
            ))
        } else {
            Ok(())
        }
    });
    assert_eq!(attempted, vec![30, 20, 10]);
    assert_eq!(kills.accepted, vec![30, 10]);
    assert_eq!(kills.failed, vec![20]);
    assert!(!kills.compositor_unreachable);
}

#[test]
fn send_rollback_kills_stops_at_the_first_unanswered_kill_and_fails_the_rest() {
    let mut attempted = Vec::new();
    let kills = send_rollback_kills(&[10, 20, 30, 40], |container_id| {
        attempted.push(container_id);
        if container_id == 30 {
            Err(sway_launch::KillError::Unreachable(
                "Sway IPC did not respond".to_string(),
            ))
        } else {
            Ok(())
        }
    });
    assert_eq!(attempted, vec![40, 30]);
    assert_eq!(kills.accepted, vec![40]);
    assert_eq!(kills.failed, vec![30, 20, 10]);
    assert!(kills.compositor_unreachable);
}

#[test]
fn send_rollback_kills_with_nothing_launched_attempts_nothing() {
    let kills = send_rollback_kills(&[], |_| panic!("no kill should be sent"));
    assert!(kills.accepted.is_empty());
    assert!(kills.failed.is_empty());
    assert!(!kills.compositor_unreachable);
}

#[test]
fn run_lock_file_name_is_stable_and_differs_by_workspace() {
    let here = run_lock_file_name("/layouts/dev.toml\0ws-3");
    assert_eq!(here, run_lock_file_name("/layouts/dev.toml\0ws-3"));
    assert_ne!(here, run_lock_file_name("/layouts/dev.toml\0ws-4"));
    assert!(here.ends_with(".lock") && here.len() == 21, "{here}");
}

#[test]
fn run_lock_identity_is_one_lock_per_layout_and_workspace_however_spelled() {
    use sway_launch::WorkspaceTarget::{Name, Number};
    let identity = |workspace| run_lock_identity("/layouts/dev.toml", workspace);
    assert_eq!(identity(Some(Name("Dev"))), identity(Some(Name("dev"))));
    assert_eq!(identity(Some(Number(3))), identity(Some(Name("3: web"))));
    assert_eq!(
        identity(Some(Name("1"))),
        identity(Some(Name("1: <icons>")))
    );
    assert_ne!(identity(Some(Name("dev"))), identity(Some(Name("web"))));
    assert_ne!(identity(Some(Name("dev"))), identity(None));
    assert_ne!(
        identity(Some(Name("dev"))),
        run_lock_identity("/layouts/web.toml", Some(Name("dev")))
    );
}

#[test]
fn run_lock_source_tells_stdin_layouts_apart_by_their_contents() {
    let stdin = Path::new("-");
    assert_eq!(
        run_lock_source(stdin, "[[step]]\ncommand = \"foot\"\n"),
        run_lock_source(stdin, "[[step]]\ncommand = \"foot\"\n")
    );
    assert_ne!(
        run_lock_source(stdin, "[[step]]\ncommand = \"foot\"\n"),
        run_lock_source(stdin, "[[step]]\ncommand = \"thunar\"\n")
    );
    assert_eq!(run_lock_source(Path::new("quad-grid"), ""), "quad-grid");
}

#[test]
fn run_lock_held_message_names_the_workspace_and_the_way_out() {
    let message = run_lock_held_message("3");
    assert!(message.contains("workspace \"3\""), "{message}");
    assert!(message.contains("--no-lock"), "{message}");
    assert!(!run_lock_held_message("").contains("workspace"));
}

#[test]
fn occupied_workspace_message_names_the_workspace_and_the_way_out() {
    let message = occupied_workspace_message("1", 2);
    assert!(message.contains("workspace \"1\" already has 2 tiled window(s)"));
    assert!(message.contains("--on-workspace"));
}

fn staged(container_id: i64) -> Result<sway_launch::RunOutcome, sway_launch::RunError> {
    Ok(sway_launch::RunOutcome {
        container_id,
        actions: Vec::new(),
        launch_ownership: Some(sway_launch::LaunchOwnership::Launched),
        warnings: Vec::new(),
    })
}

fn timed_out() -> Result<sway_launch::RunOutcome, sway_launch::RunError> {
    Err(sway_launch::RunError {
        window_timed_out: true,
        ..sway_launch::RunError::from("no window appeared".to_string())
    })
}

#[test]
fn take_staging_failure_skips_a_timeout_only_under_continue_on_timeout() {
    let mut windows: StagedWindows = [(0, staged(10)), (2, timed_out())].into_iter().collect();
    assert!(take_staging_failure(&mut windows, true).is_none());
    assert_eq!(windows.len(), 2);

    let (index, error) = take_staging_failure(&mut windows, false).expect("a failure");
    assert_eq!((index, error.message.as_str()), (2, "no window appeared"));
    assert_eq!(windows.keys().copied().collect::<Vec<_>>(), [0]);
}

#[test]
fn take_staging_failure_returns_the_first_failure_in_step_order() {
    let mut windows: StagedWindows = [
        (3, Err("later".to_string().into())),
        (1, Err("earlier".to_string().into())),
        (0, staged(10)),
    ]
    .into_iter()
    .collect();
    let (index, error) = take_staging_failure(&mut windows, true).expect("a failure");
    assert_eq!((index, error.message.as_str()), (1, "earlier"));
}

#[test]
fn reject_shared_windows_fails_every_later_step_with_the_same_window() {
    let mut windows: StagedWindows = [(0, staged(10)), (1, staged(11)), (4, staged(10))]
        .into_iter()
        .collect();
    reject_shared_windows(&mut windows);
    assert!(windows[&0].is_ok() && windows[&1].is_ok());
    let error = windows[&4]
        .as_ref()
        .err()
        .expect("step 5 shares step 1's window");
    assert!(
        error
            .message
            .starts_with("its window (container 10) is also step 1's"),
        "{}",
        error.message
    );
}

#[test]
fn staged_window_owners_maps_every_waiting_window_to_its_step_but_the_excepted_one() {
    let caught_late = Err(sway_launch::RunError {
        container_id: Some(12),
        launch_ownership: Some(sway_launch::LaunchOwnership::Launched),
        ..sway_launch::RunError::from("timed out".to_string())
    });
    let windows: StagedWindows = [
        (1, staged(10)),
        (2, staged(11)),
        (3, timed_out()),
        (5, caught_late),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        staged_window_owners(&windows, Some(2)),
        HashMap::from([(10, 1), (12, 5)])
    );
    assert_eq!(staged_window_owners(&windows, None).get(&11), Some(&2));
}

fn layout_steps(toml_text: &str) -> Vec<layout::LayoutStep> {
    layout::parse(toml_text).expect("layout should parse").step
}

fn offline_args() -> Args {
    Args::try_parse_from(["sway-launch", "--layout", "unused.toml", "--validate"])
        .expect("--layout --validate should parse")
}

#[test]
fn plan_steps_offline_resolves_a_target_id_against_an_earlier_named_step() {
    let steps = layout_steps(
        "[[step]]\nid = \"main\"\ncon_id = 7\n\n[[step]]\ntarget_id = \"main\"\nfloating = true\n",
    );
    let planned = plan_steps_offline(&steps, &offline_args()).expect("steps should plan");
    assert_eq!(planned.len(), 2);
}

#[test]
fn plan_steps_offline_reports_a_duplicate_id_with_its_step_number() {
    let steps = layout_steps(
        "[[step]]\nid = \"main\"\ncon_id = 7\n\n[[step]]\nid = \"main\"\ncon_id = 8\n",
    );
    assert_eq!(
        plan_steps_offline(&steps, &offline_args()).err(),
        Some("unused.toml: step 2: id \"main\" was already used by an earlier step".to_string())
    );
}

#[test]
fn plan_steps_offline_reports_an_unknown_target_id_with_its_step_number() {
    let steps = layout_steps("[[step]]\ncon_id = 7\n\n[[step]]\ntarget_id = \"missing\"\n");
    let error = plan_steps_offline(&steps, &offline_args())
        .err()
        .expect("an unknown target_id should fail");
    assert!(error.starts_with("unused.toml: step 2: "), "{error}");
}

#[test]
fn plan_step_rejects_an_id_already_resolved_by_a_real_run() {
    let steps = layout_steps("[[step]]\nid = \"main\"\ncon_id = 7\n");
    let resolved_ids = HashMap::from([("main".to_string(), 42)]);
    assert_eq!(
        plan_step(&steps[0], &offline_args(), &resolved_ids).err(),
        Some("id \"main\" was already used by an earlier step".to_string())
    );
}

fn named_step(id: &str) -> layout::LayoutStep {
    layout_steps(&format!("[[step]]\nid = {id:?}\ncommand = \"foot\"\n"))
        .pop()
        .expect("one step")
}

fn failed_launch() -> sway_launch::RunError {
    sway_launch::RunError {
        container_id: Some(12),
        launch_ownership: Some(sway_launch::LaunchOwnership::Launched),
        also_launched: vec![13],
        ..sway_launch::RunError::from("timed out".to_string())
    }
}

#[test]
fn a_failed_staged_step_and_a_failed_sequential_step_record_the_same_progress() {
    // #135: the --concurrent staging failure used to leave the failed step's
    // window out of `containers`.
    let step = named_step("editor");
    let mut sequential = RunProgress::default();
    assert_eq!(sequential.record_failed(&step, &failed_launch()), [12, 13]);
    let mut staged = RunProgress::default();
    assert_eq!(staged.record_staged(&step, &Err(failed_launch())), [12, 13]);
    assert_eq!(staged, sequential);
    let left_open = sequential.left_open(&[]);
    assert_eq!(left_open.container_ids, [12, 13]);
    assert_eq!(
        left_open.containers,
        HashMap::from([("editor".to_string(), 12)])
    );
    assert_eq!(sequential.launched_container_ids, [12, 13]);
}

#[test]
fn run_progress_rolls_back_only_launched_windows_but_reports_every_one() {
    let mut progress = RunProgress::default();
    progress.record(
        &named_step("adopted"),
        10,
        Some(sway_launch::LaunchOwnership::Adopted),
    );
    progress.record(&named_step("retargeted"), 11, None);
    progress.record_staged(&named_step("launched"), &staged(12));
    assert_eq!(progress.container_ids, [10, 11, 12]);
    assert_eq!(progress.launched_container_ids, [12]);
    let left_open = progress.left_open(&[12]);
    assert_eq!(left_open.container_ids, [10, 11]);
    assert_eq!(
        left_open.containers,
        HashMap::from([("adopted".to_string(), 10), ("retargeted".to_string(), 11)])
    );
}
