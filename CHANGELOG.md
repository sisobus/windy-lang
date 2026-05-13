# Changelog

All notable changes to Windy are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/).

The crate on crates.io is `windy-lang`; the language and the installed
binary are both `windy`. References to "the crate" below always mean
`windy-lang` v$X.Y.Z`.

## [Unreleased]

## [2.3.2] — 2026-05-13

### Changed

- `Cargo.toml` `repository` and `documentation` URLs updated to
  `https://github.com/sisobus/windy-lang` (the GitHub repo was renamed
  from `windy` to `windy-lang` to match the crate name). GitHub's
  permanent redirect keeps the old URLs working, but this republish
  makes the canonical link on crates.io point at the new URL
  directly. No code changes; this is a metadata-only patch.

## [2.3.1] — 2026-05-13

### Fixed

- The "windy-mine plugin not in PATH" hint pointed at the pre-publish
  install command (`cargo install --git https://github.com/sisobus/windy-coin
  windy-mine`). Now that `windy-mine` is on crates.io, the hint reads
  `cargo install windy-mine`.

## [2.3.0] — 2026-05-13

### Added

- **`windy mine` subcommand** — explicit, documented entry in `windy --help`
  that forwards every argument to the `windy-mine` binary shipped by
  [windy-coin](https://github.com/sisobus/windy-coin). Examples:
  - `windy mine programs/foo.wnd` — full Risc Zero proof + Base mainnet mint
  - `windy mine programs/foo.wnd --dry-run` — score-only, no proof, no tx
  - `windy mine --help` — forwarded to `windy-mine --help` (own clap reference)

  windy-lang itself takes no dependency on risc0 or alloy; the plugin
  is exec'd from `PATH`. If `windy-mine` isn't installed, `windy mine`
  exits 127 with the install command (`cargo install --git
  https://github.com/sisobus/windy-coin windy-mine`).

- **Git-style external plugin dispatch.** Beyond `mine`, any executable
  named `windy-<name>` in `PATH` becomes a `windy <name>` subcommand
  via clap's `external_subcommand`. Lets ecosystem tools graft onto
  the `windy` CLI without windy-lang taking on their dependencies.
  Future plugins (e.g. `windy-aria` for sonification) work the same
  way without further changes to this crate.

## [2.2.1] — 2026-05-09

### Added

- **`VmMetrics::visited_cells`** under the `metrics` feature — count
  of distinct `(x, y)` grid cells that any IP actually executed at
  during the run. Cells that an IP only flew over (e.g. high-speed
  traversal per SPEC §3.7) and cells in a sisobus signature or
  comment block that the IP never reaches do *not* count. This is
  the trace-truth size of the program; the parsed bounding box
  (`Grid::bounding_box`) reports the static layout. The two together
  give a downstream policy a clean way to ask "how much code did the
  miner *actually run*", without being inflated by punctuation in
  documentation rows.

### Changed

- `VmMetrics` now has six fields instead of five — anyone with an
  exhaustive `match` over the struct has to add a `visited_cells:` arm.
  This is technically a SemVer-flagged change for `metrics` consumers.
  Default field-initialisation (`VmMetrics::default()` or `..base`)
  continues to compile unchanged.

## [2.2.0] — 2026-05-09

### Added

- **Optional `metrics` Cargo feature** that surfaces per-run execution
  counters useful to downstream tooling. When the feature is on, the
  `Vm` carries a `VmMetrics` snapshot that records:
    - `max_alive_ips` — peak `vm.ips.len()` measured at the end of each
      tick (after collision merge + halt-retain), so it's the
      steady-state IP count rather than a mid-tick split spike;
    - `spawned_ips` — total `t` (SPLIT) invocations across the run;
    - `grid_writes` — total `p` (GRID_PUT) invocations;
    - `branch_count` — total `_` + `|` + `~` invocations;
    - `hard_opcode_bitmap` — a 16-bit bitmap of which "hard" opcodes
      ran at least once (`t`, `p`, `g`, `_`, `|`, `≫`, `≪`, `~`, `#`,
      `"`).
  The `Grid` also gains `bounding_box`, `total_grid_cells`, and
  `effective_cells` accessors under the same flag, where
  *effective* means cells whose decoded opcode is neither `Op::Nop`
  nor `Op::Unknown`. The flag is **off by default** — a default-features
  build is byte-identical to v2.1.0, and no metric work is done. The
  surface is shaped to plug directly into windy-coin's Phase 2
  mining-policy guest, but anything that wants to know "how hard did
  this windy program work" can read these numbers without
  instrumenting the interpreter itself.

## [2.1.0] — 2026-05-09

### Added

- **`windy-lang` published to npm** as a wasm-pack `web` target.
  The release workflow now builds `web/pkg` after `cargo publish`
  and runs `npm publish --access public` against
  `registry.npmjs.org`. Browser-side projects can depend on
  `"windy-lang": "^2.1.0"` and `import init, { Session } from
  'windy-lang'` without checking out this repo or running
  `wasm-pack` locally. Version is single-sourced from
  `Cargo.toml` — wasm-pack copies it into `web/pkg/package.json`,
  and the workflow asserts the two match before publishing.
- **Vim-style modal editor** in the playground. The source
  textarea now starts in **NORMAL** (a small badge above the
  editor labels the current mode); first-time visitors who only
  pick an example and hit Run/Debug never notice. Power users
  hit `i` to enter INSERT and type, `Esc` to return.
  - **NORMAL keybindings**: `hjkl` + `yubn` for the eight winds
    (rogue-like 3×3 compass), `0` / `$` for line edges, `i` /
    `a` / `o` / `O` to enter INSERT, `x` to blank a cell, arrow
    keys for navigation. Every nav move auto-pads the destination
    row with spaces, so moving "down past the end of a short
    line" lands at the same column instead of dumping the cursor
    at column 0.
  - **Glyph palette**: clicking a wind glyph inserts it at the
    cursor and advances the cursor in that wind's direction —
    the same in both modes. Chain the clicks and you "draw the
    IP's path"; each click drops a wind and moves to where the
    wind would carry the IP next. The current mode is preserved
    across clicks so the keyboard context never silently changes.
    `≫` `≪` `·` insert + step east one cell.
  - **Typing direction follows the last palette click**. If the
    user clicked `↓` (cursor stepped south, INSERT preserved),
    subsequent typed characters in INSERT also flow south — each
    keystroke drops the character and steps the cursor south,
    not east. A small direction-indicator badge next to the mode
    badge shows the current flow glyph (→ ↓ ↘ ...).
  - **2D-grid OVERWRITE typing**. Typing in INSERT now replaces
    the cell under the cursor instead of the textarea's native
    insert-and-shift, which would push every cell to the right of
    the caret one column east (a real problem the user hit while
    editing 2D source — type one character at column 4 and
    everything at column 5+ slides east, breaking aligned
    layouts). Backspace in INSERT clears the cell one step against
    the flow direction (replaces it with a space) and parks the
    cursor on that cell, ready to retype — the way every other
    text editor's backspace behaves, but in 2D.
  - **Mobile**: a small `i / Esc` toggle button next to the mode
    badge replaces the missing physical Esc key.
- **`docs/esolangs-wiki.md`** — MediaWiki-syntax source for
  the esolangs.org wiki entry, focused on Windy's own spec
  and distinguishing features (eight winds, wind speed,
  multi-IP collision merge, BigInt throughout, sparse
  bi-infinite grid, tick-determinism, Turing-completeness
  sketch). Bracketed with begin/end markers; the body
  between them is the canonical source — to update the
  live wiki, edit this file and paste the result back into
  https://esolangs.org/wiki/Windy.
- **esolangs.org wiki entry published** at
  https://esolangs.org/wiki/Windy. README's Documentation
  section now links to it.
- **`examples/add.wnd`** — minimal stdin demo: `&&+.@` reads two
  decimal integers, prints their sum.
- **`examples/sum_winds.wnd`** — diagonal-cascade calculator.
  Three winds carry the digits 4, 5, 6 down stair-stepped paths
  into a `+ +` adder chain on the left edge, print 15, and halt
  by IP collision merge — no `@` in the source. Picker entry
  added to the playground.
- **`examples/hi_windy.wnd`** — meandering wind paths with a
  `t` SPLIT thread string-mode chunks (`"Y`, `"HD`, `iN`,
  `,I`, `W`) onto the stack and print them in order to spell
  `Hi, WINDY`. Halts by IP collision merge. Picker entry added
  to the playground.
- **`examples/wind_speed.wnd`** — `≫` / `≪` gusts route a
  single IP through a labyrinth of diagonals to print
  `The wind speed is 1.` and halt cleanly. Picker entry added
  to the playground.
- **`examples/main.wnd`** — wind-tunnel showpiece written by
  the user. Outputs `WINDY` (the language's own name) by
  routing one IP through 8-direction diagonals, two
  string-mode segments (`"DY"` and `"WIN"`), and a horizontal
  print loop. Decorated with `~` turbulence cells and an
  embedded `sisobus` watermark. Promoted to the README's
  hero example, replacing the simpler `hello_winds.wnd` snippet.
- **`examples/puzzle.wnd`** — multi-IP "find the password"
  puzzle. `t` SPLIT spawns a child going west; both IPs print
  digits as they traverse the same row in opposite directions,
  then merge head-on at the `t` cell. Output:
  `1 2 4 3 5 2 6 1 5 2`. The trailing `@` is dead code — halt
  comes purely from the collision merge `(0,0)` direction-die.
  Picker entries added for both.
- **`examples/puzzle_hard.wnd`** — asymmetric variant with
  TWO `t` SPLITs and FOUR live IPs. Layout `→1.2.3t4.5t6.7←@`
  has 5 cells west of t₁ and 8 cells east — the structure is
  asymmetric in length, yet halts cleanly because the spacing
  `t₂−t₁ = ←−t₂ = 4` engineers two pairwise collisions
  `(0,0)` at distinct cells (col 4 and col 10). Output:
  `1 2 4 3 2 6 5 1 7 4`. Picker entry added.

### Fixed

- **Glyph palette click**: source textarea regained focus after
  insertion (was the existing behavior; preserved through the
  modal editor refactor).
- **Mobile auto-zoom on textarea focus**. iOS Safari zooms into
  any focused input with `font-size < 16px`; the source and
  stdin textareas were 14px / 13px on desktop and the page
  jumped on every tap. The mobile breakpoint now overrides
  both to 16px — desktop sizes are unchanged.
- **Palette operator clicks (`≫` `≪` `·`) follow the current
  flow direction**. They previously carried `data-dx="1"
  data-dy="0"` for caret default, which made every click step
  east AND reset the flow direction east — clicking ≫ in the
  middle of laying down a southbound trail yanked the caret
  east. The three operator buttons now have no `data-dx` /
  `data-dy`; the click handler treats them as direction-less
  (insert + step in current flow, leave flow unchanged).
- **Space key navigation in NORMAL mode**. Pressing SPACE was
  silently swallowed (`e.preventDefault` ran with no matching
  `case`). NORMAL now treats SPACE as a step along the current
  flow direction, matching the rest of the navigation keys.
  INSERT keeps the standard typing behavior — SPACE writes a
  literal space and advances along the flow direction, just
  like any other character.
- **2D Backspace at edges of grid (e.g., typing at col 0
  going west)**. The cursor's post-type move is clamped to
  `col >= 0`, so after dropping a glyph at column 0 going
  west the caret ends up *on top of* the glyph rather than
  one step ahead of it. Backspace's "clear opposite cell"
  rule then erased an empty cell instead of the glyph. The
  handler now falls back to clearing the cell under the caret
  whenever the opposite cell is blank but the caret is on a
  glyph — covers all four corner-edge typing directions.

## [2.0.0] — 2026-04-26

Breaking-change cut. Removes the v0.4 legacy gate, tightens the
language surface to a single set of semantics, and ships
all-mechanic example programs.

### Added

- **`examples/winds.wnd`** — a multi-IP exhibit. Four `t` SPLITs
  in a row push the live IP list up to five (1 parent + 4 children)
  at peak; each child immediately redirects south via a `↓` cell on
  its spawn position, descends ten rows of NOP space, and halts at
  its own `@` on row 10. The parent uses `#` (TRAMPOLINE) before
  each `t` to skip the `↓` redirects so they never apply to itself.
  Demonstrates cascade avoidance with multiple SPLITs, alongside
  `examples/storm.wnd` (head-on collision merge).

### Changed

- **`examples/anthem.wnd`** rewritten as a clockwise diagonal-
  cornered spiral that exercises all four v2.0 mechanics in one
  program. The IP rides the perimeter at speed 2 with `↘ ↙ ↖`
  corner glyphs, prints "code flows like wind" along the way,
  then drops to speed 1 at the eye of the spiral, runs `t` to
  spawn a counter-going child, and parent + child arrive at the
  same cell from opposite sides on the next tick. The end-of-tick
  collision pass cancels them head-on, the live IP list empties,
  and the program halts. There is no `@` anywhere in the file.
  Earlier vertical-cascade and hollow-spiral versions are gone.
- **SPEC** bumped to v2.0. §9 drops the `--v0` row; §11
  Versioning rewrites the conformance promise to refer only to
  the current single-mode language.
- **Crate version 1.0.0 → 2.0.0** on crates.io as `windy-lang`.
- **Playground UI** polish: Run / Debug now sit on their own row
  beneath the picker + inputs (they used to be visually
  clustered with the Max-steps input); Debug picks up the
  outlined `secondary` style so it reads as the alternate path
  instead of a duplicate primary; button padding tightened;
  `touch-action: manipulation` added to toolbar buttons and the
  grid view so iOS/Android no longer hijack rapid Step taps as a
  pinch-zoom; "Copy link" button removed (the URL bar already
  reflects the current source via the `#s=...` hash).

### Removed

- **`--v0` CLI flag.** The wind-speed (≫/≪) and IP-collision-merge
  semantics introduced in v1.0 were always the language going
  forward; the legacy gate let callers opt into the pre-1.0 surface
  for migration. v2.0 deletes the gate. Programs that depended on
  the legacy surface should pin a v1.x release of the reference
  implementation, or `git checkout` the repo at the v1.0.0 tag.
- **`RunOptions.v1` field**, **`Vm::with_v1` constructor**, and the
  wasm `run` / `Session::new` `v1: Option<bool>` parameter — public
  API surface that only existed to support the gate.
- **`v0_*` unit tests** and `tests/conformance_v1.rs::v0_cases_pass_under_v1_mode`
  (the additivity guard). Both were proving "v0 semantics still
  reachable when the gate is set"; with the gate gone, neither has
  anything to prove.
- **Web playground v0 toggle** and the in-browser framing that
  required users to choose a mode before running anything.

### Fixed

- **Browser playground source loading**. The `anthem` entry in
  `web/main.js`'s `EXAMPLES` map closed its template literal with
  an escaped backtick (`\\\``), which JS treats as a literal
  character — so the template stayed open, swallowed the rest of
  the file, and the module failed to load (no example source ever
  appeared in the editor when picked). Replace with a real backtick.

### Migration

- CLI: drop the `--v0` flag from your scripts. v1.0's default
  (no flag = full v1 semantics) is now the only behavior.
- Library: stop passing `v1: false` (or `v1: true`) when
  constructing `RunOptions`; the field is gone. Replace
  `Vm::with_v1(grid, seed, max, true)` with `Vm::new(grid, seed, max)`.
- wasm: stop passing the trailing `v1` argument to `run()` /
  `new Session(...)`; the parameter is gone.

## [1.0.0] — 2026-04-25

The first stable release. Wind speed and IP collision merge become
normative; the v0.4 surface remains available as a legacy gate.

### Added

- **Wind speed** (SPEC §3.7). Each IP carries an unbounded positive
  integer `speed` field (default `1`) and advances `speed` cells per
  tick. Only the destination cell decodes — intermediate cells are
  not read at all. Two new opcodes: `≫` (GUST, `speed += 1`) and
  `≪` (CALM, `speed -= 1`; runtime trap if it would yield 0).
- **IP collision merge** (SPEC §3.8). End-of-tick coincidence of two
  or more IPs on the same cell triggers a merge: stacks concatenate
  in birth order (oldest at the bottom), directions sum and clip
  per axis to `{-1, 0, +1}` (head-on `(0,0)` ⇒ merged IP dies),
  speed becomes `max`, strmode resets to off. The pass also serves
  as a runtime garbage collector for IPs in cyclic layouts.
- **Trap exit code** `134` for `≪` at speed 1 ("calm in still air").
- **`--v0` legacy gate** on the CLI, the WASI binary, the wasm
  `Session` / `run` API, and the playground toolbar. Under the gate,
  `≫` / `≪` decode as Unknown (NOP + warning) and the collision
  pass is skipped — bit-identical to v0.4.
- **`conformance/v1.json`** with 4 cases (gust skip, gust/calm cycle,
  calm@1 trap, 2× gust) and `tests/conformance_v1.rs` harness.
- **Additivity guard** (`v0_cases_pass_under_v1_mode`): every v0.4
  conformance case is re-run under v1.0 semantics to confirm that
  programs without `≫`/`≪` and without collisions behave identically
  under both gates.
- **`examples/gust.wnd`** (wind speed obstacle course — same source
  prints `WINDY` under v1, `ID\0\0\0` under v0) and
  **`examples/storm.wnd`** (head-on collision; v1 cleanly halts
  via merge, v0 fork-bombs without the merge as IP-GC).

### Changed

- **Crate version 0.4.0 → 1.0.0.** Banner picks up via
  `CARGO_PKG_VERSION`.
- **Crate name on crates.io is `windy-lang`** (the bare `windy` was
  taken by an unrelated Windows-strings library). The library and
  the installed binary are still `windy`; only the install command
  is `cargo install windy-lang`.
- **CLI: `--v1` removed; `--v0` added.** v1.0 semantics are now the
  default. The legacy gate is opt-in.
- **`Vm::new` now defaults to v1 semantics.** Use `Vm::with_v1(.., false)`
  to construct in legacy mode.
- **wasm `Session::new` / `run` defaults flipped.** `v1: Option<bool>`
  with `None` ⇒ `true` (v1 semantics).
- **SPEC promoted from v0.4 to v1.0.** The "Pre-release: v1.0
  (proposal)" section dissolves into normative §3.7 (Wind Speed),
  §3.8 (IP Collision — Merge), and §4 opcode-table additions
  (`≫` U+226B, `≪` U+226A). §11 Versioning explicitly catalogs the
  additivity promise.

### Removed

- The "Pre-release: v1.0 (proposal)" SPEC section as a discrete
  block — its content is now distributed across the normative
  sections above.

### Notes

- This release is the first crates.io publish and the first
  GitHub-public point in the project's history.
- The crate ships both `conformance/cases.json` (v0.4 surface) and
  `conformance/v1.json` (v1.0 wind speed + collision merge); any
  third-party implementation MUST pass both byte-for-byte, with
  the legacy gate honoring `cases.json` and the default mode
  honoring both.

## [0.5.0] — pre-release (folded into 1.0)

Distribution-channel polish that landed before the v1.0 cut.
Released only under the v1.0 tag; never published independently.

### Added

- **`wasm32-wasip1` target** producing a portable `windy.wasm` for
  any WASI host (`wasmtime`, `wasmer`, Node `--experimental-wasi`).
  CI builds it and serves it next to the playground.
- **MIT `LICENSE`** file.
- **`Cargo.toml` metadata** (keywords, categories, anchored include
  list) for clean `cargo package`.
- **Cache-bust mechanism** for the playground — `?v=<short-sha>` on
  static asset URLs, replaced by CI per deploy, paired with
  CloudFront `/*` invalidation.

### Changed

- `wasm-bindgen` cfg narrowed to `target_os = "unknown"` so the
  WASI target stops dragging in browser-only crates.

## [0.4.0] — 2026

Concurrent IPs.

### Added

- **`t` (SPLIT) opcode** spawning a new IP at `(x − dx, y − dy)`
  going `(−dx, −dy)` with empty stack and string mode off (SPEC
  §3.5 / §3.6 / §4).
- **Multi-IP VM** — `Vec<IpContext>`, tick-based birth-order
  scheduling, `@` removes only the executing IP.
- **wasm API multi-IP support** — `ip_count`, `ip_positions()`,
  `stack_for(i)`, `stack_len_for(i)`, `strmode_for(i)`. The
  browser debugger highlights every live IP cell and renders a
  per-IP labelled stack section.
- **`examples/split.wnd`** — visible concurrent-IP demo.

## [0.3.0] — 2026

Browser playground.

### Added

- **`wasm32-unknown-unknown` target** with `cdylib` + `wasm-bindgen`.
- **Static playground** under `web/` (HTML / CSS / JS, dark mode,
  mobile sticky toolbar, tap-to-step).
- **Browser debugger** via the `Session` API: Step / Continue /
  Restart / Exit, keyboard bindings, opcode reference panel.
- **URL hash permalink** (`#s=<base64url>`).
- **GitHub Actions deploy** to S3 + CloudFront.

## [0.2.0] — 2026

Rust rewrite. The Python scaffold is retired and the single Rust
crate at the repo root powers everything afterwards.

### Added

- Rust crate (`lib + bin`), 32 opcode VM (later 33 in v0.4 with the
  addition of SPLIT), `clap` CLI.
- `conformance/cases.json` + Rust harness.
- `windy debug` subcommand — terminal stepper, no TUI crate
  dependency (ANSI escapes + Unicode box drawing).

### Removed

- The v0.1 Python interpreter and the WASI output-baking stopgap
  (`wasm.py`). Per-program AOT became obsolete the moment the
  interpreter itself shipped as WebAssembly in v0.3.

## [0.1.0] — 2026

Initial scaffold. Python interpreter, rich-based debugger, WASI
output-baking stopgap. Retired by v0.2.

[1.0.0]: https://github.com/sisobus/windy-lang/releases/tag/v1.0.0
