# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`sway-launch` is a CLI tool for the [Sway](https://swaywm.org/) window manager. It launches an
application, waits for its window to appear via the Sway IPC event stream, then optionally runs
follow-up actions against that window (floating, split, resize, move to new row/column, mark).
Because it blocks until the window exists (and until each follow-up action completes), it's
designed to be chained in shell scripts to deterministically build up window layouts without
manual `sleep`s. See `README.md` for full CLI usage and layout-building examples.

## Commands

- Build: `cargo build`
- Run: `cargo run -- [OPTIONS] [COMMAND]` (e.g. `cargo run -- -a kitty kitty`)
- Format: `cargo fmt`
- Lint: `cargo clippy`
- Test: `cargo test` — runs the unit tests in `src/sway_launch.rs` and `src/main.rs`, covering all
  pure/logic functions (see the Testing bullet under Rust conventions for what's exempted and
  why). The IPC-touching functions that can't run headless are exercised manually by running the
  scripts in `examples/` (see below) against a live Sway session.
  - Run a single test: `cargo test <test_name>`
  - Run with debug output: `cargo test -- --nocapture`

See the CI section below for how GitHub Actions runs these same checks.

## Architecture

The crate is two files:

- `src/main.rs` — defines the `clap`-derived `Args` struct (CLI flags), validates them (e.g.
  `--height`/`--width` must match `\d+(px|ppt)`), and constructs a `sway_launch::SwayLaunch`.
- `src/sway_launch.rs` — all the actual logic.

### Core model: `SwayAction`

Every CLI flag maps to a `SwayAction` enum variant (`Exec`, `Split`, `Floating`, `NewColumn`,
`NewRow`, `Mark`, `Height`, `Width`). Each variant knows how to:

