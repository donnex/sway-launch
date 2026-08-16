# Plan: poll-based completion for the wait-time actions

**Status:** not started — design only, saved for a future session.

## Problem

Six `SwayAction` variants have no corresponding Sway IPC event to confirm completion
(`matching_window_change_events()` returns `None` for them), so `run_wait_time()` just sends the
command and sleeps `--wait-time` before and after, blind to whether anything actually happened:

- `Split` (`--split h/v`)
- `NewColumn` (`--new-column`, `move right`)
- `NewRow` (`--new-row`, `move down`)
- `Height` (`--height`)
- `Width` (`--width`)
- `Position` (`--position`)

This is honest about not knowing, but it means every one of these actions always costs the full
`2 * --wait-time` (20ms default, each direction) even when the change happens instantly, and gives
no actual confirmation that anything took effect.

## Proposed idea (from conversation, 2026-08-16)

After sending the command, poll `get_tree()` repeatedly for a short bounded window, checking
whether the container's tree state now reflects the requested change. Return as soon as it's
confirmed, instead of always waiting the full `--wait-time`.

## The catch: several of these actions have legitimate no-op semantics

This can't be "poll until confirmed or hang" — discovered empirically this session, each of these
is a real, non-exotic case where Sway accepts the command but the tree never reaches the
"requested" state:

- **`Height`/`Width`**: resizing a window that is the *sole occupant* of its workspace is silently
  clamped by Sway — the container stays at 100%. This is exactly why
  `examples/templates/master-dual-stack.toml` and `sidebar-left.toml` had to put their `width` on
  the *second*-launched slot, not the first (see their header comments and
  `tests/live_sway.rs`'s `height_alone_resizes_a_non_solo_window`, which specifically launches a
  sibling first to avoid this).
