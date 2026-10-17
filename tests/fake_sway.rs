// The confirmation drivers, run headlessly against tests/common/fake_sway.rs
// (#103): what the binary does when Sway's replies, events and tree arrive in
// a given order, including the orders a live compositor can't be made to
// produce on demand. Each scenario runs against every recorded Sway version
// unless it's about one version's behaviour.
//
// What these can't show is whether real Sway behaves the way a handler
// scripts it to; tests/live_sway.rs is still where that is checked.

mod common;

use common::{TempToml, TestTempDir};

use common::fake_sway::{FakeSway, Sway, SwayVersion, FLOATING, SCRATCHPAD, TILED, WORKSPACE};
use serde_json::Value;
use std::process::Output;
use std::time::Duration;

/// A finished run of the binary, with its `--json` result parsed: the
/// success object from stdout, or the error object from stderr's last line.
struct Run {
    output: Output,
    json: Value,
}

impl Run {
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    fn succeeded(&self) -> bool {
        self.output.status.success()
    }

    /// The `status` of the run's only action.
    fn action_status(&self) -> &str {
        let actions = self.json["actions"]
            .as_array()
            .unwrap_or_else(|| panic!("no actions in {}", self.json));
        assert_eq!(
            actions.len(),
            1,
            "expected exactly one action: {}",
            self.json
        );
        actions[0]["status"].as_str().expect("status is a string")
    }

    fn error(&self) -> &str {
        self.json["error"]
            .as_str()
            .unwrap_or_else(|| panic!("no error in {} (stderr: {})", self.json, self.stderr()))
    }
}

fn run(fake: &FakeSway, arguments: &[&str]) -> Run {
    let output = fake
        .sway_launch()
        .args(arguments)
        .arg("--json")
        .output()
        .expect("failed to run sway-launch");
    let text = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let last_line = text.lines().next_back().unwrap_or_default();
    let json = serde_json::from_str(last_line)
        .unwrap_or_else(|error| panic!("not a JSON result ({error}): {text}"));
    Run { output, json }
}

/// A handler that floats the window a `floating enable` names and confirms
/// it with Sway's `floating` event.
fn floats_on_command(sway: &mut Sway, command: &str) -> Result<(), String> {
    if command == format!("[con_id={TILED}] floating enable") {
        sway.float(TILED);
        sway.emit("floating", TILED);
    }
    Ok(())
}

#[test]
fn floating_is_confirmed_by_its_event_and_the_tree() {
    for version in SwayVersion::ALL {
        let fake = FakeSway::start(version, floats_on_command);
        let run = run(&fake, &["--con-id", "5", "--floating"]);
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        assert_eq!(run.action_status(), "changed", "{version:?}");
        assert_eq!(run.json["container_id"], TILED, "{version:?}");
    }
}

#[test]
fn an_already_floating_window_is_reported_without_sending_anything() {
    for version in SwayVersion::ALL {
        let fake = FakeSway::accepting_everything(version);
        // Sway 1.9 leaves a floating_con's own `floating` field unset, so the
        // check has to read the node type.
        if version == SwayVersion::V1_9 {
            assert!(fake.sway().node(FLOATING)["floating"].is_null());
        }
        let run = run(&fake, &["--con-id", &FLOATING.to_string(), "--floating"]);
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        assert_eq!(run.action_status(), "already_satisfied", "{version:?}");
        assert!(
            !fake
                .commands()
                .iter()
                .any(|command| command.contains("floating")),
            "{version:?}: sent {:?}",
            fake.commands()
        );
    }
}

#[test]
fn an_event_confirmed_action_with_no_event_times_out_naming_the_action() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(&fake, &["--con-id", "5", "--floating", "--timeout", "1"]);
    assert!(!run.succeeded());
    assert!(
        run.error()
            .contains("no matching window event within 1 sec"),
        "{}",
        run.error()
    );
    assert!(run.error().contains("floating"), "{}", run.error());
}

#[test]
fn an_event_without_the_requested_state_keeps_the_wait_going() {
    // Another client's event for the same window looks exactly like ours, so
    // an event alone mustn't confirm: the state has to be in effect too.
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.ends_with("floating enable") {
            sway.emit("floating", TILED);
            sway.after(Duration::from_millis(200), |sway| {
                sway.float(TILED);
                sway.emit("floating", TILED);
            });
        }
        Ok(())
    });
    let run = run(&fake, &["--con-id", "5", "--floating", "--verbose"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
    assert!(
        run.stderr()
            .contains("the requested state is not in effect"),
        "expected the first event to be turned away: {}",
        run.stderr()
    );
}

#[test]
fn an_event_whose_state_never_applies_times_out() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.ends_with("floating enable") {
            sway.emit("floating", TILED);
        }
        Ok(())
    });
    let run = run(&fake, &["--con-id", "5", "--floating", "--timeout", "1"]);
    assert!(!run.succeeded());
    assert!(
        run.error().contains("no matching window event"),
        "{}",
        run.error()
    );
}

#[test]
fn a_command_sway_refuses_fails_with_sways_own_message() {
    let fake = FakeSway::start(SwayVersion::V1_11, |_, _| Err("Refused by the fake".into()));
    let run = run(&fake, &["--con-id", "5", "--floating"]);
    assert!(!run.succeeded());
    assert!(
        run.error().contains("Refused by the fake"),
        "{}",
        run.error()
    );
}

