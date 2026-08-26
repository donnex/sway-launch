use clap::{error::ErrorKind, CommandFactory, Parser};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{io, process, time};

mod layout;
mod sway_launch;
mod template;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// app_id match. With --existing, matches an already-open window instead
    /// of the newly launched one
    #[clap(short, long, conflicts_with_all = ["class", "con_id", "mark_match"])]
    app_id: Option<String>,

    /// class match. With --existing, matches an already-open window instead
    /// of the newly launched one
    #[clap(short, long, conflicts_with_all = ["con_id", "mark_match"])]
    class: Option<String>,

    /// Mark match. With --existing, matches an already-open window carrying
    /// this mark instead of the newly launched one
    #[clap(long, conflicts_with_all = ["class", "con_id"])]
    mark_match: Option<String>,

    /// Act on an already-open window with this container id, instead of
    /// launching a new one
    #[clap(long, conflicts_with = "command")]
    con_id: Option<i64>,

    /// Act on an already-open window found via --app-id/--class/--mark-match,
    /// instead of launching a new one
    #[clap(long, conflicts_with_all = ["command", "con_id"])]
    existing: bool,

    /// Change split for new window
    #[clap(value_enum, short, long)]
    split: Option<sway_launch::Split>,

    /// Make new window floating
    #[clap(short, long)]
    floating: bool,

    /// Make new window sticky (shows on all workspaces)
    #[clap(long)]
    sticky: bool,

    /// Make new window fullscreen
    #[clap(long)]
    fullscreen: bool,

    /// Focus new window
    #[clap(long)]
    focus: bool,

    /// Add mark to new window
    #[clap(short, long)]
    mark: Option<String>,

    /// Move window to new column (move right)
    #[clap(short, long)]
    new_column: bool,

    /// Set height on new window
    #[clap(long, value_parser = sway_launch::validate_size_argument)]
    height: Option<String>,

    /// Set width on new window
    #[clap(long, value_parser = sway_launch::validate_size_argument)]
    width: Option<String>,

    /// Move window to new row (move down)
    #[clap(long, short = 'r')]
    new_row: bool,

    /// Move new window to workspace
    #[clap(long)]
    workspace: Option<String>,

    /// Move new window to output (monitor)
    #[clap(long)]
    output: Option<String>,

    /// Set position on new window. Either "center" or "<x>,<y>" in pixels (x/y may be negative)
    #[clap(long, value_parser = sway_launch::validate_position_argument, allow_hyphen_values = true)]
    position: Option<String>,

    /// Move window to the scratchpad
    #[clap(long)]
    scratchpad: bool,

    /// Print the planned sequence of Sway commands instead of running them —
    /// works with a direct command or --layout/--template. Never touches
    /// Sway IPC or launches anything
    #[clap(long, conflicts_with = "debug_events")]
    dry_run: bool,

    /// Validate a --layout/--template file (and, for --template,
    /// --bindings/--apps resolution) without launching anything or touching
    /// Sway IPC. Requires --layout or --template
    #[clap(long, conflicts_with = "debug_events")]
    validate: bool,

    /// Timeout in seconds
    #[clap(short, long, default_value_t = 5)]
    timeout: u64,

    /// Wait time in ms. Used for actions that do not have a corresponding Sway IPC event.
    #[clap(short, long, default_value_t = 20)]
    wait_time: u64,

    /// Debug events. Output all Sway IPC events until stopped.
    #[clap(short, long)]
    debug_events: bool,

    /// Generate a shell completion script and print it to stdout
    #[clap(long, value_enum, conflicts_with_all = [
        "command", "con_id", "existing", "app_id", "class", "mark_match", "split",
        "floating", "sticky", "fullscreen", "focus", "mark", "new_column", "new_row",
        "workspace", "output", "height", "width", "position", "scratchpad", "debug_events",
        "layout", "template", "list_templates", "bindings", "apps", "rollback_on_error", "dry_run", "validate",
        "show_template",
    ])]
    completions: Option<clap_complete::Shell>,

    /// Verbose output
    #[clap(short, long)]
    verbose: bool,

    /// Print the result as a JSON object instead of a bare container id
    #[clap(long)]
    json: bool,

    /// Run a declarative TOML layout file instead of a single command; see
    /// README.md for the schema. Each step is the equivalent of one
    /// sway-launch invocation's flags, so this conflicts with every
    /// per-window flag below, which would otherwise apply to no specific
    /// step
    #[clap(long, conflicts_with_all = [
        "command", "con_id", "existing", "app_id", "class", "mark_match", "split",
        "floating", "sticky", "fullscreen", "focus", "mark", "new_column", "new_row",
        "workspace", "output", "height", "width", "position", "scratchpad", "debug_events",
        "bindings", "apps",
    ])]
    layout: Option<PathBuf>,

    /// Run a reusable declarative TOML layout template instead of a single
    /// command; see README.md for the schema. Steps declare a `slot` instead
    /// of an application, resolved via --bindings or --apps. Either a path
    /// to a template file ending in .toml, or a built-in template name with
    /// no extension (see --list-templates). Conflicts with --layout and
    /// every per-window flag, same reasoning as --layout
    #[clap(long, conflicts_with_all = [
        "command", "con_id", "existing", "app_id", "class", "mark_match", "split",
        "floating", "sticky", "fullscreen", "focus", "mark", "new_column", "new_row",
        "workspace", "output", "height", "width", "position", "scratchpad", "debug_events",
        "layout",
    ])]
    template: Option<PathBuf>,

    /// List built-in --template names and exit
    #[clap(long, conflicts_with_all = [
        "command", "con_id", "existing", "app_id", "class", "mark_match", "split",
        "floating", "sticky", "fullscreen", "focus", "mark", "new_column", "new_row",
        "workspace", "output", "height", "width", "position", "scratchpad", "debug_events",
        "layout", "template", "completions", "bindings", "apps", "rollback_on_error", "dry_run", "validate",
        "show_template",
    ])]
    list_templates: bool,

    /// Print a --template's raw TOML and exit, without running it. Same
    /// NAME_OR_PATH resolution as --template: a built-in name (see
    /// --list-templates), or a path ending in .toml. With --json, prints
    /// {"name": ..., "contents": "..."}
    #[clap(long, value_name = "NAME_OR_PATH", conflicts_with_all = [
        "command", "con_id", "existing", "app_id", "class", "mark_match", "split",
        "floating", "sticky", "fullscreen", "focus", "mark", "new_column", "new_row",
        "workspace", "output", "height", "width", "position", "scratchpad", "debug_events",
        "layout", "template", "completions", "bindings", "apps", "rollback_on_error", "dry_run", "validate",
    ])]
    show_template: Option<PathBuf>,

    /// Bindings file supplying each --template slot's application identity.
    /// Requires --template; conflicts with --apps
    #[clap(long, conflicts_with = "apps")]
    bindings: Option<PathBuf>,

    /// Comma-separated list of commands to launch into --template's slots,
    /// in the order they first appear in the template. Requires --template;
    /// conflicts with --bindings
    #[clap(long)]
    apps: Option<String>,

    /// If a later --layout/--template step fails, kill every window this
    /// invocation itself launched by an earlier, already-completed step
    /// (not one it merely retargeted via con_id/existing/target_id), rather
    /// than leaving them open. Requires --layout or --template
    #[clap(long)]
    rollback_on_error: bool,

    /// Command to execute
    command: Option<String>,
}

