use clap::ArgEnum;
use std::{fmt, thread, time, vec};
use swayipc::{Connection, Event, EventStream, EventType, WindowChange, WindowEvent};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ArgEnum, Debug)]
pub enum Split {
    V,
    H,
}

impl fmt::Display for Split {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Split::V => write!(f, "Vertical"),
            Split::H => write!(f, "Horizontal"),
        }
    }
}

enum WindowEventMatch {
    WindowAppId,
    WindowClass,
    NewWindowMatchWithoutCheck,
    WindowContainerIdMatch,
}

impl fmt::Display for WindowEventMatch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowEventMatch::WindowAppId => {
                write!(f, "Window app_id match")
            }
            WindowEventMatch::WindowClass => {
                write!(f, "Window class match")
            }
            WindowEventMatch::NewWindowMatchWithoutCheck => {
                write!(f, "New window without app_id or class check")
            }
            WindowEventMatch::WindowContainerIdMatch => {
                write!(f, "Window container id match")
            }
        }
    }
}

enum WindowEventMatchError {
    EventChangeTypeMismatch,
    WindowAppIdMismatch,
    WindowClassMismatch,
    NoMatchingEvent,
}

impl fmt::Display for WindowEventMatchError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WindowEventMatchError::EventChangeTypeMismatch => {
                write!(f, "Event does not match action event matches")
            }
            WindowEventMatchError::WindowAppIdMismatch => {
                write!(f, "app_id mismatch")
            }
            WindowEventMatchError::WindowClassMismatch => {
                write!(f, "class mismatch")
            }
            WindowEventMatchError::NoMatchingEvent => {
                write!(f, "No matching event")
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum SwayAction<'a> {
    Exec {
        command: &'a str,
        app_id_match: &'a str,
        class_match: &'a str,
        verbose: bool,
        timeout: time::Duration,
    },
    Split {
        container_id: i64,
        split: Split,
        verbose: bool,
        wait_time: time::Duration,
    },
    Floating {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    NewColumn {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    NewRow {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    Mark {
        container_id: i64,
        mark: &'a str,
        verbose: bool,
        timeout: time::Duration,
    },
    Height {
        container_id: i64,
        height: &'a str,
        verbose: bool,
        wait_time: time::Duration,
    },
    Width {
        container_id: i64,
        width: &'a str,
        verbose: bool,
        wait_time: time::Duration,
    },
}

impl fmt::Display for SwayAction<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SwayAction::Exec {
                command,
                app_id_match,
                class_match,
                ..
            } => {
                write!(
                    f,
                    "Exec \"{}\" (app_id_match: \"{}\") (class_match: \"{}\")",
                    command, app_id_match, class_match
                )
            }
            SwayAction::Split {
                container_id,
                split,
                ..
            } => {
                write!(
                    f,
                    "Split (container id: {}) (split: {})",
                    container_id, split
                )
            }
            SwayAction::Floating { container_id, .. } => {
                write!(f, "Floating (container_id: {})", container_id)
            }
            SwayAction::NewColumn { container_id, .. } => {
                write!(f, "New column (container_id: {})", container_id)
            }
            SwayAction::NewRow { container_id, .. } => {
                write!(f, "New row (container_id: {})", container_id)
            }
            SwayAction::Mark {
                container_id, mark, ..
            } => {
                write!(f, "Mark (container id: {}) (mark: {})", container_id, mark)
            }
            SwayAction::Height {
                container_id,
                height,
                ..
            } => {
                write!(
                    f,
                    "Height (container id: {}) (height: {})",
                    container_id, height
                )
            }
            SwayAction::Width {
                container_id,
                width,
                ..
            } => {
                write!(
                    f,
                    "Width (container id: {}) (width: {})",
                    container_id, width
                )
            }
        }
    }
}