#[test]
fn a_scratchpad_window_is_already_in_the_scratchpad_on_every_version() {
    // Sway 1.9 leaves scratchpad_state at "none" for a hidden window, so the
    // check has to come from the __i3_scratch workspace instead.
    for version in SwayVersion::ALL {
        let fake = FakeSway::accepting_everything(version);
        let run = run(
            &fake,
            &["--con-id", &SCRATCHPAD.to_string(), "--scratchpad"],
        );
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        assert_eq!(run.action_status(), "already_satisfied", "{version:?}");
    }
}

#[test]
fn list_windows_shows_each_window_where_it_is_on_every_version() {
    // Sway 1.9 leaves a floating window's own `floating` field unset, and the
    // scratchpad is a workspace like any other in the tree.
    for version in SwayVersion::ALL {
        let fake = FakeSway::accepting_everything(version);
        let run = run(&fake, &["--list-windows"]);
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        let windows = run.json["windows"].as_array().expect("a windows array");
        let row = |id: i64| {
            windows
                .iter()
                .find(|window| window["container_id"] == id)
                .unwrap_or_else(|| panic!("{version:?}: no row for {id} in {}", run.json))
        };
        assert_eq!(windows.len(), 3, "{version:?}: {}", run.json);
        assert_eq!(row(TILED)["app_id"], "fixture-tiled");
        assert_eq!(row(TILED)["workspace"], "1");
        assert_eq!(row(TILED)["floating"], false);
        assert_eq!(row(FLOATING)["floating"], true, "{version:?}");
        assert_eq!(row(FLOATING)["focused"], true, "{version:?}");
        assert_eq!(row(SCRATCHPAD)["scratchpad"], true, "{version:?}");
        assert_eq!(row(SCRATCHPAD)["workspace"], Value::Null, "{version:?}");
        assert!(fake.commands().is_empty());
    }
}

#[test]
fn list_windows_with_a_filter_lists_what_existing_would_choose_between() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(&fake, &["--list-windows", "-a", "fixture-floating"]);
    assert!(run.succeeded(), "{}", run.stderr());
    let ids: Vec<&Value> = run.json["windows"]
        .as_array()
        .expect("a windows array")
        .iter()
        .map(|window| &window["container_id"])
        .collect();
    assert_eq!(ids, [FLOATING]);
}

#[test]
fn list_windows_table_has_a_header_and_a_row_per_window() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let output = fake
        .sway_launch()
        .arg("--list-windows")
        .output()
        .expect("failed to run sway-launch");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].starts_with("ID  APP_ID"), "{stdout}");
    assert_eq!(lines.len(), 4, "{stdout}");
    assert!(stdout.contains("(scratchpad)"), "{stdout}");
}

#[test]
fn list_windows_with_no_match_says_so_on_stderr_only() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let output = fake
        .sway_launch()
        .args(["--list-windows", "-c", "Nothing"])
        .output()
        .expect("failed to run sway-launch");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("no open window matches class \"Nothing\""));
}

#[test]
fn a_workspace_number_is_satisfied_by_the_workspace_s_num_on_every_version() {
    // The fixtures' workspace is named "1", so Sway records num 1 for it.
    for version in SwayVersion::ALL {
        let fake = FakeSway::accepting_everything(version);
        let run = run(
            &fake,
            &["--con-id", &TILED.to_string(), "--workspace-number", "1"],
        );
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        assert_eq!(run.action_status(), "already_satisfied", "{version:?}");
        assert!(fake.commands().is_empty(), "{version:?}");
    }
}

#[test]
fn a_workspace_number_move_is_confirmed_once_the_window_is_on_that_number() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command == format!("[con_id={TILED}] move workspace number 1") {
            sway.node_mut(WORKSPACE)["num"] = serde_json::json!(1);
            sway.emit("move", TILED);
        }
        Ok(())
    });
    // Renumber the window's workspace first, so the move has something to do.
    fake.sway().node_mut(WORKSPACE)["num"] = serde_json::json!(7);
    let run = run(
        &fake,
        &["--con-id", &TILED.to_string(), "--workspace-number", "1"],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
}

#[test]
fn existing_wait_takes_a_window_that_is_already_open_without_waiting() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_9);
    let started = std::time::Instant::now();
    let run = run(&fake, &["--existing", "--wait", "-a", "fixture-tiled"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], TILED);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn existing_wait_takes_a_window_that_opens_during_the_wait() {
    for version in SwayVersion::ALL {
        let fake = FakeSway::accepting_everything(version);
        fake.sway().after(Duration::from_millis(400), |sway| {
            let id = sway.open_window(Some("autostarted"), None);
            sway.emit("new", id);
        });
        let run = run(
            &fake,
            &[
                "--existing",
                "--wait",
                "-a",
                "autostarted",
                "--timeout",
                "5",
            ],
        );
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        let container_id = run.json["container_id"].as_i64().expect("a container id");
        assert!(fake.sway().contains(container_id), "{version:?}");
        assert_eq!(
            fake.sway().node(container_id)["app_id"],
            "autostarted",
            "{version:?}"
        );
    }
}