- render itself as a `swaymsg` command string (`sway_command()`) — `Mark`'s value is wrapped
  through `quote_sway_string()` before interpolation, since Sway's command parser splits on
  unquoted `,`/`;` and an unescaped mark could otherwise inject additional commands; `Height` and
  `Width` don't need this since they're already regex-constrained in `main.rs`, and `Exec`'s
  command is passed through unquoted by design (the tool's whole job is to run it)
- declare which `WindowChange` event(s) would confirm it completed (`matching_window_change_events()`)
- declare its IPC event subscription (`event_subscription()`)

`SwayAction::run()` dispatches based on whether the action has a corresponding IPC event:

- **Has an event** (`Exec`, `Floating`, `NewColumn`, `NewRow`, `Mark`) → `run_wait_matching_events()`:
  connects to Sway, sends the command, then reads the event stream until a `Window` event matches
  (checked via `matches_window_event()`, e.g. app_id/class for `Exec`, container id for others), or
  the `--timeout` is hit.
- **No event exists in Sway IPC for it** (`Split`, `Height`, `Width`) → `run_wait_time()`: sends the
  command and sleeps for `--wait-time` before and after, since Sway doesn't emit an event to
  confirm these.

### Orchestration: `SwayLaunch::run()`

Runs `Exec` first (always) to launch the command and obtain the new window's `container_id`, then
conditionally runs the other actions in a fixed order (`NewColumn` → `NewRow` → `Split` →
`Floating` → `Height` → `Width` → `Mark`) based on which CLI flags were set, each against that same
`container_id`. The final container id is printed to stdout — this is what makes commands
chainable/scriptable (see README examples).

Each Sway IPC call opens its own fresh `Connection` (`new_connection()` in `sway_launch.rs`) — there
is no persistent/shared connection across actions.

### `--debug-events`

`SwayLaunch::debug_events()` subscribes to all Sway IPC event types and prints every event until
killed. Useful for discovering event shapes when adding a new action.

## Example layout scripts

`examples/` holds tracked, user-facing example scripts, each a small standalone shell script built
out of `sway-launch` calls that demonstrates one layout (`dual-terminals`, `triple-row`,
`column-split`, `quad-terminals`). README.md's "Recreatable layouts" section links to these; they
are full scripts a user runs directly, so they follow every Scripts/Shell convention below,
including `-h`/`--help`. Keep this set and README's list of them in sync when either changes.

There is no separate ad-hoc/scratch scripts directory — a prior `layout-tests/` served that
purpose (untracked, personal iteration history) but was removed once its useful layouts had all
been polished into tracked `examples/` scripts. If a similar need for throwaway manual-verification
scripts comes up again, recreate it under the same untracked-scratch-space conventions this section
used to document, rather than letting one-off verification scripts accumulate in `examples/`
unpolished.

## Rust conventions

- Always target the latest stable Rust release, and verify the toolchain is current before
  committing. Check with `rustup check`.
- Only use a nightly toolchain, or a pre-release dependency version, if it is strictly necessary —
  confirm with the user first.
- Use the latest stable release versions of dependencies.
- For a program a user runs directly, use color and clear presentation — structured formatting,
  tables, progress/status indicators, etc. — for its terminal output when it makes sense: status
  messages that should stand out (success/warning/error), progress, or tabular data. Skip it for
  something too small to warrant it, e.g. a program whose entire output is a single line.
- Default to no comments in code — clear naming and control flow should carry the intent by
  themselves. Doc comments (`///`, `//!`), where the project requires them (e.g. via
  `#![warn(missing_docs)]`), are a separate, always-required matter, not part of this rule. Add a
  regular `//` comment only when the code is genuinely hard to follow without one: a non-obvious
  constraint, a workaround for a specific bug or quirk, or logic that isn't self-evident from
  reading it — in that case, add the comment rather than leaving a future reader to puzzle it out.
- Use `cargo fmt` for formatting and `cargo clippy` for linting. Both must pass with no errors
  before committing.
- Aim for high test coverage on every new feature or fix; agree the target threshold with the user
  per project and keep coverage at or above it as code is added, rather than letting it slip. It's
  fine for genuinely untestable paths (e.g. something that can't run headless) to stay uncovered —
  note why in the project's issue tracker rather than forcing a brittle test.
  - This project's agreed target: cover every pure/logic function (command-string building, event
    dispatch tables, window-match logic, CLI argument validation/parsing). The functions that
    open, read, or write the Sway IPC socket directly (`new_connection`, `event_loop`,
    `run_sway_command`'s connection call, `run_wait_time`, `run_wait_matching_events`,
    `SwayAction::run`, `SwayLaunch::run`, `SwayLaunch::debug_events`) are exempted — they require a
    live Sway compositor, can't run headless in GitHub Actions CI, and are exercised manually by
    running `examples/` scripts instead. No mocking layer has been introduced for these on the
    judgment that a trait-based abstraction purely to unit-test thin IPC wiring isn't worth the
    added indirection for a tool this size; revisit if the IPC-touching logic grows more complex
    than it is today.
- Measure coverage with `cargo llvm-cov` (requires the `cargo-llvm-cov` subcommand and the
  `llvm-tools-preview` rustup component):
  - `cargo llvm-cov --summary-only --ignore-filename-regex 'main\.rs'` — summary
  - `cargo llvm-cov --html --ignore-filename-regex 'main\.rs'` — line-by-line HTML report
  - Exclude `main.rs` (or an equivalent thin entry-point file) from coverage accounting if it's
    mostly wiring that's better exercised by integration tests than unit tests.
- UI/rendering tests should assert on rendered output content, not styling or presentation
  details, so tests survive cosmetic tweaks.

### Rust workflow

- After making changes, run `cargo fmt` and `cargo clippy` and fix all findings before committing.
- Run the full test suite (`cargo test`) before committing.
- Integration tests that drive a compiled binary (e.g. under a pseudo-terminal) require the binary
  to be built first — run `cargo build` before running them.
- When making an architectural or behavioral change (new module, new data flow, changed data
  types, new dependency, new major UI/output element), update this file's Architecture section in
  the same piece of work. Don't let it drift out of sync with the implementation.

## Scripts

These rules apply to every script in this repo, regardless of language.

- Always make executable scripts executable with `chmod +x`.
- Scripts a user runs directly must implement `-h`/`--help`. When passed, print usage to stdout
  and exit 0. When called due to invalid usage, print to stderr and exit 1.
  - Scripts whose intended caller is another program — hooks, daemons, a status bar's `exec`
    target — are exempt. The test is who invokes it in normal use, not whether a human *could*.
    Being runnable by hand for testing doesn't make a script user-run; if it did, nothing would
    ever be exempt.
  - Such a script still needs its header comment to explain how it's invoked and by what, since it
    has no `--help` to carry that. Configuration it reads from the environment belongs there too.
- For a script a user runs directly, use color and clear presentation — structured formatting,
  tables, progress/status indicators, etc. — for its terminal output when it makes sense: status
  messages that should stand out (success/warning/error), progress, or tabular data. Skip it for
  something too small to warrant it, e.g. a script whose entire output is a single line.
- Always include a brief description at the top of every file:
  - Shell: three comment lines directly below the shebang — a blank `#`, a one-line summary, a
    closing blank `#`.
  - Python: a module docstring at the very top of the file with a one-line summary.
- Default to no comments in code — clear naming and control flow should carry the intent by
  themselves. The file-header description above (and a docstring, where the language's own
  convention requires one) is a separate, always-required matter, not part of this rule. Add an
  inline comment only when the code is genuinely hard to follow without one: a non-obvious
  constraint, a workaround for a specific bug or quirk, or logic that isn't self-evident from
  reading it — in that case, add the comment rather than leaving a future reader to puzzle it out.
- Use long and descriptive names — avoid abbreviations (e.g. `command` not `cmd`, `character` not
  `char`).
- Handle errors explicitly and exit early — check results and surface a clear message as soon as a
  failure is detected. Never let a script continue in a broken or partially-completed state.
- Never hardcode a secret, credential, API key, or token in a script. Read it from the environment
  or from a local config file excluded via `.gitignore`, and never print it to stdout/stderr or
  write it into a log, commit, issue, or any other document. Prefer a mechanism that doesn't place
  it in a subprocess's argument list when the tool supports one (a value read from a file, stdin,
  or an env var passed to the subprocess) — a bare CLI argument is visible to other processes on
  the host for the argument's lifetime (e.g. via `ps`).
