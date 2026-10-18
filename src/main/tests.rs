use super::*;

#[test]
fn builtin_name_hint_names_the_builtin_and_the_flag_for_a_bare_builtin_filename() {
    assert_eq!(
        builtin_name_hint(Path::new("quad-grid.toml"), "--show-template"),
        "; there is a built-in template \"quad-grid\", use --show-template quad-grid"
    );
}

#[test]
fn builtin_name_hint_is_empty_for_a_path_with_a_directory_component() {
    assert_eq!(
        builtin_name_hint(Path::new("./quad-grid.toml"), "--template"),
        ""
    );
    assert_eq!(
        builtin_name_hint(Path::new("templates/quad-grid.toml"), "--template"),
        ""
    );
}

#[test]
fn builtin_name_hint_is_empty_when_no_builtin_matches_the_stem() {
    assert_eq!(
        builtin_name_hint(Path::new("not-a-real-template.toml"), "--template"),
        ""
    );
    assert_eq!(builtin_name_hint(Path::new(".toml"), "--template"), "");
}

#[test]
fn dry_run_plain_lines_number_continuously_and_escape_control_characters() {
    let steps = [
        DryRunStep {
            target: "launch foot\r\u{1b}[2K1. launch bar".to_string(),
            actions: vec![DryRunAction {
                kind: "split",
                command: "splith".to_string(),
                batched: true,
            }],
        },
        DryRunStep {
            target: "target existing container".to_string(),
            actions: vec![],
        },
    ];
    assert_eq!(
        dry_run_plain_lines(&[], &steps),
        [
            "1. launch foot\\r\\u{1b}[2K1. launch bar",
            "2. splith",
            "3. target existing container",
        ]
    );
    assert_eq!(
        dry_run_plain_lines(&["workspace \"3\"".to_string()], &steps[1..]),
        ["1. workspace \"3\"", "2. target existing container"]
    );
}

#[test]
fn dry_run_plain_lines_join_what_a_run_sends_as_one_command() {
    let action = |kind, command: &str, batched| DryRunAction {
        kind,
        command: command.to_string(),
        batched,
    };
    let steps = [DryRunStep {
        target: "target existing container".to_string(),
        actions: vec![
            action("workspace", "move workspace \"2\"", false),
            action("floating", "floating enable", true),
            action("mark", "mark \"m\"", true),
            action("scratchpad", "move scratchpad", false),
        ],
    }];
    assert_eq!(
        dry_run_plain_lines(&[], &steps),
        [
            "1. target existing container",
            "2. move workspace \"2\"",
            "3. floating enable, mark \"m\"",
            "4. move scratchpad",
        ]
    );
}

#[test]
fn severity_label_is_plain_text_without_color() {
    assert_eq!(severity_label(Severity::Error, false), "error: ");
    assert_eq!(severity_label(Severity::Warning, false), "warning: ");
}

#[test]
fn severity_label_colors_only_the_label_with_color() {
    assert_eq!(
        severity_label(Severity::Error, true),
        "\x1b[1;31merror:\x1b[0m "
    );
    assert_eq!(
        severity_label(Severity::Warning, true),
        "\x1b[1;33mwarning:\x1b[0m "
    );
}

/// Args flags with deliberately no `LayoutStep` equivalent: they select
/// or configure the invocation as a whole, so there's no per-step
/// meaning for them to have.
const CLI_ONLY_ARGS: &[&str] = &[
    "layout",
    "template",
    "list_templates",
    "list_windows",
    "show_template",
    "capture",
    "bindings",
    "apps",
    "param",
    "completions",
    "debug_events",
    "dry_run",
    "validate",
    "rollback_on_error",
    "continue_on_timeout",
    "on_workspace",
    "on_workspace_number",
    "on_output",
    "no_lock",
    "concurrent",
    "json",
    "verbose",
    // clap's own auto-generated arguments.
    "help",
    "version",
];

/// `LayoutStep` fields with deliberately no CLI equivalent: a single
/// invocation has only one step, so there's nothing to name or
/// cross-reference.
const LAYOUT_ONLY_FIELDS: &[&str] = &["id", "target_id"];

/// `LayoutStep` fields a `TemplateStep` deliberately lacks: a template
/// step names a `slot` and gets its target identity from a binding
/// instead of naming an application itself. Every entry but `id` must
/// be a real `template::Binding` field — `id` is the one `resolve()`
/// fills from the slot name rather than from the binding.
const BINDING_SUPPLIED_FIELDS: &[&str] = &[
    "command",
    "cwd",
    "env",
    "con_id",
    "existing",
    "wait",
    "app_id",
    "class",
    "mark_match",
    "title",
    "id",
];

/// `TemplateStep` fields with no `LayoutStep` equivalent.
const TEMPLATE_ONLY_FIELDS: &[&str] = &["slot"];

