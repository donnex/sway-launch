use clap::Parser;
use std::{process, time};
use swayipc::reply::{Event, WindowChange};
use swayipc::{Connection, EventType, Fallible};

mod sway_start_wait;

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
    split: Option<sway_start_wait::Split>,

    /// Timeout in seconds
    #[clap(short, long, default_value_t = 5)]
    timeout: u64,

    /// Verbose output
    #[clap(short, long)]
    verbose: bool,

    /// Command to execute
    command: String,
}

fn main() -> Fallible<()> {
    let subs = [EventType::Window];

    let args = Args::parse();
    let sway_launch = sway_start_wait::SwayLaunch {
        app_id_match: &args.app_id.unwrap_or_default(),
        class_match: &args.class.unwrap_or_default(),
        split: args.split,
        timeout: time::Duration::from_secs(args.timeout),
        verbose: args.verbose,
        command: &args.command,
    };

    // Use Sway exec to runn supplied argument command
    match sway_launch.run_sway_command(&format!("exec {}", &sway_launch.command)) {
        Ok(()) => (),
        Err(error) => {
            eprintln!("Error: {}", error);
            process::exit(1);
        }
    }

    let start_time = time::Instant::now();
    for event in Connection::new()?.subscribe(&subs)? {
        // Timeout check. This will only run on every new event but that's
        // okay for now.
        if time::Instant::now() - start_time > sway_launch.timeout {
            eprintln!(
                "Error: {} sec timeout reached",
                sway_launch.timeout.as_secs()
            );
            process::exit(1);
        }

        // Handle event. Continue to next event when event isn't a WindowEvent
        // or not of type new or move
        let event = event;
        let window = match event? {
            Event::Window(window) => window,
            _ => continue,
        };

        match window.change {
            WindowChange::New | WindowChange::Move => (),
            _ => continue,
        }
        sway_launch.print_verbose(&format!("Window event id {}", window.container.id));

        // Supplied argument app_id_match is not empty therefore
        // check if current window event matches app_id. If not
        // continue to the next window event.
        if !sway_launch.app_id_match.is_empty()
            && !sway_launch.check_app_id_window_match(&window, &sway_launch.app_id_match)
        {
            continue;
        }

        // Supplied argument class_match is not empty therefore
        // check if current window event matches class. If not
        // continue to the next window event.
        if !sway_launch.class_match.is_empty()
            && !sway_launch.check_class_window_match(&window, &sway_launch.class_match)
        {
            continue;
        }

        // Run split on the newly created window when set
        if sway_launch.split.is_some() {
            sway_launch.set_split_on_container(window.container.id, args.split.unwrap());
        }

        // Print container_id and break the event loop to exit the program
        println!("{}", window.container.id);
        break;
    }

    Ok(())
}
