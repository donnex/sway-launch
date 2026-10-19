use clap::{error::ErrorKind, CommandFactory, Parser};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::{io, process, time};

use run::{run_lock_source, run_steps};

/// `eprintln!`, minus the panic when stderr is a closed pipe.
///
/// `eprintln!` panics on a failed write, so `sway-launch -v ... 2>&1 | head
/// -n1` would abort with exit 101 on the next diagnostic, stopping between
/// two actions and leaving the window half-arranged. A diagnostic nobody can
/// read is no reason to stop the work it describes, so the write error is
/// dropped. Defined ahead of the `mod` declarations so every module can use
/// it; every stderr write in the crate goes through it.
macro_rules! diagnostic {
    ($($argument:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($argument)*);
    }};
}

mod layout;
mod run;
mod sway_launch;
mod template;

/// The two severities a plain-text stderr line can be labelled with, matching
/// the `error:`/`warning:` prefixes clap gives its own usage errors.
#[derive(Clone, Copy)]
enum Severity {
    Error,
    Warning,
}

/// `error: `/`warning: `, bold red/yellow when `color` is set, plain
/// otherwise. Pure so the rendering is testable without a terminal; see
/// `stderr_wants_color()` for when `color` is set.
fn severity_label(severity: Severity, color: bool) -> String {
    let (word, ansi_color) = match severity {
        Severity::Error => ("error:", "31"),
        Severity::Warning => ("warning:", "33"),
    };
    if color {
        format!("\x1b[1;{}m{}\x1b[0m ", ansi_color, word)
    } else {
        format!("{} ", word)
    }
}

/// Whether a severity label should be colored: only when stderr is a terminal
/// and `NO_COLOR` is unset or empty (the `NO_COLOR` convention), so a
/// redirected log or a `2>&1` capture never gets escape codes in it.
fn stderr_wants_color() -> bool {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    !no_color && io::stderr().is_terminal()
}

/// `severity_label()` for the real stderr.
fn stderr_label(severity: Severity) -> String {
    severity_label(severity, stderr_wants_color())
}

/// Every flag that selects or acts on a window: the flags a `--layout`/
/// `--template` step carries per step instead. `--layout`, `--template` and
/// every standalone mode build their `conflicts_with_all` from this one list
/// via `per_window_and()` (the standalone modes through
/// `standalone_mode_conflicts()`), so a new per-window flag added here is
/// rejected by all of them at once, and
/// `per_window_args_*` in the tests fail if it isn't added here.
///
/// A clap `ArgGroup` would say the same thing, but clap then reports a
/// conflict by listing every member of the group rather than naming the flag
/// that was actually passed.
const PER_WINDOW_ARGS: &[&str] = &[
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
    "split",
    "floating",
    "sticky",
    "fullscreen",
    "focus",
    "mark",
    "new_column",
    "new_row",
    "workspace",
    "workspace_number",
    "output",
    "height",
    "width",
    "position",
    "scratchpad",
    "sway_commands",
    "settle",
];