impl SwayAction<'_> {
    fn sway_command(&self) -> String {
        match self {
            SwayAction::Exec { command, .. } => format!("exec {}", command),
            SwayAction::Floating { container_id, .. } => {
                format!("[con_id={}] floating enable", container_id)
            }
            SwayAction::Split {
                container_id,
                split,
                ..
            } => {
                if split == &Split::V {
                    return format!("[con_id={}] splitv", container_id);
                } else if split == &Split::H {
                    return format!("[con_id={}] splith", container_id);
                } else {
                    unreachable!();
                }
            }
            SwayAction::Mark {
                container_id, mark, ..
            } => {
                format!("[con_id={}] mark {}", container_id, mark)
            }
            SwayAction::NewColumn { container_id, .. } => {
                format!("[con_id={}] move right", container_id)
            }
            SwayAction::NewRow { container_id, .. } => {
                format!("[con_id={}] move down", container_id)
            }
            SwayAction::Height {
                container_id,
                height,
                ..
            } => {
                format!("[con_id={}] resize set height {}", container_id, height)
            }
            SwayAction::Width {
                container_id,
                width,
                ..
            } => {
                format!("[con_id={}] resize set width {}", container_id, width)
            }
        }
    }

    fn verbose(self) -> bool {
        match self {
            SwayAction::Exec { verbose, .. }
            | SwayAction::Split { verbose, .. }
            | SwayAction::Floating { verbose, .. }
            | SwayAction::NewColumn { verbose, .. }
            | SwayAction::NewRow { verbose, .. }
            | SwayAction::Mark { verbose, .. }
            | SwayAction::Height { verbose, .. }
            | SwayAction::Width { verbose, .. } => verbose,
        }
    }

    fn timeout(self) -> time::Duration {
        match self {
            SwayAction::Exec { timeout, .. }
            | SwayAction::Floating { timeout, .. }
            | SwayAction::NewColumn { timeout, .. }
            | SwayAction::NewRow { timeout, .. }
            | SwayAction::Mark { timeout, .. } => timeout,
            _ => unreachable!(),
        }
    }

    fn wait_time(self) -> time::Duration {
        match self {
            SwayAction::Split { wait_time, .. }
            | SwayAction::Height { wait_time, .. }
            | SwayAction::Width { wait_time, .. } => wait_time,
            _ => unreachable!(),
        }
    }

    fn container_id(self) -> Option<i64> {
        match self {
            SwayAction::Split { container_id, .. }
            | SwayAction::Floating { container_id, .. }
            | SwayAction::NewColumn { container_id, .. }
            | SwayAction::NewRow { container_id, .. }
            | SwayAction::Mark { container_id, .. }
            | SwayAction::Height { container_id, .. }
            | SwayAction::Width { container_id, .. } => Some(container_id),
            SwayAction::Exec { .. } => None,
        }
    }

    fn matching_window_change_events(&self) -> Option<Vec<WindowChange>> {
        match self {
            SwayAction::Exec { .. } => Some(vec![WindowChange::New, WindowChange::Move]),
            SwayAction::Floating { .. } => Some(vec![WindowChange::Floating]),
            SwayAction::NewColumn { .. } | SwayAction::NewRow { .. } => {
                Some(vec![WindowChange::Move])
            }
            SwayAction::Mark { .. } => Some(vec![WindowChange::Mark]),
            // Some actions does not trigger a corresponding Sway IPC event
            SwayAction::Split { .. } | SwayAction::Height { .. } | SwayAction::Width { .. } => None,
        }
    }

    fn event_subscription(&self) -> Option<EventType> {
        match self {
            SwayAction::Exec { .. }
            | SwayAction::Floating { .. }
            | SwayAction::NewColumn { .. }
            | SwayAction::NewRow { .. }
            | SwayAction::Mark { .. } => Some(EventType::Window),
            SwayAction::Split { .. } | SwayAction::Height { .. } | SwayAction::Width { .. } => None,
        }
    }

    fn run(&self) -> Result<i64, String> {
        if self.verbose() {
            println!("Sway action: {}", self);
        }

        // If action has a corresponding change event run action and wait for event.
        // If not just run the action and sleep for set wait time.
        match self.matching_window_change_events() {
            Some(_) => self.run_wait_matching_events(),
            None => self.run_wait_time(),
        }
    }

    fn run_wait_time(&self) -> Result<i64, String> {
        let wait_time = self.wait_time();

        if self.verbose() {
            println!(
                "No matching event types for action. Will run Sway command and wait {} ms.",
                wait_time.as_millis()
            );
        }

        // Wait a few ms before and after run of Sway command.
        // Before to allow other running IPC clients to finish their commands
        // After to allow the actual command to finish before running the next
        // action.
        thread::sleep(wait_time);

        let sway_command = self.sway_command();
        if self.verbose() {
            println!("Sway command: {}", sway_command);
        }

        run_sway_command(&sway_command)?;
        thread::sleep(wait_time);

        Ok(self.container_id().unwrap())
    }

    fn run_wait_matching_events(&self) -> Result<i64, String> {
        // Start time for timeout check
        let start_time = time::Instant::now();

        // Setup event loop
        let subscription = self.event_subscription().unwrap();
        let event_loop = self::event_loop(&[subscription])?;

        // Run Sway command for action and wait for a matching event in the
        // event loop
        let sway_command = self.sway_command();
        if self.verbose() {
            println!("Sway command: {}", sway_command);
        }
        run_sway_command(&sway_command)?;
        for event in event_loop {
            // Timeout check. This will only run on every new event but that's
            // okay for now.
            if time::Instant::now() - start_time > self.timeout() {
                return Err(format!("{} sec timeout reached", self.timeout().as_secs()));
            }

            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            // Continue to next event when event isn't a window event
            let window = match event {
                Event::Window(window) => window,
                _ => continue,
            };

            // Check if window event matches the current action
            match self.matches_window_event(&window) {
                // Event match, return container id
                Ok(result) => {
                    if self.verbose() {
                        println!(
                            "Event match: {:?} container id {} ({})",
                            &window.change, &window.container.id, result
                        );
                    }

                    return Ok(window.container.id);
                }
                // No event match
                Err(error_result) => {
                    if self.verbose() {
                        println!(
                            "Event mismatch: {:?} container id {} ({})",
                            &window.change, &window.container.id, error_result
                        );
                    }
                }
            }
        }

        Err("No matching event".to_string())
    }

    fn matches_window_event(
        &self,
        window: &WindowEvent,
    ) -> Result<WindowEventMatch, WindowEventMatchError> {
        let matching_window_change_events = self.matching_window_change_events().unwrap();

        // Window event change type mismatch
        if !matching_window_change_events.contains(&window.change) {
            return Err(WindowEventMatchError::EventChangeTypeMismatch);
        }

        // Check if window matches action.
        // For exec compare app_id or class when set.
        // For all other actions compare window container id for match.
        match self {
            // Action is exec (new window). Check if window
            // is a match and return container id.
            SwayAction::Exec {
                app_id_match,
                class_match,
                ..
            } => {
                // app_id_match is set
                if !app_id_match.is_empty() {
                    match window_app_id_match(window, app_id_match) {
                        true => return Ok(WindowEventMatch::WindowAppId),
                        false => return Err(WindowEventMatchError::WindowAppIdMismatch),
                    }
                }

                // class_match is set
                if !class_match.is_empty() {
                    match window_class_match(window, class_match) {
                        true => return Ok(WindowEventMatch::WindowClass),
                        false => return Err(WindowEventMatchError::WindowClassMismatch),
                    }
                }

                // When no app_id_match or class_match set return Ok()
                // and consider the new window a match.
                return Ok(WindowEventMatch::NewWindowMatchWithoutCheck);
            }
            // All other actions check that container id of the event
            // matches our container id set on event.
            _ => {
                if self.container_id().unwrap() == window.container.id {
                    return Ok(WindowEventMatch::WindowContainerIdMatch);
                }
            }
        }

        Err(WindowEventMatchError::NoMatchingEvent)
    }
}