- When an operation may fail transiently (e.g. a network call), implement retry logic rather than
  failing on the first attempt. If it's unclear whether the operation is safe to retry (e.g. it
  isn't idempotent), confirm with the user before adding retry behavior.
- All intermediate files must be created inside a temporary directory and cleaned up on exit,
  including on errors and signals:
  - Shell: use `mktemp -d` and clean up with `trap cleanup EXIT`.
  - Python: use `tempfile.TemporaryDirectory()` as a context manager.
- After making changes to a script, review the help output and verify it accurately reflects the
  current arguments, options, and behaviour.

## Shell scripts

Run a script directly with `sh <script>`; make it executable first with `chmod +x <script>`.

### Shell conventions

- Only POSIX sh should be used. When a non-POSIX feature would simplify the code, confirm with the
  user before using it. If the POSIX alternative is significantly more complex, or the script does
  not need to run on multiple OSes, suggest the simpler non-POSIX solution and ask the user to
  confirm before falling back to it.
- Always use `printf` instead of `echo`
- Always pass a literal format string to `printf` — never `printf "$var"` (a `%` in the value
  causes silent bugs). Use `printf '%s\n' "$var"` instead.
- Quote every variable expansion and command substitution by default — `"$var"`, `"$(cmd)"` —
  including on the right-hand side of an assignment (`host="$(hostname -f 2>/dev/null ||
  hostname)"`). POSIX assignment doesn't word-split, so this one is for consistency, not
  correctness, but keep it uniform. The sole exception is deliberate word-splitting — e.g.
  iterating a space-separated list (`for recipient in $RECIPIENTS`) — which must stay unquoted;
  `shellcheck` is the backstop for catching the unintentional cases.
- Always use `read -r` — without `-r`, backslashes in input are interpreted, which is almost never
  the intent.
- Use single quotes `''` for static strings that don't need variable expansion
- All scripts must have a `usage()` function and an error-reporting mechanism (`die` by default,
  per the Error helper pattern). Three exceptions: scripts so small that these would add more
  noise than value (confirm with the user before omitting them); scripts exempt from `-h`/`--help`
  under the Scripts conventions above — those have no usage to print, so they need only the
  error-reporting mechanism; and a harness-invoked hook/daemon/action script whose stdout or exit
  code is itself part of a contract with its caller (e.g. a status line that must always exit 0
  and never write to stderr, or an action script whose plain stdout is surfaced as another tool's
  failure message) — `die`'s colored, stderr-and-exit-1 behavior would break that contract, so
  this third category is exempt from both `usage()` and the error-reporting mechanism.