/// `PER_WINDOW_ARGS` plus a standalone mode's own extra conflicts.
fn per_window_and(extras: &[&'static str]) -> Vec<&'static str> {
    PER_WINDOW_ARGS.iter().chain(extras).copied().collect()
}

/// Every flag that only means something to a `--layout`/`--template` run:
/// the other half of what a standalone mode (`--list-windows`, `--capture`,
/// `--list-templates`, `--show-template`, `--debug-events`, `--completions`)
/// would otherwise accept and discard. Each builds its `conflicts_with_all`
/// through `standalone_mode_conflicts()`, so a new run-level flag added here
/// is rejected by all of them at once. `main()` dispatches those modes before its
/// own "requires --layout or --template" checks, so they can't catch it.
/// `run_args_*` in the tests fail if a flag `main()` checks isn't here.
const RUN_ARGS: &[&str] = &[
    "bindings",
    "apps",
    "param",
    "rollback_on_error",
    "continue_on_timeout",
    "on_workspace",
    "on_workspace_number",
    "on_output",
    "concurrent",
    "no_lock",
    "validate",
];

/// `PER_WINDOW_ARGS` and `RUN_ARGS` plus a standalone mode's own extra
/// conflicts.
fn standalone_mode_conflicts(extras: &[&'static str]) -> Vec<&'static str> {
    per_window_and(RUN_ARGS)
        .into_iter()
        .chain(extras.iter().copied())
        .collect()
}

/// The per-window flags `--list-windows` accepts: they narrow the listing to
/// what `--existing` would choose between, rather than being discarded.
const LIST_WINDOWS_FILTERS: &[&str] = &["app_id", "class", "mark_match", "title"];

/// `standalone_mode_conflicts()` for `--list-windows`, which keeps its
/// filters.
fn standalone_mode_conflicts_but_filters(extras: &[&'static str]) -> Vec<&'static str> {
    standalone_mode_conflicts(extras)
        .into_iter()
        .filter(|name| !LIST_WINDOWS_FILTERS.contains(name))
        .collect()
}

/// The `RUN_ARGS` that `main()` rejects without `--layout`/`--template`, by
/// `Args` id, each with whether it was passed. `--bindings`/`--apps`/
/// `--param` aren't here: they require `--template` alone, and are checked
/// on their own.
fn layout_or_template_only_flags(args: &Args) -> [(&'static str, bool); 8] {
    [
        ("rollback_on_error", args.rollback_on_error),
        ("continue_on_timeout", args.continue_on_timeout),
        ("on_workspace", args.on_workspace.is_some()),
        ("on_workspace_number", args.on_workspace_number.is_some()),
        ("on_output", args.on_output.is_some()),
        ("concurrent", args.concurrent),
        ("no_lock", args.no_lock),
        ("validate", args.validate),
    ]
}

const HEADING_LAUNCH: &str = "Launch a window";
const HEADING_MATCH: &str = "Match the window";
const HEADING_OPEN_WINDOW: &str = "Use an open window instead";
const HEADING_ACT: &str = "Act on the window";
const HEADING_GEOMETRY: &str = "Size and position";
const HEADING_MOVE: &str = "Move the window";
const HEADING_LAYOUTS: &str = "Layouts and templates";
const HEADING_RUN: &str = "Run and output";
const HEADING_INSPECT: &str = "Inspect";
const HEADING_GENERAL: &str = "General";

const USAGE: &str = "sway-launch [OPTIONS] <COMMAND>
       sway-launch [OPTIONS] --existing <-a APP_ID|-c CLASS|--mark-match MARK|--title REGEX>
       sway-launch [OPTIONS] --con-id <ID>
       sway-launch --layout <FILE> [OPTIONS]
       sway-launch --template <NAME|FILE> <--apps APPS|--bindings FILE> [OPTIONS]
       sway-launch <--list-windows|--capture [WORKSPACE]|--list-templates|--debug-events>";

const EXAMPLES: &str = concat!(
    "Examples:
  # Launch foot and print its container id
  sway-launch foot
  # Launch htop in foot as a floating window, half the screen wide
  sway-launch -a foot -f --width 50ppt 'foot -e htop'
  # Move the open Firefox window to workspace 2
  sway-launch --existing -a firefox --workspace 2
  # Fill a 2x2 grid with four terminals
  sway-launch --template quad-grid --apps foot,foot,foot,foot
  # List the built-in layout templates
  sway-launch --list-templates

The full guide, with layout files and templates, is at ",
    env!("CARGO_PKG_REPOSITORY"),
    "#readme"
);

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None, override_usage = USAGE,
    after_help = EXAMPLES, disable_help_flag = true, disable_version_flag = true)]
struct Args {
    /// The command to launch, as one string: quote it if it has arguments
    ///
    /// Run the way Sway's own exec runs it. sway-launch waits for the
    /// command's window to appear, prints its container id and then applies
    /// the actions below to it. An unquoted , or ; is refused, since Sway
    /// would split the command on it.
    #[clap(value_parser = sway_launch::validate_exec_command, help_heading = HEADING_LAUNCH,
        display_order = 0, verbatim_doc_comment)]
    command: Option<String>,

    /// Start the command in this directory
    ///
    /// Instead of Sway's working directory. A relative path is relative to
    /// the current directory, and ~ is the home directory.
    #[clap(long, value_name = "DIR", conflicts_with_all = ["con_id", "existing"],
        help_heading = HEADING_LAUNCH, display_order = 1, verbatim_doc_comment)]
    cwd: Option<String>,

    /// Set a variable for the command (repeatable)
    ///
    /// The variables and --cwd's directory are written to a file only you
    /// can read, in $XDG_RUNTIME_DIR/sway-launch/, which the launch reads
    /// and deletes, so they never appear in the process list. Without
    /// XDG_RUNTIME_DIR they go on the launch's command line instead, with
    /// a warning: every local user can read them there while it runs.
    #[clap(long, value_name = "KEY=VALUE", value_parser = sway_launch::parse_env_assignment,
        conflicts_with_all = ["con_id", "existing"], help_heading = HEADING_LAUNCH,
        display_order = 2, verbatim_doc_comment)]
    env: Vec<(String, String)>,

    /// Match the window by app_id (exact, case-sensitive)
    ///
    /// For a launch, only a window with this app_id counts as the command's
    /// window. With --existing, finds an already-open window instead. Not a
    /// regular expression. Wayland applications have an app_id; see
    /// --list-windows for the values in use.
    #[clap(short, long, conflicts_with_all = ["class", "con_id", "mark_match"],
        value_parser = sway_launch::validate_non_blank_argument,
        help_heading = HEADING_MATCH, verbatim_doc_comment)]
    app_id: Option<String>,

    /// Match the window by X11 class (exact, case-sensitive)
    ///
    /// The same as --app-id, for applications running under XWayland, which
    /// have a class instead of an app_id.
    #[clap(short, long, conflicts_with_all = ["con_id", "mark_match"],
        value_parser = sway_launch::validate_non_blank_argument,
        help_heading = HEADING_MATCH, verbatim_doc_comment)]
    class: Option<String>,

    /// Match the window by title (a regular expression)
    ///
    /// Unanchored, like Sway's title= criteria. Narrows --app-id/--class, or
    /// works alone. A launch waits past a splash window whose title doesn't
    /// match.
    #[clap(long, value_name = "REGEX", conflicts_with = "con_id",
        value_parser = sway_launch::parse_title_pattern,
        help_heading = HEADING_MATCH, verbatim_doc_comment)]
    title: Option<sway_launch::TitlePattern>,

    /// Match an open window by one of its marks (exact, case-sensitive)
    ///
    /// Requires --existing or --list-windows.
    #[clap(long, value_name = "MARK", conflicts_with_all = ["class", "con_id"],
        value_parser = sway_launch::validate_sway_string_argument,
        help_heading = HEADING_MATCH, verbatim_doc_comment)]
    mark_match: Option<String>,

    /// Act on an open window found by the match flags, launching nothing
    ///
    /// Exactly one window must match --app-id, --class, --mark-match or
    /// --title; several matches are an error.
    #[clap(long, conflicts_with_all = ["command", "con_id"],
        help_heading = HEADING_OPEN_WINDOW, verbatim_doc_comment)]
    existing: bool,

    /// With --existing, wait for the window to open
    ///
    /// Waits up to --timeout for a matching window instead of failing when
    /// none is open yet, e.g. one an autostart entry is still starting.
    #[clap(long, help_heading = HEADING_OPEN_WINDOW, verbatim_doc_comment)]
    wait: bool,

    /// Act on the open window with this container id
    #[clap(long, value_name = "ID", conflicts_with = "command",
        help_heading = HEADING_OPEN_WINDOW)]
    con_id: Option<i64>,

    /// Split the window, so the next one opens beside (h) or below (v) it
    #[clap(value_enum, short, long, help_heading = HEADING_ACT)]
    split: Option<sway_launch::Split>,

    /// Make the window floating
    #[clap(short, long, help_heading = HEADING_ACT)]
    floating: bool,

    /// Make the window sticky (a floating window shows on every workspace)
    #[clap(long, help_heading = HEADING_ACT)]
    sticky: bool,

    /// Make the window fullscreen
    #[clap(long, help_heading = HEADING_ACT)]
    fullscreen: bool,

    /// Focus the window
    #[clap(long, help_heading = HEADING_ACT)]
    focus: bool,

    /// Mark the window, replacing its marks
    ///
    /// A mark another window holds moves to this one.
    #[clap(short, long, value_parser = sway_launch::validate_sway_string_argument,
        help_heading = HEADING_ACT, verbatim_doc_comment)]
    mark: Option<String>,

    /// Move the window to the scratchpad, after every other action
    #[clap(long, help_heading = HEADING_ACT)]
    scratchpad: bool,

    /// Send a Sway command to the window, for what no flag sets (repeatable)
    ///
    /// Sent as "[con_id=N] COMMAND", e.g. "border none" or "opacity 0.9".
    /// Only commands that act on one window are accepted, and their effect is
    /// reported as unconfirmed.
    #[clap(long = "sway-command", value_name = "COMMAND",
        value_parser = sway_launch::validate_sway_command,
        help_heading = HEADING_ACT, verbatim_doc_comment)]
    sway_commands: Vec<String>,

    /// Set the window's height: <n>px or <n>ppt (percent)
    #[clap(long, value_name = "SIZE", value_parser = sway_launch::parse_size,
        help_heading = HEADING_GEOMETRY)]
    height: Option<sway_launch::Size>,

    /// Set the window's width: <n>px or <n>ppt (percent)
    #[clap(long, value_name = "SIZE", value_parser = sway_launch::parse_size,
        help_heading = HEADING_GEOMETRY)]
    width: Option<sway_launch::Size>,

    /// Move a floating window to "center" or "<x>,<y>"
    ///
    /// Pixels relative to the window's workspace, as Sway's own move position
    /// is; x and y may be negative.
    #[clap(long, value_parser = sway_launch::parse_position, allow_hyphen_values = true,
        help_heading = HEADING_GEOMETRY, verbatim_doc_comment)]
    position: Option<sway_launch::Position>,

    /// Hold the px size and position for MS ms, reapplying them if changed
    ///
    /// For applications that resize themselves just after opening, as
    /// browsers do: once --height/--width (in px) and --position are
    /// confirmed, keep watching for MS milliseconds and apply them again if
    /// the application undoes them.
    #[clap(long, value_name = "MS", value_parser = sway_launch::validate_settle_argument,
        help_heading = HEADING_GEOMETRY, verbatim_doc_comment)]
    settle: Option<u64>,

    // Can't be combined with --workspace: the relative move would act on
    // the workspace the window opened on, and the workspace move then drops
    // it into the target as an ordinary sibling (#45).
    /// Move the window into a new column (move right)
    #[clap(short, long, conflicts_with_all = ["workspace", "workspace_number"],
        help_heading = HEADING_MOVE)]
    new_column: bool,

    /// Move the window into a new row (move down)
    #[clap(long, short = 'r', conflicts_with_all = ["workspace", "workspace_number"],
        help_heading = HEADING_MOVE)]
    new_row: bool,

    /// Move the window to this workspace
    #[clap(long, value_parser = sway_launch::validate_workspace_argument,
        help_heading = HEADING_MOVE)]
    workspace: Option<String>,

    /// Move the window to the workspace with this number
    ///
    /// Found the way Sway's "workspace number N" finds it: 2 matches a
    /// workspace named "2: web".
    #[clap(long, value_name = "N", conflicts_with = "workspace",
        value_parser = sway_launch::parse_workspace_number,
        help_heading = HEADING_MOVE, verbatim_doc_comment)]
    workspace_number: Option<i32>,

    /// Move the window to this output (monitor)
    #[clap(long, value_parser = sway_launch::validate_output_argument,
        help_heading = HEADING_MOVE)]
    output: Option<String>,

    /// Build the windows a TOML layout file describes (- reads stdin)
    ///
    /// Each step is the equivalent of one sway-launch call's flags, so the
    /// per-window flags above can't be combined with it. The format is in
    /// the guide linked at the end of this help.
    #[clap(long, value_name = "FILE",
        conflicts_with_all = per_window_and(&["debug_events", "bindings", "apps", "param"]),
        help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    layout: Option<PathBuf>,

    /// Build a template's shape: a built-in name or a .toml file
    ///
    /// A template describes a shape with slots instead of applications; fill
    /// them with --apps or --bindings. A name without .toml is a built-in
    /// (see --list-templates), a path ending in .toml is a file, and - reads
    /// stdin. The per-window flags above can't be combined with it.
    #[clap(long, value_name = "NAME|FILE",
        conflicts_with_all = per_window_and(&["debug_events", "layout"]),
        help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    template: Option<PathBuf>,

    /// Commands for the template's slots, comma-separated, in slot order
    ///
    /// Each slot launches with no app_id/class filter, so its window is the
    /// first new one that can't be traced to another launch. For an
    /// application that may already be running (most browsers and editors),
    /// use --bindings with app_id/class instead. Requires --template;
    /// conflicts with --bindings.
    #[clap(long, conflicts_with = "bindings", help_heading = HEADING_LAYOUTS,
        verbatim_doc_comment)]
    apps: Option<String>,

    /// Bindings file filling the template's slots (- reads stdin)
    ///
    /// Gives each slot its command and, optionally, the app_id or class to
    /// match, placement and timeouts. Requires --template.
    #[clap(long, value_name = "FILE", help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    bindings: Option<PathBuf>,

    /// Set one of the template's params (repeatable)
    ///
    /// Overrides the param's default and a --bindings file's [params]; see
    /// --list-templates for each template's params. Requires --template.
    #[clap(long, value_name = "NAME=VALUE", value_parser = template::parse_param_override,
        help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    param: Vec<(String, String)>,

    /// Build the run on this workspace
    ///
    /// Switches to it once before the first step, so every window opens
    /// there. Overrides a template's own [layout] workspace.
    #[clap(long, value_name = "WORKSPACE",
        value_parser = sway_launch::validate_workspace_argument,
        help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    on_workspace: Option<String>,

    /// Build the run on the workspace with this number
    ///
    /// --on-workspace by number, the way Sway's "workspace number N" finds
    /// it: 2 matches a workspace named "2: web".
    #[clap(long, value_name = "N", conflicts_with = "on_workspace",
        value_parser = sway_launch::parse_workspace_number,
        help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    on_workspace_number: Option<i32>,

    /// Build the run on this output (monitor)
    ///
    /// Focuses it once before the first step, so a new workspace is created
    /// there. Overrides a template's own [layout] output.
    #[clap(long, value_name = "OUTPUT",
        value_parser = sway_launch::validate_output_argument,
        help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    on_output: Option<String>,

    /// Start every step's application at once, then arrange them in order
    ///
    /// The applications open on a workspace of their own and are moved into
    /// place in step order, so the run takes about as long as its slowest
    /// application rather than all of them together. Fails if two steps'
    /// launches end up with the same window, as single-instance applications
    /// can. A layout or bindings file can also turn it on, with a top-level
    /// concurrent = true.
    #[clap(long, help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    concurrent: bool,

    /// Carry on past a step whose window doesn't appear in time
    ///
    /// The step is reported and the run goes on with the next one; later
    /// steps that target the missing window are skipped. The run still exits
    /// non-zero.
    #[clap(long, help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    continue_on_timeout: bool,

    /// Close the windows this run launched if a step fails
    ///
    /// Only windows this run opened itself, by an earlier step or by the
    /// failing one, never ones it retargeted through con_id, existing or
    /// target_id.
    #[clap(long, help_heading = HEADING_LAYOUTS, verbatim_doc_comment)]
    rollback_on_error: bool,

    /// Run even if another layout is already being built
    #[clap(long, help_heading = HEADING_LAYOUTS)]
    no_lock: bool,

    /// Seconds to wait for the window, and for each action to be confirmed
    #[clap(short, long, value_name = "SECONDS", default_value_t = 5,
        value_parser = sway_launch::validate_timeout_argument, help_heading = HEADING_RUN)]
    timeout: u64,

    /// Milliseconds to wait before checking actions Sway sends no event for
    #[clap(short, long, value_name = "MS", default_value_t = 20, help_heading = HEADING_RUN)]
    wait_time: u64,

    /// Print the Sway commands a run would send, without running them
    ///
    /// Works with a command or --layout/--template. Never touches Sway or
    /// launches anything.
    #[clap(long, conflicts_with_all = ["debug_events", "validate"],
        help_heading = HEADING_RUN, verbatim_doc_comment)]
    dry_run: bool,

    /// Check a --layout/--template run's files without running it
    ///
    /// For --template, also checks the --bindings/--apps resolution. Never
    /// touches Sway or launches anything, and checks only the format of a
    /// cwd, not that the directory exists on this machine.
    #[clap(long, conflicts_with = "debug_events", help_heading = HEADING_RUN,
        verbatim_doc_comment)]
    validate: bool,

    /// Print the result as a JSON object instead of a bare container id
    #[clap(long, help_heading = HEADING_RUN)]
    json: bool,

    /// Explain what the run is doing on stderr
    #[clap(short, long, help_heading = HEADING_RUN)]
    verbose: bool,

    /// List every open window and exit
    ///
    /// Shows each window's container id, app_id, class, workspace, marks and
    /// title. --app-id, --class, --mark-match and --title narrow it to the
    /// windows --existing would choose between.
    #[clap(long, conflicts_with_all = standalone_mode_conflicts_but_filters(&[
        "debug_events", "layout", "template", "completions", "list_templates", "show_template",
        "dry_run",
    ]), help_heading = HEADING_INSPECT, verbatim_doc_comment)]
    list_windows: bool,

    /// Print a layout file that rebuilds a workspace, and exit
    ///
    /// The focused workspace if none is named: its windows' splits, sizes and
    /// marks, and floating windows' size and position. Each window's command
    /// is a guess from its app_id or class. Tabbed and stacked containers
    /// can't be captured.
    #[clap(long, value_name = "WORKSPACE", num_args = 0..=1, conflicts_with_all = standalone_mode_conflicts(&[
        "debug_events", "layout", "template", "completions", "list_templates", "list_windows",
        "show_template", "dry_run", "json",
    ]), help_heading = HEADING_INSPECT, verbatim_doc_comment)]
    capture: Option<Option<String>>,

    /// List the built-in templates and exit
    #[clap(long, conflicts_with_all = standalone_mode_conflicts(&[
        "debug_events", "layout", "template", "completions", "dry_run", "show_template",
    ]), help_heading = HEADING_INSPECT)]
    list_templates: bool,

    /// Print a template's TOML and exit
    ///
    /// A built-in name or a path ending in .toml, as for --template. With
    /// --json, prints {"name": ..., "contents": "..."}.
    #[clap(long, value_name = "NAME|FILE", conflicts_with_all = standalone_mode_conflicts(&[
        "debug_events", "layout", "template", "completions", "dry_run",
    ]), help_heading = HEADING_INSPECT, verbatim_doc_comment)]
    show_template: Option<PathBuf>,

    // A standalone mode like --completions/--list-templates: it never acts
    // on a window, so a command or per-window flag alongside it could only
    // be discarded. Rejected rather than ignored, same as those.
    //
    // Kept as `//` rather than `///`: clap renders every doc-comment
    // paragraph into `--help`, where these notes mean nothing to a user.
    /// Print every Sway event until interrupted
    ///
    /// One JSON object per line with --json. Can't be combined with a command
    /// or per-window flags.
    #[clap(short, long, conflicts_with_all = standalone_mode_conflicts(&[]), help_heading = HEADING_INSPECT, verbatim_doc_comment)]
    debug_events: bool,

    // Conflicts with --json, unlike the other standalone modes: a completion
    // script is shell source, so there is no JSON shape for it to take, and
    // accepting the flag could only discard it. --list-templates and
    // --show-template both do have one, which is why they don't conflict.
    /// Print a shell completion script and exit
    #[clap(long, value_enum, value_name = "SHELL", conflicts_with_all = standalone_mode_conflicts(&[
        "debug_events", "layout", "template", "list_templates", "dry_run", "show_template", "json",
    ]), help_heading = HEADING_GENERAL)]
    completions: Option<clap_complete::Shell>,

    // Declared by hand only to list them under "General", last, instead of
    // clap's own "Options" heading at the top.
    /// Print help (see more with --help)
    #[clap(short, long, action = clap::ArgAction::Help, help_heading = HEADING_GENERAL)]
    help: Option<bool>,

    /// Print the version
    #[clap(short = 'V', long, action = clap::ArgAction::Version, help_heading = HEADING_GENERAL)]
    version: Option<bool>,
}

