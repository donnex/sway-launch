// The two cross-field placement rules (a workspace by name or by number, not
// both; no workspace move alongside `new_column`/`new_row`, #45) are written
// once, in `workspace_choice()` and `check_relative_move()`, but the CLI
// enforces them with clap's own conflicts instead, and each file schema calls
// them from its own place: a layout step from `to_sway_launch()`, a binding
// from `resolve()`, a template's `[layout]` table from `parse()`. This drives
// one table of combinations through every front end and requires the same
// accept/reject answer from each, as `target_rules.rs` does for target
// selection. Headless: the CLI runs with `--dry-run` and the files with
// `--validate`, neither of which touches Sway.

mod common;

use common::{sway_launch, TempToml};

/// One combination of placement fields.
struct Case {
    name: &'static str,
    workspace: Option<&'static str>,
    workspace_number: Option<i64>,
    new_column: bool,
    new_row: bool,
    accepted: bool,
    /// Rejected only for combining a workspace with a relative move.
    relative_move_alone_rejects: bool,
}

const fn case(name: &'static str, accepted: bool) -> Case {
    Case {
        name,
        workspace: None,
        workspace_number: None,
        new_column: false,
        new_row: false,
        accepted,
        relative_move_alone_rejects: false,
    }
}

fn cases() -> Vec<Case> {
    vec![
        case("no placement", true),
        Case {
            workspace: Some("web"),
            ..case("workspace by name", true)
        },
        Case {
            workspace_number: Some(3),
            ..case("workspace by number", true)
        },
        Case {
            workspace: Some("web"),
            workspace_number: Some(3),
            ..case("workspace by name and by number", false)
        },
        Case {
            workspace_number: Some(-1),
            ..case("negative workspace number", false)
        },
        Case {
            workspace_number: Some(i64::from(i32::MAX) + 1),
            ..case("workspace number past i32", false)
        },
        Case {
            workspace: Some("next"),
            ..case("workspace keyword", false)
        },
        Case {
            new_column: true,
            new_row: true,
            ..case("new_column and new_row", true)
        },
        Case {
            workspace: Some("web"),
            new_column: true,
            relative_move_alone_rejects: true,
            ..case("workspace with new_column", false)
        },
        Case {
            workspace_number: Some(3),
            new_row: true,
            relative_move_alone_rejects: true,
            ..case("workspace number with new_row", false)
        },
    ]
}

fn succeeds(command: &mut std::process::Command) -> bool {
    command
        .output()
        .expect("failed to run sway-launch binary")
        .status
        .success()
}

/// The case's workspace pair as CLI arguments, under `name_flag`/`number_flag`.
fn workspace_args(case: &Case, name_flag: &str, number_flag: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(workspace) = case.workspace {
        args.push(format!("{}={}", name_flag, workspace));
    }
    if let Some(number) = case.workspace_number {
        args.push(format!("{}={}", number_flag, number));
    }
    args
}

fn cli_accepts(case: &Case) -> bool {
    let mut command = sway_launch();
    command.arg("--dry-run");
    command.args(workspace_args(case, "--workspace", "--workspace-number"));
    if case.new_column {
        command.arg("--new-column");
    }
    if case.new_row {
        command.arg("--new-row");
    }
    command.args(["--", "foot"]);
    succeeds(&mut command)
}

fn workspace_toml(case: &Case) -> String {
    let mut lines = String::new();
    if let Some(workspace) = case.workspace {
        lines.push_str(&format!("workspace = {:?}\n", workspace));
    }
    if let Some(number) = case.workspace_number {
        lines.push_str(&format!("workspace_number = {}\n", number));
    }
    lines
}

fn relative_move_toml(case: &Case) -> String {
    let mut lines = String::new();
    if case.new_column {
        lines.push_str("new_column = true\n");
    }
    if case.new_row {
        lines.push_str("new_row = true\n");
    }
    lines
}

fn layout_accepts(case: &Case) -> bool {
    let layout = TempToml::write(
        "placement-layout",
        &format!(
            "[[step]]\ncommand = \"foot\"\n{}{}",
            workspace_toml(case),
            relative_move_toml(case)
        ),
    );
    succeeds(sway_launch().args(["--layout", layout.to_str().unwrap(), "--validate"]))
}

const TEMPLATE_HEADER: &str =
    "[template]\ndescription = \"Test template.\"\ncategory = \"Test\"\n\n";

fn binding_accepts(case: &Case) -> bool {
    let template = TempToml::write(
        "placement-template",
        &format!(
            "{}[[step]]\nslot = \"only\"\n{}",
            TEMPLATE_HEADER,
            relative_move_toml(case)
        ),
    );
    let bindings = TempToml::write(
        "placement-bindings",
        &format!(
            "[[binding]]\nslot = \"only\"\ncommand = \"foot\"\n{}",
            workspace_toml(case)
        ),
    );
    succeeds(sway_launch().args([
        "--template",
        template.to_str().unwrap(),
        "--bindings",
        bindings.to_str().unwrap(),
        "--validate",
    ]))
}

/// A template's `[layout]` table: a build location, so only the workspace
/// pair applies (a step building on it may use `new_column`/`new_row`).
fn layout_table_accepts(case: &Case) -> bool {
    let template = TempToml::write(
        "placement-layout-table",
        &format!(
            "{}[layout]\n{}\n[[step]]\nslot = \"only\"\n{}",
            TEMPLATE_HEADER,
            workspace_toml(case),
            relative_move_toml(case)
        ),
    );
    succeeds(sway_launch().args([
        "--template",
        template.to_str().unwrap(),
        "--apps",
        "foot",
        "--validate",
    ]))
}

/// `--on-workspace`/`--on-workspace-number`, the CLI's build location.
fn cli_build_location_accepts(case: &Case) -> bool {
    let layout = TempToml::write(
        "placement-on-workspace",
        &format!("[[step]]\ncommand = \"foot\"\n{}", relative_move_toml(case)),
    );
    let mut command = sway_launch();
    command.args(["--layout", layout.to_str().unwrap(), "--validate"]);
    command.args(workspace_args(
        case,
        "--on-workspace",
        "--on-workspace-number",
    ));
    succeeds(&mut command)
}

#[test]
fn cli_layout_steps_bindings_and_build_locations_agree_on_every_placement() {
    let mut disagreements = Vec::new();
    for case in cases() {
        // A build location is where the relative move happens, so it's
        // judged on the workspace pair alone.
        let pair_accepted = case.accepted || case.relative_move_alone_rejects;
        let answers = [
            ("CLI", cli_accepts(&case), case.accepted),
            ("layout step", layout_accepts(&case), case.accepted),
            ("binding", binding_accepts(&case), case.accepted),
            ("[layout] table", layout_table_accepts(&case), pair_accepted),
            (
                "--on-workspace",
                cli_build_location_accepts(&case),
                pair_accepted,
            ),
        ];
        for (front_end, accepted, expected) in answers {
            if accepted != expected {
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