#[test]
fn existing_wait_notices_an_app_id_set_after_mapping_without_an_event() {
    // The window maps with no app_id and sets one later; Sway sends no event
    // for that, so only the periodic tree read can see it.
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    fake.sway().after(Duration::from_millis(200), |sway| {
        let id = sway.open_window(None, None);
        sway.emit("new", id);
        sway.after(Duration::from_millis(400), move |sway| {
            sway.node_mut(id)["app_id"] = serde_json::json!("late-app");
        });
    });
    let run = run(
        &fake,
        &["--existing", "--wait", "-a", "late-app", "--timeout", "5"],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    let container_id = run.json["container_id"].as_i64().expect("a container id");
    assert_eq!(fake.sway().node(container_id)["app_id"], "late-app");
}

#[test]
fn existing_wait_times_out_naming_what_it_waited_for() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let started = std::time::Instant::now();
    let run = run(
        &fake,
        &[
            "--existing",
            "--wait",
            "-a",
            "never-opens",
            "--timeout",
            "1",
        ],
    );
    assert!(!run.succeeded());
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(
        run.error(),
        "no window with app_id \"never-opens\" appeared within 1 sec (--list-windows shows \
         every open window)"
    );
}

#[test]
fn existing_wait_fails_at_once_when_several_windows_match() {
    // Waiting can't make an ambiguous choice less ambiguous.
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    {
        let mut sway = fake.sway();
        sway.open_window(Some("twin"), None);
        sway.open_window(Some("twin"), None);
    }
    let started = std::time::Instant::now();
    let run = run(
        &fake,
        &["--existing", "--wait", "-a", "twin", "--timeout", "5"],
    );
    assert!(!run.succeeded());
    assert!(
        run.error().contains("2 windows match app_id \"twin\""),
        "{}",
        run.error()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn existing_wait_applies_its_actions_to_the_window_it_waited_for() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if let Some(id) = command
            .strip_prefix("[con_id=")
            .and_then(|rest| rest.strip_suffix("] floating enable"))
            .and_then(|id| id.parse().ok())
        {
            sway.float(id);
            sway.emit("floating", id);
        }
        Ok(())
    });
    fake.sway().after(Duration::from_millis(300), |sway| {
        let id = sway.open_window(Some("float-me"), None);
        sway.emit("new", id);
    });
    let run = run(
        &fake,
        &["--existing", "--wait", "-a", "float-me", "--floating"],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
}

/// A handler that carries out `floating enable`, `resize set height|width
/// <N>px`, `move position <X> <Y>` and `mark "<M>"` on whichever window a
/// command names, emitting the events Sway would. `move position` on a tiled
/// window is refused with Sway's own message.
fn arranges_on_command(sway: &mut Sway, command: &str) -> Result<(), String> {
    let (target, verb) = command
        .strip_prefix("[con_id=")
        .and_then(|rest| rest.split_once("] "))
        .expect("every follow-up command names its container");
    let id: i64 = target.parse().expect("con_id is a number");
    let words: Vec<&str> = verb.split_whitespace().collect();
    let pixels = |word: &str| -> i64 { word.trim_end_matches("px").parse().unwrap() };
    match words.as_slice() {
        ["floating", "enable"] => {
            sway.float(id);
            sway.emit("floating", id);
        }
        ["resize", "set", "height", height] => {
            let decoration = sway.node(id)["deco_rect"]["height"].as_i64().unwrap();
            sway.node_mut(id)["rect"]["height"] = (pixels(height) - decoration).into();
        }
        ["resize", "set", "width", width] => {
            sway.node_mut(id)["rect"]["width"] = pixels(width).into();
        }
        ["move", "position", x, y] => {
            if sway.node(id)["type"] != "floating_con" {
                return Err("Only floating containers can be moved to an absolute position".into());
            }
            sway.node_mut(id)["deco_rect"]["x"] = x.parse::<i64>().unwrap().into();
            sway.node_mut(id)["deco_rect"]["y"] = y.parse::<i64>().unwrap().into();
        }
        ["mark", mark] => {
            let mark = mark.trim_matches('"').to_string();
            sway.node_mut(id)["marks"]
                .as_array_mut()
                .expect("marks is an array")
                .push(mark.into());
            sway.emit("mark", id);
        }
        _ => {}
    }
    Ok(())
}