/// The man page, rendered from the same `Args` definition `--help` comes
/// from, so it can't drift from the real flags. Built only with the
/// `generate-man` feature, which the release job uses (#109).
///
/// Rendered section by section so the examples get an EXAMPLES heading:
/// `Man::render()` files `after_help` under a generic "EXTRA".
#[cfg(feature = "generate-man")]
fn render_man_page(out: &mut dyn std::io::Write) -> std::io::Result<()> {
    let man = clap_mangen::Man::new(Args::command());
    man.render_title(out)?;
    man.render_name_section(out)?;
    man.render_synopsis_section(out)?;
    man.render_description_section(out)?;
    man.render_options_section(out)?;
    out.write_all(man_examples_section().as_bytes())?;
    man.render_version_section(out)
}

/// `EXAMPLES` as a roff section, its own "Examples:" line replaced by the
/// heading and every line kept as written.
#[cfg(feature = "generate-man")]
fn man_examples_section() -> String {
    let body = EXAMPLES.split_once('\n').map_or(EXAMPLES, |(_, body)| body);
    let mut section = String::from(".SH EXAMPLES\n.nf\n");
    for line in body.lines() {
        section.push_str(&line.replace('\\', "\\e").replace('-', "\\-"));
        section.push('\n');
    }
    section.push_str(".fi\n");
    section
}

