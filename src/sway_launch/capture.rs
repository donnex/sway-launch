//! `--capture` (#31): turning a workspace arranged by hand into a `--layout`
//! file that rebuilds it.
//!
//! A step can only open a window next to the focused one, split a window,
//! focus it, and size it, so the file can't simply list the tree depth-first:
//! splitting a window that's its container's only child changes that
//! container's layout rather than nesting a new one. `capture()` instead
//! builds each split's children level by level. It opens the first window of
//! every child side by side, then goes back to each child that is itself a
//! split, splits its first window (which now has siblings, so Sway wraps it in
//! a new container) and focuses it, and builds that child the same way.
//! Sizes come next, once every window exists; see `size_steps()`.
//!
//! Fullscreen comes last of all, on a step of its own. Sway leaves focus on a
//! fullscreen window when another opens on its workspace and places the new
//! one next to it, so a window made fullscreen at launch would have every
//! later window open after it, in reverse order; and focusing another window
//! takes it out of fullscreen.
//!
//! Every built-in template round-trips through it (the live suite captures
//! each one and rebuilds it), so this is the one place outside `tree.rs` that
//! recurses over a tree: the shape *is* the recursion.

use super::tree::node_position;
use super::values::escape_control_characters;
use swayipc::{Node, NodeLayout, NodeType};

#[cfg(test)]
mod tests;

