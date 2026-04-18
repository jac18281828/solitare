# AI Contributing Requirements

These rules apply to all AI-assisted changes in this repository.

## Workflow
1. Read every file you plan to change and directly related modules.
2. Summarize current behavior and invariants (deck integrity, selection/scoring
   rules, victory/defeat conditions, UI gating when `interactions_locked`).
3. Propose a minimal patch plan (diff and rationale).
4. Obtain user approval before editing code.
5. Affirm all `Completion Gates` are met.

## Code Design
- Prioritize correctness, then idiomatic and reviewable code.
- Prefer clarity over cleverness.
- Write small single-purpose functions with clear names.
- Expand to single-purpose modules composed of concise functions.
- Prefer decomposition over accretion: extract helpers as behavior grows.
- Prefer canonical, widely understood solutions.
- Treat these rules as defaults; escalate exceptions before implementation.
- Keep diffs focused; avoid idiosyncratic churn.
- Write comments that explain enduring intent or constraints, no editorial comments.

## Naming
- Naming must be semantic.
- Do not encode type or structural primitives in names (int, object, string, etc).
- Avoid namespace prefixes or suffixes. If everything starts with or ends with
  `_fix_`, nothing should.
- Use names like `State`, `Context`, or `Manager` only if a clear abstraction
  requires it at a systemic level.

## Abstraction
- Abstract to remove duplication or enforce invariants.
- Prefer concrete types over generic wrappers.
- Avoid `unwrap`/`expect` outside of tests. Use effective error handling
  patterns including `Result` and `Option`.

## Dependencies and Imports
- Prefer the standard library.
- Add external crates only with user approval.
- Declare imports at the top of each module; keep them explicit and organized
  so dependencies are clear.
- Respect the WASM target: any new dependency must build on
  `wasm32-unknown-unknown` and not pull in native-only syscalls.

## Tests
- Test project behavior and contracts, not language or dependency internals.
- Avoid vacuous tests: removing or breaking target code must cause a test to fail.
- Unit tests are required to be hermetic: no network, no external assets, no
  wall-clock or entropy dependencies (use `GameState::empty` and deterministic
  fixtures rather than `new_shuffled`).
- Add or update tests for every behavior change.

## UI and Game Rules
- Game logic (`src/game.rs`) must not depend on browser APIs, `yew`, or
  `wasm-bindgen`. Keep it pure so unit tests run on the host target.
- Respect the scoring model: gold is earned on foundation placements and on
  waste/foundation→tableau placements; tableau→tableau reshuffles earn nothing;
  recycles cost one gold and end the game at zero.
- Do not introduce gold-farming loops. If a move can award gold, confirm the
  inverse move cannot reclaim that gold for a net positive.
- UI components must honor `App::interactions_locked()` when an end state is
  active or `all_to_temple_running` is true. Add any new interaction to that
  gate unless it is deliberately available post-game.

## Completion Gates

Before marking work complete, run and report:

1. `cargo check`
2. `cargo fmt --all -- --check`
3. `cargo clippy --all-targets --all-features --no-deps -- -D warnings`
4. `cargo test` — all unit, doc, and integration tests pass
5. `cargo check --target wasm32-unknown-unknown` — WASM build compiles
6. For UI-visible changes: `trunk build --release` and a visual check in a
   browser of the golden path plus any affected end-state (victory rain,
   out-of-gold lockout, Zeus' Vision reveal)

Do not mark work complete until all gates pass.