/// `BINDING_SUPPLIED_FIELDS` entries a `template::Binding` deliberately
/// doesn't carry: `resolve()` sets a slot step's `id` to the slot name.
const RESOLVE_SUPPLIED_FIELDS: &[&str] = &["id"];

/// `template::Binding` fields with no `BINDING_SUPPLIED_FIELDS` entry:
/// `slot` is the key a binding is looked up by, not a value it supplies.
const BINDING_ONLY_FIELDS: &[&str] = &["slot"];

/// `template::Binding` fields that override the template step's own
/// field of the same name (#30), rather than supplying a missing one.
const BINDING_OVERRIDE_FIELDS: &[&str] = &[
    "workspace",
    "workspace_number",
    "output",
    "timeout",
    "wait_time",
    "settle",
];

fn field_names<T: serde::Serialize>(value: &T) -> std::collections::BTreeSet<String> {
    serde_json::to_value(value)
        .expect("schema struct should serialize")
        .as_object()
        .expect("schema struct should serialize as an object")
        .keys()
        .cloned()
        .collect()
}

fn difference(
    left: &std::collections::BTreeSet<String>,
    right: &std::collections::BTreeSet<String>,
    allowed: &[&str],
) -> Vec<String> {
    left.difference(right)
        .filter(|name| !allowed.contains(&name.as_str()))
        .cloned()
        .collect()
}

#[test]
fn schemas_mirror_args_field_for_field() {
    // LayoutStep/TemplateStep mirror Args by design (see CLAUDE.md), and
    // there's no compiler check keeping the three in sync — this test is
    // what does. Adding a flag to
    // Args without the matching schema field is silent: every existing
    // test still passes, CI stays green, and --layout/--template simply
    // lack the capability until someone notices. The three allowlists
    // above are the intentional differences; a genuinely new one has to
    // be added there deliberately, which is the review moment this
    // otherwise relies on a human to remember.
    //
    // Both schema structs deserialize from empty TOML (every field is
    // either Option or #[serde(default)]), so serializing that gives the
    // field list without needing to construct one by hand.
    let layout_step: layout::LayoutStep =
        toml::from_str("").expect("LayoutStep should deserialize from an empty document");
    let template_step: template::TemplateStep =
        toml::from_str("").expect("TemplateStep should deserialize from an empty document");

    let layout_fields = field_names(&layout_step);
    let template_fields = field_names(&template_step);
    let args_fields: std::collections::BTreeSet<String> = Args::command()
        .get_arguments()
        .map(|argument| argument.get_id().to_string())
        .collect();

    assert_eq!(
        difference(&args_fields, &layout_fields, CLI_ONLY_ARGS),
        Vec::<String>::new(),
        "every Args flag needs a matching layout::LayoutStep field (or an entry in \
         CLI_ONLY_ARGS saying why it has none)"
    );
    assert_eq!(
        difference(&layout_fields, &args_fields, LAYOUT_ONLY_FIELDS),
        Vec::<String>::new(),
        "every layout::LayoutStep field needs a matching Args flag (or an entry in \
         LAYOUT_ONLY_FIELDS)"
    );
    assert_eq!(
        difference(&layout_fields, &template_fields, BINDING_SUPPLIED_FIELDS),
        Vec::<String>::new(),
        "every layout::LayoutStep field needs a matching template::TemplateStep field (or an \
         entry in BINDING_SUPPLIED_FIELDS)"
    );
    assert_eq!(
        difference(&template_fields, &layout_fields, TEMPLATE_ONLY_FIELDS),
        Vec::<String>::new(),
        "every template::TemplateStep field needs a matching layout::LayoutStep field (or an \
         entry in TEMPLATE_ONLY_FIELDS)"
    );

    // BINDING_SUPPLIED_FIELDS says where a template step's missing
    // target fields come from, but nothing above checks that Binding
    // actually has them. Without this, a new matcher added to Args and
    // LayoutStep passes the assertions above once it's listed there,
    // while --template still can't use it: a bindings file naming it is
    // rejected by deny_unknown_fields and resolve() never copies it.
    // `slot` is Binding's one required field.
    let binding: template::Binding =
        toml::from_str("slot = \"example\"").expect("Binding should deserialize from just a slot");
    let binding_fields = field_names(&binding);
    let binding_supplied_fields: std::collections::BTreeSet<String> = BINDING_SUPPLIED_FIELDS
        .iter()
        .map(|name| name.to_string())
        .collect();
    assert_eq!(
        difference(
            &binding_supplied_fields,
            &binding_fields,
            RESOLVE_SUPPLIED_FIELDS
        ),
        Vec::<String>::new(),
        "every BINDING_SUPPLIED_FIELDS entry needs a matching template::Binding field, \
         copied into the LayoutStep by template::resolve() (or an entry in \
         RESOLVE_SUPPLIED_FIELDS)"
    );
    assert_eq!(
        difference(
            &binding_fields,
            &binding_supplied_fields,
            &[BINDING_ONLY_FIELDS, BINDING_OVERRIDE_FIELDS].concat()
        ),
        Vec::<String>::new(),
        "every template::Binding field needs an entry in BINDING_SUPPLIED_FIELDS (or in \
         BINDING_ONLY_FIELDS/BINDING_OVERRIDE_FIELDS)"
    );
    for field in BINDING_OVERRIDE_FIELDS {
        assert!(
            binding_fields.contains(*field) && template_fields.contains(*field),
            "BINDING_OVERRIDE_FIELDS entry {field:?} must be a field of both \
             template::Binding and template::TemplateStep"
        );
    }
}