fn action_statuses(run: &Run) -> Vec<(String, String)> {
    run.json["actions"]
        .as_array()
        .unwrap_or_else(|| panic!("no actions in {}", run.json))
        .iter()
        .map(|action| {
            (
                action["kind"].as_str().unwrap().to_string(),
                action["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn follow_up_actions_reach_sway_as_one_command_and_are_each_confirmed() {
    // #32: one command, so the window takes its whole arrangement at once.
    for version in SwayVersion::ALL {
        let fake = FakeSway::start(version, arranges_on_command);
        let run = run(
            &fake,
            &[
                "--con-id",
                "5",
                "--floating",
                "--height",
                "500px",
                "--width",
                "700px",
                "--position",
                "100,100",
                "--mark",
                "m",
            ],
        );
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        assert_eq!(
            fake.commands(),
            vec![
                "[con_id=5] floating enable, resize set height 500px, resize set width 700px, \
                 move position 100 100, mark \"m\""
                    .to_string()
            ],
            "{version:?}"
        );
        let changed = |kind: &str| (kind.to_string(), "changed".to_string());
        assert_eq!(
            action_statuses(&run),
            vec![
                changed("floating"),
                changed("height"),
                changed("width"),
                changed("position"),
                changed("mark"),
            ],
            "{version:?}"
        );
    }
}

#[test]
fn an_action_already_in_effect_is_left_out_of_the_command() {
    let fake = FakeSway::start(SwayVersion::V1_11, arranges_on_command);
    let run = run(
        &fake,
        &[
            "--con-id",
            &FLOATING.to_string(),
            "--floating",
            "--mark",
            "m",
        ],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(
        fake.commands(),
        vec![format!("[con_id={FLOATING}] mark \"m\"")]
    );
    assert_eq!(
        action_statuses(&run),
        vec![
            ("floating".to_string(), "already_satisfied".to_string()),
            ("mark".to_string(), "changed".to_string()),
        ]
    );
}

#[test]
fn a_part_of_the_command_sway_refuses_fails_naming_that_action() {
    let fake = FakeSway::start(SwayVersion::V1_11, arranges_on_command);
    let run = run(
        &fake,
        &["--con-id", "5", "--position", "10,10", "--mark", "m"],
    );
    assert!(!run.succeeded());
    assert_eq!(
        run.error(),
        "move position 10 10: command failed with 'Only floating containers can be moved to an \
         absolute position'"
    );
    assert_eq!(fake.commands().len(), 1, "sent {:?}", fake.commands());
}

#[test]
fn a_batched_action_whose_event_never_comes_times_out_naming_it() {
    // The mark's event arrives and confirms it; the floating one never does.
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.ends_with("floating enable") {
            return Ok(());
        }
        arranges_on_command(sway, command)
    });
    let run = run(
        &fake,
        &[
            "--con-id",
            "5",
            "--floating",
            "--mark",
            "m",
            "--timeout",
            "1",
        ],
    );
    assert!(!run.succeeded());
    assert_eq!(
        run.error(),
        "floating enable: no matching window event within 1 sec"
    );
}

#[test]
fn a_batch_with_nothing_left_to_send_sends_nothing() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(
        &fake,
        &["--con-id", &FLOATING.to_string(), "--floating", "--focus"],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    assert!(fake.commands().is_empty(), "sent {:?}", fake.commands());
}

#[test]
fn a_new_column_is_confirmed_once_the_window_has_moved() {
    // Sent on its own, outside the batch, so it's polled by run_wait_time().
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command == format!("[con_id={TILED}] move right") {
            sway.node_mut(TILED)["rect"]["x"] = 640.into();
        }
        Ok(())
    });
    let run = run(&fake, &["--con-id", "5", "--new-column", "--verbose"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
    assert!(
        run.stderr()
            .contains("Confirmed via poll (container id: 5)"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_new_column_that_never_moves_the_window_is_unconfirmed() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(&fake, &["--con-id", "5", "--new-column", "--verbose"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "unconfirmed");
    assert!(
        run.stderr()
            .contains("Poll grace period elapsed without confirmation, falling back"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_move_whose_event_arrives_before_its_state_keeps_waiting() {
    // The lone-action counterpart of the batch's same rule.
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.ends_with("move workspace \"2\"") {
            sway.emit("move", TILED);
            sway.after(Duration::from_millis(200), |sway| {
                sway.node_mut(WORKSPACE)["name"] = "2".into();
                sway.emit("move", TILED);
            });
        }
        Ok(())
    });
    let run = run(&fake, &["--con-id", "5", "--workspace", "2", "--verbose"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
    assert!(
        run.stderr()
            .contains("the requested state is not in effect"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_batch_explains_each_action_under_verbose() {
    let fake = FakeSway::start(SwayVersion::V1_11, arranges_on_command);
    let run = run(
        &fake,
        &[
            "--con-id",
            &FLOATING.to_string(),
            "--floating",
            "--split",
            "v",
            "--mark",
            "m",
            "--verbose",
        ],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    let stderr = run.stderr();
    for expected in [
        "Already in effect, not sent: floating enable",
        "Some actions have no matching event type. Will wait 20 ms",
        "Sway command: [con_id=6] splitv, mark \"m\"",
        "Poll grace period elapsed without confirmation: splitv",
        "Event match: Mark container id 6 (mark \"m\")",
    ] {
        assert!(stderr.contains(expected), "missing {expected:?}: {stderr}");
    }
    assert_eq!(
        action_statuses(&run),
        vec![
            ("split".to_string(), "unconfirmed".to_string()),
            ("floating".to_string(), "already_satisfied".to_string()),
            ("mark".to_string(), "changed".to_string()),
        ]
    );
}

#[test]
fn a_split_is_confirmed_once_the_parent_layout_changes() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command == format!("[con_id={TILED}] splitv") {
            sway.node_mut(WORKSPACE)["layout"] = "splitv".into();
        }
        Ok(())
    });
    let run = run(&fake, &["--con-id", "5", "--split", "v"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
}

#[test]
fn a_polled_action_that_never_shows_in_the_tree_is_unconfirmed_not_failed() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(&fake, &["--con-id", "5", "--split", "v"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "unconfirmed");
}

#[test]
fn a_width_is_confirmed_once_the_window_has_it() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.ends_with("resize set width 600px") {
            sway.node_mut(TILED)["rect"]["width"] = 600.into();
        }
        Ok(())
    });
    let run = run(&fake, &["--con-id", "5", "--width", "600px"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "changed");
}

#[test]
fn a_new_column_for_a_floating_window_is_skipped_without_sending_it() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(&fake, &["--con-id", &FLOATING.to_string(), "--new-column"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.action_status(), "skipped");
    assert!(
        !fake
            .commands()
            .iter()
            .any(|command| command.contains("move right")),
        "sent {:?}",
        fake.commands()
    );
}

#[test]
fn a_window_closing_during_the_wait_time_fails_before_its_command_is_sent() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    fake.sway().after(Duration::from_millis(200), |sway| {
        sway.remove(TILED);
    });
    let run = run(
        &fake,
        &["--con-id", "5", "--split", "v", "--wait-time", "800"],
    );
    assert!(!run.succeeded());
    assert!(run.error().contains("no longer exists"), "{}", run.error());
    assert!(
        !fake
            .commands()
            .iter()
            .any(|command| command.contains("splitv")),
        "sent {:?}",
        fake.commands()
    );
}

/// A handler that runs every `exec` for real and, once the launched process
/// exists, opens a window for it with `app_id`, owned by that process.
fn opens_a_window_for_each_launch(
    app_id: &'static str,
) -> impl FnMut(&mut Sway, &str) -> Result<(), String> + Send + 'static {
    move |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            sway.when_marked(token, move |sway, pid| {
                let id = sway.open_window(Some(app_id), Some(pid));
                sway.emit("new", id);
            });
        }
        Ok(())
    }
}

#[test]
fn a_launched_window_carrying_the_marker_is_ours() {
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_a_window_for_each_launch("fake-app"),
    );
    let run = run(&fake, &["-a", "fake-app", "--verbose", "sleep 30"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], 100);
    assert!(
        run.stderr().contains("(PID-marker-confirmed)"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_matching_window_from_another_process_is_adopted_after_the_grace_period() {
    // The single-instance case: the launched process hands off to one that
    // was never given the marker, and the window belongs to that one.
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            sway.launch(command);
            let id = sway.open_window(Some("fake-app"), Some(std::process::id()));
            sway.emit("new", id);
        }
        Ok(())
    });
    let run = run(&fake, &["-a", "fake-app", "--verbose", "sleep 30"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], 100);
    assert!(
        run.stderr()
            .contains("adopted, not confirmed as launched by this run"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_matching_window_is_ours_once_the_marked_process_has_come_and_gone() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            sway.when_marked(token, |sway, _| {
                let id = sway.open_window(Some("fake-app"), Some(std::process::id()));
                sway.emit("new", id);
            });
        }
        Ok(())
    });
    let run = run(&fake, &["-a", "fake-app", "--verbose", "sleep 0.5"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], 100);
    assert!(
        run.stderr().contains("Marked process no longer running"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_window_the_filter_rejects_is_named_in_the_timeout() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            sway.launch(command);
            let id = sway.open_window(Some("other-app"), Some(std::process::id()));
            sway.emit("new", id);
        }
        Ok(())
    });
    let run = run(&fake, &["-a", "fake-app", "--timeout", "1", "sleep 30"]);
    assert!(!run.succeeded());
    assert!(
        run.error()
            .contains("1 other new window did (con_id 100, app_id \"other-app\")"),
        "{}",
        run.error()
    );
    assert_eq!(run.json["unmatched_windows"][0]["app_id"], "other-app");
}

#[test]
fn a_launched_window_that_sets_its_app_id_late_is_found_by_the_recheck() {
    // Sway sends no event when a mapped window changes its app_id.
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            sway.when_marked(token, |sway, pid| {
                let id = sway.open_window(Some("not-yet"), Some(pid));
                sway.emit("new", id);
                sway.after(Duration::from_millis(300), move |sway| {
                    sway.node_mut(id)["app_id"] = "fake-app".into();
                });
            });
        }
        Ok(())
    });
    let run = run(&fake, &["-a", "fake-app", "--verbose", "sleep 30"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], 100);
    assert!(run.stderr().contains("Recheck match"), "{}", run.stderr());
}

