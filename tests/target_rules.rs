// The target-selection rules (exactly one of a command, a container id or
// `existing`; at most one matcher; `existing` needs one and `mark_match` needs
// `existing`; no blank value) are written once, in `Target::from_fields()`,
// but each front end reaches it differently: the CLI through clap's own
// conflicts first, a layout step through `to_sway_launch()`, and a template
// binding through `resolve()`. This drives one table of combinations through
// all three and requires the same accept/reject answer from each, so a rule
// added to one path and not another fails here. Headless: the CLI runs with
// `--dry-run` and the files with `--validate`, neither of which touches Sway.

mod common;

use common::TempToml;
use std::process::Command;

/// One combination of target fields, as each front end spells it.
struct Case {
    name: &'static str,
    command: Option<&'static str>,
    con_id: Option<i64>,
    existing: bool,
    app_id: Option<&'static str>,
    class: Option<&'static str>,
    mark_match: Option<&'static str>,
    accepted: bool,
}

const fn case(name: &'static str, accepted: bool) -> Case {
    Case {
        name,
        command: None,
        con_id: None,
        existing: false,
        app_id: None,
        class: None,
        mark_match: None,
        accepted,
    }
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            command: Some("foot"),
            ..case("command", true)
        },
        Case {
            command: Some("foot"),
            app_id: Some("foot"),
            ..case("command filtered on app_id", true)
        },
        Case {
            command: Some("foot"),
            class: Some("Foot"),
            ..case("command filtered on class", true)
        },
        Case {
            command: Some("foot"),
            mark_match: Some("m"),
            ..case("command with mark_match", false)
        },
        Case {
            con_id: Some(42),
            ..case("con_id", true)
        },
        Case {
            con_id: Some(42),
            app_id: Some("foot"),
            ..case("con_id with app_id", false)
        },
        Case {
            existing: true,
            ..case("existing without a matcher", false)
        },
        Case {
            existing: true,
            app_id: Some("foot"),
            ..case("existing by app_id", true)
        },
        Case {
            existing: true,
            mark_match: Some("m"),
            ..case("existing by mark", true)
        },
        Case {
            existing: true,
            app_id: Some("foot"),
            class: Some("Foot"),
            ..case("existing with two matchers", false)
        },
        Case {
            command: Some("foot"),
            con_id: Some(42),
            ..case("command and con_id", false)
        },
        Case {
            command: Some("foot"),
            existing: true,
            app_id: Some("foot"),
            ..case("command and existing", false)
        },
        Case {
            command: Some("   "),
            ..case("blank command", false)
        },
        Case {
            command: Some("foot"),
            app_id: Some(" "),
            ..case("blank app_id", false)
        },
        Case {
            command: Some("foot; firefox"),
            ..case("command Sway would split", false)
        },
        case("no target at all", false),
    ]
}

fn cli_accepts(case: &Case) -> bool {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sway-launch"));
    command.arg("--dry-run");
    if let Some(con_id) = case.con_id {
        command.args(["--con-id", &con_id.to_string()]);
    }
    if case.existing {
        command.arg("--existing");
    }
    for (flag, value) in [
        ("--app-id", case.app_id),
        ("--class", case.class),
        ("--mark-match", case.mark_match),
    ] {
        if let Some(value) = value {
            command.args([flag, value]);
        }
    }
    if let Some(launch) = case.command {
        command.args(["--", launch]);
    }
    command
        .output()
        .expect("failed to run sway-launch binary")
        .status
        .success()
}

/// The case's fields as TOML lines, shared by a layout step and a binding.
fn toml_fields(case: &Case) -> String {
    let mut lines = String::new();
    if let Some(command) = case.command {
        lines.push_str(&format!("command = {:?}\n", command));
    }
    if let Some(con_id) = case.con_id {
        lines.push_str(&format!("con_id = {}\n", con_id));
    }
    if case.existing {
        lines.push_str("existing = true\n");
    }
    for (field, value) in [
        ("app_id", case.app_id),
        ("class", case.class),
        ("mark_match", case.mark_match),
    ] {
        if let Some(value) = value {
            lines.push_str(&format!("{} = {:?}\n", field, value));
        }
    }
    lines
}

fn layout_accepts(case: &Case) -> bool {
    let layout = TempToml::write("rules-layout", &format!("[[step]]\n{}", toml_fields(case)));
    Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args(["--layout", layout.to_str().unwrap(), "--validate"])
        .output()
        .expect("failed to run sway-launch binary")
        .status
        .success()
}

fn binding_accepts(case: &Case) -> bool {
    let template = TempToml::write(
        "rules-template",
        "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n\
         [[step]]\nslot = \"only\"\n",
    );
    let bindings = TempToml::write(
        "rules-bindings",
        &format!("[[binding]]\nslot = \"only\"\n{}", toml_fields(case)),
    );
    Command::new(env!("CARGO_BIN_EXE_sway-launch"))
        .args([
            "--template",
            template.to_str().unwrap(),
            "--bindings",
            bindings.to_str().unwrap(),
            "--validate",
        ])
        .output()
        .expect("failed to run sway-launch binary")
        .status
        .success()
}

#[test]
fn cli_layout_steps_and_bindings_agree_on_every_target_combination() {
    let mut disagreements = Vec::new();
    for case in cases() {
        let answers = [
            ("CLI", cli_accepts(&case)),
            ("layout step", layout_accepts(&case)),
            ("binding", binding_accepts(&case)),
        ];
        for (front_end, accepted) in answers {
            if accepted != case.accepted {
                disagreements.push(format!(
                    "{}: {} {} it",
                    case.name,
                    front_end,
                    if accepted { "accepted" } else { "rejected" }
                ));
            }
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
}
