use clap::ValueEnum;
use regex::Regex;
use std::sync::mpsc;
use std::{fmt, thread, time};
use swayipc::{Connection, Event, EventStream, EventType, Node, WindowChange, WindowEvent};

#[derive(Copy, Clone, PartialEq, ValueEnum, serde::Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
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

#[derive(Copy, Clone, Debug)]
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
    Fullscreen {
        container_id: i64,
        verbose: bool,
        timeout: time::Duration,
    },
    NewColumn {
        container_id: i64,
        verbose: bool,
        wait_time: time::Duration,
    },
    NewRow {
        container_id: i64,
        verbose: bool,
        wait_time: time::Duration,
    },
    Workspace {
        container_id: i64,
        workspace: &'a str,
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
    Position {
        container_id: i64,
        position: &'a str,
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
            SwayAction::Fullscreen { container_id, .. } => {
                write!(f, "Fullscreen (container_id: {})", container_id)
            }
            SwayAction::NewColumn { container_id, .. } => {
                write!(f, "New column (container_id: {})", container_id)
            }
            SwayAction::NewRow { container_id, .. } => {
                write!(f, "New row (container_id: {})", container_id)
            }
            SwayAction::Workspace {
                container_id,
                workspace,
                ..
            } => {
                write!(
                    f,
                    "Workspace (container id: {}) (workspace: {})",
                    container_id, workspace
                )
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
            SwayAction::Position {
                container_id,
                position,
                ..
            } => {
                write!(
                    f,
                    "Position (container id: {}) (position: {})",
                    container_id, position
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
            SwayAction::Fullscreen { container_id, .. } => {
                format!("[con_id={}] fullscreen enable", container_id)
            }
            SwayAction::Split {
                container_id,
                split,
                ..
            } => match split {
                Split::V => format!("[con_id={}] splitv", container_id),
                Split::H => format!("[con_id={}] splith", container_id),
            },
            SwayAction::Mark {
                container_id, mark, ..
            } => {
                format!("[con_id={}] mark {}", container_id, quote_sway_string(mark))
            }
            SwayAction::NewColumn { container_id, .. } => {
                format!("[con_id={}] move right", container_id)
            }
            SwayAction::NewRow { container_id, .. } => {
                format!("[con_id={}] move down", container_id)
            }
            SwayAction::Workspace {
                container_id,
                workspace,
                ..
            } => {
                format!(
                    "[con_id={}] move workspace {}",
                    container_id,
                    quote_sway_string(workspace)
                )
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
            SwayAction::Position {
                container_id,
                position,
                ..
            } => {
                let position_command = match *position {
                    "center" => "center".to_string(),
                    coords => coords.replace(',', " "),
                };
                format!(
                    "[con_id={}] move position {}",
                    container_id, position_command
                )
            }
        }
    }

    fn verbose(self) -> bool {
        match self {
            SwayAction::Exec { verbose, .. }
            | SwayAction::Split { verbose, .. }
            | SwayAction::Floating { verbose, .. }
            | SwayAction::Fullscreen { verbose, .. }
            | SwayAction::NewColumn { verbose, .. }
            | SwayAction::NewRow { verbose, .. }
            | SwayAction::Workspace { verbose, .. }
            | SwayAction::Mark { verbose, .. }
            | SwayAction::Height { verbose, .. }
            | SwayAction::Width { verbose, .. }
            | SwayAction::Position { verbose, .. } => verbose,
        }
    }

    fn timeout(self) -> time::Duration {
        match self {
            SwayAction::Exec { timeout, .. }
            | SwayAction::Floating { timeout, .. }
            | SwayAction::Fullscreen { timeout, .. }
            | SwayAction::Workspace { timeout, .. }
            | SwayAction::Mark { timeout, .. } => timeout,
            _ => unreachable!(),
        }
    }

    fn wait_time(self) -> time::Duration {
        match self {
            SwayAction::Split { wait_time, .. }
            | SwayAction::NewColumn { wait_time, .. }
            | SwayAction::NewRow { wait_time, .. }
            | SwayAction::Height { wait_time, .. }
            | SwayAction::Width { wait_time, .. }
            | SwayAction::Position { wait_time, .. } => wait_time,
            _ => unreachable!(),
        }
    }

    fn container_id(self) -> Option<i64> {
        match self {
            SwayAction::Split { container_id, .. }
            | SwayAction::Floating { container_id, .. }
            | SwayAction::Fullscreen { container_id, .. }
            | SwayAction::NewColumn { container_id, .. }
            | SwayAction::NewRow { container_id, .. }
            | SwayAction::Workspace { container_id, .. }
            | SwayAction::Mark { container_id, .. }
            | SwayAction::Height { container_id, .. }
            | SwayAction::Width { container_id, .. }
            | SwayAction::Position { container_id, .. } => Some(container_id),
            SwayAction::Exec { .. } => None,
        }
    }

    fn matching_window_change_events(&self) -> Option<Vec<WindowChange>> {
        match self {
            // Only `New` identifies a window as the one just launched.
            // `Move` ("the view has been reparented in the tree") fires for
            // any window whenever Sway's tree is restructured, including
            // pre-existing windows unrelated to this exec — accepting it
            // here let sway-launch return the wrong container id.
            SwayAction::Exec { .. } => Some(vec![WindowChange::New]),
            SwayAction::Floating { .. } => Some(vec![WindowChange::Floating]),
            SwayAction::Fullscreen { .. } => Some(vec![WindowChange::FullscreenMode]),
            SwayAction::Workspace { .. } => Some(vec![WindowChange::Move]),
            SwayAction::Mark { .. } => Some(vec![WindowChange::Mark]),
            // NewColumn/NewRow ("move right"/"move down") were event-based
            // via WindowChange::Move too, until live-Sway testing showed
            // "move right" doesn't fire it when the window is already at
            // the tree's rightmost position (the ordinary two-window case)
            // — it silently no-ops instead, hanging until --timeout.
            // "move down" happened to work in that same test, but relies on
            // the identical unverified assumption for a different tree
            // shape, so both moved to the same wait-time pattern as
            // Split/Height/Width/Position rather than leaving one exposed.
            SwayAction::Split { .. }
            | SwayAction::NewColumn { .. }
            | SwayAction::NewRow { .. }
            | SwayAction::Height { .. }
            | SwayAction::Width { .. }
            | SwayAction::Position { .. } => None,
        }
    }

    fn event_subscription(&self) -> Option<EventType> {
        match self {
            SwayAction::Exec { .. }
            | SwayAction::Floating { .. }
            | SwayAction::Fullscreen { .. }
            | SwayAction::Workspace { .. }
            | SwayAction::Mark { .. } => Some(EventType::Window),
            SwayAction::Split { .. }
            | SwayAction::NewColumn { .. }
            | SwayAction::NewRow { .. }
            | SwayAction::Height { .. }
            | SwayAction::Width { .. }
            | SwayAction::Position { .. } => None,
        }
    }

    fn run(&self) -> Result<i64, String> {
        if self.verbose() {
            eprintln!("Sway action: {}", self);
        }

        match self.matching_window_change_events() {
            Some(_) => self.run_wait_matching_events(),
            None => self.run_wait_time(),
        }
    }

    fn run_wait_time(&self) -> Result<i64, String> {
        let wait_time = self.wait_time();

        if self.verbose() {
            eprintln!(
                "No matching event types for action. Will run Sway command and wait {} ms.",
                wait_time.as_millis()
            );
        }

        // Wait before and after running the Sway command: before, to let
        // other running IPC clients finish their own commands; after, to
        // let this command finish before the next action runs.
        thread::sleep(wait_time);

        let sway_command = self.sway_command();
        if self.verbose() {
            eprintln!("Sway command: {}", sway_command);
        }

        run_sway_command(&sway_command)?;
        thread::sleep(wait_time);

        Ok(self.container_id().unwrap())
    }

    fn run_wait_matching_events(&self) -> Result<i64, String> {
        let subscription = self.event_subscription().unwrap();
        let event_loop = self::event_loop(&[subscription])?;

        let sway_command = self.sway_command();
        if self.verbose() {
            eprintln!("Sway command: {}", sway_command);
        }
        run_sway_command(&sway_command)?;

        // Read events on a separate thread and forward them through a
        // channel, so recv_timeout() below enforces a real deadline even if
        // the event stream itself never produces another event (a blocking
        // iterator has no way to time out on its own). The thread may
        // outlive this function if it's still blocked on the socket when we
        // return — harmless, since sway-launch is a short-lived process and
        // the thread dies with it.
        let (event_sender, event_receiver) = mpsc::channel();
        thread::spawn(move || {
            for event in event_loop {
                if event_sender.send(event).is_err() {
                    break;
                }
            }
        });

        // checked_add rather than a plain `+`: an unrepresentable deadline
        // (a pathological --timeout) is treated as already expired instead
        // of panicking.
        let deadline = match time::Instant::now().checked_add(self.timeout()) {
            Some(deadline) => deadline,
            None => return Err(format!("{} sec timeout reached", self.timeout().as_secs())),
        };
        loop {
            let remaining = deadline.saturating_duration_since(time::Instant::now());
            if remaining.is_zero() {
                return Err(format!("{} sec timeout reached", self.timeout().as_secs()));
            }

            let event = match event_receiver.recv_timeout(remaining) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!("{} sec timeout reached", self.timeout().as_secs()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Event stream closed unexpectedly".to_string());
                }
            };

            let event = match event {
                Ok(event) => event,
                Err(error) => return Err(error.to_string()),
            };

            let window = match event {
                Event::Window(window) => window,
                _ => continue,
            };

            match self.matches_window_event(&window) {
                Ok(result) => {
                    if self.verbose() {
                        eprintln!(
                            "Event match: {:?} container id {} ({})",
                            window.change, window.container.id, result
                        );
                    }

                    return Ok(window.container.id);
                }
                Err(error_result) => {
                    if self.verbose() {
                        eprintln!(
                            "Event mismatch: {:?} container id {} ({})",
                            window.change, window.container.id, error_result
                        );
                    }
                }
            }
        }
    }

    fn matches_window_event(
        &self,
        window: &WindowEvent,
    ) -> Result<WindowEventMatch, WindowEventMatchError> {
        let matching_window_change_events = self.matching_window_change_events().unwrap();

        if !matching_window_change_events.contains(&window.change) {
            return Err(WindowEventMatchError::EventChangeTypeMismatch);
        }

        match self {
            SwayAction::Exec {
                app_id_match,
                class_match,
                ..
            } => {
                if !app_id_match.is_empty() {
                    match window_app_id_match(&window.container, app_id_match) {
                        true => return Ok(WindowEventMatch::WindowAppId),
                        false => return Err(WindowEventMatchError::WindowAppIdMismatch),
                    }
                }

                if !class_match.is_empty() {
                    match window_class_match(&window.container, class_match) {
                        true => return Ok(WindowEventMatch::WindowClass),
                        false => return Err(WindowEventMatchError::WindowClassMismatch),
                    }
                }

                return Ok(WindowEventMatch::NewWindowMatchWithoutCheck);
            }
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

    first_outcome_error(outcomes, command)
}

/// Sway splits a command string into multiple sub-commands on unquoted
/// `,`/`;`, so `run_command()` can return more than one outcome for a single
/// call. Report the first failure found among all of them, rather than only
/// the first outcome — an early success must not hide a later failure.
fn first_outcome_error<E: fmt::Display>(
    outcomes: Vec<Result<(), E>>,
    command: &str,
) -> Result<(), String> {
    if outcomes.is_empty() {
        return Err(format!("{} command failed", command));
    }

    for outcome in outcomes {
        if let Err(error) = outcome {
            return Err(error.to_string());
        }
    }

    Ok(())
}

/// Quotes a value for safe interpolation into a Sway IPC command string.
/// Sway's command parser splits on `,`/`;` and whitespace outside quotes, so
/// an unquoted value containing one of those could inject additional
/// commands; wrapping it in escaped double quotes forces it to be read back
/// as a single literal argument.
fn quote_sway_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Validates a `--height`/`--width` value, or a `LayoutStep`'s `height`/
/// `width` field — both take the same `\d+(px|ppt)` format.
pub fn validate_size_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^\d+(px|ppt)$").unwrap();
    match re.is_match(value) {
        true => Ok(value.to_string()),
        false => {
            Err("Must be in format <HEIGHT>px|ppt. E.g. 300px/20ppt. ppt = percent".to_string())
        }
    }
}

/// Validates a `--position` value, or a `LayoutStep`'s `position` field —
/// both take the same `center`/`<x>,<y>` format.
pub fn validate_position_argument(value: &str) -> Result<String, String> {
    let re = Regex::new(r"^center$|^\d+,\d+$").unwrap();
    match re.is_match(value) {
        true => Ok(value.to_string()),
        false => {
            Err("Must be \"center\" or \"<X>,<Y>\" in pixels. E.g. center/100,200".to_string())
        }
    }
}

fn window_app_id_match(node: &Node, app_id_match: &str) -> bool {
    let node_app_id = match node.app_id.as_ref().ok_or(()) {
        Ok(app_id) => app_id,
        Err(_) => return false,
    };

    matches!(node_app_id, _ if node_app_id == app_id_match)
}

fn window_class_match(node: &Node, class_match: &str) -> bool {
    let window_properties = match node.window_properties.as_ref().ok_or(()) {
        Ok(window_properties) => window_properties,
        Err(_) => return false,
    };

    let node_class = match window_properties.class.as_ref().ok_or(()) {
        Ok(class) => class,
        Err(_) => return false,
    };

    matches!(node_class, _ if node_class == class_match)
}

/// Recursively collects the container ids of every node in `tree` (tiling
/// and floating children at every level) whose app_id/class matches, used to
/// target an already-open window instead of launching a new one. app_id
/// takes priority over class when both are set, mirroring
/// `matches_window_event`'s Exec-matching precedence.
fn matching_container_ids(tree: &Node, app_id_match: &str, class_match: &str) -> Vec<i64> {
    let matches = if !app_id_match.is_empty() {
        window_app_id_match(tree, app_id_match)
    } else if !class_match.is_empty() {
        window_class_match(tree, class_match)
    } else {
        false
    };

    let mut ids = if matches { vec![tree.id] } else { vec![] };

    for child in tree.nodes.iter().chain(tree.floating_nodes.iter()) {
        ids.extend(matching_container_ids(child, app_id_match, class_match));
    }

    ids
}

/// Finds exactly one already-open window matching `app_id_match`/
/// `class_match` via `get_tree()`, for `Target::Existing`. Errors — rather
/// than silently picking one — if zero or more than one window matches,
/// since guessing which of several matches the caller meant would be a
/// worse default than asking them to retarget with `--con-id`.
fn find_existing_container_id(app_id_match: &str, class_match: &str) -> Result<i64, String> {
    let tree = match self::new_connection()?.get_tree() {
        Ok(tree) => tree,
        Err(error) => return Err(error.to_string()),
    };

    let criteria = if !app_id_match.is_empty() {
        format!("app_id \"{}\"", app_id_match)
    } else {
        format!("class \"{}\"", class_match)
    };

    resolve_matches(
        matching_container_ids(&tree, app_id_match, class_match),
        &criteria,
    )
}

/// Turns the container ids `matching_container_ids()` found into a single
/// target, erroring — rather than silently picking one — on zero or more
/// than one match.
fn resolve_matches(matches: Vec<i64>, criteria: &str) -> Result<i64, String> {
    match matches.len() {
        0 => Err(format!("No existing window matches {}", criteria)),
        1 => Ok(matches[0]),
        _ => {
            let ids = matches
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "{} windows match {}: {} — retarget with --con-id",
                matches.len(),
                criteria,
                ids
            ))
        }
    }
}

/// What `SwayLaunch::run()` should act on: launch a new window (the
/// original, still-default behavior), a specific already-open window by
/// container id, or an already-open window found by matching
/// `app_id_match`/`class_match` against currently open windows.
pub enum Target<'a> {
    Exec { command: &'a str },
    ConId(i64),
    Existing,
}

pub struct SwayLaunch<'a> {
    pub target: Target<'a>,

    pub app_id_match: &'a str,
    pub class_match: &'a str,

    pub split: Option<Split>,
    pub floating: bool,
    pub fullscreen: bool,
    pub mark: &'a str,
    pub new_column: bool,
    pub new_row: bool,
    pub workspace: Option<&'a str>,
    pub height: Option<&'a str>,
    pub width: Option<&'a str>,
    pub position: Option<&'a str>,

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

    fn resolve_container_id(&self) -> Result<i64, String> {
        match self.target {
            Target::Exec { command } => SwayAction::Exec {
                command,
                app_id_match: self.app_id_match,
                class_match: self.class_match,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run(),
            Target::ConId(container_id) => Ok(container_id),
            Target::Existing => {
                self::find_existing_container_id(self.app_id_match, self.class_match)
            }
        }
    }

    pub fn run(&self) -> Result<i64, String> {
        let container_id = self.resolve_container_id()?;

        if self.verbose {
            eprintln!("Target container id: {}", container_id);
        }

        if self.new_column {
            SwayAction::NewColumn {
                container_id,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if self.new_row {
            SwayAction::NewRow {
                container_id,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if let Some(workspace) = self.workspace {
            SwayAction::Workspace {
                container_id,
                workspace,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()?;
        }
        if let Some(split) = self.split {
            SwayAction::Split {
                container_id,
                split,
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
        if self.fullscreen {
            SwayAction::Fullscreen {
                container_id,
                verbose: self.verbose,
                timeout: self.timeout,
            }
            .run()?;
        }
        if let Some(height) = self.height {
            SwayAction::Height {
                container_id,
                height,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if let Some(width) = self.width {
            SwayAction::Width {
                container_id,
                width,
                verbose: self.verbose,
                wait_time: self.wait_time,
            }
            .run()?;
        }
        if let Some(position) = self.position {
            SwayAction::Position {
                container_id,
                position,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn window_event(
        change: &str,
        container_id: i64,
        app_id: Option<&str>,
        class: Option<&str>,
    ) -> WindowEvent {
        let value = serde_json::json!({
            "change": change,
            "container": {
                "id": container_id,
                "type": "con",
                "border": "normal",
                "current_border_width": 0,
                "layout": "none",
                "orientation": "none",
                "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
                "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
                "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
                "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
                "urgent": false,
                "focused": false,
                "focus": [],
                "floating_nodes": [],
                "sticky": false,
                "app_id": app_id,
                "window_properties": class.map(|class| serde_json::json!({"class": class})),
            }
        });

        serde_json::from_value(value).expect("valid WindowEvent test fixture")
    }

    fn leaf_node_value(
        container_id: i64,
        app_id: Option<&str>,
        class: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": container_id,
            "type": "con",
            "border": "normal",
            "current_border_width": 0,
            "layout": "none",
            "orientation": "none",
            "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
            "urgent": false,
            "focused": false,
            "focus": [],
            "floating_nodes": [],
            "sticky": false,
            "app_id": app_id,
            "window_properties": class.map(|class| serde_json::json!({"class": class})),
        })
    }

    fn container_node_value(
        container_id: i64,
        nodes: Vec<serde_json::Value>,
        floating_nodes: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": container_id,
            "type": "con",
            "border": "normal",
            "current_border_width": 0,
            "layout": "none",
            "orientation": "none",
            "rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "window_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "deco_rect": {"x": 0, "y": 0, "width": 0, "height": 0},
            "geometry": {"x": 0, "y": 0, "width": 0, "height": 0},
            "urgent": false,
            "focused": false,
            "focus": [],
            "nodes": nodes,
            "floating_nodes": floating_nodes,
            "sticky": false,
        })
    }

    fn node_tree(
        container_id: i64,
        nodes: Vec<serde_json::Value>,
        floating_nodes: Vec<serde_json::Value>,
    ) -> Node {
        serde_json::from_value(container_node_value(container_id, nodes, floating_nodes))
            .expect("valid Node test fixture")
    }

    // quote_sway_string

    #[test]
    fn quote_sway_string_wraps_plain_value() {
        assert_eq!(quote_sway_string("foo"), "\"foo\"");
    }

    #[test]
    fn quote_sway_string_escapes_embedded_quotes() {
        assert_eq!(quote_sway_string("foo\"bar"), "\"foo\\\"bar\"");
    }

    #[test]
    fn quote_sway_string_escapes_backslashes() {
        assert_eq!(quote_sway_string("foo\\bar"), "\"foo\\\\bar\"");
    }

    #[test]
    fn quote_sway_string_neutralizes_command_separators() {
        // Regression test: an unquoted mark containing a command separator
        // used to let extra Sway commands be injected into the same call.
        let injected = "foo, exec malicious-command";
        let quoted = quote_sway_string(injected);
        assert_eq!(quoted, "\"foo, exec malicious-command\"");
        assert!(!quoted.trim_matches('"').contains('"'));
    }

    // validate_size_argument / validate_position_argument

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

    // Split

    #[test]
    fn split_display_v_is_vertical() {
        assert_eq!(Split::V.to_string(), "Vertical");
    }

    #[test]
    fn split_display_h_is_horizontal() {
        assert_eq!(Split::H.to_string(), "Horizontal");
    }

    // SwayAction::sway_command

    #[test]
    fn sway_command_exec() {
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "exec kitty");
    }

    #[test]
    fn sway_command_floating() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] floating enable");
    }

    #[test]
    fn sway_command_fullscreen() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] fullscreen enable");
    }

    #[test]
    fn sway_command_split_v() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::V,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] splitv");
    }

    #[test]
    fn sway_command_split_h() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] splith");
    }

    #[test]
    fn sway_command_mark_quotes_the_mark() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo, exec evil",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.sway_command(), "[con_id=42] mark \"foo, exec evil\"");
    }

    #[test]
    fn sway_command_new_column() {
        let action = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move right");
    }

    #[test]
    fn sway_command_new_row() {
        let action = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move down");
    }

    #[test]
    fn sway_command_workspace_quotes_the_workspace() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web, exec evil",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.sway_command(),
            "[con_id=42] move workspace \"web, exec evil\""
        );
    }

    #[test]
    fn sway_command_height() {
        let action = SwayAction::Height {
            container_id: 42,
            height: "300px",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] resize set height 300px");
    }

    #[test]
    fn sway_command_width() {
        let action = SwayAction::Width {
            container_id: 42,
            width: "20ppt",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] resize set width 20ppt");
    }

    #[test]
    fn sway_command_position_center() {
        let action = SwayAction::Position {
            container_id: 42,
            position: "center",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move position center");
    }

    #[test]
    fn sway_command_position_coordinates() {
        let action = SwayAction::Position {
            container_id: 42,
            position: "100,200",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.sway_command(), "[con_id=42] move position 100 200");
    }

    // SwayAction::Display

    #[test]
    fn display_exec() {
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "kitty",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.to_string(),
            "Exec \"kitty\" (app_id_match: \"kitty\") (class_match: \"\")"
        );
    }

    #[test]
    fn display_split() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::H,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Split (container id: 42) (split: Horizontal)"
        );
    }

    #[test]
    fn display_floating() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Floating (container_id: 42)");
    }

    #[test]
    fn display_fullscreen() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Fullscreen (container_id: 42)");
    }

    #[test]
    fn display_new_column() {
        let action = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.to_string(), "New column (container_id: 42)");
    }

    #[test]
    fn display_new_row() {
        let action = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.to_string(), "New row (container_id: 42)");
    }

    #[test]
    fn display_workspace() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.to_string(),
            "Workspace (container id: 42) (workspace: web)"
        );
    }

    #[test]
    fn display_mark() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.to_string(), "Mark (container id: 42) (mark: foo)");
    }

    #[test]
    fn display_height() {
        let action = SwayAction::Height {
            container_id: 42,
            height: "300px",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Height (container id: 42) (height: 300px)"
        );
    }

    #[test]
    fn display_width() {
        let action = SwayAction::Width {
            container_id: 42,
            width: "20ppt",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Width (container id: 42) (width: 20ppt)"
        );
    }

    #[test]
    fn display_position() {
        let action = SwayAction::Position {
            container_id: 42,
            position: "center",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(
            action.to_string(),
            "Position (container id: 42) (position: center)"
        );
    }

    // SwayAction accessors

    #[test]
    fn verbose_reflects_the_flag() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: true,
            timeout: time::Duration::from_secs(5),
        };
        assert!(action.verbose());
    }

    #[test]
    fn timeout_returns_the_configured_duration() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(7),
        };
        assert_eq!(action.timeout(), time::Duration::from_secs(7));
    }

    #[test]
    #[should_panic]
    fn timeout_panics_for_actions_without_one() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::V,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        action.timeout();
    }

    #[test]
    fn wait_time_returns_the_configured_duration() {
        let action = SwayAction::Height {
            container_id: 42,
            height: "300px",
            verbose: false,
            wait_time: time::Duration::from_millis(42),
        };
        assert_eq!(action.wait_time(), time::Duration::from_millis(42));
    }

    #[test]
    #[should_panic]
    fn wait_time_panics_for_actions_without_one() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        action.wait_time();
    }

    #[test]
    fn container_id_is_none_for_exec() {
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.container_id(), None);
    }

    #[test]
    fn container_id_is_set_for_other_actions() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.container_id(), Some(42));
    }

    // SwayAction::matching_window_change_events / event_subscription

    #[test]
    fn exec_only_matches_new_window_change() {
        // Regression test: `Move` used to be accepted here too, which meant
        // any pre-existing window being reparented (not just the one we
        // just launched) could be mistaken for a match.
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::New])
        );
    }

    #[test]
    fn floating_matches_floating_window_change() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Floating])
        );
    }

    #[test]
    fn fullscreen_matches_fullscreen_mode_window_change() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::FullscreenMode])
        );
    }

    #[test]
    fn workspace_matches_move_window_change() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Move])
        );
    }

    #[test]
    fn mark_matches_mark_window_change() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(
            action.matching_window_change_events(),
            Some(vec![WindowChange::Mark])
        );
    }

    #[test]
    fn split_height_width_have_no_matching_window_change() {
        let split = SwayAction::Split {
            container_id: 42,
            split: Split::V,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let new_column = SwayAction::NewColumn {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let new_row = SwayAction::NewRow {
            container_id: 42,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let height = SwayAction::Height {
            container_id: 42,
            height: "300px",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let width = SwayAction::Width {
            container_id: 42,
            width: "20ppt",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        let position = SwayAction::Position {
            container_id: 42,
            position: "center",
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(split.matching_window_change_events(), None);
        assert_eq!(new_column.matching_window_change_events(), None);
        assert_eq!(new_row.matching_window_change_events(), None);
        assert_eq!(height.matching_window_change_events(), None);
        assert_eq!(width.matching_window_change_events(), None);
        assert_eq!(position.matching_window_change_events(), None);
    }

    #[test]
    fn event_subscription_is_window_for_event_backed_actions() {
        let action = SwayAction::Mark {
            container_id: 42,
            mark: "foo",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        assert_eq!(action.event_subscription(), Some(EventType::Window));
    }

    #[test]
    fn event_subscription_is_none_for_time_based_actions() {
        let action = SwayAction::Split {
            container_id: 42,
            split: Split::V,
            verbose: false,
            wait_time: time::Duration::from_millis(20),
        };
        assert_eq!(action.event_subscription(), None);
    }

    // SwayAction::matches_window_event

    #[test]
    fn exec_without_filter_matches_any_new_window() {
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("kitty"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::NewWindowMatchWithoutCheck)
        ));
    }

    #[test]
    fn exec_without_filter_rejects_non_new_window_change() {
        // Regression test for the Move-over-matching bug: with no
        // app_id/class filter, only a `New` event may identify the window
        // just launched — not a `Move` (or any other) event belonging to
        // some other, pre-existing window.
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("move", 99, Some("kitty"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::EventChangeTypeMismatch)
        ));
    }

    #[test]
    fn exec_with_app_id_match_accepts_matching_app_id() {
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "kitty",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("kitty"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowAppId)
        ));
    }

    #[test]
    fn exec_with_app_id_match_rejects_different_app_id() {
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "kitty",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("alacritty"), None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowAppIdMismatch)
        ));
    }

    #[test]
    fn exec_with_app_id_match_rejects_missing_app_id() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "firefox",
            class_match: "",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowAppIdMismatch)
        ));
    }

    #[test]
    fn exec_with_class_match_accepts_matching_class() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "",
            class_match: "Firefox",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, Some("Firefox"));
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowClass)
        ));
    }

    #[test]
    fn exec_with_class_match_rejects_different_class() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "",
            class_match: "Firefox",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, Some("Chromium"));
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowClassMismatch)
        ));
    }

    #[test]
    fn exec_with_class_match_rejects_missing_window_properties() {
        let action = SwayAction::Exec {
            command: "firefox",
            app_id_match: "",
            class_match: "Firefox",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::WindowClassMismatch)
        ));
    }

    #[test]
    fn exec_with_app_id_match_ignores_class_match_when_both_set() {
        // Documents current behavior: when both are set, app_id_match takes
        // priority and class_match is never consulted (see also
        // Args::app_id's conflicts_with in main.rs, which now prevents a
        // caller from setting both in the first place).
        let action = SwayAction::Exec {
            command: "kitty",
            app_id_match: "kitty",
            class_match: "SomethingElseEntirely",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("new", 99, Some("kitty"), Some("SomethingElseEntirely"));
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowAppId)
        ));
    }

    #[test]
    fn non_exec_action_matches_on_container_id() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("floating", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn workspace_matches_on_container_id() {
        let action = SwayAction::Workspace {
            container_id: 42,
            workspace: "web",
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("move", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn fullscreen_matches_on_container_id() {
        let action = SwayAction::Fullscreen {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("fullscreen_mode", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Ok(WindowEventMatch::WindowContainerIdMatch)
        ));
    }

    #[test]
    fn non_exec_action_rejects_different_container_id() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("floating", 99, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::NoMatchingEvent)
        ));
    }

    #[test]
    fn non_exec_action_rejects_wrong_change_type() {
        let action = SwayAction::Floating {
            container_id: 42,
            verbose: false,
            timeout: time::Duration::from_secs(5),
        };
        let event = window_event("mark", 42, None, None);
        assert!(matches!(
            action.matches_window_event(&event),
            Err(WindowEventMatchError::EventChangeTypeMismatch)
        ));
    }

    // window_app_id_match / window_class_match

    #[test]
    fn window_app_id_match_true_when_equal() {
        let event = window_event("new", 1, Some("kitty"), None);
        assert!(window_app_id_match(&event.container, "kitty"));
    }

    #[test]
    fn window_app_id_match_false_when_different() {
        let event = window_event("new", 1, Some("kitty"), None);
        assert!(!window_app_id_match(&event.container, "alacritty"));
    }

    #[test]
    fn window_app_id_match_false_when_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_app_id_match(&event.container, "kitty"));
    }

    #[test]
    fn window_class_match_true_when_equal() {
        let event = window_event("new", 1, None, Some("Firefox"));
        assert!(window_class_match(&event.container, "Firefox"));
    }

    #[test]
    fn window_class_match_false_when_different() {
        let event = window_event("new", 1, None, Some("Firefox"));
        assert!(!window_class_match(&event.container, "Chromium"));
    }

    #[test]
    fn window_class_match_false_when_window_properties_absent() {
        let event = window_event("new", 1, None, None);
        assert!(!window_class_match(&event.container, "Firefox"));
    }

    // matching_container_ids

    #[test]
    fn matching_container_ids_finds_tiling_and_floating_matches() {
        let tree = node_tree(
            1,
            vec![
                leaf_node_value(10, Some("kitty"), None),
                leaf_node_value(11, Some("firefox"), None),
            ],
            vec![leaf_node_value(20, Some("kitty"), None)],
        );
        let mut ids = matching_container_ids(&tree, "kitty", "");
        ids.sort();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn matching_container_ids_empty_when_no_match() {
        let tree = node_tree(1, vec![leaf_node_value(10, Some("kitty"), None)], vec![]);
        assert_eq!(
            matching_container_ids(&tree, "nonexistent", ""),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn matching_container_ids_matches_by_class() {
        let tree = node_tree(1, vec![leaf_node_value(10, None, Some("Firefox"))], vec![]);
        assert_eq!(matching_container_ids(&tree, "", "Firefox"), vec![10]);
    }

    #[test]
    fn matching_container_ids_recurses_into_nested_containers() {
        let inner = container_node_value(2, vec![leaf_node_value(10, Some("kitty"), None)], vec![]);
        let tree = node_tree(1, vec![inner], vec![]);
        assert_eq!(matching_container_ids(&tree, "kitty", ""), vec![10]);
    }

    #[test]
    fn matching_container_ids_prefers_app_id_over_class_when_both_set() {
        let tree = node_tree(
            1,
            vec![leaf_node_value(10, Some("kitty"), Some("NoMatch"))],
            vec![],
        );
        assert_eq!(matching_container_ids(&tree, "kitty", "NoMatch"), vec![10]);
    }

    // resolve_matches

    #[test]
    fn resolve_matches_errors_on_zero_matches() {
        assert_eq!(
            resolve_matches(vec![], "app_id \"kitty\""),
            Err("No existing window matches app_id \"kitty\"".to_string())
        );
    }

    #[test]
    fn resolve_matches_ok_on_single_match() {
        assert_eq!(resolve_matches(vec![42], "app_id \"kitty\""), Ok(42));
    }

    #[test]
    fn resolve_matches_errors_listing_ids_on_multiple_matches() {
        assert_eq!(
            resolve_matches(vec![42, 91], "app_id \"kitty\""),
            Err("2 windows match app_id \"kitty\": 42, 91 — retarget with --con-id".to_string())
        );
    }

    // first_outcome_error

    #[test]
    fn first_outcome_error_ok_when_all_succeed() {
        let outcomes: Vec<Result<(), String>> = vec![Ok(()), Ok(())];
        assert_eq!(first_outcome_error(outcomes, "cmd"), Ok(()));
    }

    #[test]
    fn first_outcome_error_fails_when_empty() {
        let outcomes: Vec<Result<(), String>> = vec![];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("cmd command failed".to_string())
        );
    }

    #[test]
    fn first_outcome_error_surfaces_a_leading_failure() {
        let outcomes: Vec<Result<(), String>> = vec![Err("boom".to_string()), Ok(())];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("boom".to_string())
        );
    }

    #[test]
    fn first_outcome_error_surfaces_a_trailing_failure() {
        // Regression test: a prior version only inspected the first outcome
        // and returned Ok(()) here, silently dropping this failure.
        let outcomes: Vec<Result<(), String>> = vec![Ok(()), Err("boom".to_string())];
        assert_eq!(
            first_outcome_error(outcomes, "cmd"),
            Err("boom".to_string())
        );
    }

    // Display impls for the private matching-result enums

    #[test]
    fn window_event_match_display() {
        assert_eq!(
            WindowEventMatch::WindowAppId.to_string(),
            "Window app_id match"
        );
        assert_eq!(
            WindowEventMatch::WindowClass.to_string(),
            "Window class match"
        );
        assert_eq!(
            WindowEventMatch::NewWindowMatchWithoutCheck.to_string(),
            "New window without app_id or class check"
        );
        assert_eq!(
            WindowEventMatch::WindowContainerIdMatch.to_string(),
            "Window container id match"
        );
    }

    #[test]
    fn window_event_match_error_display() {
        assert_eq!(
            WindowEventMatchError::EventChangeTypeMismatch.to_string(),
            "Event does not match action event matches"
        );
        assert_eq!(
            WindowEventMatchError::WindowAppIdMismatch.to_string(),
            "app_id mismatch"
        );
        assert_eq!(
            WindowEventMatchError::WindowClassMismatch.to_string(),
            "class mismatch"
        );
        assert_eq!(
            WindowEventMatchError::NoMatchingEvent.to_string(),
            "No matching event"
        );
    }
}