fn main() {
    let args = Args::parse();

    if let Some(shell) = args.completions {
        clap_complete::generate(
            shell,
            &mut Args::command(),
            "sway-launch",
            &mut io::stdout(),
        );
        process::exit(0);
    }

    if args.list_templates {
        print_builtin_templates(args.json);
        process::exit(0);
    }

    if let Some(show_template_arg) = &args.show_template {
        print_template_contents(show_template_arg, args.json);
        process::exit(0);
    }

    if let Some(layout_path) = &args.layout {
        run_layout(layout_path, &args);
    }

    if args.template.is_none() && (args.bindings.is_some() || args.apps.is_some()) {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--bindings/--apps require --template",
            )
            .exit();
    }

    if let Some(template_path) = &args.template {
        let bindings_source = match (&args.bindings, &args.apps) {
            (Some(bindings_path), None) => BindingsSource::File(bindings_path),
            (None, Some(apps)) => BindingsSource::Apps(apps),
            _ => Args::command()
                .error(
                    ErrorKind::MissingRequiredArgument,
                    "--template requires one of --bindings/--apps",
                )
                .exit(),
        };
        run_template(template_path, bindings_source, &args);
    }

    if args.rollback_on_error && args.layout.is_none() && args.template.is_none() {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--rollback-on-error requires --layout or --template",
            )
            .exit();
    }

    if args.validate && args.layout.is_none() && args.template.is_none() {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--validate requires --layout or --template",
            )
            .exit();
    }

    let command = args.command.unwrap_or_default();
    let app_id_match = args.app_id.unwrap_or_default();
    let class_match = args.class.unwrap_or_default();
    let mark_match = args.mark_match.unwrap_or_default();

    if !args.debug_events && command.is_empty() && args.con_id.is_none() && !args.existing {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "Missing COMMAND, or one of --con-id/--existing",
            )
            .exit();
    }

    if args.existing && app_id_match.is_empty() && class_match.is_empty() && mark_match.is_empty() {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--existing requires --app-id, --class, or --mark-match",
            )
            .exit();
    }

    if !mark_match.is_empty() && !args.existing {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--mark-match requires --existing",
            )
            .exit();
    }

    let target = if let Some(con_id) = args.con_id {
        sway_launch::Target::ConId(con_id)
    } else if args.existing {
        sway_launch::Target::Existing
    } else {
        sway_launch::Target::Exec { command: &command }
    };

    let sway_launch = sway_launch::SwayLaunch {
        target,
        app_id_match: &app_id_match,
        class_match: &class_match,
        mark_match: &mark_match,
        split: args.split,
        floating: args.floating,
        sticky: args.sticky,
        fullscreen: args.fullscreen,
        focus: args.focus,
        mark: &args.mark.unwrap_or_default(),
        new_column: args.new_column,
        new_row: args.new_row,
        workspace: args.workspace.as_deref(),
        output: args.output.as_deref(),
        height: args.height.as_deref().map(sway_launch::parse_size),
        width: args.width.as_deref().map(sway_launch::parse_size),
        position: args.position.as_deref().map(sway_launch::parse_position),
        scratchpad: args.scratchpad,
        timeout: time::Duration::from_secs(args.timeout),
        wait_time: time::Duration::from_millis(args.wait_time),
        verbose: args.verbose,
    };

    if args.dry_run {
        print_dry_run_steps(&[dry_run_step(&sway_launch)], args.json);
        process::exit(0);
    }

    if args.debug_events {
        match sway_launch.debug_events() {
            Ok(_) => process::exit(0),
            Err(error) => fail(args.json, &error),
        }
    }

    match sway_launch.run() {
        Ok(outcome) => {
            if args.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "container_id": outcome.container_id,
                        "actions": actions_json(&outcome.actions),
                    })
                );
            } else {
                println!("{}", outcome.container_id);
            }
        }
        Err(error) => fail(args.json, &error),
    };
}