/// The standalone modes: each never acts on a single window, so each has
/// to reject every per-window flag rather than accept and discard it.
const STANDALONE_MODES: &[&str] = &[
    "debug_events",
    "completions",
    "layout",
    "template",
    "list_templates",
    "list_windows",
    "show_template",
    "capture",
];

/// Flags a `LayoutStep` mirrors that still aren't per-window: they're
/// invocation-wide defaults a step can override, and the standalone modes
/// accept them unused rather than rejecting them.
const MIRRORED_DEFAULTS: &[&str] = &["timeout", "wait_time"];

#[test]
fn per_window_args_lists_every_flag_a_layout_step_mirrors() {
    // A flag with a LayoutStep equivalent is per-window by definition
    // (CLI_ONLY_ARGS is the complement), bar MIRRORED_DEFAULTS, so a new
    // one missing from PER_WINDOW_ARGS would be silently accepted and
    // discarded by every standalone mode. This is what makes adding it
    // there unforgettable.
    let per_window: std::collections::BTreeSet<String> = PER_WINDOW_ARGS
        .iter()
        .map(|name| name.to_string())
        .collect();
    let expected: std::collections::BTreeSet<String> = Args::command()
        .get_arguments()
        .map(|argument| argument.get_id().to_string())
        .filter(|name| {
            !CLI_ONLY_ARGS.contains(&name.as_str()) && !MIRRORED_DEFAULTS.contains(&name.as_str())
        })
        .collect();
    assert_eq!(per_window, expected);
}

#[test]
fn per_window_args_conflict_with_every_standalone_mode() {
    let command = Args::command();
    for mode in STANDALONE_MODES {
        let mode_argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == *mode)
            .expect("every standalone mode should be an Args flag");
        let conflicts: Vec<String> = command
            .get_arg_conflicts_with(mode_argument)
            .iter()
            .map(|argument| argument.get_id().to_string())
            .collect();
        for per_window in PER_WINDOW_ARGS {
            let is_a_filter = *mode == "list_windows" && LIST_WINDOWS_FILTERS.contains(per_window);
            assert_eq!(
                conflicts.iter().any(|name| name == per_window),
                !is_a_filter,
                "--{mode} should conflict with {per_window} unless it narrows the listing"
            );
        }
    }
}

#[test]
fn run_args_conflict_with_every_standalone_mode_but_layout_and_template() {
    let command = Args::command();
    for mode in STANDALONE_MODES {
        if *mode == "layout" || *mode == "template" {
            continue;
        }
        let mode_argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == *mode)
            .expect("every standalone mode should be an Args flag");
        let conflicts: Vec<String> = command
            .get_arg_conflicts_with(mode_argument)
            .iter()
            .map(|argument| argument.get_id().to_string())
            .collect();
        for run_arg in RUN_ARGS {
            assert!(
                conflicts.iter().any(|name| name == run_arg),
                "--{mode} should conflict with {run_arg}, which only a run uses"
            );
        }
    }
}

/// `CLI_ONLY_ARGS` that are neither a standalone mode nor in `RUN_ARGS`:
/// they mean something to a direct-CLI run and to the standalone modes
/// alike, so no list of conflicts covers them all.
const GENERAL_ARGS: &[&str] = &["dry_run", "json", "verbose", "help", "version"];

/// The `RUN_ARGS` that `main()` checks require `--template`, on their own
/// rather than through `layout_or_template_only_flags()`.
const TEMPLATE_ONLY_ARGS: &[&str] = &["bindings", "apps", "param"];

#[test]
fn run_args_lists_every_flag_main_checks_requires_a_run() {
    let args = Args::parse_from(["sway-launch", "foot"]);
    let checked: std::collections::BTreeSet<&str> = layout_or_template_only_flags(&args)
        .iter()
        .map(|(name, _)| *name)
        .chain(TEMPLATE_ONLY_ARGS.iter().copied())
        .collect();
    let run_args: std::collections::BTreeSet<&str> = RUN_ARGS.iter().copied().collect();
    assert_eq!(checked, run_args);
}