- Always use `set -eu` at the top of every script. Be aware that `set -e` does not catch failures
  inside pipelines — in `cmd1 | cmd2` only `cmd2`'s exit code is checked. Handle pipeline errors
  explicitly by storing intermediate output in a variable or temp file rather than piping
  directly.
- Never parse the output of `ls` — use glob patterns (`for file in ./*.txt`) or `find` instead.
- Use 2 spaces for indentation
- Use uppercase variable names only for static settings and constants, not for regular variables.
  Declare them with `readonly`: `readonly VAR=value`
- Prefer a single-line `&&`/`||` guard over a full `if`/`then`/`fi` block for a simple,
  single-action condition (e.g. `[ "$var" = 1 ] && printf 'ok'`) — it keeps the script shorter and
  cuts down on block count. Only do this when it stays readable; fall back to a full `if` block
  once the condition or action gets complex.
- For a guard like `... || die ...`, let line length decide the layout. When the command before
  the guard is short, keep the whole thing on one line (`command -v jq >/dev/null 2>&1 || die "jq
  not found"`). When the command is long or complex — a pipeline, a command substitution, an
  interactive prompt — put the `||` at the end of the command line and the action on the next
  line, indented, so the guard doesn't run past a comfortable width.
- Do not cram multiple statements into a one-line `{ }` block (e.g. `foo() { cmd1; cmd2; cmd3;
  }`) — it hurts readability. Keep one-liner `{ }` blocks to a single statement in regular script
  code; once a function needs more than one statement, write it as a normal multi-line function.
  Snippet functions are the exception — see Reusable snippets below, they're never one-liners
  regardless of statement count.
- Aim for reusable building blocks, but never as standalone scripts other scripts call out to at
  runtime — see Reusable snippets below. Before implementing any non-trivial functionality, check
  this repo's `snippets/` library (if one exists yet) for something that already does it and copy
  it in. If something you're implementing feels reusable, extract it into a single-function
  snippet and add it to the snippet index below.
- Use a shared `retry` snippet for retry logic instead of a bespoke loop — extract one into
  `snippets/` if one doesn't exist yet.
- Before a script comes to depend on a new external command not already used elsewhere in the
  project, confirm the choice with the user first. This excludes standard POSIX/base-OS utilities
  guaranteed present on the target system (e.g. `sort`, `cut`, `awk`); confirmation is for
  genuinely new tooling (e.g. `jq`, `httpie`, `fzf`), not coreutils.
- Do not use a `.sh` file extension
- Name scripts in lowercase. Use `-` as word separator for scripts run directly by the user (e.g.
  `run-commands`). No separator for scripts called from other scripts (e.g. `runcommands`). If
  unsure, ask. Snippet files are named after the function they contain (function names can't
  contain `-`, so this is never ambiguous).

### Reusable snippets

Reusable code lives in a `snippets/` directory (create it the first time a script needs one), not
as standalone scripts. Each file in `snippets/` contains exactly one shell function — a short
usage comment above it, and nothing else: no shebang, no `set -eu`, no CLI wrapper (`-h`/`--help`,
its own `usage()`, etc.). A snippet is copied and pasted verbatim into the script that needs it; it
is never executed or sourced at runtime. This keeps every script self-contained (no dependency on
other scripts being on `PATH`) while keeping the *implementation* in one canonical place.

Snippet functions are always written in full multi-line form — never a single-line `{ ...; }`
block, even when the body is a single statement. This keeps every snippet visually consistent and
easy to diff, independent of how simple the function happens to be.

**Workflow**:

- Before implementing non-trivial functionality, check `snippets/` for a snippet that already does
  it, and paste it in.
- When something you're implementing feels reusable, extract it into `snippets/<name>` as a single
  function, note it in this file's snippet index (add one if this is the first snippet), then
  paste it into the script(s) that need it.