/// Renders one `ActionRecord` as `--json`'s per-action object:
/// `{"action": ..., "status": "changed"|"already_satisfied"|"skipped"}`,
/// with a `"reason"` field added only for `"skipped"` (the same
/// machine-readable `SkippedAction.reason` identifier this project has
/// always used, not prose).
fn action_record_json(record: &sway_launch::ActionRecord) -> serde_json::Value {
    match record.status {
        sway_launch::ActionStatus::Changed => serde_json::json!({
            "action": record.action,
            "status": "changed",
        }),
        sway_launch::ActionStatus::AlreadySatisfied => serde_json::json!({
            "action": record.action,
            "status": "already_satisfied",
        }),
        sway_launch::ActionStatus::Skipped { reason } => serde_json::json!({
            "action": record.action,
            "status": "skipped",
            "reason": reason,
        }),
    }
}

/// Renders a single invocation's `RunOutcome.actions` as `--json`'s
/// `"actions"` array. `run_steps()` (`--layout`/`--template`) doesn't reuse
/// this directly — it needs each entry tagged with a 1-based step number
/// too (there can be several steps, not just one invocation) — see
/// `step_action_records_json()` below.
fn actions_json(actions: &[sway_launch::ActionRecord]) -> serde_json::Value {
    serde_json::json!(actions.iter().map(action_record_json).collect::<Vec<_>>())
}

/// `run_steps()`'s per-step counterpart to `actions_json()`: the same
/// per-action object shape, plus a `"step"` field (the step's 1-based
/// number, matching the `"step {}: ..."` convention every other per-step
/// error message in this file already uses) so an action is traceable to
/// which step produced it across a multi-step `--layout`/`--template` run.
/// A small pure function specifically so the step-number arithmetic
/// (`index + 1`) is unit-testable headlessly with a synthetic
/// `ActionRecord`, without needing a live Sway session just to verify it.
fn step_action_records_json(
    step_index: usize,
    actions: Vec<sway_launch::ActionRecord>,
) -> Vec<serde_json::Value> {
    actions
        .iter()
        .map(|record| {
            let mut value = action_record_json(record);
            value["step"] = serde_json::json!(step_index + 1);
            value
        })
        .collect()
}