#[test]
fn a_launch_whose_command_exits_without_a_window_says_so() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            sway.launch(command);
        }
        Ok(())
    });
    let run = run(&fake, &["-a", "fake-app", "--timeout", "1", "true"]);
    assert!(!run.succeeded());
    assert!(
        run.error()
            .contains("the launched command exited without opening a window"),
        "{}",
        run.error()
    );
}

#[test]
fn a_launch_whose_command_is_still_running_says_so() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            sway.launch(command);
        }
        Ok(())
    });
    let run = run(&fake, &["-a", "fake-app", "--timeout", "1", "sleep 30"]);
    assert!(!run.succeeded());
    assert!(
        run.error()
            .contains("the launched command is still running"),
        "{}",
        run.error()
    );
}

#[test]
fn a_launch_runs_in_its_cwd_with_its_env() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let fake = FakeSway::start(SwayVersion::V1_11, move |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            let sender = sender.clone();
            sway.when_marked(token, move |sway, pid| {
                let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
                let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
                let _ = sender.send((cwd, environ));
                let id = sway.open_window(Some("fake-app"), Some(pid));
                sway.emit("new", id);
            });
        }
        Ok(())
    });
    let directory = TestTempDir::new();
    let run = run(
        &fake,
        &[
            "-a",
            "fake-app",
            "--cwd",
            directory.to_str().unwrap(),
            "--env",
            "FAKE_SWAY_GREETING=hello world",
            "sleep 30",
        ],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    let (cwd, environ) = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("the launched process was never seen");
    assert_eq!(cwd.as_deref(), Some(&*directory));
    assert!(
        environ
            .split(|byte| *byte == 0)
            .any(|variable| variable == b"FAKE_SWAY_GREETING=hello world"),
        "{:?}",
        String::from_utf8_lossy(&environ)
    );
}

