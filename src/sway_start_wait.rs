use clap::ArgEnum;
use std::{process, thread, time};
use swayipc::reply::WindowEvent;
use swayipc::Connection;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ArgEnum, Debug)]
pub enum Split {
    V,
    H,
}

#[derive(Debug)]
pub struct SwayLaunch<'a> {
    pub app_id_match: &'a str,
    pub class_match: &'a str,
    pub split: Option<Split>,
    pub timeout: time::Duration,
    pub verbose: bool,
    pub command: &'a str,
}

impl SwayLaunch<'_> {
    pub fn print_verbose(&self, message: &str) {
        match self.verbose {
            false => return,
            true => eprintln!("{}", message),
        }
    }

    pub fn run_sway_command(&self, command: &str) -> Result<(), String> {
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

    pub fn check_app_id_window_match(&self, window: &WindowEvent, app_id_match: &str) -> bool {
        let app_id = match window.container.app_id.as_ref().ok_or(()) {
            Ok(app_id) => app_id,
            Err(_) => return false,
        };

        if app_id_match == app_id {
            self.print_verbose(&format!(
                "app_id match {} matches window app_id {}",
                app_id_match, app_id
            ));
            return true;
        }

        self.print_verbose(&format!(
            "app_id match {} does not match window app_id {}",
            app_id_match, app_id
        ));
        false
    }

    pub fn check_class_window_match(&self, window: &WindowEvent, class_match: &str) -> bool {
        let window_properties = match window.container.window_properties.as_ref().ok_or(()) {
            Ok(window_properties) => window_properties,
            Err(_) => return false,
        };

        let class = match window_properties.class.as_ref().ok_or(()) {
            Ok(class) => class,
            Err(_) => return false,
        };

        if class_match == class {
            self.print_verbose(&format!(
                "class match {} matches window class {}",
                class_match, class
            ));
            return true;
        }

        self.print_verbose(&format!(
            "class match {} does not match window class {}",
            class_match, class
        ));
        false
    }

    pub fn set_split_on_container(&self, container_id: i64, split: Split) {
        let split = match split {
            Split::H => "splith",
            Split::V => "splitv",
        };
        let split_command = &format!("[con_id={}] {}", container_id, split).to_string();

        // Sleep a short amount of time to allow other IPC clients
        // that changes split to run.
        thread::sleep(time::Duration::from_millis(5));

        match self.run_sway_command(split_command) {
            Ok(()) => (),
            Err(error) => {
                eprintln!("Error: {}", error);
                process::exit(1);
            }
        }
    }
}