/// Reports a runtime failure — file I/O, TOML parsing, or the
/// `SwayLaunch`/step operation itself failing against Sway — to stderr and
/// exits 1. When `--json` is set, the message is wrapped in a
/// `{"error": "..."}` object instead of printed as plain text, mirroring the
/// structured `{"container_id": ...}`/`{"container_ids": [...]}` success
/// shapes below rather than leaving `--json` callers to also parse plain-text
/// stderr on failure. Deliberately scoped to these runtime failures only — a
/// bad CLI invocation (missing/conflicting flags, caught via clap's `.error()`
/// calls above) is still reported via clap's own usage-error formatting
/// regardless of `--json`, the same as `--help` itself isn't JSON.
fn fail(json: bool, message: &str) -> ! {
    fail_with_rollback(json, message, &[]);
}

/// `fail()`, plus the container ids `run_steps()`'s `--rollback-on-error`
/// killed before giving up — `rolled_back` is always empty outside that path.
fn fail_with_rollback(json: bool, message: &str, rolled_back: &[i64]) -> ! {
    if json {
        eprintln!(
            "{}",
            serde_json::json!({ "error": message, "rolled_back": rolled_back })
        );
    } else {
        eprintln!("{}", message);
        if !rolled_back.is_empty() {
            let ids = rolled_back
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("rolled back {} container(s): {}", rolled_back.len(), ids);
        }
    }
    process::exit(1);
}

/// One step's `--dry-run` preview: its target description plus every
/// planned action's verb, in order. Shared representation for both the
/// direct-CLI path and `--layout`/`--template`'s `run_steps_dry_run()`, so
/// both print identically and share one JSON shape.
struct DryRunStep {
    target: String,
    actions: Vec<String>,
}

/// A one-line description of what a `SwayLaunch`'s `target` would do,
/// deliberately never naming a container id — for `Target::Exec`/
/// `Target::Existing`, there isn't a real one yet (nothing has launched);
/// for `Target::ConId`, the id is already known, but showing it here while
/// every action line below stays id-free (see
/// `SwayAction::sway_command_verb()`) would be an inconsistent preview
/// format for one target mode only.
fn describe_target(
    target: &sway_launch::Target,
    app_id_match: &str,
    class_match: &str,
    mark_match: &str,
) -> String {
    match target {
        sway_launch::Target::Exec { command } => format!("launch {}", command),
        sway_launch::Target::ConId(_) => "target existing container".to_string(),
        sway_launch::Target::Existing if !app_id_match.is_empty() => {
            format!("target existing window (app_id={:?})", app_id_match)
        }
        sway_launch::Target::Existing if !class_match.is_empty() => {
            format!("target existing window (class={:?})", class_match)
        }
        sway_launch::Target::Existing => {
            format!("target existing window (mark_match={:?})", mark_match)
        }
    }
}

fn dry_run_step(sway_launch: &sway_launch::SwayLaunch) -> DryRunStep {
    DryRunStep {
        target: describe_target(
            &sway_launch.target,
            sway_launch.app_id_match,
            sway_launch.class_match,
            sway_launch.mark_match,
        ),
        actions: sway_launch
            .build_actions_for_preview()
            .iter()
            .map(sway_launch::SwayAction::sway_command_verb)
            .collect(),
    }
}

/// Prints every `DryRunStep` as a continuously-numbered plain-text list
/// (target line included as one of the numbered items), or as a single
/// `{"steps": [{"target": ..., "actions": [...]}, ...]}` object under
/// `--json`.
fn print_dry_run_steps(steps: &[DryRunStep], json: bool) {
    if json {
        let steps: Vec<_> = steps
            .iter()
            .map(|step| serde_json::json!({ "target": step.target, "actions": step.actions }))
            .collect();
        println!("{}", serde_json::json!({ "steps": steps }));
        return;
    }

    let mut n = 1;
    for step in steps {
        println!("{}. {}", n, step.target);
        n += 1;
        for action in &step.actions {
            println!("{}. {}", n, action);
            n += 1;
        }
    }
}

/// Reads and parses a `--layout` file, then hands its steps to `run_steps()`.
fn run_layout(path: &Path, args: &Args) -> ! {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => fail(args.json, &format!("{}: {}", path.display(), error)),
    };

    let parsed_layout = match layout::parse(&contents) {
        Ok(parsed_layout) => parsed_layout,
        Err(error) => fail(args.json, &format!("{}: {}", path.display(), error)),
    };

    run_steps(&parsed_layout.step, args);
}

