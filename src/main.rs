use clap::{error::ErrorKind, CommandFactory, Parser};
use regex::Regex;
use std::{process, time};

mod sway_launch;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// app_id match
    #[clap(short, long, conflicts_with = "class")]
    app_id: Option<String>,

    /// class match
    #[clap(short, long)]
    class: Option<String>,

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
    #[clap(long, value_parser = validate_size_argument)]
    height: Option<String>,

    /// Set width on new window
    #[clap(long, value_parser = validate_size_argument)]
    width: Option<String>,

    /// Move window to new row (move down)
    #[clap(short, long, short = 'r')]
    new_row: bool,

    /// Move new window to workspace
    #[clap(long)]
    workspace: Option<String>,

    /// Set position on new window. Either "center" or "<x>,<y>" in pixels
    #[clap(long, value_parser = validate_position_argument)]
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

    /// Verbose output
    #[clap(short, long)]
    verbose: bool,

    /// Command to execute
    command: Option<String>,
}

fn main() {
    let args = Args::parse();

    let command = args.command.unwrap_or_default();
    if !args.debug_events && command.is_empty() {
        Args::command()
            .error(ErrorKind::MissingRequiredArgument, "Missing COMMAND")
            .exit();
    }

    let sway_launch = sway_launch::SwayLaunch {
        app_id_match: &args.app_id.unwrap_or_default(),
        class_match: &args.class.unwrap_or_default(),
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
        command: &command,
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
        Ok(container_id) => println!("{}", container_id),
        Err(error) => {
            eprint!("{}", error);
            process::exit(1);
        }
    };
}

fn validate_size_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^\d+(px|ppt)$").unwrap();
    match re.is_match(value) {
        true => Ok(value.to_string()),
        false => {
            Err("Must be in format <HEIGHT>px|ppt. E.g. 300px/20ppt. ppt = percent".to_string())
        }
    }
}

fn validate_position_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^center$|^\d+,\d+$").unwrap();
    match re.is_match(value) {
        true => Ok(value.to_string()),
        false => {
            Err("Must be \"center\" or \"<X>,<Y>\" in pixels. E.g. center/100,200".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_size_argument_accepts_px() {
        assert_eq!(validate_size_argument("300px"), Ok("300px".to_string()));
    }

    #[test]
    fn validate_size_argument_accepts_ppt() {
        assert_eq!(validate_size_argument("20ppt"), Ok("20ppt".to_string()));
    }

    #[test]
    fn validate_size_argument_accepts_zero() {
        assert_eq!(validate_size_argument("0px"), Ok("0px".to_string()));
    }

    #[test]
    fn validate_size_argument_rejects_missing_unit() {
        assert!(validate_size_argument("300").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_unknown_unit() {
        assert!(validate_size_argument("300pixels").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_negative() {
        assert!(validate_size_argument("-5px").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_decimal() {
        assert!(validate_size_argument("3.5px").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_empty() {
        assert!(validate_size_argument("").is_err());
    }

    #[test]
    fn validate_size_argument_rejects_trailing_garbage() {
        assert!(validate_size_argument("300px ").is_err());
    }

    #[test]
    fn validate_position_argument_accepts_center() {
        assert_eq!(
            validate_position_argument("center"),
            Ok("center".to_string())
        );
    }

    #[test]
    fn validate_position_argument_accepts_coordinates() {
        assert_eq!(
            validate_position_argument("100,200"),
            Ok("100,200".to_string())
        );
    }

    #[test]
    fn validate_position_argument_rejects_missing_y() {
        assert!(validate_position_argument("100").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_negative() {
        assert!(validate_position_argument("-1,200").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_unknown_word() {
        assert!(validate_position_argument("middle").is_err());
    }

    #[test]
    fn validate_position_argument_rejects_empty() {
        assert!(validate_position_argument("").is_err());
    }

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
}
