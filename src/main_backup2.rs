// TODO
// - debug flag with debug output
// - test as async
use clap::Parser;
use std::process;
use swayipc::reply::{Event, WindowChange, WindowProperties};
use swayipc::{Connection, EventType, Fallible};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Timeout in seconds
    #[clap(short, long, default_value_t = 5)]
    timeout: u8,

    /// app_id match
    #[clap(short, long)]
    app_id: Option<String>,

    /// class match
    #[clap(short, long)]
    class: Option<String>,

    /// Command to execute
    command: String,
}

fn main() -> Fallible<()> {
    let subs = [
        // EventType::Workspace,
        // EventType::Input,
        // EventType::Tick,
        // EventType::Shutdown,
        // EventType::Mode,
        EventType::Window,
        // EventType::BarStateUpdate,
        // EventType::BarConfigUpdate,
        // EventType::Binding,
    ];
    // let tree = Connection::new()?.get_tree();
    // print!("{:?}", tree);
    // for event in Connection::new()?.subscribe(&subs)? {
    //     println!("{:?}\n", event?);
    // }
    let args = Args::parse();

    // for _ in 0..args.count {
    //     println!("Hello {}!", args.name)
    // }
    let args = dbg!(args);
    // dbg!(args.timeout);
    // dbg!(args.app_id);
    // dbg!(args.class);
    let app_id_match = args.app_id.unwrap_or_default();
    let class_match = args.class.unwrap_or_default();

    let mut connection = Connection::new()?;
    let command = format!("exec {}", args.command);
    for outcome in connection.run_command(command)? {
        // dbg!(outcome);
        if !outcome.success {
            eprintln!("failure '{:?}'", outcome.error);

            process::exit(1);
            // clap::Error("test");
        }
        //     println!("success");
        // }
    }

    // for event in Connection::new()?.subscribe(&subs)? {
    //     let event = match event {
    //         Ok(event) => event,
    //         Err(e) => return Err(e),
    //     };

    //     println!("{:?}\n", event);
    //     break;
    // }
    // println!("{}", event)
    // for event in Connection::new()?.subscribe([EventType::Window])? {
    // println!("{:?}", args.app_id.unwrap_or_default());
    // let app_id_match = args.app_id.unwrap_or_default();
    for event in Connection::new()?.subscribe(&subs)? {
        let event = event;

        let window = match event? {
            Event::Window(w) => w,
            _ => unreachable!(),
        };

        match window.change {
            WindowChange::New | WindowChange::Move => (),
            _ => continue,
        }

        let app_id = dbg!(window.container.app_id.unwrap_or_default());
        let class = dbg!(window
            .container
            .window_properties
            .and_then(|wp| wp.class)
            .unwrap_or_default());

        // Skip to next event when app_id_match is set but not matching app_id
        if !app_id_match.is_empty() && app_id_match != app_id {
            println!("APP_ID NO MATCH: {}", app_id);
            continue;
        }

        // Skip to next event when class_match is set but not matching class
        if !class_match.is_empty() && class_match != class {
            println!("CLASS NO MATCH: {}", class);
            continue;
        }

        // let window = window.container;

        // Matching app_id, class or new window with no match set
        println!("{:?}", &window);

        break;
    }

    Ok(())
}