fn new_connection() -> Result<Connection, String> {
    match Connection::new() {
        Ok(connection) => Ok(connection),
        Err(error) => Err(error.to_string()),
    }
}

fn event_loop(subscriptions: &[EventType]) -> Result<EventStream, String> {
    match self::new_connection()?.subscribe(subscriptions) {
        Ok(event_iterator) => Ok(event_iterator),
        Err(error) => Err(error.to_string()),
    }
}

fn run_sway_command(command: &str) -> Result<(), String> {
    let outcomes = match self::new_connection()?.run_command(command) {
        Ok(outcomes) => outcomes,
        Err(error) => return Err(error.to_string()),
    };

    if let Some(outcome) = outcomes.into_iter().next() {
        match outcome {
            Ok(()) => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }

    Err(format!("{} command failed", command))
}

fn window_app_id_match(window: &WindowEvent, app_id_match: &str) -> bool {
    let window_app_id = match window.container.app_id.as_ref().ok_or(()) {
        Ok(app_id) => app_id,
        Err(_) => return false,
    };

    matches!(window_app_id, _ if window_app_id == app_id_match)
}

fn window_class_match(window: &WindowEvent, class_match: &str) -> bool {
    let window_properties = match window.container.window_properties.as_ref().ok_or(()) {
        Ok(window_properties) => window_properties,
        Err(_) => return false,
    };

    let window_class = match window_properties.class.as_ref().ok_or(()) {
        Ok(class) => class,
        Err(_) => return false,
    };

    matches!(window_class, _ if window_class == class_match)
}

pub struct SwayLaunch<'a> {
    pub command: &'a str,

    pub app_id_match: &'a str,
    pub class_match: &'a str,

    pub split: Option<Split>,
    pub floating: bool,
    pub mark: &'a str,
    pub new_column: bool,
    pub new_row: bool,
    pub height: Option<&'a str>,
    pub width: Option<&'a str>,

    pub verbose: bool,
    pub timeout: time::Duration,
    pub wait_time: time::Duration,
}