/// Where `run_template()` gets each `--template` slot's application
/// identity from: a `--bindings` file, or the `--apps` shorthand's raw
/// comma-separated value.
enum BindingsSource<'a> {
    File(&'a Path),
    Apps(&'a str),
}

/// Resolves `--template`'s argument to raw TOML contents. A value ending in
/// `.toml` is read as an external file, exactly as before this existed;
/// anything else is looked up as a built-in template name via
/// `template::builtin()` — matching every shipped template file's own
/// `.toml` extension means there's no ambiguity between the two, and
/// nothing on disk to stat to decide which one a given value means.
fn resolve_template_contents(template_arg: &Path) -> Result<String, String> {
    // template_arg.extension() is checked via a direct suffix match on the
    // full string, not Path::extension() — extension() returns None for a
    // filename that's only an extension with nothing before it (e.g. a file
    // literally named ".toml"), which would otherwise be misclassified as a
    // built-in-name lookup instead of a file read.
    let is_file = template_arg
        .to_str()
        .is_some_and(|value| value.ends_with(".toml"));

    if is_file {
        std::fs::read_to_string(template_arg)
            .map_err(|error| format!("{}: {}", template_arg.display(), error))
    } else {
        let name = template_arg.to_string_lossy();
        template::builtin(&name).map(str::to_string).ok_or_else(|| {
            format!(
                "no built-in template named {:?} (run --list-templates to see available \
                 names, or pass a path ending in .toml to use a template file)",
                name
            )
        })
    }
}

/// The `--list-templates` standalone mode: prints every built-in
/// `--template` name and its one-line description, or (with `--json`) the
/// same as a `{"templates": [...]}` array. Doesn't touch Sway IPC, so it's
/// checked and short-circuits right after `--completions`, same as
/// `--layout`/`--template`.
fn print_builtin_templates(json: bool) {
    let templates = template::builtin_templates();

    if json {
        let templates: Vec<_> = templates
            .iter()
            .map(|template| {
                serde_json::json!({
                    "name": template.name,
                    "category": template.category,
                    "description": template.description,
                    "slots": template.slots,
                    "slot_names": template.slot_names,
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "templates": templates }));
        return;
    }

    let name_width = templates
        .iter()
        .map(|template| template.name.len())
        .max()
        .unwrap_or(0);
    let category_width = templates
        .iter()
        .map(|template| template.category.len())
        .max()
        .unwrap_or(0);
    for template in &templates {
        println!(
            "{:<name_width$}  {:<category_width$}  {} ({} slot{}: {})",
            template.name,
            template.category,
            template.description,
            template.slots,
            if template.slots == 1 { "" } else { "s" },
            template.slot_names.join(", ")
        );
    }
}

/// The `--show-template` standalone mode: resolves its argument the exact
/// same way `--template` itself does (via `resolve_template_contents()`),
/// then prints the raw TOML instead of running it. Doesn't touch Sway IPC,
/// so it's checked and short-circuits right after `--list-templates`, same
/// as `--layout`/`--template`.
fn print_template_contents(template_arg: &Path, json: bool) {
    let contents = match resolve_template_contents(template_arg) {
        Ok(contents) => contents,
        Err(error) => fail(json, &error),
    };

    if json {
        println!(
            "{}",
            serde_json::json!({ "name": template_arg.to_string_lossy(), "contents": contents })
        );
        return;
    }

    print!("{contents}");
}

/// Reads and parses a `--template` file, resolves it against `--bindings`/
/// `--apps` into ordinary layout steps via `template::resolve()`, then hands
/// them to `run_steps()` — the same execution path `--layout` uses, since a
/// resolved template is just a `Vec<layout::LayoutStep>`.
fn run_template(path: &Path, bindings_source: BindingsSource, args: &Args) -> ! {
    let contents = match resolve_template_contents(path) {
        Ok(contents) => contents,
        Err(error) => fail(args.json, &error),
    };

    let parsed_template = match template::parse(&contents) {
        Ok(parsed_template) => parsed_template,
        Err(error) => fail(args.json, &format!("{}: {}", path.display(), error)),
    };

    let bindings = match bindings_source {
        BindingsSource::File(bindings_path) => {
            let contents = match std::fs::read_to_string(bindings_path) {
                Ok(contents) => contents,
                Err(error) => fail(
                    args.json,
                    &format!("{}: {}", bindings_path.display(), error),
                ),
            };
            match template::parse_bindings(&contents) {
                Ok(bindings) => bindings,
                Err(error) => fail(
                    args.json,
                    &format!("{}: {}", bindings_path.display(), error),
                ),
            }
        }
        BindingsSource::Apps(apps) => match bindings_from_apps(&parsed_template, apps) {
            Ok(bindings) => bindings,
            Err(error) => fail(args.json, &format!("{}: {}", path.display(), error)),
        },
    };

    let resolved_steps = match template::resolve(&parsed_template, &bindings) {
        Ok(resolved_steps) => resolved_steps,
        Err(error) => fail(args.json, &format!("{}: {}", path.display(), error)),
    };

    run_steps(&resolved_steps, args);
}

/// Builds `Bindings` from `--apps`' comma-separated command list, mapping
/// them 1:1 onto the template's distinct `slot` names in first-appearance
/// order. Each binding launches its command unfiltered (no `app_id`/`class`
/// match), the same as a plain `sway-launch <command>` with no `-a`/`-c`.
fn bindings_from_apps(
    parsed_template: &template::Template,
    apps: &str,
) -> Result<template::Bindings, String> {
    let slots = template::distinct_slot_names(parsed_template);

    let apps: Vec<&str> = apps.split(',').map(str::trim).collect();
    if apps.len() != slots.len() {
        return Err(format!(
            "template needs {} application(s) ({}), got {}",
            slots.len(),
            slots.join(", "),
            apps.len()
        ));
    }

    if let Some(empty_slot) = slots
        .iter()
        .zip(&apps)
        .find(|(_, app)| app.is_empty())
        .map(|(slot, _)| slot)
    {
        return Err(format!(
            "--apps: empty application for slot {:?}",
            empty_slot
        ));
    }

    let binding = slots
        .into_iter()
        .zip(apps)
        .map(|(slot, command)| template::Binding {
            slot: slot.to_string(),
            command: Some(command.to_string()),
            con_id: None,
            existing: false,
            app_id: None,
            class: None,
            mark_match: None,
        })
        .collect();

    Ok(template::Bindings { binding })
}

/// Runs every step sequentially, in the order they appear, stopping at the
/// first error — the same `set -eu`-chained behavior as the shell-script
/// examples `--layout`/`--template` are meant to replace. Exits the process
/// directly (success or failure) rather than returning, matching the rest
/// of main()'s error-handling style. Shared by `run_layout()` and
/// `run_template()`, since a resolved template is just more layout steps.
fn run_steps(steps: &[layout::LayoutStep], args: &Args) -> ! {
    if args.dry_run {
        run_steps_dry_run(steps, args);
    }
    if args.validate {
        run_steps_validate(steps, args);
    }

    let default_timeout = time::Duration::from_secs(args.timeout);
    let default_wait_time = time::Duration::from_millis(args.wait_time);
    let mut container_ids = Vec::new();
    let mut resolved_ids = HashMap::new();
    // Only ids this invocation itself launched via `command` (Target::Exec),
    // not ones it merely retargeted via con_id/existing/target_id — see
    // fail_step()'s doc comment.
    let mut launched_container_ids = Vec::new();
    // Every action across every step, each tagged with its 1-based step
    // number (steps.len() can be several, unlike a single direct-CLI
    // invocation) — see step_action_records_json()'s doc comment.
    let mut actions = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        if let Some(id) = step.id.as_deref() {
            if resolved_ids.contains_key(id) {
                fail_step(
                    args,
                    &launched_container_ids,
                    index,
                    format!("id {:?} was already used by an earlier step", id),
                );
            }
        }

        let sway_launch = match step.to_sway_launch(
            default_timeout,
            default_wait_time,
            args.verbose,
            &resolved_ids,
        ) {
            Ok(sway_launch) => sway_launch,
            Err(error) => fail_step(args, &launched_container_ids, index, error),
        };

        // Equivalent to matching Target::Exec on the resolved SwayLaunch,
        // read directly off the step instead: to_sway_launch() having just
        // succeeded above guarantees exactly one of command/con_id/existing/
        // target_id was set, so a non-empty `command` here is exactly the
        // case it resolved to Target::Exec (a step this invocation is about
        // to launch a new window for, as opposed to retargeting one that
        // already existed via con_id/existing/target_id).
        let launched_a_new_window = step.command.is_some();

        match sway_launch.run() {
            Ok(outcome) => {
                if !args.json {
                    println!("{}", outcome.container_id);
                }
                container_ids.push(outcome.container_id);
                if launched_a_new_window {
                    launched_container_ids.push(outcome.container_id);
                }
                if let Some(id) = step.id.as_deref() {
                    resolved_ids.insert(id.to_string(), outcome.container_id);
                }
                actions.extend(step_action_records_json(index, outcome.actions));
            }
            Err(error) => fail_step(args, &launched_container_ids, index, error),
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "container_ids": container_ids,
                "containers": resolved_ids,
                "actions": actions,
            })
        );
    }

    process::exit(0);
}