#[test]
fn run_args_lists_every_cli_only_flag_that_is_not_a_mode_or_general() {
    // A new invocation-wide flag lands in CLI_ONLY_ARGS (the schema mirror
    // test forces that); this makes the author decide whether it's run-only,
    // and so rejected by every standalone mode, or general.
    let expected: std::collections::BTreeSet<&str> = CLI_ONLY_ARGS
        .iter()
        .copied()
        .filter(|name| !STANDALONE_MODES.contains(name) && !GENERAL_ARGS.contains(name))
        .collect();
    let run_args: std::collections::BTreeSet<&str> = RUN_ARGS.iter().copied().collect();
    assert_eq!(run_args, expected);
}

#[test]
fn layout_or_template_only_flags_reports_each_flag_by_its_args_id() {
    let flags_with_values: &[(&str, &[&str])] = &[
        ("rollback_on_error", &["--rollback-on-error"]),
        ("continue_on_timeout", &["--continue-on-timeout"]),
        ("on_workspace", &["--on-workspace", "example"]),
        ("on_workspace_number", &["--on-workspace-number", "2"]),
        ("on_output", &["--on-output", "HEADLESS-1"]),
        ("concurrent", &["--concurrent"]),
        ("no_lock", &["--no-lock"]),
        ("validate", &["--validate"]),
    ];
    for (id, flag) in flags_with_values {
        let argv = ["sway-launch"]
            .into_iter()
            .chain(flag.iter().copied())
            .chain(["foot"]);
        let args = Args::parse_from(argv);
        let set: Vec<&str> = layout_or_template_only_flags(&args)
            .iter()
            .filter(|(_, is_set)| *is_set)
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(set, [*id], "{flag:?}");
    }
}

fn listed_window(container_id: i64) -> sway_launch::ListedWindow {
    sway_launch::ListedWindow {
        container_id,
        app_id: None,
        class: None,
        title: None,
        marks: vec![],
        workspace: Some("1".to_string()),
        output: Some("HEADLESS-1".to_string()),
        scratchpad: false,
        floating: false,
        focused: false,
        split_container: false,
    }
}

#[test]
fn window_listing_table_aligns_columns_and_fills_gaps_with_a_dash() {
    let mut foot = listed_window(7);
    foot.app_id = Some("foot".to_string());
    foot.title = Some("shell".to_string());
    foot.focused = true;
    foot.floating = true;
    let mut code = listed_window(1234);
    code.class = Some("Code".to_string());
    code.marks = vec!["editor".to_string(), "main".to_string()];
    code.workspace = None;
    code.scratchpad = true;
    assert_eq!(
        window_listing_table(&[foot, code]),
        vec![
            "ID    APP_ID  CLASS  WORKSPACE     FLAGS             MARKS        TITLE",
            "7     foot    -      1             focused,floating  -            shell",
            "1234  -       Code   (scratchpad)  -                 editor,main  -",
        ]
    );
}

#[test]
fn window_listing_table_shortens_long_titles_and_escapes_control_characters() {
    let mut window = listed_window(1);
    window.title = Some(format!("a\tb{}", "x".repeat(60)));
    let table = window_listing_table(&[window]);
    let title = table[1].rsplit("  ").next().unwrap();
    assert!(title.starts_with("a\\tb"), "{title}");
    assert_eq!(title.chars().count(), LISTED_TITLE_LIMIT);
    assert!(title.ends_with('…'));
}

#[test]
fn window_listing_json_keeps_every_field_and_the_full_title() {
    let mut window = listed_window(5);
    window.title = Some("t".repeat(80));
    window.split_container = true;
    let json = window_listing_json(&[window]);
    assert_eq!(json["windows"][0]["container_id"], 5);
    assert_eq!(json["windows"][0]["app_id"], serde_json::Value::Null);
    assert_eq!(json["windows"][0]["title"].as_str().unwrap().len(), 80);
    assert_eq!(json["windows"][0]["output"], "HEADLESS-1");
    assert_eq!(json["windows"][0]["split_container"], true);
}

#[cfg(feature = "generate-man")]
#[test]
fn man_page_is_rendered_from_the_cli_definition() {
    let mut page = Vec::new();
    render_man_page(&mut page).expect("rendering the man page should succeed");
    let page = String::from_utf8(page).expect("the man page should be UTF-8");
    assert!(page.starts_with(".ie"), "unexpected roff start: {page:.80}");
    assert!(page.contains("sway\\-launch"), "{page:.200}");
    assert!(
        page.contains("debug\\-events"),
        "every flag should be documented"
    );
}