#[test]
fn existing_resolves_the_one_window_with_the_app_id() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    let run = run(&fake, &["--existing", "-a", "fixture-tiled"]);
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], TILED);
}

#[test]
fn existing_refuses_to_guess_between_two_matching_windows() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_11);
    fake.sway().open_window(Some("fixture-tiled"), None);
    let run = run(&fake, &["--existing", "-a", "fixture-tiled"]);
    assert!(!run.succeeded());
    assert!(run.error().contains("100"), "{}", run.error());
}

/// Two steps: a launch that opens its window, then one that never does.
const LAYOUT_WHOSE_SECOND_STEP_TIMES_OUT: &str = r#"
[[step]]
command = "sleep 30"
app_id = "fake-app"

[[step]]
command = "sleep 30"
app_id = "never-appears"
timeout = 1
"#;

#[test]
fn rollback_closes_the_window_an_earlier_step_launched() {
    let mut opens = opens_a_window_for_each_launch("fake-app");
    let fake = FakeSway::start(SwayVersion::V1_11, move |sway, command| {
        if command == "[con_id=100] kill" {
            sway.remove(100);
            return Ok(());
        }
        opens(sway, command)
    });
    let layout = TempToml::write("rollback", LAYOUT_WHOSE_SECOND_STEP_TIMES_OUT);
    let run = run(
        &fake,
        &["--layout", layout.to_str().unwrap(), "--rollback-on-error"],
    );
    assert!(!run.succeeded());
    assert_eq!(
        run.json["rolled_back"],
        serde_json::json!([100]),
        "{}",
        run.json
    );
    assert!(run.json["rollback_failed"].is_null(), "{}", run.json);
}

#[test]
fn rollback_reports_a_window_that_stayed_open_after_its_kill() {
    // `kill` asks the client to close; Sway replies before it has, and a
    // client can refuse (an unsaved-changes prompt).
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_a_window_for_each_launch("fake-app"),
    );
    let layout = TempToml::write("rollback", LAYOUT_WHOSE_SECOND_STEP_TIMES_OUT);
    let run = run(
        &fake,
        &["--layout", layout.to_str().unwrap(), "--rollback-on-error"],
    );
    assert!(!run.succeeded());
    assert_eq!(
        run.json["rolled_back"],
        serde_json::json!([]),
        "{}",
        run.json
    );
    assert!(
        fake.commands().contains(&"[con_id=100] kill".to_string()),
        "{:?}",
        fake.commands()
    );
    assert_eq!(
        run.json["rollback_failed"],
        serde_json::json!([100]),
        "{}",
        run.json
    );
}

/// Runs `arguments` with `--json` and parses stdout, for a run that exits
/// non-zero but still reports its result there (`--continue-on-timeout`).
fn run_reporting_on_stdout(fake: &FakeSway, arguments: &[&str]) -> (Output, Value) {
    let output = fake
        .sway_launch()
        .args(arguments)
        .arg("--json")
        .output()
        .expect("failed to run sway-launch");
    let json = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout isn't a JSON result ({error}); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, json)
}