/// `run_steps()`'s `--dry-run` handling: converts every step to a
/// `SwayLaunch` and prints its planned actions (via `print_dry_run_steps()`),
/// continuing the numbering across steps, without ever calling `.run()` or
/// touching Sway IPC. A step's own `id`/`target_id` chain still needs *some*
/// value to resolve `target_id` lookups against — since nothing actually
/// launches, there's no real container id yet, so each step's own 1-based
/// index is used as a synthetic placeholder; that placeholder is never
/// printed (`dry_run_step()`'s target/action text never names a container
/// id at all), it only exists to keep `to_sway_launch()`'s `target_id`
/// lookups from failing on a step that legitimately references an earlier
/// one.
fn run_steps_dry_run(steps: &[layout::LayoutStep], args: &Args) -> ! {
    let default_timeout = time::Duration::from_secs(args.timeout);
    let default_wait_time = time::Duration::from_millis(args.wait_time);
    let mut resolved_ids = HashMap::new();
    let mut dry_run_steps = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        if let Some(id) = step.id.as_deref() {
            if resolved_ids.contains_key(id) {
                fail(
                    args.json,
                    &format!(
                        "step {}: id {:?} was already used by an earlier step",
                        index + 1,
                        id
                    ),
                );
            }
        }

        let sway_launch = match step.to_sway_launch(
            default_timeout,
            default_wait_time,
            args.verbose,
            &resolved_ids,
        ) {
            Ok(sway_launch) => sway_launch,
            Err(error) => fail(args.json, &format!("step {}: {}", index + 1, error)),
        };

        dry_run_steps.push(dry_run_step(&sway_launch));

        if let Some(id) = step.id.as_deref() {
            resolved_ids.insert(id.to_string(), index as i64);
        }
    }

    print_dry_run_steps(&dry_run_steps, args.json);
    process::exit(0);
}