fn main() {
    // Checked before parsing: it isn't an `Args` flag, so it never appears in
    // --help or in the shipped binary, which is built without the feature.
    #[cfg(feature = "generate-man")]
    if std::env::args().nth(1).as_deref() == Some("--generate-man") {
        if let Err(error) = render_man_page(&mut std::io::stdout()) {
            exit_on_write_failure(&error);
        }
        process::exit(0);
    }

    let args = Args::parse();

    if let Some(shell) = args.completions {
        // Generated into a buffer rather than straight to stdout:
        // clap_complete::generate() panics on a failed write, so a closed pipe
        // or a full disk ended in exit 101 instead of going through
        // print_str()'s clean exit.
        let mut script = Vec::new();
        clap_complete::generate(shell, &mut Args::command(), "sway-launch", &mut script);
        print_str(&String::from_utf8_lossy(&script));
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

    if args.list_windows {
        print_window_listing(&args);
        process::exit(0);
    }

    if let Some(workspace) = &args.capture {
        match sway_launch::capture_workspace(workspace.as_deref()) {
            Ok(layout) => {
                print_str(&layout);
                process::exit(0);
            }
            Err(error) => fail(false, &format!("--capture: {}", error)),
        }
    }

    if args.debug_events {
        match sway_launch::debug_events(args.json) {
            Ok(_) => process::exit(0),
            Err(error) => fail(args.json, &error),
        }
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

    if args.template.is_none() && !args.param.is_empty() {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--param requires --template",
            )
            .exit();
    }

    if let Some(template_path) = &args.template {
        if is_stdin(template_path) && args.bindings.as_deref().is_some_and(is_stdin) {
            Args::command()
                .error(
                    ErrorKind::ArgumentConflict,
                    "--template and --bindings can't both read stdin (-)",
                )
                .exit();
        }
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

    if args.layout.is_none() && args.template.is_none() {
        if let Some((name, _)) = layout_or_template_only_flags(&args)
            .into_iter()
            .find(|(_, is_set)| *is_set)
        {
            Args::command()
                .error(
                    ErrorKind::MissingRequiredArgument,
                    format!(
                        "--{} requires --layout or --template",
                        name.replace('_', "-")
                    ),
                )
                .exit();
        }
    }

    let target = match sway_launch::Target::from_fields(sway_launch::TargetFields {
        command: args.command.as_deref(),
        con_id: args.con_id,
        existing: args.existing,
        wait: args.wait,
        target_id: None,
        app_id: args.app_id.as_deref(),
        class: args.class.as_deref(),
        mark_match: args.mark_match.as_deref(),
        title: args.title.as_ref(),
    }) {
        Ok(target) => target,
        Err(error) => {
            let kind = match error {
                sway_launch::TargetError::NoTarget
                | sway_launch::TargetError::ExistingWithoutMatcher
                | sway_launch::TargetError::MarkMatchWithoutExisting
                | sway_launch::TargetError::WaitWithoutExisting => {
                    ErrorKind::MissingRequiredArgument
                }
                sway_launch::TargetError::SeveralTargets
                | sway_launch::TargetError::SeveralMatchers
                | sway_launch::TargetError::MatcherWithContainerId => ErrorKind::ArgumentConflict,
                sway_launch::TargetError::Blank(_) | sway_launch::TargetError::Command(_) => {
                    ErrorKind::InvalidValue
                }
            };
            Args::command()
                .error(kind, error.message(sway_launch::FieldStyle::Cli))
                .exit();
        }
    };

    let launch_setup = match sway_launch::launch_setup(
        args.command.is_some(),
        args.cwd.as_deref(),
        &args.env,
        sway_launch::DirectoryCheck::for_offline(args.dry_run),
    ) {
        Ok(launch_setup) => launch_setup,
        Err(error) => Args::command()
            .error(ErrorKind::InvalidValue, format!("--cwd/--env: {}", error))
            .exit(),
    };

    let settle =
        match sway_launch::settle_duration(args.settle, args.height, args.width, args.position) {
            Ok(settle) => settle,
            Err(error) => Args::command()
                .error(
                    ErrorKind::MissingRequiredArgument,
                    format!("--settle: {}", error),
                )
                .exit(),
        };

    let sway_launch = sway_launch::SwayLaunch {
        target,
        launch_setup,
        actions: sway_launch::ActionRequest {
            split: args.split,
            floating: args.floating,
            sticky: args.sticky,
            fullscreen: args.fullscreen,
            focus: args.focus,
            mark: args.mark.as_deref(),
            new_column: args.new_column,
            new_row: args.new_row,
            workspace: sway_launch::WorkspaceTarget::from_pair(
                args.workspace.as_deref(),
                args.workspace_number,
            ),
            output: args.output.as_deref(),
            height: args.height,
            width: args.width,
            position: args.position,
            scratchpad: args.scratchpad,
            sway_commands: &args.sway_commands,
            settle,
        },
        timeout: time::Duration::from_secs(args.timeout),
        wait_time: time::Duration::from_millis(args.wait_time),
        verbose: args.verbose,
        build_workspace: None,
        warn_on_misplaced_launch: false,
        catch_late_window: false,
        excluded_containers: Vec::new(),
    };

    if args.dry_run {
        print_dry_run_steps(&[], &[dry_run_step(&sway_launch)], args.json);
        process::exit(0);
    }

    if sway_launch.needs_compositor() {
        check_compositor_version(args.verbose);
    }

    match sway_launch.run() {
        Ok(outcome) => {
            if args.json {
                print_line(
                    &serde_json::json!({
                        "container_id": outcome.container_id,
                        "actions": actions_json(&outcome.actions),
                    })
                    .to_string(),
                );
            } else {
                print_line(&outcome.container_id.to_string());
            }
        }
        Err(error) => fail_run(args.json, &error),
    };
}

/// Asks the compositor its version ahead of a run's first request to it,
/// printing it under `verbose` and warning when it isn't a supported Sway
/// (#147): on Sway 1.7 or i3 a run still goes ahead, and otherwise fails
/// however the difference surfaces, usually as a timeout that sends the user
/// to `--timeout` instead of to their Sway version. The warning goes to
/// stderr under `--json` too, like the other warnings about the environment
/// rather than about the run's result, so it's there when the run then fails.
/// A compositor that can't be reached is left for the run's own first request
/// to report.
fn check_compositor_version(verbose: bool) {
    match sway_launch::check_compositor_version() {
        Ok((line, warning)) => {
            if verbose {
                diagnostic!("{}", line);
            }
            if let Some(warning) = warning {
                diagnostic!("{}{}", stderr_label(Severity::Warning), warning);
            }
        }
        Err(error) if verbose => {
            diagnostic!("Couldn't ask the compositor its version: {}", error);
        }
        Err(_) => {}
    }
}

/// Writes one line to stdout, exiting cleanly if stdout has already been
/// closed — `sway-launch --debug-events | head -5` and friends.
///
/// Rust ignores `SIGPIPE`, so a plain `println!` into a closed pipe panics
/// ("failed printing to stdout: Broken pipe") and exits 101, which is a poor
/// answer to an ordinary shell idiom. Handled here rather than by restoring
/// the default `SIGPIPE` disposition, which would mean an `unsafe` libc call
/// and a new dependency for something this narrow.
///
/// Short outputs usually fit the pipe buffer and never notice; the case that
/// reliably hits this is `--debug-events`, which writes until killed.
fn print_line(line: &str) {
    if let Err(error) = write_line(line) {
        exit_on_write_failure(&error);
    }
}

fn write_line(line: &str) -> io::Result<()> {
    use std::io::Write;
    writeln!(io::stdout(), "{}", line)
}

/// `print_line()` for output that's a side channel to work still in
/// progress: a failed write is recorded in `failure` instead of ending the
/// process, and nothing further is written once one has failed.
///
/// `print_line()`'s immediate exit is right where the output *is* the job.
/// In a multi-step run it isn't — the job is the steps still to come — and
/// exiting there stopped a layout partway and reported success:
/// `sway-launch --layout four.toml | head -n1` opened three windows, never
/// ran step 4, and exited 0. The caller hands `failure` to
/// `exit_on_write_failure_with_status()` once every step has finished
/// instead, with the status the steps earned.
fn print_line_deferring_failure(line: &str, failure: &mut Option<io::Error>) {
    if failure.is_none() {
        if let Err(error) = write_line(line) {
            *failure = Some(error);
        }
    }
}

/// `print_line()` without the trailing newline, for output that carries its
/// own (a template file's raw contents). Flushes explicitly, since stdout is
/// line-buffered and the trailing partial line would otherwise sit in the
/// buffer.
fn print_str(text: &str) {
    use std::io::Write;
    let result = write!(io::stdout(), "{}", text).and_then(|()| io::stdout().flush());
    if let Err(error) = result {
        exit_on_write_failure(&error);
    }
}

fn exit_on_write_failure(error: &io::Error) -> ! {
    exit_on_write_failure_with_status(error, 0)
}

/// `exit_on_write_failure()` for a run whose exit status was already decided
/// when the write failed: a closed stdout still exits silently, but with
/// `status` rather than 0. A `--continue-on-timeout` run whose steps timed
/// out exits 1, and a reader that closed the pipe early (`| head -n1` under
/// `pipefail`) mustn't turn that into success (#165).
fn exit_on_write_failure_with_status(error: &io::Error, status: i32) -> ! {
    if error.kind() == io::ErrorKind::BrokenPipe {
        // The reader is gone, so there's nothing left to say and nothing to
        // report it to. The status is all that's left to tell the caller.
        process::exit(status);
    }
    diagnostic!(
        "{}failed writing to stdout: {}",
        stderr_label(Severity::Error),
        error
    );
    process::exit(1);
}

/// Renders one `ActionRecord` as `--json`'s per-action object:
/// `{"kind": ..., "command": ..., "status":
/// "changed"|"already_satisfied"|"unconfirmed"|"skipped"}`. `"kind"` is the
/// action's stable flag name (`"new_column"`, `"mark"`), the field to match
/// on; `"command"` is the Sway command text it ran, absent for a skip, which
/// never rendered one. A `"reason"` field is added only for `"skipped"`: the
/// machine-readable `ActionStatus::Skipped` reason (`SkipReason::as_str()`),
/// not prose.
///
/// `"unconfirmed"` is deliberately its own status rather than folded into
/// `"changed"`: the command was sent and the wait elapsed, but the change was
/// never observed. That's a weaker claim than the other two make, and a caller
/// chaining actions on the assumption the previous one took effect is exactly
/// who needs to be able to tell.
fn action_record_json(record: &sway_launch::ActionRecord) -> serde_json::Value {
    let status = match record.status {
        sway_launch::ActionStatus::Changed => "changed",
        sway_launch::ActionStatus::AlreadySatisfied => "already_satisfied",
        sway_launch::ActionStatus::Unconfirmed => "unconfirmed",
        sway_launch::ActionStatus::Skipped { .. } => "skipped",
    };
    let mut value = serde_json::json!({ "kind": record.kind, "status": status });
    if let Some(command) = &record.command {
        value["command"] = serde_json::json!(command);
    }
    if let sway_launch::ActionStatus::Skipped { reason } = record.status {
        value["reason"] = serde_json::json!(sway_launch::SkipReason::as_str(reason));
    }
    value
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
/// `{"error": "...", "rolled_back": [...]}` object instead of printed as plain
/// text (plus whichever optional fields `fail_with_rollback()` documents),
/// mirroring the structured `{"container_id": ...}`/`{"container_ids": [...]}` success
/// shapes below rather than leaving `--json` callers to also parse plain-text
/// stderr on failure. Deliberately scoped to these runtime failures only — a
/// bad CLI invocation (missing/conflicting flags, caught via clap's `.error()`
/// calls above) is still reported via clap's own usage-error formatting
/// regardless of `--json`, the same as `--help` itself isn't JSON.
fn fail(json: bool, message: &str) -> ! {
    fail_run(json, &sway_launch::RunError::from(message.to_string()));
}

/// `fail()` for a `SwayLaunch::run()` failure, which may also carry windows a
/// launch saw but rejected (see `sway_launch::RunError`).
fn fail_run(json: bool, error: &sway_launch::RunError) -> ! {
    fail_with_rollback(
        json,
        error,
        &RollbackOutcome::default(),
        &PartialProgress::default(),
    );
}

/// What `--rollback-on-error`'s cleanup actually managed to do: the ids that
/// were asked to close and then left the tree, and the ids it couldn't
/// confirm closed.
///
/// A successful `[con_id] kill` is not a close. Sway sends the client a close
/// request and replies as soon as it has, and the client decides whether and
/// when to act on it — an editor with unsaved changes, or a browser asking
/// before closing several tabs, keeps its window open. So `rolled_back` only
/// holds ids that were seen gone from the tree within
/// `ROLLBACK_CLOSE_GRACE` of the kill (see `rollback()`); everything else
/// lands in `failed`.
///
/// The split exists because the two mean genuinely different things to a
/// caller, and collapsing them loses information either way. A rolled-back
/// window is gone. A `failed` one is in an unknown state: its kill failed —
/// overwhelmingly likely because it had already closed on its own (the case
/// `rollback_on_error_handles_a_launched_window_that_already_closed_itself`
/// covers), though a kill can fail for other reasons too — or its close
/// request was accepted and the window was still open when the wait ran out.
/// Reporting it as still-open progress could assert something untrue;
/// reporting it as closed could too; silently dropping it would hide a
/// window the caller may still need to deal with. So it gets its own field
/// and is excluded from `PartialProgress`, leaving the three sets
/// unambiguous: `container_ids` is open, `rolled_back` is closed,
/// `rollback_failed` needs checking.
#[derive(Default)]
struct RollbackOutcome {
    rolled_back: Vec<i64>,
    failed: Vec<i64>,
}

impl RollbackOutcome {
    /// Every id this rollback touched, whatever the result — what
    /// `PartialProgress` filters out, since none of them can be honestly
    /// reported as a window still open.
    fn attempted(&self) -> impl Iterator<Item = &i64> {
        self.rolled_back.iter().chain(self.failed.iter())
    }
}

/// What a multi-step `--layout`/`--template` run had already accomplished
/// when a later step failed — the same `container_ids`/`containers` a
/// successful run reports, but for the steps that did complete.
///
/// Plain output prints each step's container id as that step finishes, so a
/// failure there still leaves the caller holding every id. `--json` collects
/// them for one object at the end instead, so without this a failure
/// reported nothing but the error, leaving a caller with real windows open
/// on screen and no way to identify them short of walking the tree and
/// guessing — the exact ambiguity `id`/`containers` exists to remove.
///
/// The failing step's own window counts too, when it got as far as launching
/// one: a step can launch its window and then fail on a later action, and
/// that window is just as real (see `run_steps()`'s `Err` branch). A window
/// the failing step only retargeted doesn't, since it may never have
/// existed. Always empty outside `run_steps()`: a single invocation reports
/// its one resolved container as the error's own `container_id` instead (see
/// `fail_with_rollback()`), since there is no list of steps for it to be part
/// of.
#[derive(Default)]
struct PartialProgress {
    container_ids: Vec<i64>,
    containers: HashMap<String, i64>,
    /// `--concurrent` launches that may still open a window on the staging
    /// workspace (#126): reported, under `--json` as `unresolved_launches`,
    /// whether or not anything else was.
    unresolved_launches: Vec<run::UnresolvedLaunch>,
}

impl PartialProgress {
    fn is_empty(&self) -> bool {
        self.container_ids.is_empty() && self.containers.is_empty()
    }
}

/// `fail()`, plus what `run_steps()`'s `--rollback-on-error` managed to close
/// before giving up (see `RollbackOutcome`; both of its lists are empty
/// outside that path) and whatever earlier steps had already completed (see
/// `PartialProgress`).
///
/// Under `--json`, a launch's rejected windows (`RunError.unmatched_windows`)
/// are reported as `unmatched_windows`, omitted when empty like the other
/// optional fields. Plain output needs nothing extra: the message already
/// names them.
///
/// `RunError.container_id` — the container the run had already resolved when
/// a later action failed — is reported as `container_id` under `--json` and
/// as its own stderr line in plain output, and omitted when the failure came
/// before anything was resolved. Plain output can't put it on stdout, which a
/// failed run leaves empty so `id="$(sway-launch ...)"` never captures half a
/// result. "Was the target", not "is still open": the failure may itself be
/// that the container closed.
fn fail_with_rollback(
    json: bool,
    error: &sway_launch::RunError,
    rollback: &RollbackOutcome,
    progress: &PartialProgress,
) -> ! {
    let message = &error.message;
    if json {
        let mut value =
            serde_json::json!({ "error": message, "rolled_back": rollback.rolled_back });
        if !error.unmatched_windows.is_empty() {
            value["unmatched_windows"] = unmatched_windows_json(&error.unmatched_windows);
        }
        if let Some(container_id) = error.container_id {
            value["container_id"] = serde_json::json!(container_id);
        }
        // Omitted rather than reported as empty, so an ordinary failed
        // rollback-free run's error object keeps exactly the shape it always
        // had — a caller only ever sees this field when there is genuinely
        // something it needs to check.
        if !rollback.failed.is_empty() {
            value["rollback_failed"] = serde_json::json!(rollback.failed);
        }
        // Omitted entirely rather than reported as empty for a single
        // invocation, where the concept doesn't apply at all — an empty
        // array there would suggest a multi-step run that got nowhere.
        if !progress.is_empty() {
            value["container_ids"] = serde_json::json!(progress.container_ids);
            value["containers"] = serde_json::json!(progress.containers);
        }
        if !progress.unresolved_launches.is_empty() {
            value["unresolved_launches"] = progress
                .unresolved_launches
                .iter()
                .map(run::UnresolvedLaunch::json)
                .collect();
        }
        diagnostic!("{}", value);
    } else {
        // Ahead of the message rather than after it, so the error itself
        // stays the last line for a single invocation, as it always was.
        if let Some(container_id) = error.container_id {
            diagnostic!("container {} was the target when this failed", container_id);
        }
        // A message can quote file content — a TOML parse error echoes the
        // line it failed on — so it's escaped before reaching the terminal.
        // `--json` needs nothing: JSON already escapes control characters.
        diagnostic!(
            "{}{}",
            stderr_label(Severity::Error),
            sway_launch::escape_control_characters(message, true)
        );
        if !rollback.rolled_back.is_empty() {
            diagnostic!(
                "rolled back {} container(s): {}",
                rollback.rolled_back.len(),
                join_ids(&rollback.rolled_back)
            );
        }
        // The per-failure reason was already reported by rollback() itself as
        // it happened; this is the summary line, so the count is visible
        // without re-reading the warnings above it.
        if !rollback.failed.is_empty() {
            diagnostic!(
                "failed to roll back {} container(s): {} — check whether they are still open",
                rollback.failed.len(),
                join_ids(&rollback.failed)
            );
        }
        for launch in &progress.unresolved_launches {
            diagnostic!(
                "{}{}",
                stderr_label(Severity::Warning),
                sway_launch::escape_control_characters(&launch.message(), true)
            );
        }
    }
    // An interrupted run ends the way the signal itself would have ended it,
    // so a calling shell sees the interruption.
    process::exit(sway_launch::interrupt::exit_status().unwrap_or(1));
}

/// The fields every `--json` description of a window starts with, so
/// `unmatched_windows` and `--list-windows` name a window the same way.
fn window_identity_json(identity: &sway_launch::WindowIdentity) -> serde_json::Value {
    serde_json::json!({
        "container_id": identity.container_id,
        "app_id": identity.app_id,
        "class": identity.class,
        "title": identity.title,
    })
}

fn unmatched_windows_json(windows: &[sway_launch::UnmatchedWindow]) -> serde_json::Value {
    windows
        .iter()
        .map(|window| {
            let mut value = window_identity_json(&window.identity);
            value["launched_by_this_run"] = serde_json::json!(window.launched_by_this_run);
            value
        })
        .collect()
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One step's `--dry-run` preview: its target description plus every
/// planned action's kind and command, in order. Shared representation for
/// both the direct-CLI path and `--layout`/`--template`'s
/// `run_steps_dry_run()`, so both print identically and share one JSON shape.
struct DryRunStep {
    target: String,
    actions: Vec<DryRunAction>,
}

/// One planned action in a `--dry-run` preview: the same `kind`/`command`
/// pair a real run's `--json` reports (see `action_record_json()`), so a
/// preview and a run can be compared record for record.
struct DryRunAction {
    kind: &'static str,
    command: String,
    /// Sent in one command with its batching neighbours (#32), so the plain
    /// preview shows them on one line.
    batched: bool,
}

/// A one-line description of what a `SwayLaunch`'s `target` would do,
/// deliberately never naming a container id — for `Target::Exec`/
/// `Target::Existing`, there isn't a real one yet (nothing has launched);
/// for `Target::ConId`, the id is already known, but showing it here while
/// every action line below stays id-free (see
/// `SwayAction::sway_command_verb()`) would be an inconsistent preview
/// format for one target mode only.
fn describe_target(sway_launch: &sway_launch::SwayLaunch) -> String {
    match &sway_launch.target {
        sway_launch::Target::Exec { command, .. } => match &sway_launch.launch_setup {
            Some(setup) => format!("launch {} && {}", setup, command),
            None => format!("launch {}", command),
        },
        sway_launch::Target::ConId(_) => "target existing container".to_string(),
        sway_launch::Target::Existing { criteria, wait } => format!(
            "{} existing window ({})",
            if *wait { "wait for" } else { "target" },
            criteria.describe()
        ),
    }
}

fn dry_run_step(sway_launch: &sway_launch::SwayLaunch) -> DryRunStep {
    DryRunStep {
        target: describe_target(sway_launch),
        actions: sway_launch
            .build_actions_for_preview()
            .iter()
            .map(|action| DryRunAction {
                kind: action.kind_name(),
                command: action.sway_command_verb(),
                batched: action.joins_batch(),
            })
            .collect(),
    }
}

/// Prints every `DryRunStep` as a continuously-numbered plain-text list
/// (target line included as one of the numbered items), or as a single
/// `{"steps": [{"target": ..., "actions": [{"kind": ..., "command": ...},
/// ...]}, ...]}` object under `--json`. `setup` is the Sway commands a
/// `--layout`/`--template` run sends before its first step (see
/// `sway_launch::BuildLocation`): numbered first in plain output, and a
/// `"setup"` list under `--json`, present only when there are any.
fn print_dry_run_steps(setup: &[String], steps: &[DryRunStep], json: bool) {
    if json {
        let steps: Vec<_> = steps
            .iter()
            .map(|step| {
                let actions: Vec<_> = step
                    .actions
                    .iter()
                    .map(|action| {
                        serde_json::json!({ "kind": action.kind, "command": action.command })
                    })
                    .collect();
                serde_json::json!({ "target": step.target, "actions": actions })
            })
            .collect();
        let mut preview = serde_json::json!({ "steps": steps });
        if !setup.is_empty() {
            preview["setup"] = serde_json::json!(setup);
        }
        print_line(&preview.to_string());
        return;
    }

    for line in dry_run_plain_lines(setup, steps) {
        print_line(&line);
    }
}

/// `print_dry_run_steps()`'s plain-text lines: one per setup command, step
/// target and command sent, so actions a real run sends together as one
/// command (#32) share a line, joined the way Sway receives them. Control
/// and bidirectional formatting characters in a command, mark or name are
/// escaped (see `sway_launch::escape_control_characters()`), so a layout file
/// can't use a carriage return, a terminal escape or a right-to-left override
/// to make the preview show something other than what it will run.
fn dry_run_plain_lines(setup: &[String], steps: &[DryRunStep]) -> Vec<String> {
    let mut texts: Vec<String> = setup.to_vec();
    for step in steps {
        texts.push(step.target.clone());
        let mut batch: Vec<&str> = Vec::new();
        for action in &step.actions {
            if action.batched {
                batch.push(&action.command);
                continue;
            }
            if !batch.is_empty() {
                texts.push(std::mem::take(&mut batch).join(", "));
            }
            texts.push(action.command.clone());
        }
        if !batch.is_empty() {
            texts.push(batch.join(", "));
        }
    }
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            format!(
                "{}. {}",
                index + 1,
                sway_launch::escape_control_characters(text, false)
            )
        })
        .collect()
}

/// Whether a `--layout`/`--template`/`--bindings` value is `-`, meaning
/// "read stdin", so a generated file can be piped in without a temp file.
fn is_stdin(path: &Path) -> bool {
    path.as_os_str() == "-"
}

/// How errors and `--validate` name an input: `<stdin>` for `-`, otherwise
/// the path as given.
fn input_label(path: &Path) -> String {
    if is_stdin(path) {
        "<stdin>".to_string()
    } else {
        path.display().to_string()
    }
}

/// Reads a `--layout`/`--template`/`--bindings` input: stdin for `-`,
/// otherwise the file.
fn read_input(path: &Path) -> std::io::Result<String> {
    if is_stdin(path) {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(path)
    }
}

/// Reads and parses a `--layout` file, then hands its steps to `run_steps()`.
fn run_layout(path: &Path, args: &Args) -> ! {
    let contents = match read_input(path) {
        Ok(contents) => contents,
        Err(error) => fail(args.json, &format!("{}: {}", input_label(path), error)),
    };

    let parsed_layout = match layout::parse(&contents) {
        Ok(parsed_layout) => parsed_layout,
        Err(error) => fail(args.json, &format!("{}: {}", input_label(path), error)),
    };

    let build = sway_launch::BuildLocation {
        workspace: on_workspace(args),
        output: args.on_output.as_deref(),
    };
    run_steps(
        &parsed_layout.step,
        build,
        args.concurrent || parsed_layout.concurrent,
        &run_lock_source(path, &contents),
        args,
    );
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
///
/// `flag` names the option the argument came from (`--template` or
/// `--show-template`), used only to word the hint a missing `<name>.toml`
/// gets when `<name>` is a built-in — see `builtin_name_hint()`.
fn resolve_template_contents(template_arg: &Path, flag: &str) -> Result<String, String> {
    // template_arg.extension() is checked via a direct suffix match on the
    // full string, not Path::extension() — extension() returns None for a
    // filename that's only an extension with nothing before it (e.g. a file
    // literally named ".toml"), which would otherwise be misclassified as a
    // built-in-name lookup instead of a file read.
    let is_file = template_arg
        .to_str()
        .is_some_and(|value| value.ends_with(".toml"));

    if is_stdin(template_arg) {
        read_input(template_arg).map_err(|error| format!("<stdin>: {}", error))
    } else if is_file {
        std::fs::read_to_string(template_arg).map_err(|error| {
            let hint = if error.kind() == std::io::ErrorKind::NotFound {
                builtin_name_hint(template_arg, flag)
            } else {
                String::new()
            };
            format!("{}: {}{}", template_arg.display(), error, hint)
        })
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

/// The hint appended to a missing `.toml` file's error when the value is a
/// bare filename whose stem names a built-in template — writing a built-in's
/// name with the extension its repo file carries is an easy mistake, and the
/// bare OS error alone doesn't say the name itself was right. Only wording:
/// nothing resolves differently, so a `.toml` value still never falls back to
/// a built-in. Empty when the value has a directory component or no built-in
/// matches.
fn builtin_name_hint(template_arg: &Path, flag: &str) -> String {
    let has_directory = template_arg
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    let name = template_arg
        .to_str()
        .and_then(|value| value.strip_suffix(".toml"))
        .filter(|name| !name.is_empty());
    match name {
        Some(name) if !has_directory && template::builtin(name).is_some() => format!(
            "; there is a built-in template {:?}, use {} {}",
            name, flag, name
        ),
        _ => String::new(),
    }
}

/// The `--list-templates` standalone mode: prints every built-in
/// `--template` name with its category, one-line description, and slot info,
/// or (with `--json`) the same as a `{"templates": [...]}` array. Doesn't
/// touch Sway IPC, so it's checked and short-circuits right after
/// `--completions`, before the `--layout`/`--template` dispatch.
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
                    "params": template.params,
                })
            })
            .collect();
        print_line(&serde_json::json!({ "templates": templates }).to_string());
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
        let params = if template.params.is_empty() {
            String::new()
        } else {
            let params: Vec<String> = template
                .params
                .iter()
                .map(|(name, default)| format!("{}={}", name, default))
                .collect();
            format!("; params: {}", params.join(", "))
        };
        print_line(&format!(
            "{:<name_width$}  {:<category_width$}  {} ({} slot{}: {}{})",
            template.name,
            template.category,
            template.description,
            template.slots,
            if template.slots == 1 { "" } else { "s" },
            template.slot_names.join(", "),
            params
        ));
    }
}

