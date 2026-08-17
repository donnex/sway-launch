# Plan: poll-based completion for the wait-time actions

**Status:** done (2026-08-16) — all six variants (`Split`, `Height`, `Width`, `Position`,
`NewColumn`, `NewRow`) now confirm via `SwayAction::poll_matches()`/`run_poll_then_fallback()` in
`src/sway_launch.rs` instead of always blind-sleeping `2 * --wait-time`. See "Per-action matching
logic" below for what each variant actually checks, and "Corrections found during implementation"
for two assumptions this doc originally got wrong that live-Sway testing caught before they
shipped. `WAIT_TIME_POLL_GRACE`/`WAIT_TIME_POLL_INTERVAL` were hardcoded (200ms/10ms) per the
"Open questions" resolution below, not exposed as a CLI flag.

## Corrections found during implementation

Two assumptions in earlier drafts of this doc turned out to be wrong once checked against a real
compositor — both caught via manual `swaymsg`/`get_tree` probing against a throwaway headless
compositor before the corresponding matcher shipped, not after. Validate any future matcher the
same way rather than trusting an assumption about which node an action's effect actually lands on,
or what Sway actually does for a given command.

- **`Split`'s own tree node never reflects its split direction.** Splitting a window with siblings
  wraps it in a *new* split container one level up (whose `layout` is the requested direction);
  splitting a solo window sets the `layout` of the *workspace* it's already the sole child of,
  directly. Either way, the node to poll is `container_id`'s **parent**, not `container_id`
  itself — implemented as `parent_node_layout()`/`find_parent_layout()`.