/// `--validate`'s handling for `--layout`/`--template`: converts every step
/// to a `SwayLaunch` — exercising exactly the same parsing/validation
/// `to_sway_launch()` already does for a real run (height/width/position
/// formats, target-field consistency, `target_id` resolution) — without
/// ever calling `.run()` or touching Sway IPC. A `--template`'s own
/// `--bindings`/`--apps` resolution (slot count, duplicate slots, binding
/// correctness) already happens unconditionally before `run_steps()` is
/// ever reached (`run_template()`'s own `template::resolve()` call), so
/// `--validate` doesn't need to repeat that separately — by the time
/// `steps` reaches here, that part has already succeeded. Uses the same
/// synthetic-`target_id`-placeholder approach `run_steps_dry_run()` does,
/// for the same reason (nothing actually launches, so there's no real
/// container id for a later step's `target_id` to resolve against).
///
/// Success output names the `--layout`/`--template` argument as given
/// (`args.layout`/`args.template`, whichever is set — exactly one is
/// guaranteed by the `--validate` guard before `run_steps()` is ever
/// reached), not a canonicalized path, the same "echo back what was passed"
/// convention `--show-template`'s `"name"` field already uses: `valid:
/// <source> (N step(s))`, or `{"source": ..., "steps": N, "valid": true}`
/// under `--json`.
fn run_steps_validate(steps: &[layout::LayoutStep], args: &Args) -> ! {
    let source = args
        .layout
        .as_deref()
        .or(args.template.as_deref())
        .expect(
            "--validate requires --layout or --template, enforced before run_steps() is reached",
        )
        .display()
        .to_string();
    let default_timeout = time::Duration::from_secs(args.timeout);
    let default_wait_time = time::Duration::from_millis(args.wait_time);
    let mut resolved_ids = HashMap::new();

    for (index, step) in steps.iter().enumerate() {
        if let Some(id) = step.id.as_deref() {
            if resolved_ids.contains_key(id) {
                fail(
                    args.json,
                    &format!(
                        "step {}: id {:?} was already used by an earlier step",
                        index + 1,
                        id
                    ),
                );
            }
        }

        if let Err(error) = step.to_sway_launch(
            default_timeout,
            default_wait_time,
            args.verbose,
            &resolved_ids,
        ) {
            fail(args.json, &format!("step {}: {}", index + 1, error));
        }

        if let Some(id) = step.id.as_deref() {
            resolved_ids.insert(id.to_string(), index as i64);
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::json!({ "source": source, "valid": true, "steps": steps.len() })
        );
    } else {
        println!("valid: {} ({} step(s))", source, steps.len());
    }
    process::exit(0);
}