/// The `--list-windows` standalone mode: one row per open window, narrowed by
/// whichever of `--app-id`/`--class`/`--mark-match` was given (clap allows at
/// most one) and by `--title`.
fn print_window_listing(args: &Args) {
    let criteria = sway_launch::WindowCriteria {
        matcher: args
            .app_id
            .as_deref()
            .map(sway_launch::WindowMatch::AppId)
            .or(args.class.as_deref().map(sway_launch::WindowMatch::Class))
            .or(args
                .mark_match
                .as_deref()
                .map(sway_launch::WindowMatch::Mark)),
        title: args.title.as_ref(),
    };
    let windows = match sway_launch::list_windows(criteria) {
        Ok(windows) => windows,
        Err(error) => fail(args.json, &error),
    };

    if args.json {
        print_line(&window_listing_json(&windows).to_string());
        return;
    }
    if windows.is_empty() {
        if criteria == sway_launch::WindowCriteria::default() {
            diagnostic!("no open windows");
        } else {
            diagnostic!("no open window matches {}", criteria);
        }
        return;
    }
    for line in window_listing_table(&windows) {
        print_line(&line);
    }
}

fn window_listing_json(windows: &[sway_launch::ListedWindow]) -> serde_json::Value {
    let windows: Vec<_> = windows
        .iter()
        .map(|window| {
            let mut value = window_identity_json(&window.identity);
            for (key, field) in [
                ("marks", serde_json::json!(window.marks)),
                ("workspace", serde_json::json!(window.workspace)),
                ("output", serde_json::json!(window.output)),
                ("scratchpad", serde_json::json!(window.scratchpad)),
                ("floating", serde_json::json!(window.floating)),
                ("focused", serde_json::json!(window.focused)),
                ("split_container", serde_json::json!(window.split_container)),
            ] {
                value[key] = field;
            }
            value
        })
        .collect();
    serde_json::json!({ "windows": windows })
}

