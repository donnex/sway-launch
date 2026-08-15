use clap::{error::ErrorKind, CommandFactory, Parser};
use std::path::{Path, PathBuf};
use std::{io, process, time};

mod layout;
mod sway_launch;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// app_id match. With --existing, matches an already-open window instead
    /// of the newly launched one
    #[clap(short, long, conflicts_with_all = ["class", "con_id"])]
    app_id: Option<String>,

    /// class match. With --existing, matches an already-open window instead
    /// of the newly launched one
    #[clap(short, long, conflicts_with = "con_id")]
    class: Option<String>,

    /// Act on an already-open window with this container id, instead of
    /// launching a new one
    #[clap(long, conflicts_with = "command")]
    con_id: Option<i64>,

    /// Act on an already-open window found via --app-id/--class, instead of
    /// launching a new one
    #[clap(long, conflicts_with_all = ["command", "con_id"])]
    existing: bool,

    /// Change split for new window
    #[clap(value_enum, short, long)]
    split: Option<sway_launch::Split>,

    /// Make new window floating
    #[clap(short, long)]
    floating: bool,

    /// Make new window fullscreen
    #[clap(long)]
    fullscreen: bool,

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
    #[clap(short, long, short = 'r')]
    new_row: bool,

    /// Move new window to workspace
    #[clap(long)]
    workspace: Option<String>,

    /// Set position on new window. Either "center" or "<x>,<y>" in pixels
    #[clap(long, value_parser = sway_launch::validate_position_argument)]
    position: Option<String>,

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
    #[clap(long, value_enum)]
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
        "command", "con_id", "existing", "app_id", "class", "split",
        "floating", "fullscreen", "mark", "new_column", "new_row",
        "workspace", "height", "width", "position", "debug_events",
    ])]
    layout: Option<PathBuf>,

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

    if let Some(layout_path) = &args.layout {
        run_layout(layout_path, &args);
    }

    let command = args.command.unwrap_or_default();
    let app_id_match = args.app_id.unwrap_or_default();
    let class_match = args.class.unwrap_or_default();

    if !args.debug_events && command.is_empty() && args.con_id.is_none() && !args.existing {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "Missing COMMAND, or one of --con-id/--existing",
            )
            .exit();
    }

    if args.existing && app_id_match.is_empty() && class_match.is_empty() {
        Args::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--existing requires --app-id or --class",
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
        split: args.split,
        floating: args.floating,
        fullscreen: args.fullscreen,
        mark: &args.mark.unwrap_or_default(),
        new_column: args.new_column,
        new_row: args.new_row,
        workspace: args.workspace.as_deref(),
        height: args.height.as_deref(),
        width: args.width.as_deref(),
        position: args.position.as_deref(),
        timeout: time::Duration::from_secs(args.timeout),
        wait_time: time::Duration::from_millis(args.wait_time),
        verbose: args.verbose,
    };

    if args.debug_events {
        match sway_launch.debug_events() {
            Ok(_) => process::exit(0),
            Err(error) => {
                eprintln!("{}", error);
                process::exit(1);
            }
        }
    }

    match sway_launch.run() {
        Ok(container_id) => {
            if args.json {
                println!("{}", serde_json::json!({ "container_id": container_id }));
            } else {
                println!("{}", container_id);
            }
        }
        Err(error) => {
            eprint!("{}", error);
            process::exit(1);
        }
    };
}

