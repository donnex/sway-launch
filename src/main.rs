use clap::{CommandFactory, ErrorKind, Parser};
use regex::Regex;
use std::{process, time};

mod sway_launch;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// app_id match
    #[clap(short, long)]
    app_id: Option<String>,

    /// class match
    #[clap(short, long)]
    class: Option<String>,

    /// Change split for new window
    #[clap(arg_enum, short, long)]
    split: Option<sway_launch::Split>,

    /// Make new window floating
    #[clap(short, long)]
    floating: bool,

    /// Add mark to new window
    #[clap(short, long)]
    mark: Option<String>,

    /// Move window to new column
    #[clap(short, long)]
    new_column: bool,

    /// Set height on new window
    #[clap(long, parse(try_from_str=validate_size_argument))]
    height: Option<String>,

    /// Set width on new window
    #[clap(long, parse(try_from_str=validate_size_argument))]
    width: Option<String>,

    /// Move window to new row
    #[clap(short, long, short = 'r')]
    new_row: bool,

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

    // Validate that command is non empty when not running with --debug-events
    let command = args.command.unwrap_or_default();
    if !args.debug_events && command.is_empty() {
        Args::command()
            .error(ErrorKind::EmptyValue, "Missing COMMAND")
            .exit();
    }

    // Setup SwayLaunch
    let sway_launch = sway_launch::SwayLaunch {
        app_id_match: &args.app_id.unwrap_or_default(),
        class_match: &args.class.unwrap_or_default(),
        split: args.split,
        floating: args.floating,
        mark: &args.mark.unwrap_or_default(),
        new_column: args.new_column,
        new_row: args.new_row,
        height: args.height.as_deref(),
        width: args.width.as_deref(),
        timeout: time::Duration::from_secs(args.timeout),
        wait_time: time::Duration::from_millis(args.wait_time),
        verbose: args.verbose,
        command: &command,
    };

    // Run debug events and exit
    if args.debug_events {
        match sway_launch.debug_events() {
            Ok(_) => process::exit(0),
            Err(error) => {
                eprintln!("{}", error);
                process::exit(1);
            }
        }
    }

    // Normal run
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