/// Titles past this many characters are cut short in the table (never in
/// `--json`), so one long browser title doesn't wrap every row.
const LISTED_TITLE_LIMIT: usize = 50;

/// `--list-windows`' aligned table, header first. Every value a client chose
/// (app_id, class, title, marks, workspace name) is passed through
/// `escape_control_characters()`, since a title can hold anything.
fn window_listing_table(windows: &[sway_launch::ListedWindow]) -> Vec<String> {
    let text = |value: Option<&str>| {
        value.map_or("-".to_string(), |value| {
            sway_launch::escape_control_characters(value, false)
        })
    };
    let mut rows = vec![[
        "ID",
        "APP_ID",
        "CLASS",
        "WORKSPACE",
        "FLAGS",
        "MARKS",
        "TITLE",
    ]
    .map(String::from)];
    for window in windows {
        let workspace = if window.scratchpad {
            "(scratchpad)".to_string()
        } else {
            text(window.workspace.as_deref())
        };
        let flags: Vec<&str> = [
            (window.focused, "focused"),
            (window.floating, "floating"),
            (window.split_container, "split"),
        ]
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect();
        let marks = if window.marks.is_empty() {
            "-".to_string()
        } else {
            sway_launch::escape_control_characters(&window.marks.join(","), false)
        };
        let mut title = text(window.identity.title.as_deref());
        if title.chars().count() > LISTED_TITLE_LIMIT {
            title = title
                .chars()
                .take(LISTED_TITLE_LIMIT - 1)
                .collect::<String>()
                + "…";
        }
        rows.push([
            window.identity.container_id.to_string(),
            text(window.identity.app_id.as_deref()),
            text(window.identity.class.as_deref()),
            workspace,
            if flags.is_empty() {
                "-".to_string()
            } else {
                flags.join(",")
            },
            marks,
            title,
        ]);
    }

    let mut widths = [0; 7];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    rows.iter()
        .map(|row| {
            let mut line = String::new();
            for (index, cell) in row.iter().enumerate() {
                if index + 1 == row.len() {
                    line.push_str(cell);
                } else {
                    let padding = widths[index] - cell.chars().count();
                    line.push_str(cell);
                    line.push_str(&" ".repeat(padding + 2));
                }
            }
            line
        })
        .collect()
}