- Mark every pasted-in snippet function with a `# snippet: <name>` comment directly above its
  definition. This makes sync audits trivial: `grep -rn '# snippet:' .`.
- A pasted copy must match its snippet file exactly. If a script seems to need different behavior
  from a snippet it already uses, that means the *snippet* should change — update `snippets/<name>`
  first, then re-copy it into every script that uses it, so implementations never drift apart.
  Don't let a local copy silently diverge.
- Whenever you edit a file in `snippets/`, for any reason, immediately find every script with a
  matching `# snippet: <name>` comment (`grep -rln '# snippet: <name>' .`) and update each copy to
  match. A snippet change is not done until every consumer is resynced in the same change.
- When creating a new script or editing an existing one, check that any `# snippet: <name>`-marked
  function in it still matches the current `snippets/<name>` — resync if not.

### Shell workflow

- After making changes to a script, always review the summary comment below the shebang and update
  it if it no longer accurately describes what the script does.
- After making changes to a script, check every `# snippet: <name>`-marked function against its
  source in `snippets/<name>` — if you changed one, update the snippet and resync every other
  script that copies it.
- Run `shellcheck --shell=sh <script>` after making changes and fix any findings before committing.
- Run `shfmt -i 2 -w <script>` after making changes to format the script.
- Neither tool is guaranteed to be present in a fresh environment. Install them via the OS's
  package manager if missing (e.g. `sudo apt-get install -y shellcheck shfmt` on Debian/Ubuntu,
  `brew install shellcheck shfmt` on macOS) rather than skipping the checks.

### Testing

Most scripts here are small enough that careful manual testing during development is enough — a
dedicated test suite would cost more than it's worth. Once a script grows large or complex (many
options, non-trivial branching logic, behavior that's easy to regress silently without noticing),
ask the user whether to set up a shell test framework (e.g. `bats-core`) rather than assuming
either way.

## Git

### Branching

- Default to the `dev` branch for all work, unless a different branch has already been checked out
  or explicitly set for the current task — in that case, keep working on that branch instead of
  switching to `dev`.
- Never commit, amend, rebase onto, or push directly to `master`, and never push `dev` to trigger
  anything master-facing on your own initiative. `master` only moves via an explicit
  user-requested merge (see "Merging to master" below).
- Before starting work on a tracked issue or a larger/multi-commit piece of work, ask whether to
  create and switch to a new topic branch for it, rather than committing straight to `dev`.
- When the work is tied to a tracked issue — whether a local file-based checklist (e.g.
  `ISSUES.md`) or an external tracker (Gitea, GitHub, or similar) — name the topic branch with
  that issue's identifier: `issue-<number>-<short-kebab-case-description>` (e.g.
  `issue-123-fix-broken-parser`). This mirrors the `(#N)` suffix used in commit titles, so the
  branch, its commits, and the tracker entry are all traceable to each other, regardless of where
  the issue itself lives.
- Keep `dev` linear on top of `master`: rebase `dev` onto `master` (never merge `master` into
  `dev`), so `dev` stays a fast-forwardable descendant of `master`.

#### History rewriting

- History rewriting — amending, rebasing, force-pushing — is allowed on `dev` (and topic branches
  based on it), but only for commits not yet merged into `master`, and only as long as `dev` stays
  linear on top of `master` per the rule above. This overrides the general "always create new
  commits, never amend/force-push" default, but *only* for `dev`/topic branches — never rewrite
  `master` history.