- **`Position`**: only meaningful for a floating window (README's own Position section: "a tiled
  window's position is determined by the layout, not by coordinates"). On a tiled window it's a
  no-op.
- **`NewColumn`/`NewRow`**: a no-op when the window is already at the tree's rightmost/bottommost
  edge — the *ordinary* two-window case, and the exact reason these were moved off
  `WindowChange::Move` event-confirmation in the first place (see
  `matching_window_change_events()`'s comment and
  `tests/live_sway.rs`'s `new_column_and_new_row_complete_promptly_when_already_at_the_edge`).

If polling waits for the tree to reach the "expected" state, these cases would hang until some
poll-timeout — the same fundamental ambiguity (can't tell "hasn't happened yet" from "happened as a
no-op") that killed the event-based approach for `NewColumn`/`NewRow`, just relocated from the
event stream to tree-diffing. Naive replacement is a regression, not a fix.

## Design: bounded poll with fallback to today's behavior

Don't wait-until-confirmed-or-error. Poll for a **short, fixed grace period** (much shorter than
`--timeout`) for the expected tree state; if it converges, return immediately (fast path). If the
grace period elapses without convergence, fall back to today's behavior — assume success, sleep
the remainder of `--wait-time`, return — rather than erroring. This mirrors the pattern already
used for `SwayAction::Exec`'s PID-marker fallback (`PID_MARKER_FALLBACK_GRACE`,
`run_wait_matching_exec_event()` in `src/sway_launch.rs`): try to confirm precisely, fall back
gracefully within a bounded extra delay if confirmation never comes, never regress into a hang.

Sketch:

```rust
fn run_wait_time(&self) -> Result<i64, String> {
    // ... existing pre-sleep, container_exists() check, send command ...

    let confirmed = self.poll_for_expected_state(POLL_GRACE_PERIOD); // new
    if !confirmed {
        thread::sleep(remaining_wait_time); // today's fallback behavior
    }
    Ok(container_id)
}
```

`poll_for_expected_state` would loop `get_tree()` + a short per-iteration sleep (e.g. 5-10ms, per
the "5ms" figure floated in conversation) until either the expected state is observed or the grace
period elapses.

## Per-action matching logic

This is the real cost, not the polling loop itself — each action needs its own definition of
"confirmed":

- **`Split`** — simplest, cleanest starting point. Compare the container node's `layout` field
  (`"splith"`/`"splitv"`) to the requested `Split::H`/`Split::V`. No known no-op case: setting a
  split direction the container already has is idempotent and matches on the very first poll, so
  even the "no-op" case resolves instantly rather than needing the grace-period fallback. Good
  first candidate to prototype in isolation before touching the other five.
- **`Height`/`Width`** — needs tolerance-aware matching, not exact equality. This session found the
  border/decoration accounting is inconsistent depending on tiled-vs-floating state:
  - A window resized while already floating matches the requested value exactly (`rect.width`, see
    `floating_with_width_and_height_applies_all_three`).
  - A tiled window later floated and resized comes out `2 * current_border_width` short on width
    (confirmed in `retarget_by_id_layout_floats_the_first_step_by_name`) and needs
    `rect.height + deco_rect.height` for height (confirmed in the same test and
    `height_alone_resizes_a_non_solo_window`).
  Matching logic needs to check both possible adjustments (or a small tolerance window) rather than
  one fixed formula. Must also tolerate the solo-window-clamped no-op case via the grace-period
  fallback (this is the primary case that fallback exists for).
- **`Position`** — same decoration-aware matching as above (`deco_rect.x`/`deco_rect.y`, confirmed
  in `position_moves_a_floating_window_to_given_coordinates`/`position_center_centers_a_floating_window`),
  plus needs to detect "not floating" up front and skip straight to the fallback path, since a
  tiled-window no-op is expected/routine, not exceptional.
- **`NewColumn`/`NewRow`** — structural comparison, not a value match: snapshot the container's
  parent id (or sibling list) before sending the command, poll for it to differ afterward. Must
  fall back gracefully for the documented edge-of-tree no-op case. Keep this last — it's the
  trickiest to get the "what changed" comparison right, and the existing
  `relocates_to_another_output()` multi-monitor guard already runs *before* this action fires, so
  that interaction needs care (the guard's own `get_tree()` call happens before this feature would
  add more).

## Constants / config

- Reuse the naming pattern from `PID_MARKER_FALLBACK_GRACE` for a new constant, e.g.
  `WAIT_TIME_POLL_GRACE` — needs its own empirically-tuned value the same way `PID_MARKER_FALLBACK_GRACE`
  went from 500ms to 2000ms after live-load testing surfaced flakiness at the shorter value. Don't
  assume 5ms/short values proposed in conversation are sufficient without the same kind of repeated
  live-Sway testing this project has relied on throughout (see `scripts/run-live-sway-tests`).
- Per-iteration poll interval (how often to call `get_tree()` within the grace period) is a
  separate tunable from the grace period itself — needs its own justification (cheap enough on a
  local Unix socket, but a busy loop with no sleep at all would be wasteful).

## Rollout suggestion

1. Prototype `Split` alone (simplest matcher, no no-op ambiguity) — validate the poll-loop
   mechanics and grace-period fallback shape against live Sway before generalizing.
2. `Height`/`Width` together (share the tolerance-matching logic).
3. `Position`.
4. `NewColumn`/`NewRow` last (structural diff, interacts with the existing multi-monitor guard).

Land each as its own commit/change, not one big-bang rewrite of `run_wait_time()` — consistent with
this project's usual small-concern-per-commit convention.

## Testing (per CLAUDE.md's live-Sway coverage policy)

Every action touched needs `tests/live_sway.rs` coverage for **both** paths:

- Fast path: a scenario where the action causes a real, observable change — assert it returns
  quickly (well under the grace period) with the change confirmed.
- Fallback path: the known no-op scenario for that action (solo-window resize, tiled-window
  position, edge-of-tree new-column/new-row) — assert it still returns promptly (via the grace-period
  fallback) rather than hanging or erroring, mirroring the existing
  `new_column_and_new_row_complete_promptly_when_already_at_the_edge`-style timing assertions.

## Open questions for whoever picks this up

- Is the added `get_tree()` polling overhead (however small on a local socket) worth the latency
  win, given this tool's actual usage pattern (a handful of actions per invocation, not a hot
  loop)? Worth benchmarking before/after on a real Sway session, not just headless.
- Should the grace period be user-configurable (a new CLI flag) or hardcoded like
  `PID_MARKER_FALLBACK_GRACE`? Leans toward hardcoded for consistency with the existing precedent,
  but worth confirming with the user.
- Does this change `--verbose`'s diagnostic output shape (e.g. worth logging "confirmed via poll"
  vs. "fell back to wait-time" the way `run_wait_matching_exec_event()` logs its
  marker-confirmed/fallback distinction)? Probably yes, for consistency.

## Docs to update once implemented

- CLAUDE.md's "Core model: `SwayAction`" section (the "No event exists in Sway IPC for it" bullet)
  — needs to describe the new poll-then-fallback dispatch, mirroring how the `Exec` PID-marker
  mechanism is documented.
- README.md's "Wait time" section — the "effective delay is roughly double `--wait-time`" note
  would need updating once actions can complete faster than that via polling.