- **A tiled window's `--position` isn't a silent no-op.** The original plan (and README, before
  this correction) assumed it was, the same way a solo window's `--height`/`--width` clamp is.
  Live testing found Sway instead rejects the command outright ("Only floating containers can be
  moved to an absolute position"), which `run_sway_command()`'s `?` propagates as an error
  *before* `poll_matches()`/`poll_baseline()` are ever reached — there's no poll-then-fallback
  case to write for this at all. `position_matches()` still has no special-cased "not floating"
  branch (unnecessary — see "Per-action matching logic" below), and
  `tests/live_sway.rs`'s `position_errors_clearly_for_a_tiled_window` confirms the pre-existing
  error path is unaffected by this feature.

A third, more significant correction surfaced while writing the `NewColumn`/`NewRow` fallback-path
test — not wrong in this doc, but a real pre-existing bug in `relocates_to_another_output()`
(unrelated to polling, existed before this feature) that blocked writing a reliable test until
fixed. See "`relocates_to_another_output()` bug found and fixed" below.

## Problem

Six `SwayAction` variants have no corresponding Sway IPC event to confirm completion
(`matching_window_change_events()` returns `None` for them), so `run_wait_time()` used to just send
the command and sleep `--wait-time` before and after, blind to whether anything actually happened:

- `Split` (`--split h/v`)
- `NewColumn` (`--new-column`, `move right`)
- `NewRow` (`--new-row`, `move down`)
- `Height` (`--height`)
- `Width` (`--width`)
- `Position` (`--position`)

This was honest about not knowing, but meant every one of these actions always cost the full
`2 * --wait-time` (20ms default, each direction) even when the change happened instantly, and gave
no actual confirmation that anything took effect.

## The catch: several of these actions have legitimate no-op semantics

This couldn't be "poll until confirmed or hang" — each of these is a real, non-exotic case where
Sway accepts the command but the tree never reaches the "requested" state:

- **`Height`/`Width`**: resizing a window that is the *sole occupant* of its workspace is silently
  clamped by Sway — the container stays at 100%. This is exactly why
  `examples/templates/master-dual-stack.toml` and `sidebar-left.toml` had to put their `width` on
  the *second*-launched slot, not the first (see their header comments and
  `tests/live_sway.rs`'s `height_alone_resizes_a_non_solo_window`, which specifically launches a
  sibling first to avoid this).
- **`NewColumn`/`NewRow`**: a no-op when the window is already at the tree's rightmost/bottommost
  edge — the *ordinary* two-window case, and the exact reason these were moved off
  `WindowChange::Move` event-confirmation in the first place (see
  `matching_window_change_events()`'s comment and
  `tests/live_sway.rs`'s `new_column_and_new_row_complete_promptly_when_already_at_the_edge`).
- **`Position`** turned out *not* to belong on this list — see "Corrections found during
  implementation" above. It errors, rather than silently no-oping, on a tiled window.

If polling waited for the tree to reach the "expected" state unconditionally, these cases would
hang until some poll-timeout — the same fundamental ambiguity (can't tell "hasn't happened yet"
from "happened as a no-op") that killed the event-based approach for `NewColumn`/`NewRow`, just
relocated from the event stream to tree-diffing.

## Design: bounded poll with fallback to today's behavior

Don't wait-until-confirmed-or-error. Poll for a **short, fixed grace period**
(`WAIT_TIME_POLL_GRACE`, much shorter than `--timeout`) for the expected tree state; if it
converges, return immediately (fast path). If the grace period elapses without convergence, fall
back to the original behavior — assume success, sleep the remainder of `--wait-time`, return —
rather than erroring. This mirrors the pattern already used for `SwayAction::Exec`'s PID-marker
fallback (`PID_MARKER_FALLBACK_GRACE`, `run_wait_matching_exec_event()` in `src/sway_launch.rs`):
try to confirm precisely, fall back gracefully within a bounded extra delay if confirmation never
comes, never regress into a hang. Implemented as `SwayAction::poll_matches()` +
`run_poll_then_fallback()`, called from `run_wait_time()`.

## Per-action matching logic

Each action's own definition of "confirmed", as shipped:

- **`Split`** — `parent_node_layout()`/`find_parent_layout()` compare the container's *parent*
  node's `layout` field to the requested `Split::H`/`Split::V` (see "Corrections found during
  implementation" above for why it's the parent, not the container itself). No known no-op case:
  setting a split direction the container's parent already has is idempotent and matches on the
  very first poll. Confirmed live by `tests/live_sway.rs`'s
  `split_confirms_via_poll_well_under_a_large_wait_time`/
  `split_is_idempotent_and_still_confirms_promptly_when_already_set`.
- **`Height`/`Width`** — `parse_pixel_value()` opts a value out of polling entirely (`None`, not
  `Some(false)`) unless it's in `px`; a `ppt` percentage has no pixel figure to poll for without
  also resolving the reference dimension it's a percentage of. For `px` values,
  `height_matches()`/`width_matches()` (via `node_by_id()`) needed tolerance-aware matching, not
  one fixed formula — live testing found the border/decoration accounting genuinely inconsistent
  for width specifically (this project never found a single deterministic rule for it):
  - A window resized while it's been floating since the very command that floated it matches
    `rect.width` exactly (see `floating_with_width_and_height_applies_all_three`).
  - A window that had already been tiled for a while before being floated and resized comes out
    `2 * current_border_width` short on `rect.width` alone (confirmed in
    `retarget_by_id_layout_floats_the_first_step_by_name`). `width_matches()` accepts either
    formula rather than picking one.
  - Height had no such inconsistency: `rect.height + deco_rect.height` (the decoration-inclusive
    outer height) held in both the freshly-floated and plain-tiled cases (confirmed in the same
    test and `height_alone_resizes_a_non_solo_window`).

  The solo-window clamp no-op falls back gracefully with no special-case code — the poll simply
  never matches, and `run_poll_then_fallback()`'s grace period elapses. Confirmed live by
  `tests/live_sway.rs`'s `height_confirms_via_poll_when_resized_with_a_sibling`/
  `width_confirms_via_poll_when_resized_with_a_sibling` (fast path) and
  `height_and_width_fall_back_gracefully_when_solo_window_clamps_the_resize` (fallback path).

  **Accepted residual risk (found in a later code-review pass):** `width_matches()`'s two-formula
  tolerance widens, slightly, the odds that the very first poll — which runs immediately after the
  command, with no minimum settle time — could coincidentally match a *pre-resize* width against
  the *newly requested* one, reporting "confirmed" before Sway has actually processed the command.
  Judged low-risk enough not to warrant requiring a genuine change (a `poll_baseline()`-style
  snapshot, like `NewColumn`/`NewRow` use) purely for this; see `width_matches()`'s own doc comment
  in `src/sway_launch.rs`.
- **`Position`** — `position_matches()` compares `deco_rect.x`/`deco_rect.y` (the
  decoration-inclusive frame `move position` actually targets, confirmed in
  `position_moves_a_floating_window_to_given_coordinates`) against either the parsed `<x>,<y>` or,
  for `center`, a computed target (`compute_center_position()`, using a second `get_outputs()`
  call for the output's own rect — `get_tree()` alone doesn't carry output geometry — centered
  against the window's current outer footprint, confirmed in
  `position_center_centers_a_floating_window`). No "not floating → skip to fallback" branch was
  needed, unlike originally planned — see "Corrections found during implementation" above: Sway
  errors outright for a tiled window, so `poll_matches()` is never reached for that case at all.
- **`NewColumn`/`NewRow`** — the one pair with no fixed target to check against (a successful move
  can land the window almost anywhere in the tree). These are the only variants using
  `poll_baseline()`: `run_wait_time()` snapshots the container's own `rect` (via `node_by_id()`)
  *before* sending the command; `poll_matches()` compares the *current* `rect` against that
  snapshot afterward. An earlier design (structural: snapshot the parent's full children-id list,
  poll for it to differ) was live-tested and found to **false-positive on exactly the documented
  no-op edge case** — Sway can still incidentally restructure *other* siblings there (wrapping one
  in a new split container) even though the target window's own `rect` never changes, which would
  have made the poll "confirm" a no-op as if it were a real move. Comparing the target's own `rect`
  instead avoids this. Confirmed live by `tests/live_sway.rs`'s
  `new_column_confirms_via_poll_when_swapping_past_a_sibling`/
  `new_row_confirms_via_poll_when_swapping_past_a_sibling` (fast path, a real sibling swap) and
  `new_column_falls_back_gracefully_at_the_edge_with_a_large_wait_time` (fallback path).

## `relocates_to_another_output()` bug found and fixed

While writing `new_column_falls_back_gracefully_at_the_edge_with_a_large_wait_time` (a *non-solo*,
two-window "at the edge" scenario), manual `swaymsg`/`get_tree` probing against a live 3-output
compositor found that `relocates_to_another_output()`'s original guard — "is `container_id` the
only window in its workspace" — was too narrow. A **non-solo** workspace can escalate too: two
windows side by side, `[con_id=<rightmost>] move right` relocated it to a different output, not a
same-workspace no-op, even with a sibling to its left. This is a pre-existing bug, unrelated to
polling (it exists in `SwayLaunch::run()`'s pre-action guard, entirely separate from
`run_wait_time()`), but blocked writing a reliable test until fixed, so it was fixed in the same
change rather than deferred.

Fixed by replacing the child-count check with `is_at_the_trailing_workspace_edge()`: `container_id`
is a *direct* child of its workspace (not nested in a sub-container), the workspace's own `layout`
matches the move's axis (`SplitH` for `NewColumn`, `SplitV` for `NewRow`), and it's the *last*
child in that list. This subsumes the original solo-window case (trivially both direct- and
last-child of its workspace) while also catching the multi-window case the old check missed. It
also *avoids* a new false positive the old check didn't have: a solo window whose workspace layout
doesn't match the axis (e.g. stacked via `splitv`, then moved right) was confirmed live to
restructure in place rather than escalate, so the old "solo = always skip" rule would have blocked
a move that was actually safe. A window nested inside a sub-container is conservatively never
flagged — that case wasn't confirmed live either way, so this only guards the confirmed risk.
Confirmed live by `tests/live_sway.rs`'s
`new_column_does_not_relocate_a_solo_window_to_a_different_output` (existing, still passes) and
`new_column_does_not_relocate_a_non_solo_window_at_the_trailing_edge` (new).

## Constants / config

- `WAIT_TIME_POLL_GRACE` = 200ms, `WAIT_TIME_POLL_INTERVAL` = 10ms, both hardcoded in
  `src/sway_launch.rs`, naming pattern borrowed from `PID_MARKER_FALLBACK_GRACE`. Not
  re-benchmarked per-action after `Split`'s initial tuning — see "Open questions" below.

## Testing (per CLAUDE.md's live-Sway coverage policy)

Every action has `tests/live_sway.rs` coverage for both paths where applicable:

- Fast path: a scenario where the action causes a real, observable change, asserted to return
  quickly (well under `2 * --wait-time`) with the change confirmed.
- Fallback path: the known no-op scenario for that action (solo-window resize, edge-of-tree
  new-column/new-row) — asserted to still return without hanging or erroring. `Position` has no
  fallback-path test since it turned out to have no no-op case (see "Corrections" above); it has
  an error-path test instead (`position_errors_clearly_for_a_tiled_window`).

## Open questions — resolved (2026-08-16)

- **Grace period configurability:** hardcoded, matching `PID_MARKER_FALLBACK_GRACE`'s precedent —
  no new CLI flag.
- **`--verbose` logging:** yes — `run_poll_then_fallback()` logs "Confirmed via poll" or "Poll
  grace period elapsed without confirmation, falling back to wait-time", mirroring
  `run_wait_matching_exec_event()`'s marker-confirmed/fallback logging.
- **Polling overhead vs. latency win:** not separately benchmarked per-action beyond `Split`'s
  initial live-Sway timing checks (each new matcher's fast-path test confirms it completes well
  under `2 * --wait-time`, which is the property that actually matters for this tool's usage
  pattern — a handful of actions per invocation, not a hot loop). No further formal benchmarking
  was judged necessary once all six variants confirmed this live.

## Docs updated

- CLAUDE.md's "Core model: `SwayAction`" section (the "No event exists in Sway IPC for it" bullet)
  now describes all six variants' poll-then-fallback dispatch, mirroring how the `Exec` PID-marker
  mechanism is documented. Its "Orchestration: `SwayLaunch::run()`" section describes the
  `relocates_to_another_output()` fix. Its Rust-conventions exemption list gained the new
  IPC-touching helper functions.
- README.md's "Wait time" section describes the fast path/fallback split generally (no longer
  singling out `--split`). Its "Position" section notes the tiled-window error behavior.