- Aim to keep `dev`'s commit count low. Whenever a later commit would touch the same change as one
  already on `dev` and not yet merged to `master` — most often a fix or refinement found via
  testing or review of something just committed — rewrite the earlier commit(s) instead of
  stacking a new one on top: `git commit --amend` for the tip commit, or a soft-reset-and-recommit
  for an earlier one (Claude Code's tooling disallows interactive git flags, so `git rebase -i`
  isn't an option). Do not carry multiple commits into a merge to `master` that are really just
  successive fixes or changes to the same not-yet-merged work — combine them into the commit(s)
  they fix before merging. This is about the same change accruing fixes over time, not about
  splitting distinct work: the one-commit-per-concern rule under Commits still applies to
  genuinely separate concerns. Keep each commit correct and self-contained, since it hasn't
  shipped yet; re-run lint and push with `--force-with-lease` afterward per the rule below. Once a
  commit is merged to `master`, this no longer applies — fix it forward with a new commit as
  usual.
- A topic branch gets the same treatment relative to `dev` that `dev` gets relative to `master`:
  rewrite its commits — amend, or soft-reset and recommit — to fold in fixes and refinements found
  while working on it, rather than stacking new fix-up commits on top. Clean it up before merging
  into `dev`, the same way `dev` gets cleaned up before merging into `master`, so what lands on
  `dev` is already tidy rather than a commit plus a trail of its own fixes.
- Always push `dev` after committing or rewriting its history, so the remote never lags local. The
  same applies to a topic branch: push it after every commit made on it, not only once it's ready
  to merge, so it's never sitting local-only. When history was rewritten (amend/rebase), push with
  `--force-with-lease` (never a bare `--force`), so the push fails safely instead of clobbering
  anything unexpectedly added to the remote branch since the last fetch.

#### Merging to master

Merging to `master` happens only on explicit request, as a sequence:

1. Present a summary of what's about to land — the commit range (`git log --oneline
   master..dev`) and a nutshell of what changed.
2. Get explicit confirmation on that summary. Asking for the merge and confirming its contents are
   two separate steps; don't collapse them just because the user already said "merge."
3. Once confirmed, fast-forward `master` to `dev`: `git merge --ff-only dev` — no merge commit, no
   squash, since `dev`'s history is already linear and clean. If there's no local `master` checkout
   to merge into, push `dev`'s tip directly to `master` on the remote instead.
4. If a true fast-forward isn't possible (something moved `master` independently), stop and ask
   rather than falling back to a merge commit or force-push.

Remind the user to merge when it seems due: when a large feature/fix on `dev` looks finished, or
`dev` has accumulated a lot of commits ahead of `master`, say so and suggest merging. This is a
reminder only — never merge to `master` automatically or without explicit confirmation, no matter
how done the work looks or how many commits have piled up.

#### Branch cleanup

Once a topic branch has been merged into `dev`, delete it — both the local branch and its `origin`
remote counterpart (`git branch -d <branch>`, then `git push origin --delete <branch>`) — no need
to ask first. A merged branch is fully redundant the moment its commits live on `dev`; its only
purpose was getting them there. `dev` and `master` themselves are never deleted — this applies
only to topic branches.

### Commits

- Every commit Claude Code creates must end with a `Co-Authored-By:` trailer identifying the
  active model, e.g. `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>`. Never omit this —
  if a commit is later found to be missing it and hasn't been merged to `master` yet, fix it via
  the history-rewriting rules above rather than leaving it out.
- One commit = one concern, not necessarily one file. Bundle files that share the same concern
  into a single commit — a source change routinely spans several files, and this applies to
  documentation too: if a single change is documented across more than one file (e.g. a design doc
  and `README.md` both describing the same change), commit them together rather than splitting per
  file. Keep genuinely separate concerns in separate commits even when they land together for the
  same task — don't bundle work together just because it happened at the same time if the concerns
  themselves are distinct.
- Prefix commit titles with the **scope** of the change, followed by a colon and an uppercase
  description. Match the prefix casing to the actual file or directory name. One file → its exact
  path (`src/main.rs:`, `CLAUDE.md:`). Several files sharing one concern → the directory that
  contains them (`src:`). End the title with the issue number when the work is tied to a tracked
  issue: `(#12)`.
- Keep the title under 80 characters. Only exceed this if absolutely unavoidable.
- Put detailed descriptions in the commit body, not the title. For a commit spanning several
  files, use a one- or two-line prose summary followed by per-file bullets (`- src/main.rs: ...`)
  saying what each file's part does.
- Examples: `src/main.rs: Add retry prompt`, `CLAUDE.md: Add indentation rule`
- When adding a new file with no meaningful description, use `filename: Add file`. If there is a
  reason worth stating, describe it instead: `src/sway_launch.rs: Add height/width resize action`
- Never modify `.gitignore` files without explicit confirmation from the user.
- Before every commit, all lint/format checks required by this file (`cargo fmt`/`cargo clippy`
  per Rust conventions, `shellcheck`/`shfmt` per Shell scripts, and `markdownlint` per Workflow
  below) must pass with zero errors. Never commit with outstanding failures — fix them, or ask the
  user how to handle a check that genuinely should be skipped.

## Workflow

- Commits to `dev` or any other in-progress branch do not need confirmation before committing —
  proceed directly once the change is ready. After the commit completes, show a summary of what
  changed and why. If the work produces multiple commits, commit each as its own concern per Git
  conventions above, then show one summary covering the full set once they are all done.
- Before committing any `.md` file, run `markdownlint <file>` and fix all findings. The file must
  pass with no errors before it is committed.
- Disable MD013 (line-length) in `.markdownlint.json` — prose and instruction files can't
  reasonably be wrapped at 80 characters. Once set, don't remove it or attempt to reformat files to
  satisfy it.
- If the project generates a review report (e.g. a code-review agent instruction that writes
  `REVIEW.md`), leave that file intentionally untracked (not gitignored) in the repo root. Its
  presence signals that a review has been generated and needs to be processed. Never add it to
  `.gitignore`.
- Before running a prompt that generates `REVIEW.md` (e.g. a deep-code-review or an
  agent-instructions audit prompt), check whether an untracked `REVIEW.md` already exists in the
  repo root. If it does, ask whether it has already been processed — its presence signals
  unfinished review work per the rule above. If the user confirms it's been processed, proceed
  (the new prompt will overwrite it); otherwise stop and let the user decide how to handle the
  existing report before running the new prompt.
- When a large task wraps up, or context usage is running high (roughly 20% or more used), remind
  the user to run `/clear` to start the next task with a fresh context. Output the reminder as a
  `> [!WARNING]` markdown callout so it stands out from surrounding prose. This is a reminder
  only — `/clear` is a built-in CLI command, not something invocable through a tool, so it can
  never be run on the user's behalf.

## CI

### Suggesting CI setup

If this project has no CI workflow configured yet, suggest setting one up — don't wait to be
asked. This is a suggestion, not something to set up unprompted: propose it and let the user
decide.

- Check `git remote -v` to see which host(s) the project actually uses. Suggest a workflow for
  each one present: GitHub Actions (`.github/workflows/`) for a `github.com` remote, Gitea Actions
  (`.gitea/workflows/`) for a self-hosted Gitea remote. If the project is pushed to both, suggest
  both — the two are close enough in syntax that a workflow's content is largely shareable between
  them as-is.
- The workflow must run every check the project actually enforces before a commit or change is
  considered clean — derive this from this file's own conventions (the Rust conventions and Shell
  scripts sections above), not a generic template. For example: `cargo fmt --check`, `cargo
  clippy`, `cargo test`, `shellcheck`, `shfmt -d`, and any build step — run linters/formatters in
  check mode, never autofix mode, in CI. If the project has no automated checks at all yet,
  there's nothing to wire up yet either — say so instead of inventing checks that don't exist.
- If the user declines CI setup (not now, not wanted, whatever the reason), note that decision —
  and the date — in this file, then drop it for the rest of the conversation. In a later session,
  once this file carries that note, treat it as a standing prompt to ask again: whether real time
  has clearly passed, or the project has grown enough that the case for CI is stronger than when it
  was declined.

### Keeping a workflow up to date

A CI workflow is a second copy of "what needs to pass before this is clean" — the same failure
mode as any other duplicated logic applies: whenever the project's tools or checks change (a new
linter, a new required check, a build step added or removed), the workflow file(s) must change
with it, in the same change that changed the tooling. Don't let this slip to a follow-up — treat an
out-of-date workflow as a bug, the same way a stale doc would be.