/// Reports a step failure (`step N: <message>`) — shared by every failure
/// branch in `run_steps()`, since an id collision or a step-conversion error
/// is caught *before* that step's own `SwayLaunch::run()` ever executes, but
/// earlier steps may already have launched real windows needing the same
/// `--rollback-on-error` cleanup as a later step's own `run()` failure.
/// `launched_container_ids` is only ever ids `Target::Exec` resolved —
/// windows this invocation retargeted via con_id/existing/target_id were
/// already open before this invocation started, so rolling them back would
/// close something the user, not this run, created.
fn fail_step(args: &Args, launched_container_ids: &[i64], index: usize, error: String) -> ! {
    let rolled_back = if args.rollback_on_error {
        rollback(launched_container_ids)
    } else {
        Vec::new()
    };
    fail_with_rollback(
        args.json,
        &format!("step {}: {}", index + 1, error),
        &rolled_back,
    );
}

/// Best-effort `--rollback-on-error` cleanup: kills every id in
/// `launched_container_ids`, most-recently-launched first, via a fresh
/// `[con_id] kill` command each. A kill that itself fails (e.g. the window
/// already closed on its own in the meantime) is logged to stderr and
/// skipped rather than treated as fatal — it doesn't stop the rest of the
/// rollback, and the original step failure being reported by `fail_step()`
/// stays the primary error either way. Returns the ids actually killed, for
/// `fail_with_rollback()` to report alongside that error.
fn rollback(launched_container_ids: &[i64]) -> Vec<i64> {
    let mut rolled_back = Vec::new();
    for &container_id in launched_container_ids.iter().rev() {
        match sway_launch::kill_container(container_id) {
            Ok(()) => rolled_back.push(container_id),
            Err(error) => {
                eprintln!(
                    "warning: --rollback-on-error failed to close container {}: {}",
                    container_id, error
                );
            }
        }
    }
    rolled_back
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(args.position, Some("center".to_string()));
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
        let args =
            Args::try_parse_from(["sway-launch", "--existing", "--mark-match", "dropdown-term"])
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
        let args =
            Args::try_parse_from(["sway-launch", "--existing", "--mark-match", "dropdown-term"])
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
    fn args_new_row_short_flag_is_r() {
        let args = Args::try_parse_from(["sway-launch", "-r", "foot"]).unwrap();
        assert!(args.new_row);
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
        assert_eq!(args.height, Some("80ppt".to_string()));
        assert_eq!(args.width, Some("1200px".to_string()));
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
    fn json_result_serializes_container_id() {
        let value = serde_json::json!({ "container_id": 42 });
        assert_eq!(value.to_string(), "{\"container_id\":42}");
    }

    #[test]
    fn actions_json_renders_changed_already_satisfied_and_skipped() {
        let actions = [
            sway_launch::ActionRecord {
                action: "floating enable".to_string(),
                status: sway_launch::ActionStatus::Changed,
            },
            sway_launch::ActionRecord {
                action: "focus".to_string(),
                status: sway_launch::ActionStatus::AlreadySatisfied,
            },
            sway_launch::ActionRecord {
                action: "new_column".to_string(),
                status: sway_launch::ActionStatus::Skipped {
                    reason: "trailing_workspace_edge",
                },
            },
        ];
        assert_eq!(
            actions_json(&actions),
            serde_json::json!([
                { "action": "floating enable", "status": "changed" },
                { "action": "focus", "status": "already_satisfied" },
                { "action": "new_column", "status": "skipped", "reason": "trailing_workspace_edge" },
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
            action: "new_row".to_string(),
            status: sway_launch::ActionStatus::Skipped {
                reason: "trailing_workspace_edge",
            },
        }];
        assert_eq!(
            step_action_records_json(1, actions),
            vec![serde_json::json!({
                "step": 2,
                "action": "new_row",
                "status": "skipped",
                "reason": "trailing_workspace_edge",
            })]
        );
    }

    #[test]
    fn step_action_records_json_is_empty_for_no_actions() {
        assert!(step_action_records_json(0, Vec::new()).is_empty());
    }
}