/// The `--show-template` standalone mode: resolves its argument the exact
/// same way `--template` itself does (via `resolve_template_contents()`),
/// then prints the raw TOML instead of running it. Doesn't touch Sway IPC,
/// so it's checked and short-circuits right after `--list-templates`, before
/// the `--layout`/`--template` dispatch.
fn print_template_contents(template_arg: &Path, json: bool) {
    let contents = match resolve_template_contents(template_arg, "--show-template") {
        Ok(contents) => contents,
        Err(error) => fail(json, &error),
    };

    if json {
        print_line(
            &serde_json::json!({ "name": template_arg.to_string_lossy(), "contents": contents })
                .to_string(),
        );
        return;
    }

    print_str(&contents);
}

/// Reads and parses a `--template` file, resolves it against `--bindings`/
/// `--apps` into ordinary layout steps via `template::resolve()`, then hands
/// them to `run_steps()` — the same execution path `--layout` uses, since a
/// resolved template is just a `Vec<layout::LayoutStep>`.
fn run_template(path: &Path, bindings_source: BindingsSource, args: &Args) -> ! {
    let contents = match resolve_template_contents(path, "--template") {
        Ok(contents) => contents,
        Err(error) => fail(args.json, &error),
    };

    let parsed_template = match template::parse(&contents) {
        Ok(parsed_template) => parsed_template,
        Err(error) => fail(args.json, &format!("{}: {}", input_label(path), error)),
    };

    let bindings = match bindings_source {
        BindingsSource::File(bindings_path) => {
            let contents = match read_input(bindings_path) {
                Ok(contents) => contents,
                Err(error) => fail(
                    args.json,
                    &format!("{}: {}", input_label(bindings_path), error),
                ),
            };
            match template::parse_bindings(&contents) {
                Ok(bindings) => bindings,
                Err(error) => fail(
                    args.json,
                    &format!("{}: {}", input_label(bindings_path), error),
                ),
            }
        }
        BindingsSource::Apps(apps) => match bindings_from_apps(&parsed_template, apps) {
            Ok(bindings) => bindings,
            Err(error) => fail(args.json, &format!("--apps: {}", error)),
        },
    };

    // A binding mistake is named after the file it's in (or --apps), not the
    // template: with a built-in, the template's name would read as a fault
    // in the shipped file rather than in the user's bindings.
    let resolved_steps = match template::resolve(
        &parsed_template,
        &bindings,
        &args.param,
        sway_launch::DirectoryCheck::for_offline(args.dry_run || args.validate),
    ) {
        Ok(resolved_steps) => resolved_steps,
        Err(error @ template::ResolveError::Bindings(_)) => {
            let bindings_label = match bindings_source {
                BindingsSource::File(bindings_path) => input_label(bindings_path),
                BindingsSource::Apps(_) => "--apps".to_string(),
            };
            fail(args.json, &format!("{}: {}", bindings_label, error))
        }
        Err(template::ResolveError::Param(error)) => {
            fail(args.json, &format!("--param: {}", error))
        }
        Err(error @ template::ResolveError::Step { .. }) => {
            fail(args.json, &format!("{}: {}", input_label(path), error))
        }
    };

    // Each of --on-workspace and --on-output wins over its own half of the
    // template's [layout] table, so either can be overridden alone.
    let build = sway_launch::BuildLocation {
        workspace: on_workspace(args).or(parsed_template.layout.workspace_target()),
        output: args
            .on_output
            .as_deref()
            .or(parsed_template.layout.output.as_deref()),
    };
    run_steps(
        &resolved_steps,
        build,
        args.concurrent || bindings.concurrent,
        &run_lock_source(path, &contents),
        args,
    );
}