/// Runs every step of a `--layout` file sequentially, in the order they
/// appear, stopping at the first error — the same `set -eu`-chained
/// behavior as the shell-script examples this mode is meant to replace.
/// Exits the process directly (success or failure) rather than returning,
/// matching the rest of main()'s error-handling style.
fn run_layout(path: &Path, args: &Args) -> ! {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            eprintln!("{}: {}", path.display(), error);
            process::exit(1);
        }
    };

    let parsed_layout = match layout::parse(&contents) {
        Ok(parsed_layout) => parsed_layout,
        Err(error) => {
            eprintln!("{}: {}", path.display(), error);
            process::exit(1);
        }
    };

    let default_timeout = time::Duration::from_secs(args.timeout);
    let default_wait_time = time::Duration::from_millis(args.wait_time);
    let mut container_ids = Vec::new();

    for (index, step) in parsed_layout.step.iter().enumerate() {
        let sway_launch =
            match step.to_sway_launch(default_timeout, default_wait_time, args.verbose) {
                Ok(sway_launch) => sway_launch,
                Err(error) => {
                    eprintln!("step {}: {}", index + 1, error);
                    process::exit(1);
                }
            };

        match sway_launch.run() {
            Ok(container_id) => {
                if !args.json {
                    println!("{}", container_id);
                }
                container_ids.push(container_id);
            }
            Err(error) => {
                eprintln!("step {}: {}", index + 1, error);
                process::exit(1);
            }
        }
    }

    if args.json {
        println!("{}", serde_json::json!({ "container_ids": container_ids }));
    }

    process::exit(0);
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
            "kitty",
        ])
        .unwrap();
        assert_eq!(args.workspace, Some("2".to_string()));
        assert_eq!(args.position, Some("center".to_string()));
    }

    #[test]
    fn args_rejects_invalid_position() {
        let result = Args::try_parse_from(["sway-launch", "--position", "notvalid", "kitty"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_accepts_app_id_alone() {
        let args = Args::try_parse_from(["sway-launch", "-a", "kitty", "kitty"]).unwrap();
        assert_eq!(args.app_id, Some("kitty".to_string()));
        assert_eq!(args.class, None);
    }

    #[test]
    fn args_accepts_class_alone() {
        let args = Args::try_parse_from(["sway-launch", "-c", "Kitty", "kitty"]).unwrap();
        assert_eq!(args.class, Some("Kitty".to_string()));
        assert_eq!(args.app_id, None);
    }

    #[test]
    fn args_rejects_app_id_and_class_together() {
        // Regression test: combining -a/-c used to silently ignore -c
        // instead of being rejected.
        let result = Args::try_parse_from(["sway-launch", "-a", "kitty", "-c", "Kitty", "kitty"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_accepts_con_id_alone() {
        let args = Args::try_parse_from(["sway-launch", "--con-id", "42"]).unwrap();
        assert_eq!(args.con_id, Some(42));
        assert_eq!(args.command, None);
    }

    #[test]
    fn args_rejects_con_id_and_command_together() {
        let result = Args::try_parse_from(["sway-launch", "--con-id", "42", "kitty"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_rejects_con_id_and_app_id_together() {
        let result = Args::try_parse_from(["sway-launch", "--con-id", "42", "-a", "kitty"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_rejects_con_id_and_class_together() {
        let result = Args::try_parse_from(["sway-launch", "--con-id", "42", "-c", "Kitty"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_accepts_existing_with_app_id() {
        let args = Args::try_parse_from(["sway-launch", "--existing", "-a", "kitty"]).unwrap();
        assert!(args.existing);
        assert_eq!(args.app_id, Some("kitty".to_string()));
    }

    #[test]
    fn args_rejects_existing_and_command_together() {
        let result = Args::try_parse_from(["sway-launch", "--existing", "-a", "kitty", "kitty"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_rejects_existing_and_con_id_together() {
        let result = Args::try_parse_from(["sway-launch", "--existing", "--con-id", "42"]);
        assert!(result.is_err());
    }

    #[test]
    fn args_accepts_neither_app_id_nor_class() {
        let args = Args::try_parse_from(["sway-launch", "kitty"]).unwrap();
        assert_eq!(args.app_id, None);
        assert_eq!(args.class, None);
    }

    #[test]
    fn args_new_row_short_flag_is_r() {
        let args = Args::try_parse_from(["sway-launch", "-r", "kitty"]).unwrap();
        assert!(args.new_row);
    }

    #[test]
    fn args_rejects_invalid_height() {
        let result = Args::try_parse_from(["sway-launch", "--height", "notasize", "kitty"]);
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
            "kitty",
        ])
        .unwrap();
        assert_eq!(args.height, Some("80ppt".to_string()));
        assert_eq!(args.width, Some("1200px".to_string()));
    }

    #[test]
    fn args_defaults_timeout_and_wait_time() {
        let args = Args::try_parse_from(["sway-launch", "kitty"]).unwrap();
        assert_eq!(args.timeout, 5);
        assert_eq!(args.wait_time, 20);
    }

    #[test]
    fn args_json_defaults_to_false() {
        let args = Args::try_parse_from(["sway-launch", "kitty"]).unwrap();
        assert!(!args.json);
    }

    #[test]
    fn args_accepts_json_flag() {
        let args = Args::try_parse_from(["sway-launch", "--json", "kitty"]).unwrap();
        assert!(args.json);
    }

    #[test]
    fn json_result_serializes_container_id() {
        let value = serde_json::json!({ "container_id": 42 });
        assert_eq!(value.to_string(), "{\"container_id\":42}");
    }
}
