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
        // dbg!(&event);

        // match event? {
        //     Event::Window(w) => println!(
        //         "{}",
        //         w.container.name.unwrap_or_else(|| "unnamed".to_owned())
        //     ),
        //     _ => unreachable!(),
        // }
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

        // let class = window.container.window_properties.ok_or("");
        // match window.container.window_properties {
        //     WindowProperties => let class = dbg!(window.container.window_properties?);
        //     _ if !class_match.is_empty() => continue;
        // }
        // let test = dbg!(window.container.window_properties.is_some());
        // let test = dbg!(window.container.window_properties.ok_or("NOT OK"));
        // let class = match window.container.window_properties.ok_or(0) {
        //     Ok(v) => v.class.unwrap_or_default(),
        //     Err(_) => String::default(),
        // };

        // if !class.is_empty() {
        //     dbg!(class);
        // }
        // match window.container.window_properties {
        // WindowProperties => {
        //     println!("TEST");
        // }
        // Some(test) => {
        //     dbg!(test.class.unwrap_or_default());
        // }
        // _ if !class_match.is_empty() => continue,
        // _ => continue,
        // }
        // match window.container.window_properties {
        //     _ => continue,
        // }

        // let app_id = dbg!(app_id);
        // let app_id_match = dbg!(app_id_match);

        if !app_id.is_empty() && app_id == app_id_match {
            println!("APP_ID MATCH: {}", app_id);
            break;
        }
        if !class.is_empty() && class == class_match {
            println!("CLASS MATCH: {}", class);
            break;
        }

        // match app_id {
        //     // "kitty" => {
        //     //     println!("{}", app_id)
        //     // }
        //     String(app_id_match) => {
        //         println!("{}", app_id)
        //     }
        //     _ => unreachable!(),
        // }

        // break;
    }

    Ok(())
}