#[test]
fn readme_help_block_matches_the_rendered_help() {
    // README.md embeds the CLI's own help output verbatim, and nothing
    // kept the two in sync: it drifted silently once before, when giving
    // --debug-events a multi-paragraph doc comment switched clap into its
    // long-help layout and `--help` stopped matching the block entirely
    // (while `-h` still did, bar one line). Found only by diffing by hand.
    //
    // Compared against render_help() (the short form, what `-h` prints)
    // rather than render_long_help(), because that's the compact layout
    // the README carries deliberately -- a field's extra doc paragraphs
    // would crowd out the rest of the file.
    //
    // Read via CARGO_MANIFEST_DIR rather than a relative path so this
    // doesn't depend on the test harness's working directory.
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("README.md should be readable");

    let rendered = Args::command().render_help().to_string();
    let rendered = rendered.trim_end();

    assert!(
        readme.contains(rendered),
        "README.md's help block is out of sync with the CLI's own `-h` output. \
         Replace the block with:\n\n{rendered}\n"
    );
}

/// Every `` `name()` `` in CLAUDE.md's code spans, by its last path
/// segment (`Args::command().render_help()` gives `command` and
/// `render_help`). Fenced blocks are skipped.
fn function_names_in_code_spans(markdown: &str) -> Vec<String> {
    let mut in_fence = false;
    let prose: String = markdown
        .lines()
        .filter(|line| {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                return false;
            }
            !in_fence
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut names = Vec::new();
    for span in prose.split('`').skip(1).step_by(2) {
        for (index, _) in span.match_indices("()") {
            let before = &span[..index];
            let start = before
                .rfind(|character: char| !(character.is_alphanumeric() || character == '_'))
                .map_or(0, |position| position + 1);
            if start < index {
                names.push(span[start..index].to_string());
            }
        }
    }
    names
}

fn source_files(directory: &std::path::Path, files: &mut Vec<String>) {
    for entry in std::fs::read_dir(directory).expect("source directory should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        if path.is_dir() {
            source_files(&path, files);
        } else if let Ok(contents) = std::fs::read_to_string(&path) {
            files.push(contents);
        }
    }
}

#[test]
fn claude_md_names_only_functions_that_exist() {
    // CLAUDE.md points at functions for the reasoning behind each rule, so
    // a renamed or removed one leaves a rule pointing nowhere. Names from
    // outside this repository (std, clap, swayipc, Sway's own C source,
    // Python's tempfile) and the shared conventions' illustrative `foo()`
    // are listed here instead.
    const EXTERNAL: &[&str] = &[
        "argsep",
        "cmd_move_in_direction",
        "container_move_in_direction",
        "contents_utf8",
        "files",
        "foo",
        "generate",
        "get_file",
        "get_outputs",
        "get_tree",
        "render_help",
        "render_long_help",
        "TemporaryDirectory",
        "try_clone",
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let claude_md =
        std::fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md should be readable");
    let mut files = Vec::new();
    for directory in ["src", "tests", "scripts"] {
        source_files(&root.join(directory), &mut files);
    }

    let mut missing: Vec<String> = function_names_in_code_spans(&claude_md)
        .into_iter()
        .filter(|name| !EXTERNAL.contains(&name.as_str()))
        .filter(|name| {
            let definitions = [
                format!("fn {name}("),
                format!("fn {name}<"),
                format!("def {name}("),
                format!("{name}() {{"),
            ];
            !files.iter().any(|contents| {
                definitions
                    .iter()
                    .any(|definition| contents.contains(definition.as_str()))
            })
        })
        .collect();
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "CLAUDE.md names functions that don't exist in src/, tests/ or scripts/: {missing:?}"
    );
}