#[test]
fn continue_on_timeout_carries_on_past_a_missing_window_and_skips_its_dependents() {
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_a_window_for_each_launch("fake-app"),
    );
    let layout = TempToml::write(
        "continue-on-timeout",
        r#"
[[step]]
id = "slow"
command = "sleep 30"
app_id = "slow-app"
timeout = 1

[[step]]
target_id = "slow"
floating = true

[[step]]
command = "sleep 30"
app_id = "fake-app"
"#,
    );
    let (output, json) = run_reporting_on_stdout(
        &fake,
        &[
            "--layout",
            layout.to_str().unwrap(),
            "--continue-on-timeout",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{json}");
    // The first launch's window (100) failed its filter; the third step's is 101.
    assert_eq!(json["container_ids"], serde_json::json!([101]), "{json}");
    assert_eq!(json["timed_out_steps"][0]["step"], 1, "{json}");
    assert_eq!(
        json["timed_out_steps"][0]["unmatched_windows"][0]["container_id"], 100,
        "{json}"
    );
    assert_eq!(
        json["skipped_steps"],
        serde_json::json!([{ "step": 2, "target_id": "slow" }]),
        "{json}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("step 1: launch") && stderr.contains("continuing: --continue-on-timeout"),
        "{stderr}"
    );
    assert!(
        !fake
            .commands()
            .iter()
            .any(|command| command.contains("floating")),
        "the skipped step sent {:?}",
        fake.commands()
    );
}

#[test]
fn continue_on_timeout_also_covers_an_existing_wait_that_sees_no_window() {
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_a_window_for_each_launch("fake-app"),
    );
    let layout = TempToml::write(
        "continue-on-wait-timeout",
        r#"
[[step]]
existing = true
wait = true
app_id = "never-opens"
timeout = 1

[[step]]
command = "sleep 30"
app_id = "fake-app"
"#,
    );
    let (output, json) = run_reporting_on_stdout(
        &fake,
        &[
            "--layout",
            layout.to_str().unwrap(),
            "--continue-on-timeout",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{json}");
    assert_eq!(json["container_ids"], serde_json::json!([100]), "{json}");
    assert_eq!(json["timed_out_steps"][0]["step"], 1, "{json}");
}

#[test]
fn continue_on_timeout_still_stops_on_any_other_failure() {
    let fake = FakeSway::start(SwayVersion::V1_11, |_, command| {
        if command.contains("floating") {
            return Err("refused".to_string());
        }
        Ok(())
    });
    let layout = TempToml::write(
        "continue-on-timeout-other-failure",
        &format!("[[step]]\ncon_id = {TILED}\nfloating = true\n"),
    );
    let run = run(
        &fake,
        &[
            "--layout",
            layout.to_str().unwrap(),
            "--continue-on-timeout",
        ],
    );
    assert!(!run.succeeded());
    assert!(run.error().contains("step 1:"), "{}", run.error());
}

/// A handler that opens a `fake-app` window for each launch, `delays[n]`
/// after the nth launch's process exists, and closes a window on `kill`.
fn opens_windows_after(
    delays: Vec<Duration>,
) -> impl FnMut(&mut Sway, &str) -> Result<(), String> + Send + 'static {
    let mut launches = 0;
    move |sway, command| {
        if let Some(id) = command
            .strip_prefix("[con_id=")
            .and_then(|rest| rest.strip_suffix("] kill"))
        {
            sway.remove(id.parse().expect("a kill names a container id"));
        } else if command.starts_with("exec ") {
            let delay = delays[launches];
            launches += 1;
            let token = sway.launch(command);
            sway.when_marked(token, move |sway, pid| {
                sway.after(delay, move |sway| {
                    let id = sway.open_window(Some("fake-app"), Some(pid));
                    sway.emit("new", id);
                });
            });
        }
        Ok(())
    }
}

/// The error object a failed `--json` run printed last on stderr.
fn stderr_error_json(output: &Output) -> Value {
    let stderr = String::from_utf8_lossy(&output.stderr);
    serde_json::from_str(stderr.lines().next_back().unwrap_or_default())
        .unwrap_or_else(|error| panic!("no JSON error on stderr ({error}): {stderr}"))
}

#[test]
fn a_signal_during_a_layout_fails_it_with_rollback_and_the_signal_s_status() {
    for (signal, status) in [("INT", 130), ("TERM", 143)] {
        let fake = FakeSway::start(
            SwayVersion::V1_11,
            opens_windows_after(vec![Duration::ZERO, Duration::from_secs(60)]),
        );
        let layout = TempToml::write(
            "interrupted",
            "[[step]]\ncommand = \"sleep 30\"\napp_id = \"fake-app\"\n\n\
             [[step]]\ncommand = \"sleep 30\"\napp_id = \"fake-app\"\ntimeout = 30\n",
        );
        let child = fake
            .sway_launch()
            .args([
                "--layout",
                layout.to_str().unwrap(),
                "--rollback-on-error",
                "--json",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to start sway-launch");
        // Signalled once the second step is waiting for its window.
        let waiting_since = std::time::Instant::now();
        while fake
            .commands()
            .iter()
            .filter(|command| command.starts_with("exec "))
            .count()
            < 2
        {
            assert!(waiting_since.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));
        let killed = std::process::Command::new("kill")
            .args([format!("-{signal}"), child.id().to_string()])
            .status()
            .expect("failed to run kill");
        assert!(killed.success());

        let output = child.wait_with_output().expect("sway-launch should exit");
        assert_eq!(output.status.code(), Some(status), "{output:?}");
        let error = stderr_error_json(&output);
        assert!(
            error["error"].as_str().unwrap().contains(&format!(
                "step 2: launch \"sleep 30\": interrupted by SIG{signal}"
            )),
            "{error}"
        );
        assert_eq!(error["rolled_back"], serde_json::json!([100]), "{error}");
    }
}

#[test]
fn rollback_closes_a_window_that_opens_just_after_its_launch_timed_out() {
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_windows_after(vec![Duration::ZERO, Duration::from_millis(1500)]),
    );
    let layout = TempToml::write(
        "late-window",
        "[[step]]\ncommand = \"sleep 30\"\napp_id = \"fake-app\"\n\n\
         [[step]]\ncommand = \"sleep 30\"\napp_id = \"fake-app\"\ntimeout = 1\n",
    );
    let run = run(
        &fake,
        &["--layout", layout.to_str().unwrap(), "--rollback-on-error"],
    );
    assert!(!run.succeeded());
    assert!(
        run.error()
            .contains("its window (con_id 101) opened within 2 sec after that"),
        "{}",
        run.error()
    );
    assert_eq!(
        run.json["rolled_back"],
        serde_json::json!([101, 100]),
        "{}",
        run.json
    );
    assert!(!fake.sway().contains(101));
}

#[test]
fn without_rollback_a_timed_out_launch_doesn_t_wait_for_a_late_window() {
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_windows_after(vec![Duration::from_millis(1500)]),
    );
    let layout = TempToml::write(
        "late-window-no-rollback",
        "[[step]]\ncommand = \"sleep 30\"\napp_id = \"fake-app\"\ntimeout = 1\n",
    );
    let started = std::time::Instant::now();
    let run = run(&fake, &["--layout", layout.to_str().unwrap()]);
    assert!(!run.succeeded());
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(run.json["container_id"].is_null(), "{}", run.json);
}

/// A handler that runs every `exec` and, once the launched process exists,
/// opens one window per title for it, in order, all with `app_id`.
fn opens_titled_windows_for_each_launch(
    app_id: &'static str,
    titles: &'static [&'static str],
) -> impl FnMut(&mut Sway, &str) -> Result<(), String> + Send + 'static {
    move |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            sway.when_marked(token, move |sway, pid| {
                for title in titles {
                    let id = sway.open_window(Some(app_id), Some(pid));
                    sway.node_mut(id)["name"] = serde_json::json!(title);
                    sway.emit("new", id);
                }
            });
        }
        Ok(())
    }
}