impl SwayLaunch<'_> {
    pub fn debug_events(&self) -> Result<(), String> {
        let subscriptions = [
            EventType::Workspace,
            EventType::Mode,
            EventType::Window,
            EventType::BarConfigUpdate,
            EventType::Binding,
            EventType::Shutdown,
            EventType::Tick,
            EventType::BarStateUpdate,
            EventType::Input,
        ];

        for (i, event) in self::event_loop(&subscriptions)?.enumerate() {
            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            println!("Event: {}", i);
            println!("{:?}\n", event);
        }

        Ok(())
    }

    pub fn run(&self) -> Result<i64, String> {
        // Run command by using exec
        let container_id = SwayAction::Exec {
            command: self.command,
            app_id_match: self.app_id_match,
            class_match: self.class_match,
            verbose: self.verbose,
            timeout: self.timeout,
        }
        .run()?;

        if self.verbose {
            println!("New window match container id: {}", container_id);
        }

        // Run actions on new window
        if self.new_column {
            SwayAction::NewColumn {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()?;
        }
        if self.new_row {
            SwayAction::NewRow {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()?;
        }
        if self.split.is_some() {
            SwayAction::Split {
                container_id,
                split: self.split.unwrap(),
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if self.floating {
            SwayAction::Floating {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()?;
        }
        if self.height.is_some() {
            SwayAction::Height {
                container_id,
                height: self.height.unwrap(),
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if self.width.is_some() {
            SwayAction::Width {
                container_id,
                width: self.width.unwrap(),
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if !self.mark.is_empty() {
            SwayAction::Mark {
                container_id,
                mark: self.mark,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()?;
        }

        Ok(container_id)
    }
}