#[test]
fn integration_tests_run_the_binary_only_through_the_shared_helpers() {
    // A test that builds its own `Command` for the binary inherits the
    // developer's SWAYSOCK and XDG_RUNTIME_DIR, so only the code under test
    // keeps it off their real session (#137). tests/common/'s `sway_launch()`
    // isolates it; the live suite brings its own compositor.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(root.join("tests")).expect("tests/ should be readable") {
        let path = entry.expect("directory entry should be readable").path();
        let is_rust_file = path.extension().is_some_and(|extension| extension == "rs");
        if !is_rust_file || path.file_name().is_some_and(|name| name == "live_sway.rs") {
            continue;
        }
        let contents = std::fs::read_to_string(&path).expect("test file should be readable");
        if contents.contains("env!(\"CARGO_BIN_EXE_") {
            offenders.push(path.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "run the binary through tests/common/'s sway_launch(), not Command::new: {offenders:?}"
    );
}

/// Each run of consecutive `//`, `///` or `//!` lines in `source`, joined
/// into one string with the markers stripped, so a code span or a quoted
/// section name wrapped onto the next line reads as one.
fn comment_blocks(source: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in source.lines() {
        match line.trim_start().strip_prefix("//") {
            Some(comment) => current.push(comment.trim_start_matches(['/', '!']).trim_start()),
            None if !current.is_empty() => {
                blocks.push(current.join(" "));
                current.clear();
            }
            None => {}
        }
    }
    if !current.is_empty() {
        blocks.push(current.join(" "));
    }
    blocks
}

/// Every `.rs` file under `src/` and `tests/`, as (path, contents).
fn rust_sources() -> Vec<(String, String)> {
    fn walk(directory: &std::path::Path, files: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(directory).expect("source directory should be readable") {
            let path = entry.expect("directory entry should be readable").path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let contents =
                    std::fs::read_to_string(&path).expect("source file should be readable");
                let relative = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&path);
                files.push((relative.display().to_string(), contents));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for directory in ["src", "tests"] {
        walk(&root.join(directory), &mut files);
    }
    files
}

/// The CLAUDE.md sections a comment names: a quoted heading straight
/// after "CLAUDE.md's", or a one-word one followed by "section".
fn claude_md_section_references(comment: &str) -> Vec<String> {
    let mut references = Vec::new();
    for (index, _) in comment.match_indices("CLAUDE.md's ") {
        let rest = &comment[index + "CLAUDE.md's ".len()..];
        if let Some(quoted) = rest.strip_prefix('"') {
            if let Some(end) = quoted.find('"') {
                references.push(quoted[..end].to_string());
            }
        } else if let Some((word, after)) = rest.split_once(' ') {
            if after.starts_with("section") {
                references.push(word.to_string());
            }
        }
    }
    references
}

#[test]
fn source_comments_name_only_claude_md_sections_that_exist() {
    // A comment sending the reader to a CLAUDE.md section that was
    // renamed or moved to docs/design-notes.md points nowhere (#131).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let claude_md =
        std::fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md should be readable");
    let headings: Vec<&str> = claude_md
        .lines()
        .filter_map(|line| line.strip_prefix('#'))
        .map(|heading| heading.trim_start_matches('#').trim())
        .collect();
    let mut dangling = Vec::new();
    for (path, contents) in rust_sources() {
        for block in comment_blocks(&contents) {
            for section in claude_md_section_references(&block) {
                if !headings.contains(&section.as_str()) {
                    dangling.push(format!("{path}: {section:?}"));
                }
            }
        }
    }
    assert!(
        dangling.is_empty(),
        "comments name CLAUDE.md sections that don't exist: {dangling:?}"
    );
}

#[test]
fn claude_md_section_references_reads_quoted_and_one_word_sections() {
    let comment = "see CLAUDE.md's \"Core model\" section and CLAUDE.md's Tests section, \
                   not CLAUDE.md's coverage note";
    assert_eq!(
        claude_md_section_references(comment),
        ["Core model", "Tests"]
    );
}

#[test]
fn source_comments_name_only_functions_that_exist() {
    // The same check as claude_md_names_only_functions_that_exist, over
    // the comments in src/ and tests/: a doc comment pointing at a renamed
    // function sends the reader nowhere (#131).
    // Names from outside this repository (std, swayipc, Sway's and libc's
    // C source) and the illustrations in this file's own test comments.
    const EXTERNAL: &[&str] = &[
        "argsep",
        "atoi",
        "bind",
        "cmd_move_in_direction",
        "command",
        "container_move_in_direction",
        "create",
        "create_dir_all",
        "foo",
        "get_file",
        "get_outputs",
        "get_tree",
        "now",
        "recv_timeout",
        "render",
        "render_help",
        "subscribe",
        "success",
        "try_clone",
        "unwrap_or_default",
    ];
    let sources = rust_sources();
    let mut missing = Vec::new();
    for (path, contents) in &sources {
        for block in comment_blocks(contents) {
            for name in function_names_in_code_spans(&block) {
                if EXTERNAL.contains(&name.as_str()) {
                    continue;
                }
                let definitions = [format!("fn {name}("), format!("fn {name}<")];
                let defined = sources.iter().any(|(_, contents)| {
                    definitions
                        .iter()
                        .any(|definition| contents.contains(definition.as_str()))
                });
                if !defined {
                    missing.push(format!("{path}: {name}"));
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "comments name functions that don't exist in src/ or tests/: {missing:?}"
    );
}

#[test]
fn function_names_in_code_spans_takes_each_call_s_last_segment() {
    let markdown = "see `a::b()` and `x().y()`, not `str::parse::<i32>()` or\n\
                    ```shell\nfenced()\n```\n";
    assert_eq!(function_names_in_code_spans(markdown), ["b", "x", "y"]);
}

#[test]
fn long_help_carries_no_maintainer_notes() {
    // clap renders every `///` paragraph on an `Args` field into
    // `--help`, so a rationale note written for whoever edits `Args`
    // (naming a function, or a backticked identifier) ends up in front
    // of users. Such notes belong in `//` comments above the field.
    let rendered = Args::command().render_long_help().to_string();
    for line in rendered.lines() {
        assert!(
            !line.contains('`') && !line.contains("()"),
            "--help line reads like a maintainer note; move it to a `//` comment:\n{line}"
        );
    }
}

#[test]
fn args_accepts_valid_workspace_and_position() {
    let args = Args::try_parse_from([
        "sway-launch",
        "--workspace",
        "2",
        "--position",
        "center",
        "foot",
    ])
    .unwrap();
    assert_eq!(args.workspace, Some("2".to_string()));
    assert_eq!(args.position, Some(sway_launch::Position::Center));
}

#[test]
fn args_rejects_invalid_position() {
    let result = Args::try_parse_from(["sway-launch", "--position", "notvalid", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_app_id_alone() {
    let args = Args::try_parse_from(["sway-launch", "-a", "foot", "foot"]).unwrap();
    assert_eq!(args.app_id, Some("foot".to_string()));
    assert_eq!(args.class, None);
}

#[test]
fn args_accepts_class_alone() {
    let args = Args::try_parse_from(["sway-launch", "-c", "Foot", "foot"]).unwrap();
    assert_eq!(args.class, Some("Foot".to_string()));
    assert_eq!(args.app_id, None);
}

#[test]
fn args_rejects_app_id_and_class_together() {
    // Regression test: combining -a/-c used to silently ignore -c
    // instead of being rejected.
    let result = Args::try_parse_from(["sway-launch", "-a", "foot", "-c", "Foot", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_mark_match_alone() {
    let args = Args::try_parse_from(["sway-launch", "--existing", "--mark-match", "dropdown-term"])
        .unwrap();
    assert_eq!(args.mark_match, Some("dropdown-term".to_string()));
    assert_eq!(args.app_id, None);
    assert_eq!(args.class, None);
}

#[test]
fn args_rejects_app_id_and_mark_match_together() {
    let result = Args::try_parse_from([
        "sway-launch",
        "-a",
        "foot",
        "--mark-match",
        "dropdown-term",
        "foot",
    ]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_class_and_mark_match_together() {
    let result = Args::try_parse_from([
        "sway-launch",
        "-c",
        "Foot",
        "--mark-match",
        "dropdown-term",
        "foot",
    ]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_con_id_and_mark_match_together() {
    let result = Args::try_parse_from([
        "sway-launch",
        "--con-id",
        "42",
        "--mark-match",
        "dropdown-term",
    ]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_existing_with_mark_match() {
    let args = Args::try_parse_from(["sway-launch", "--existing", "--mark-match", "dropdown-term"])
        .unwrap();
    assert!(args.existing);
    assert_eq!(args.mark_match, Some("dropdown-term".to_string()));
}

#[test]
fn args_accepts_con_id_alone() {
    let args = Args::try_parse_from(["sway-launch", "--con-id", "42"]).unwrap();
    assert_eq!(args.con_id, Some(42));
    assert_eq!(args.command, None);
}

#[test]
fn args_rejects_con_id_and_command_together() {
    let result = Args::try_parse_from(["sway-launch", "--con-id", "42", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_con_id_and_app_id_together() {
    let result = Args::try_parse_from(["sway-launch", "--con-id", "42", "-a", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_con_id_and_class_together() {
    let result = Args::try_parse_from(["sway-launch", "--con-id", "42", "-c", "Foot"]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_existing_with_app_id() {
    let args = Args::try_parse_from(["sway-launch", "--existing", "-a", "foot"]).unwrap();
    assert!(args.existing);
    assert_eq!(args.app_id, Some("foot".to_string()));
}

#[test]
fn args_rejects_existing_and_command_together() {
    let result = Args::try_parse_from(["sway-launch", "--existing", "-a", "foot", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_existing_and_con_id_together() {
    let result = Args::try_parse_from(["sway-launch", "--existing", "--con-id", "42"]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_neither_app_id_nor_class() {
    let args = Args::try_parse_from(["sway-launch", "foot"]).unwrap();
    assert_eq!(args.app_id, None);
    assert_eq!(args.class, None);
}

#[test]
fn args_accepts_sticky_flag() {
    let args = Args::try_parse_from(["sway-launch", "--sticky", "foot"]).unwrap();
    assert!(args.sticky);
}

#[test]
fn args_rejects_sticky_and_layout_together() {
    let result = Args::try_parse_from(["sway-launch", "--sticky", "--layout", "x.toml"]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_dry_run_and_validate_together() {
    let result = Args::try_parse_from([
        "sway-launch",
        "--layout",
        "x.toml",
        "--dry-run",
        "--validate",
    ]);
    assert!(result.is_err());
}

#[test]
fn args_new_row_short_flag_is_r() {
    let args = Args::try_parse_from(["sway-launch", "-r", "foot"]).unwrap();
    assert!(args.new_row);
}

#[test]
fn args_rejects_a_mark_containing_a_double_quote() {
    // Regression test: Sway stores the escape character literally rather
    // than unescaping it (confirmed live), so such a mark could never be
    // matched again by --mark-match. See
    // sway_launch::validate_sway_string_argument's doc comment.
    let result = Args::try_parse_from(["sway-launch", "--mark", "dropdown\"term", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_a_mark_match_containing_a_double_quote() {
    let result = Args::try_parse_from([
        "sway-launch",
        "--existing",
        "--mark-match",
        "dropdown\"term",
    ]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_a_blank_app_id() {
    // Regression test: an empty --app-id was indistinguishable from an
    // absent one after unwrap_or_default(), so `--existing --app-id ''`
    // reported "--existing requires --app-id, --class, or --mark-match"
    // at a caller who had just passed --app-id. Rejected at parse time
    // now, naming the flag.
    assert!(Args::try_parse_from(["sway-launch", "--existing", "--app-id", ""]).is_err());
    assert!(Args::try_parse_from(["sway-launch", "--existing", "-a", "   "]).is_err());
}

#[test]
fn args_rejects_a_blank_class() {
    assert!(Args::try_parse_from(["sway-launch", "--existing", "-c", "  "]).is_err());
}

#[test]
fn args_rejects_a_blank_workspace() {
    let result = Args::try_parse_from(["sway-launch", "--workspace", "  ", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_rejects_an_output_containing_a_backslash() {
    let result = Args::try_parse_from(["sway-launch", "--output", "HDMI\\A-1", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_a_mark_containing_command_separators() {
    // quote_sway_string() neutralizes these, so validation must not
    // over-reject them.
    let args =
        Args::try_parse_from(["sway-launch", "--mark", "foo, exec bar; baz", "foot"]).unwrap();
    assert_eq!(args.mark, Some("foo, exec bar; baz".to_string()));
}

#[test]
fn args_rejects_invalid_height() {
    let result = Args::try_parse_from(["sway-launch", "--height", "notasize", "foot"]);
    assert!(result.is_err());
}

#[test]
fn args_accepts_valid_height_and_width() {
    let args = Args::try_parse_from([
        "sway-launch",
        "--height",
        "80ppt",
        "--width",
        "1200px",
        "foot",
    ])
    .unwrap();
    assert_eq!(args.height, Some(sway_launch::Size::Percent(80)));
    assert_eq!(args.width, Some(sway_launch::Size::Pixels(1200)));
}

#[test]
fn args_defaults_timeout_and_wait_time() {
    let args = Args::try_parse_from(["sway-launch", "foot"]).unwrap();
    assert_eq!(args.timeout, 5);
    assert_eq!(args.wait_time, 20);
}

#[test]
fn args_json_defaults_to_false() {
    let args = Args::try_parse_from(["sway-launch", "foot"]).unwrap();
    assert!(!args.json);
}

#[test]
fn args_accepts_json_flag() {
    let args = Args::try_parse_from(["sway-launch", "--json", "foot"]).unwrap();
    assert!(args.json);
}

#[test]
fn actions_json_renders_changed_already_satisfied_and_skipped() {
    let actions = [
        sway_launch::ActionRecord {
            kind: "floating",
            command: Some("floating enable".to_string()),
            status: sway_launch::ActionStatus::Changed,
        },
        sway_launch::ActionRecord {
            kind: "focus",
            command: Some("focus".to_string()),
            status: sway_launch::ActionStatus::AlreadySatisfied,
        },
        sway_launch::ActionRecord {
            kind: "width",
            command: Some("resize set width 400px".to_string()),
            status: sway_launch::ActionStatus::Unconfirmed,
        },
        sway_launch::ActionRecord {
            kind: "new_column",
            command: None,
            status: sway_launch::ActionStatus::Skipped {
                reason: "trailing_workspace_edge",
            },
        },
    ];
    assert_eq!(
        actions_json(&actions),
        serde_json::json!([
            { "kind": "floating", "command": "floating enable", "status": "changed" },
            { "kind": "focus", "command": "focus", "status": "already_satisfied" },
            { "kind": "width", "command": "resize set width 400px", "status": "unconfirmed" },
            { "kind": "new_column", "status": "skipped", "reason": "trailing_workspace_edge" },
        ])
    );
}

#[test]
fn step_action_records_json_tags_each_entry_with_a_1_based_step_number() {
    // Regression test for the exact off-by-one that would be invisible
    // against a real skip on a --layout's first step: step_index is
    // 0-based (an iterator position), but the reported "step" must be
    // 1-based, matching every other per-step error message in this
    // file's own "step {}: ..." convention. Uses a synthetic
    // ActionRecord rather than a real live skip (only reachable via a
    // multi-output Sway session) specifically so this arithmetic is
    // verifiable headlessly.
    let actions = vec![sway_launch::ActionRecord {
        kind: "new_row",
        command: None,
        status: sway_launch::ActionStatus::Skipped {
            reason: "trailing_workspace_edge",
        },
    }];
    assert_eq!(
        step_action_records_json(1, actions),
        vec![serde_json::json!({
            "step": 2,
            "kind": "new_row",
            "status": "skipped",
            "reason": "trailing_workspace_edge",
        })]
    );
}

#[test]
fn step_action_records_json_is_empty_for_no_actions() {
    assert!(step_action_records_json(0, Vec::new()).is_empty());
}