/// A workspace's tiled tree as `capture()` rebuilds it: windows, and splits
/// holding each child with its share of the split.
enum Shape<'a> {
    Window(&'a Node),
    Split {
        horizontal: bool,
        children: Vec<(f64, Shape<'a>)>,
    },
}

impl<'a> Shape<'a> {
    /// The tiled tree under `node`. A container holding one child looks like
    /// that child, so it's collapsed into it.
    fn of(node: &'a Node) -> Result<Shape<'a>, String> {
        match node.nodes.as_slice() {
            [] => Ok(Shape::Window(node)),
            [only] => Shape::of(only),
            children => {
                let horizontal = match node.layout {
                    NodeLayout::SplitH => true,
                    NodeLayout::SplitV => false,
                    other => {
                        return Err(format!(
                            "it holds a {} container, and only splith/splitv layouts can be \
                             rebuilt",
                            format!("{:?}", other).to_lowercase()
                        ))
                    }
                };
                let children = children
                    .iter()
                    .map(|child| Ok((share_of(child, children), Shape::of(child)?)))
                    .collect::<Result<_, String>>()?;
                Ok(Shape::Split {
                    horizontal,
                    children,
                })
            }
        }
    }

    fn first_window(&self) -> &'a Node {
        match self {
            Shape::Window(node) => node,
            Shape::Split { children, .. } => children[0].1.first_window(),
        }
    }
}

/// `child`'s share of the split holding it among `siblings` (`child`
/// included). Sway reports a fullscreen container's `percent` as `1.0`, not
/// its share of the split, so a fullscreen child takes what the others leave,
/// or the even share when that isn't known.
fn share_of(child: &Node, siblings: &[Node]) -> f64 {
    let even = 1.0 / siblings.len() as f64;
    if !is_fullscreen(child) {
        return child.percent.unwrap_or(even);
    }
    siblings
        .iter()
        .filter(|sibling| sibling.id != child.id)
        .map(|sibling| sibling.percent.filter(|_| !is_fullscreen(sibling)))
        .sum::<Option<f64>>()
        .map(|others| 1.0 - others)
        .filter(|share| *share > 0.0 && *share < 1.0)
        .unwrap_or(even)
}

fn is_fullscreen(node: &Node) -> bool {
    node.fullscreen_mode == Some(1)
}

/// One `[[step]]` of the captured file, as the lines that render it.
struct Step {
    comment: Option<String>,
    fields: Vec<(&'static str, String)>,
}

/// A TOML basic string holding `value`.
fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// Builds the captured file. Windows are named after their app_id (or
/// class), numbered when several share one, and each one's step id is that
/// name, so a later step can retarget it.
struct Capture<'a> {
    steps: Vec<Step>,
    names: Vec<(i64, String)>,
    notes: Vec<String>,
    fullscreen: Vec<&'a Node>,
    workspace: &'a Node,
}

impl<'a> Capture<'a> {
    fn name(&mut self, window: &Node) -> String {
        if let Some((_, name)) = self.names.iter().find(|(id, _)| *id == window.id) {
            return name.clone();
        }
        let base = identity(window)
            .map(|(_, value)| value.to_string())
            .unwrap_or_else(|| "window".to_string());
        let mut name = base.clone();
        let mut number = 1;
        while self.names.iter().any(|(_, taken)| *taken == name) {
            number += 1;
            name = format!("{}-{}", base, number);
        }
        self.names.push((window.id, name.clone()));
        name
    }

    /// A step launching `window`, guessing its command from its app_id or
    /// class, and carrying its marks and state.
    fn launch(&mut self, window: &'a Node, split: Option<bool>) -> Result<(), String> {
        let (key, value) = identity(window).ok_or_else(|| {
            format!(
                "window {} has no app_id or class to guess a command from",
                window.id
            )
        })?;
        let id = self.name(window);
        let mut fields = vec![
            ("id", toml_string(&id)),
            ("command", self.command_field(&id, key, value)),
            (key, toml_string(value)),
        ];
        if let Some(horizontal) = split {
            fields.push(("split", toml_string(split_value(horizontal))));
        }
        if let Some(mark) = window.marks.first() {
            fields.push(("mark", toml_string(mark)));
            if window.marks.len() > 1 {
                self.notes.push(format!(
                    "{} also has the marks {}; a step sets only one",
                    id,
                    window.marks[1..].join(", ")
                ));
            }
        }
        if is_fullscreen(window) {
            self.fullscreen.push(window);
        }
        if window.node_type == NodeType::FloatingCon {
            fields.push(("floating", "true".to_string()));
            if window.sticky {
                fields.push(("sticky", "true".to_string()));
            }
            fields.push(("width", toml_string(&format!("{}px", window.rect.width))));
            fields.push((
                "height",
                toml_string(&format!(
                    "{}px",
                    window.rect.height + window.deco_rect.height
                )),
            ));
            let (x, y) = node_position(window, self.workspace.rect);
            fields.push(("position", toml_string(&format!("{},{}", x, y))));
        }
        self.steps.push(Step {
            comment: window
                .name
                .as_deref()
                .map(|title| escape_control_characters(title, false)),
            fields,
        });
        Ok(())
    }

    /// The `command` value guessed from a window's app_id or class: the
    /// string form for a plain program name (`foot`), and otherwise the list
    /// form, so whatever the client named itself reaches `sh` as one quoted
    /// word. An app_id is the client's to set at any time (foot takes one from
    /// an escape sequence anything printed in the terminal can send), so a
    /// guess written as a string could replay `foot && rm ...` as shell code.
    fn command_field(&mut self, id: &str, key: &str, value: &str) -> String {
        let guess = command_guess(key, value);
        if is_plain_program_name(&guess) {
            return toml_string(&guess);
        }
        self.notes.push(format!(
            "{}'s {} isn't a plain program name, so its command is written as one quoted word; \
             check it before running this",
            id, key
        ));
        format!("[{}]", toml_string(&guess))
    }

    fn retarget(&mut self, window: &Node, fields: Vec<(&'static str, String)>) {
        let id = self.name(window);
        let mut all = vec![("target_id", toml_string(&id))];
        all.extend(fields);
        self.steps.push(Step {
            comment: None,
            fields: all,
        });
    }

    /// Opens every child's first window after `children[0]`'s, then builds
    /// each child that is a split itself. The first child's window is open
    /// and focused when this is called.
    fn build(&mut self, children: &[(f64, Shape<'a>)]) -> Result<(), String> {
        for (_, child) in &children[1..] {
            self.launch(child.first_window(), None)?;
        }
        for (_, child) in children {
            if let Shape::Split {
                horizontal,
                children: grandchildren,
            } = child
            {
                self.retarget(
                    child.first_window(),
                    vec![
                        ("split", toml_string(split_value(*horizontal))),
                        ("focus", "true".to_string()),
                    ],
                );
                self.build(grandchildren)?;
            }
        }
        Ok(())
    }

    /// Sizes every split whose children aren't even, through each child's
    /// first window: Sway resizes the window's nearest ancestor along the
    /// split's axis, which is the child itself.
    ///
    /// Sway versions share a resized child's change with its siblings
    /// differently (measured live): 1.9 and 1.11 take it from the neighbours,
    /// all from the one neighbour of a first or last child and half from each
    /// side of one in between, while 1.12 shares it among every sibling. So
    /// the order is chosen to come out the same on both wherever it can:
    ///
    /// - two children: size the second; the other takes the rest.
    /// - three whose outer two are equal: size the middle one; either way
    ///   the change is split evenly between the two sides.
    /// - any other three: size the first, then the last. On 1.9 and 1.11
    ///   each only moves the middle one, so it's exact there; on 1.12 it's
    ///   approximate, and the file says so.
    /// - four or more: sized in order, and noted as approximate.
    fn size_steps(&mut self, shape: &Shape<'a>) {
        let Shape::Split {
            horizontal,
            children,
        } = shape
        else {
            return;
        };
        let count = children.len();
        let even = (100.0 / count as f64).round() as i64;
        let percent = |share: f64| (share * 100.0).round() as i64;
        if children.iter().any(|(share, _)| percent(*share) != even) {
            let order: Vec<usize> = match count {
                2 => vec![1],
                3 if percent(children[0].0) == percent(children[2].0) => vec![1],
                3 => vec![0, 2],
                _ => (0..count - 1).collect(),
            };
            if count == 3 && order.len() == 2 {
                let first = self.name(children[0].1.first_window());
                self.notes.push(format!(
                    "the 3-window {} split holding {} is sized exactly on Sway 1.9 to 1.11 \
                     and approximately on newer ones, which share a resize among all siblings",
                    split_value(*horizontal),
                    first,
                ));
            }
            if count > 3 {
                let first = self.name(children[0].1.first_window());
                self.notes.push(format!(
                    "the {}-window {} split holding {} is sized approximately: Sway shares \
                     a resize between neighbours",
                    count,
                    split_value(*horizontal),
                    first,
                ));
            }
            let dimension = if *horizontal { "width" } else { "height" };
            for index in order {
                let (share, child) = &children[index];
                if matches!(child, Shape::Split { horizontal: inner, .. } if inner == horizontal) {
                    let first = self.name(child.first_window());
                    self.notes.push(format!(
                        "{}'s split isn't sized: it runs the same way as the split holding it",
                        first
                    ));
                    continue;
                }
                self.retarget(
                    child.first_window(),
                    vec![(dimension, toml_string(&format!("{}ppt", percent(*share))))],
                );
            }
        }
        for (_, child) in children {
            self.size_steps(child);
        }
    }
}

/// A window's app_id, or failing that its class, as the step key matching it.
fn identity(window: &Node) -> Option<(&'static str, &str)> {
    window
        .app_id
        .as_deref()
        .filter(|app_id| !app_id.is_empty())
        .map(|app_id| ("app_id", app_id))
        .or_else(|| {
            window
                .window_properties
                .as_ref()
                .and_then(|properties| properties.class.as_deref())
                .filter(|class| !class.is_empty())
                .map(|class| ("class", class))
        })
}

/// The command a window was most likely started with: its app_id as is
/// (`foot`), or its class lowercased (`Code` is `code`). Only a guess, and
/// the file says so.
fn command_guess(key: &str, value: &str) -> String {
    if key == "class" {
        value.to_lowercase()
    } else {
        value.to_string()
    }
}

/// Whether `name` is safe to write as a shell command as is: nothing but
/// letters, digits and `.`, `_`, `+`, `-`, and not starting with `-`.
fn is_plain_program_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

fn split_value(horizontal: bool) -> &'static str {
    if horizontal {
        "h"
    } else {
        "v"
    }
}

/// The `--layout` file rebuilding `workspace`: its tiled windows in the same
/// splits and sizes, then its floating ones with their size and position.
pub(super) fn capture(workspace: &Node) -> Result<String, String> {
    let workspace_name = workspace.name.as_deref().unwrap_or("?");
    let in_workspace = |error: String| format!("workspace {:?}: {}", workspace_name, error);
    let tiled = match workspace.nodes.is_empty() {
        true => None,
        false => Some(Shape::of(workspace).map_err(in_workspace)?),
    };
    if tiled.is_none() && workspace.floating_nodes.is_empty() {
        return Err(in_workspace("it has no windows to capture".to_string()));
    }

    let mut capture = Capture {
        steps: Vec::new(),
        names: Vec::new(),
        notes: Vec::new(),
        fullscreen: Vec::new(),
        workspace,
    };
    match &tiled {
        Some(Shape::Window(window)) => capture.launch(window, None).map_err(in_workspace)?,
        Some(Shape::Split {
            horizontal,
            children,
        }) => {
            capture
                .launch(children[0].1.first_window(), Some(*horizontal))
                .map_err(in_workspace)?;
            capture.build(children).map_err(in_workspace)?;
        }
        None => {}
    }
    for window in &workspace.floating_nodes {
        let window = Shape::of(window).map_err(in_workspace)?.first_window();
        capture.launch(window, None).map_err(in_workspace)?;
    }
    if let Some(shape) = &tiled {
        capture.size_steps(shape);
    }
    for window in std::mem::take(&mut capture.fullscreen) {
        capture.retarget(window, vec![("fullscreen", "true".to_string())]);
    }

    let mut rendered = format!(
        "# Captured from workspace {} by sway-launch --capture.\n\
         # Each command is a guess from its window's app_id or class: check them\n\
         # before running this with sway-launch --layout.\n",
        toml_string(&escape_control_characters(workspace_name, false))
    );
    for note in &capture.notes {
        rendered.push_str(&format!(
            "# Note: {}.\n",
            escape_control_characters(note, false)
        ));
    }
    for step in &capture.steps {
        rendered.push('\n');
        if let Some(comment) = &step.comment {
            rendered.push_str(&format!("# {}\n", comment));
        }
        rendered.push_str("[[step]]\n");
        for (key, value) in &step.fields {
            rendered.push_str(&format!("{} = {}\n", key, value));
        }
    }
    Ok(rendered)
}