GitHub Actions is set up: `.github/workflows/check.yml` runs `cargo fmt --check`, `cargo clippy`,
`cargo build`, and `cargo test` on every push and pull request, plus a `cargo audit` job via the
`rustsec/audit-check` action. `.github/workflows/release.yml` re-runs the same checks against the
exact tagged commit, then builds and publishes a release archive when a `v*` tag is pushed. Keep
both in sync with this file's Rust conventions above whenever the checks change.

## Content

Applies to every file in this repository — not just test fixtures, but source code, comments,
commit messages, and the repo's own documentation (`CLAUDE.md`, `README.md`, any design doc,
etc).

Real content must never be committed, full stop — no confirmation step, no exception, no "just
this once." Always use placeholders or made-up content instead. Real data leaking into the
repository at all is the failure mode to guard against here, not just an unconfirmed one — this
matters more once the repo is public.

- Never write a real person's name anywhere in the repository. Use a generic placeholder instead
  (e.g. "the user", "example-user").
- Never write a real hostname, URL, IP address, command, or any other identifying detail. Use a
  made-up placeholder instead (e.g. `example-host`, `example.com`, or a documentation-range
  address like `203.0.113.10` — never a real one).
- This applies even when writing a rule *about* real data: cite a placeholder, never an actual
  value, even as an illustrative example.
