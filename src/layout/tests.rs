use super::*;

/// `step.to_sway_launch()` with the defaults every test here shares
/// (`--timeout 5s`, `--wait-time 20ms`, not verbose) and no earlier steps.
fn convert(step: &LayoutStep) -> Result<sway_launch::SwayLaunch<'_>, String> {
    convert_with(step, &HashMap::new())
}

/// `convert()`, with `resolved_ids` standing in for earlier named steps.
fn convert_with<'a>(
    step: &'a LayoutStep,
    resolved_ids: &HashMap<String, i64>,
) -> Result<sway_launch::SwayLaunch<'a>, String> {
    step.to_sway_launch(
        time::Duration::from_secs(5),
        time::Duration::from_millis(20),
        false,
        resolved_ids,
        sway_launch::DirectoryCheck::Exists,
    )
}

#[test]
fn parse_reads_a_representative_layout() {
    let layout = parse(
        r#"
            [[step]]
            command = "foot"
            app_id = "foot"
            split = "h"

            [[step]]
            command = "foot"
            app_id = "foot"
            "#,
    )
    .expect("valid layout should parse");

    assert_eq!(layout.step.len(), 2);
    assert_eq!(layout.step[0].command, Some("foot".to_string()));
    assert!(matches!(layout.step[0].split, Some(Split::H)));
    assert_eq!(layout.step[1].split, None);
}

#[test]
fn parse_reads_a_top_level_concurrent_key_off_by_default() {
    let step = "[[step]]\ncommand = \"foot\"\n";
    assert!(!parse(step).expect("layout should parse").concurrent);
    let concurrent = parse(&format!("concurrent = true\n{step}")).expect("layout should parse");
    assert!(concurrent.concurrent);
    assert!(parse(&format!("concurrent = \"yes\"\n{step}")).is_err());
}

#[test]
fn parse_rejects_malformed_toml() {
    assert!(parse("this is not toml [[[").is_err());
}

#[test]
fn parse_rejects_misspelled_step_field() {
    // Regression test: a prior version silently dropped unknown
    // fields, so a typo like "flaoting" for "floating" did nothing
    // instead of erroring.
    let result = parse(
        r#"
            [[step]]
            con_id = 42
            flaoting = true
            "#,
    );
    assert!(result.is_err());
}

#[test]
fn parse_reads_sway_commands_and_rejects_one_acting_on_the_session() {
    let layout =
        parse("[[step]]\ncon_id = 42\nsway_commands = [\" border none\", \"opacity 0.9\"]\n")
            .expect("window commands should parse");
    assert_eq!(layout.step[0].sway_commands, ["border none", "opacity 0.9"]);

    let error = parse("[[step]]\ncon_id = 42\nsway_commands = [\"border none\", \"exit\"]\n")
        .err()
        .expect("exit must be refused");
    assert!(error.contains("\"exit\""), "{error}");
    assert!(error.contains("acts on one window"), "{error}");
}

#[test]
fn parse_rejects_wrong_table_name() {
    // Regression test: a prior version silently parsed [[steps]]
    // (plural, wrong) as a completely empty, successfully-parsed
    // layout instead of erroring.
    assert!(parse("[[steps]]\ncon_id = 42\n").is_err());
}

#[test]
fn parse_defaults_to_no_steps_when_step_key_is_absent() {
    let layout = parse("").expect("empty file should still parse");
    assert!(layout.step.is_empty());
}

fn minimal_step() -> LayoutStep {
    LayoutStep {
        command: Some("foot".to_string()),
        cwd: None,
        env: Default::default(),
        con_id: None,
        existing: false,
        wait: false,
        target_id: None,
        id: None,
        app_id: None,
        class: None,
        mark_match: None,
        title: None,
        split: None,
        floating: false,
        sticky: false,
        fullscreen: false,
        focus: false,
        mark: None,
        new_column: false,
        new_row: false,
        workspace: None,
        workspace_number: None,
        output: None,
        height: None,
        width: None,
        position: None,
        scratchpad: false,
        sway_commands: Vec::new(),
        timeout: None,
        wait_time: None,
        settle: None,
    }
}

