# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--scratchpad` flag (and the matching `--layout`/`--template` step field) to move a window to
  Sway's scratchpad, runs last after every other action — pairs well with `--mark`/`--floating`/
  `--width`/`--height`/`--position` for the classic "dropdown terminal" pattern.
- `--rollback-on-error`: if a later `--layout`/`--template` step fails, kill every window this
  invocation itself launched by an earlier, already-completed step, instead of leaving them open.
  Requires `--layout` or `--template`.
- `master-dual-stack-left`/`master-triple-stack-left` built-in templates — `master-dual-stack`/
  `master-triple-stack` mirrored to the other side (stack on the left, main window on the right).
- `--sticky` flag (and the matching `--layout`/`--template` step field) to make a window show on
  every workspace instead of just the one it was launched on. Works regardless of floating state,
  pairs well with `--floating`.
- `--dry-run`: prints the planned sequence of Sway commands instead of running them, numbered
  continuously across every step — never touches Sway IPC or launches anything. Works with a direct
  command or `--layout`/`--template`; `--json` prints a structured `{"steps": [...]}` object.
- `--validate`: parses and validates a `--layout`/`--template` file (formats, target-field
  consistency, `target_id` references, and for `--template`, `--bindings`/`--apps` resolution)
  without launching anything or touching Sway IPC. Requires `--layout` or `--template`.
- `--show-template <NAME_OR_PATH>`: prints a `--template`'s raw TOML and exits, without running it
  — the same built-in-name-or-file-path resolution `--template` itself uses. `--json` prints
  `{"name": ..., "contents": "..."}`.
- A `--template` file may now declare an optional `[layout]` table (`workspace`/`output`) applied
  to every step that doesn't set its own — pins the whole template to a specific workspace/output
  instead of always operating on whatever's currently focused when it runs. A step's own
  `workspace`/`output` still wins when set, applied per field.

### Changed

- `--position <x>,<y>` (and the matching `--layout`/`--template` step field) now accepts negative
  coordinates — a real, valid position on a multi-monitor setup where an output sits left of or
  above the primary one.
- `--json` now also applies to error output: a failure prints `{"error": "...", "rolled_back": [...]}`
  instead of a plain-text message, so a `--json` caller doesn't need to also parse plain stderr on
  failure.
- `--json`'s success output is richer: a single invocation now also reports `"actions"` (every
  action that actually ran, in order); `--layout`/`--template` now also reports `"containers"` (a
  map from each named step's `id`/`slot` to its container id). No compatibility shim — every field
  from the previous shape is still present, just alongside the new ones.
- Every `--template` file now requires a `[template]` table with `description` and `category`
  fields, replacing the old convention of scraping the description from the file's first header
  comment line. `--list-templates`/`--show-template --json` now also report each template's
  `category`. This is a breaking change for a hand-authored `--template <file>.toml` without the
  table — add one (see README.md's "Templates" section) to keep it working. Every built-in template
  under `templates/` has been migrated to the new format.
- `floating-overlay` template's overlay window is now explicitly centered (`position = "center"`),
  so it lands somewhere deterministic every run instead of wherever Sway's own default floating
  placement happened to put it.
- A `--bindings` binding setting both `app_id` and `class` now errors immediately, naming the
  offending slot, instead of only being caught later by a more generic step-level error.
- Two `--template` steps sharing the same `slot` name now error immediately with a message naming
  the slot (`template: slot "..." is used more than once`), instead of a generic, implementation-detail-flavored
  "id already used by an earlier step" message.