#[test]
fn a_launch_with_a_title_skips_the_splash_window_for_the_titled_one() {
    for version in SwayVersion::ALL {
        let fake = FakeSway::start(
            version,
            opens_titled_windows_for_each_launch("fake-ide", &["Loading…", "project-a — Fake IDE"]),
        );
        let run = run(
            &fake,
            &["-a", "fake-ide", "--title", "project-a", "sleep 30"],
        );
        assert!(run.succeeded(), "{version:?}: {}", run.stderr());
        assert_eq!(run.json["container_id"], 101, "{version:?}");
    }
}

#[test]
fn a_launched_window_that_retitles_itself_is_taken_on_its_title_event() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            sway.when_marked(token, |sway, pid| {
                let id = sway.open_window(Some("fake-ide"), Some(pid));
                sway.node_mut(id)["name"] = serde_json::json!("Loading…");
                sway.emit("new", id);
                sway.after(Duration::from_millis(300), move |sway| {
                    sway.node_mut(id)["name"] = serde_json::json!("project-a — Fake IDE");
                    sway.emit("title", id);
                });
            });
        }
        Ok(())
    });
    let run = run(
        &fake,
        &[
            "-a",
            "fake-ide",
            "--title",
            "^project-a",
            "--verbose",
            "sleep 30",
        ],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], 100);
    assert!(
        run.stderr().contains("(PID-marker-confirmed)"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_launched_window_retitled_without_an_event_is_found_by_the_recheck() {
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            let token = sway.launch(command);
            sway.when_marked(token, |sway, pid| {
                let id = sway.open_window(Some("fake-ide"), Some(pid));
                sway.node_mut(id)["name"] = serde_json::json!("Loading…");
                sway.emit("new", id);
                sway.after(Duration::from_millis(300), move |sway| {
                    sway.node_mut(id)["name"] = serde_json::json!("project-a — Fake IDE");
                });
            });
        }
        Ok(())
    });
    let run = run(
        &fake,
        &["-a", "fake-ide", "--title", "project-a", "sleep 30"],
    );
    assert!(run.succeeded(), "{}", run.stderr());
    assert_eq!(run.json["container_id"], 100);
}

#[test]
fn an_older_window_renamed_during_a_launch_is_never_taken() {
    // Only a window that opened during the run gets a second look on a title
    // change; a pre-existing one renaming itself (a browser tab switching)
    // is someone else's window.
    let fake = FakeSway::start(SwayVersion::V1_11, |sway, command| {
        if command.starts_with("exec ") {
            sway.launch(command);
            sway.after(Duration::from_millis(200), |sway| {
                sway.node_mut(FLOATING)["name"] = serde_json::json!("project-a");
                sway.emit("title", FLOATING);
            });
        }
        Ok(())
    });
    let run = run(
        &fake,
        &[
            "-a",
            "fixture-floating",
            "--title",
            "project-a",
            "--timeout",
            "1",
            "sleep 30",
        ],
    );
    assert!(!run.succeeded());
    assert!(
        run.error().contains(
            "no window with app_id \"fixture-floating\" and title matching \"project-a\" \
             appeared within 1 sec"
        ),
        "{}",
        run.error()
    );
}

#[test]
fn a_title_mismatch_is_reported_with_the_window_s_title_on_timeout() {
    let fake = FakeSway::start(
        SwayVersion::V1_11,
        opens_titled_windows_for_each_launch("fake-ide", &["Welcome"]),
    );
    let run = run(
        &fake,
        &[
            "-a",
            "fake-ide",
            "--title",
            "project-a",
            "--timeout",
            "1",
            "sleep 30",
        ],
    );
    assert!(!run.succeeded());
    assert!(
        run.error().contains(
            "(con_id 100, app_id \"fake-ide\", title \"Welcome\", opened by this command)"
        ),
        "{}",
        run.error()
    );
    assert!(run.error().contains("check --app-id/--class/--title"));
    assert_eq!(run.json["unmatched_windows"][0]["title"], "Welcome");
}

#[test]
fn existing_and_list_windows_narrow_by_title() {
    let fake = FakeSway::accepting_everything(SwayVersion::V1_9);
    let (left, right) = {
        let mut sway = fake.sway();
        let left = sway.open_window(Some("twin"), None);
        sway.node_mut(left)["name"] = serde_json::json!("left pane");
        let right = sway.open_window(Some("twin"), None);
        sway.node_mut(right)["name"] = serde_json::json!("right pane");
        (left, right)
    };

    let ambiguous = run(&fake, &["--existing", "-a", "twin"]);
    assert!(!ambiguous.succeeded());
    let narrowed = run(&fake, &["--existing", "-a", "twin", "--title", "^right"]);
    assert!(narrowed.succeeded(), "{}", narrowed.stderr());
    assert_eq!(narrowed.json["container_id"], right);
    let title_alone = run(&fake, &["--existing", "--title", "left pane"]);
    assert_eq!(title_alone.json["container_id"], left);

    let listed = run(&fake, &["--list-windows", "--title", "pane$"]);
    let ids: Vec<&Value> = listed.json["windows"]
        .as_array()
        .expect("a windows array")
        .iter()
        .map(|window| &window["container_id"])
        .collect();
    assert_eq!(ids, [left, right]);
}