- If the user supplies a real, non-generic example (a real hostname, a real command, a real name)
  to illustrate a request, generalize it into a placeholder before writing it down anywhere in the
  repository — never commit it verbatim, no matter who provided it or why.
- There is no confirm-and-proceed path here: if something might be real data, treat it as real
  data and replace it with a placeholder. When in doubt, default to a placeholder rather than
  asking whether the real value is fine to use.

**Approved exception:** `README.md`'s Installation section links directly to this repo's own
GitHub Releases page. That's a real URL, but it's self-referential (the repo linking to its own
page) rather than a leak of unrelated real-world data, and the user explicitly signed off on it
after being asked. Don't flag it in a future content-policy review, and don't use it as precedent
for adding other real URLs without the same explicit confirmation.

## Issues

For tracking issues on GitHub (github.com or a GitHub Enterprise Server host), managed with the
`gh` CLI (<https://cli.github.com/>) rather than the web UI for routine operations. This repo now
has a GitHub `origin` remote configured, so these conventions are active.

**GitHub issues are frequently public, and even a private repo's issues can be read by anyone with
access to it — treat every issue title, body, comment, and label as content that could leak beyond
the intended audience.** Never write any of the following into an issue: private or confidential
data, real system information (hostnames, IP addresses, internal file paths, internal URLs,
infrastructure details), credentials, tokens or secrets of any kind, real personal names, or any
other identifying or sensitive detail. Use a placeholder or generic description instead — the same
discipline as the Content conventions above.

**If it is ever unclear whether something counts as sensitive, stop and ask the user for explicit
confirmation before creating or posting anything** — do not guess, and do not proceed on the
assumption that a repo or issue is private enough to relax this.

Before actually running `gh issue create` or `gh issue comment`, re-read the fully drafted
title/body a second time as a distinct check, looking specifically for anything that violates the
rule above. This second pass happens after the content is written and before the command runs —
never skip it, even for a small or seemingly obvious issue. Only submit once that second read
confirms it's clean.

- Install `gh` via the OS's package manager (e.g. `apt install gh`, `brew install gh`) or from
  GitHub's own release page.
- Authenticate once per machine: `gh auth login`, following its interactive prompts (browser or
  token; add `--hostname <host>` for a GitHub Enterprise Server host). The resulting credentials
  live only in `gh`'s own local config (`~/.config/gh/hosts.yml`) on the machine running it — never
  commit a token or write one into the repo.
- Run `gh` from inside the project's repo; it auto-detects the repository from the git remote, so
  `--repo <owner>/<name>` isn't needed unless operating on a different repo than the current
  checkout.
- List issues: `gh issue list`
- View a single issue: `gh issue view <number>`
- Create an issue: `gh issue create --title "..." --body "..."`
- Comment on an issue: `gh issue comment <number> --body "..."`
- Close an issue via `gh issue close <number>`, once it's actually fixed and the fix is committed
  (and pushed, if that's part of the workflow in play).
- Label an issue: `gh issue edit <number> --add-label "..."`
- Unlike some other issue-tracker CLIs, `gh`'s issue subcommands cover the full set of routine
  operations natively (list, view, create, comment, close, label) — there's no need to fall back to
  the API or the web UI for any of these.