/// `--on-workspace`/`--on-workspace-number` (clap allows at most one).
fn on_workspace(args: &Args) -> Option<sway_launch::WorkspaceTarget<'_>> {
    sway_launch::WorkspaceTarget::from_pair(args.on_workspace.as_deref(), args.on_workspace_number)
}

/// Builds `Bindings` from `--apps`' comma-separated command list, mapping
/// them 1:1 onto the template's `slot` steps in the order they appear.
/// Each binding launches its command unfiltered (no `app_id`/`class`
/// match), the same as a plain `sway-launch <command>` with no `-a`/`-c`.
fn bindings_from_apps(
    parsed_template: &template::Template,
    apps: &str,
) -> Result<template::Bindings, String> {
    let slots = template::slot_names(parsed_template);

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
        return Err(format!("empty application for slot {:?}", empty_slot));
    }

    let binding = slots
        .into_iter()
        .zip(apps)
        .map(|(slot, command)| template::Binding {
            slot: slot.to_string(),
            command: Some(command.to_string()),
            con_id: None,
            existing: false,
            wait: false,
            app_id: None,
            class: None,
            mark_match: None,
            title: None,
            workspace: None,
            workspace_number: None,
            output: None,
            timeout: None,
            wait_time: None,
            settle: None,
            cwd: None,
            env: Default::default(),
        })
        .collect();

    Ok(template::Bindings {
        binding,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;
