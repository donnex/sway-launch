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

### Changed

- `--json` now also applies to error output: a failure prints `{"error": "...", "rolled_back": [...]}`
  instead of a plain-text message, so a `--json` caller doesn't need to also parse plain stderr on
  failure.
- `floating-overlay` template's overlay window is now explicitly centered (`position = "center"`),
  so it lands somewhere deterministic every run instead of wherever Sway's own default floating
  placement happened to put it.
