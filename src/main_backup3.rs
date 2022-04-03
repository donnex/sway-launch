// TODO
// - debug flag with debug output
// - test as async
// - change to os fork command instead?
// - test reuse connection?
use clap::{ArgEnum, Parser};
use std::{process, thread, time};
use swayipc::reply::{Event, WindowChange, WindowEvent};
use swayipc::{Connection, EventType, Fallible};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ArgEnum, Debug)]
enum Split {
    V,
    H,
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Timeout in seconds
    #[clap(short, long, default_value_t = 5)]
    timeout: u64,

    /// app_id match
    #[clap(short, long)]
    app_id: Option<String>,

    /// class match
    #[clap(short, long)]
    class: Option<String>,

    /// Change split for new window
    #[clap(arg_enum, short, long)]
    split: Option<Split>,

    /// Verbose output
    #[clap(short, long)]
    verbose: bool,

    /// Command to execute
    command: String,
}

fn main() -> Fallible<()> {
    let subs = [EventType::Window];

    let args = Args::parse();
    let timeout = time::Duration::from_secs(args.timeout);
    let app_id_match = args.app_id.unwrap_or_default();
    let class_match = args.class.unwrap_or_default();
    let split = args.split;
    let verbose = args.verbose;

    // Use Sway exec to runn supplied argument command
    match run_sway_command(&String::from(format!("exec {}", args.command))) {
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
        if time::Instant::now() - start_time > timeout {
            eprintln!("Error: {} sec timeout reached", timeout.as_secs());
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

        // Supplied argument app_id_match is not empty therefore
        // check if current window event matches app_id. If not
        // continue to the next window event.
        if !app_id_match.is_empty() && !check_app_id_window_match(&window, &app_id_match) {
            continue;
        }

        // Supplied argument class_match is not empty therefore
        // check if current window event matches class. If not
        // continue to the next window event.
        if !class_match.is_empty() && !check_class_window_match(&window, &class_match) {
            continue;
        }

        // Run split on the newly created window when set
        if split.is_some() {
            set_split_on_container(window.container.id, args.split.unwrap());
        }

        // Print container_id and break the event loop to exit the program
        println!("{}", window.container.id);
        break;
    }

    Ok(())
}

fn print_verbose(verbose: &bool, message: String) {
    match verbose {
        false => return,
        true => println!("{}", message),
    }
}

fn run_sway_command(command: &String) -> Result<(), String> {
    let mut connection = match Connection::new() {
        Ok(connection) => connection,
        Err(error) => return Err(format!("{}", error)),
    };

    let outcomes = match connection.run_command(command) {
        Ok(outcomes) => outcomes,
        Err(error) => return Err(format!("{}", error)),
    };

    for outcome in outcomes {
        match outcome.success {
            true => return Ok(()),
            false => return Err(outcome.error.unwrap_or_default()),
        }
    }

    Ok(())
}

fn check_app_id_window_match(window: &WindowEvent, app_id_match: &String) -> bool {
    let app_id = match window.container.app_id.as_ref().ok_or(()) {
        Ok(app_id) => app_id,
        Err(_) => return false,
    };

    if app_id_match == app_id {
        println!("APP_ID MATCH: {}", app_id);
        return true;
    }

    println!("APP_ID NO MATCH: {}", app_id);
    false
}

fn check_class_window_match(window: &WindowEvent, class_match: &String) -> bool {
    let window_properties = match window.container.window_properties.as_ref().ok_or(()) {
        Ok(window_properties) => window_properties,
        Err(_) => return false,
    };

    let class = match window_properties.class.as_ref().ok_or(()) {
        Ok(class) => class,
        Err(_) => return false,
    };

    if class_match == class {
        println!("CLASS MATCH: {}", class);
        return true;
    }

    println!("CLASS NO MATCH: {}", class);
    false
}

fn set_split_on_container(container_id: i64, split: Split) {
    let split = match split {
        Split::H => "splith",
        Split::V => "splitv",
    };
    let split_command = &format!("[con_id={}] {}", container_id, split).to_string();

    // Sleep a short amount of time to allow other IPC clients
    // that changes split to run.
    thread::sleep(time::Duration::from_millis(5));

    match run_sway_command(split_command) {
        Ok(()) => (),
        Err(error) => {
            eprintln!("Error: {}", error);
            process::exit(1);
        }
    }
}