#[test]
fn to_sway_launch_exec_step_uses_command() {
    let step = minimal_step();
    let sway_launch = convert(&step).expect("valid step should convert");
    assert!(matches!(
        sway_launch.target,
        sway_launch::Target::Exec {
            command: "foot",
            filter: None,
            title: None
        }
    ));
}

#[test]
fn to_sway_launch_con_id_step_targets_that_container() {
    let mut step = minimal_step();
    step.command = None;
    step.con_id = Some(42);
    let sway_launch = convert(&step).expect("valid step should convert");
    assert!(matches!(sway_launch.target, sway_launch::Target::ConId(42)));
}

#[test]
fn to_sway_launch_existing_step_requires_app_id_or_class() {
    let mut step = minimal_step();
    step.command = None;
    step.existing = true;
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_existing_step_with_app_id_succeeds() {
    let mut step = minimal_step();
    step.command = None;
    step.existing = true;
    step.app_id = Some("foot".to_string());
    let sway_launch = convert(&step).expect("existing step with app_id should convert");
    assert!(matches!(
        sway_launch.target,
        sway_launch::Target::Existing { wait: false, .. }
    ));
}

#[test]
fn to_sway_launch_step_without_a_target_errors() {
    let mut step = minimal_step();
    step.command = None;
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_app_id_and_class_together() {
    let mut step = minimal_step();
    step.app_id = Some("foot".to_string());
    step.class = Some("Foot".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_app_id_and_mark_match_together() {
    let mut step = minimal_step();
    step.app_id = Some("foot".to_string());
    step.mark_match = Some("dropdown-term".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_con_id_and_mark_match_together() {
    let mut step = minimal_step();
    step.command = None;
    step.con_id = Some(42);
    step.mark_match = Some("dropdown-term".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_mark_match_without_existing() {
    let mut step = minimal_step();
    step.mark_match = Some("dropdown-term".to_string());
    let error = convert(&step)
        .err()
        .expect("mark_match without existing should be rejected");
    assert!(error.contains("mark_match"));
}

#[test]
fn to_sway_launch_rejects_class_and_mark_match_together() {
    let mut step = minimal_step();
    step.class = Some("Foot".to_string());
    step.mark_match = Some("dropdown-term".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_existing_step_with_mark_match_succeeds() {
    let mut step = minimal_step();
    step.command = None;
    step.existing = true;
    step.mark_match = Some("dropdown-term".to_string());
    let sway_launch = convert(&step).expect("existing step with mark_match should convert");
    assert_eq!(
        format!("{:?}", sway_launch.target),
        "Existing { criteria: WindowCriteria { matcher: Some(Mark(\"dropdown-term\")), title: None }, \
         wait: false }"
    );
}

#[test]
fn to_sway_launch_rejects_empty_command() {
    let mut step = minimal_step();
    step.command = Some(String::new());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_whitespace_only_command() {
    let mut step = minimal_step();
    step.command = Some("   ".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_blank_id() {
    let mut step = minimal_step();
    step.id = Some("  ".to_string());
    let error = convert(&step).err().expect("blank id should be rejected");
    assert!(
        error.contains("id"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn to_sway_launch_rejects_blank_target_id() {
    let mut step = minimal_step();
    step.command = None;
    step.target_id = Some("   ".to_string());
    let error = convert(&step)
        .err()
        .expect("blank target_id should be rejected");
    assert!(
        error.contains("target_id"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn to_sway_launch_rejects_command_and_con_id_together() {
    // Regression test: a prior version silently preferred con_id over
    // command instead of erroring, unlike the CLI's conflicts_with.
    let mut step = minimal_step();
    step.con_id = Some(42);
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_con_id_and_app_id_together() {
    let mut step = minimal_step();
    step.command = None;
    step.con_id = Some(42);
    step.app_id = Some("foot".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_con_id_and_class_together() {
    let mut step = minimal_step();
    step.command = None;
    step.con_id = Some(42);
    step.class = Some("Foot".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_command_and_existing_together() {
    let mut step = minimal_step();
    step.existing = true;
    step.app_id = Some("foot".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_con_id_and_existing_together() {
    let mut step = minimal_step();
    step.command = None;
    step.con_id = Some(42);
    step.existing = true;
    step.app_id = Some("foot".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_command_and_target_id_together() {
    let mut step = minimal_step();
    step.target_id = Some("first".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_target_id_and_app_id_together() {
    // Regression test: target_id resolves to Target::ConId, so a match
    // criterion alongside it is discarded exactly as silently as one
    // alongside con_id -- which this step already rejected. Mirrors
    // to_sway_launch_rejects_con_id_and_app_id_together.
    let mut step = minimal_step();
    step.command = None;
    step.target_id = Some("first".to_string());
    step.app_id = Some("foot".to_string());
    let mut resolved_ids = HashMap::new();
    resolved_ids.insert("first".to_string(), 42);
    let error = convert_with(&step, &resolved_ids)
        .err()
        .expect("target_id combined with app_id should be rejected");
    assert!(
        error.contains("target_id"),
        "error should name the offending field: {error:?}"
    );
}

#[test]
fn to_sway_launch_rejects_target_id_and_class_together() {
    let mut step = minimal_step();
    step.command = None;
    step.target_id = Some("first".to_string());
    step.class = Some("Foot".to_string());
    let mut resolved_ids = HashMap::new();
    resolved_ids.insert("first".to_string(), 42);
    assert!(convert_with(&step, &resolved_ids).is_err());
}

#[test]
fn to_sway_launch_target_id_resolves_via_resolved_ids() {
    let mut step = minimal_step();
    step.command = None;
    step.target_id = Some("first".to_string());
    let mut resolved_ids = HashMap::new();
    resolved_ids.insert("first".to_string(), 42);
    let sway_launch = convert_with(&step, &resolved_ids)
        .expect("target_id present in resolved_ids should convert");
    assert!(matches!(sway_launch.target, sway_launch::Target::ConId(42)));
}

#[test]
fn to_sway_launch_rejects_unresolved_target_id() {
    let mut step = minimal_step();
    step.command = None;
    step.target_id = Some("missing".to_string());
    let error = convert(&step)
        .err()
        .expect("unresolved target_id should error");
    assert!(error.contains("missing"));
}

#[test]
fn to_sway_launch_rejects_a_mark_containing_a_double_quote() {
    // Regression test: confirmed live that Sway stores such a mark with
    // the escape character intact, so --mark/--mark-match could never
    // round-trip it. See validate_sway_string_argument's doc comment.
    let mut step = minimal_step();
    step.mark = Some("dropdown\"term".to_string());
    let error = convert(&step)
        .err()
        .expect("a mark containing a double quote should be rejected");
    assert!(
        error.contains("mark"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn to_sway_launch_rejects_a_blank_mark() {
    let mut step = minimal_step();
    step.mark = Some("   ".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_a_blank_app_id() {
    // A blank matcher isn't empty, so the target logic reads it as "set"
    // while every later use can only ever match nothing -- the same
    // silently-unsatisfiable input already rejected for mark/workspace/
    // output.
    let mut step = minimal_step();
    step.command = None;
    step.existing = true;
    step.app_id = Some("   ".to_string());
    let error = convert(&step)
        .err()
        .expect("a blank app_id should be rejected");
    assert!(
        error.contains("app_id"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn to_sway_launch_rejects_a_blank_class() {
    let mut step = minimal_step();
    step.command = None;
    step.existing = true;
    step.class = Some("".to_string());
    let error = convert(&step)
        .err()
        .expect("a blank class should be rejected");
    assert!(
        error.contains("class"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn to_sway_launch_rejects_a_blank_workspace() {
    let mut step = minimal_step();
    step.workspace = Some("".to_string());
    let error = convert(&step)
        .err()
        .expect("a blank workspace should be rejected");
    assert!(
        error.contains("workspace"),
        "error should name the field: {error:?}"
    );
}

#[test]
fn to_sway_launch_turns_workspace_number_into_a_numbered_target() {
    let mut step = minimal_step();
    step.workspace_number = Some(2);
    let launch = convert(&step).expect("a workspace number should convert");
    assert_eq!(
        launch.actions.workspace,
        Some(sway_launch::WorkspaceTarget::Number(2))
    );
}

#[test]
fn to_sway_launch_rejects_workspace_and_workspace_number_together() {
    let mut step = minimal_step();
    step.workspace = Some("web".to_string());
    step.workspace_number = Some(2);
    let error = convert(&step).err().expect("both should be rejected");
    assert!(error.contains("workspace_number"), "{error:?}");
}

#[test]
fn to_sway_launch_rejects_workspace_number_with_a_relative_move_or_out_of_range() {
    let mut step = minimal_step();
    step.workspace_number = Some(2);
    step.new_column = true;
    let error = convert(&step)
        .err()
        .expect("a relative move should be rejected");
    assert!(error.contains("new_column"), "{error:?}");

    let mut step = minimal_step();
    step.workspace_number = Some(-1);
    let error = convert(&step).err().expect("-1 should be rejected");
    assert!(error.starts_with("workspace_number: "), "{error:?}");
}

#[test]
fn to_sway_launch_rejects_an_output_containing_a_backslash() {
    let mut step = minimal_step();
    step.output = Some("HDMI\\A-1".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_accepts_a_mark_containing_command_separators() {
    // quote_sway_string() genuinely neutralizes these (confirmed live),
    // so the new validation must not over-reject them.
    let mut step = minimal_step();
    step.mark = Some("foo, exec bar; baz".to_string());
    assert!(convert(&step).is_ok());
}

#[test]
fn to_sway_launch_rejects_invalid_height() {
    let mut step = minimal_step();
    step.height = Some("notasize".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_invalid_width() {
    let mut step = minimal_step();
    step.width = Some("notasize".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_rejects_invalid_position() {
    let mut step = minimal_step();
    step.position = Some("notaposition".to_string());
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_uses_step_timeout_when_set() {
    let mut step = minimal_step();
    step.timeout = Some(7);
    let sway_launch = convert(&step).expect("valid step should convert");
    assert_eq!(sway_launch.timeout, time::Duration::from_secs(7));
}

#[test]
fn to_sway_launch_falls_back_to_default_timeout() {
    let step = minimal_step();
    let sway_launch = convert(&step).expect("valid step should convert");
    assert_eq!(sway_launch.timeout, time::Duration::from_secs(5));
}

#[test]
fn to_sway_launch_rejects_a_zero_timeout() {
    let mut step = minimal_step();
    step.timeout = Some(0);
    let error = convert(&step)
        .err()
        .expect("a zero timeout should be rejected");
    assert!(error.starts_with("timeout:"), "{error}");
}

#[test]
fn to_sway_launch_rejects_an_unrepresentable_timeout() {
    let mut step = minimal_step();
    step.timeout = Some(u64::MAX);
    assert!(convert(&step).is_err());
}

#[test]
fn to_sway_launch_accepts_an_explicit_timeout() {
    let mut step = minimal_step();
    step.timeout = Some(30);
    let launch = convert(&step).expect("a 30-second timeout should be accepted");
    assert_eq!(launch.timeout, time::Duration::from_secs(30));
}

#[test]
fn to_sway_launch_carries_settle_and_refuses_it_without_geometry() {
    let mut step = minimal_step();
    step.settle = Some(300);
    step.width = Some("700px".to_string());
    let sway_launch = convert(&step).expect("settle with a width is valid");
    assert_eq!(
        sway_launch.actions.settle,
        Some(time::Duration::from_millis(300))
    );

    step.width = Some("70ppt".to_string());
    assert_eq!(
        convert(&step).err(),
        Some("settle: needs a height or width in px, or a position, to hold".to_string())
    );
}
