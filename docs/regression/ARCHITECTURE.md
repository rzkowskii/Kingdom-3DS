# Gameplay regression and parity framework

This document covers how the regression framework (`crates/regress`) fits into the port, what it can and cannot prove today, and the order the work is done in.

**Rule one: gameplay fidelity comes before optimization.** An optimization must leave gameplay outcomes as they were. A 3DS rendering compromise must never change the simulation.

## Phase 0 audit: what the repository gives us

### Simulation entry points

| What | Where | Notes |
|---|---|---|
| Whole island state | `game::state::GameState` | One struct holds everything. It derives serde under the `serde` feature. |
| Fresh run | `GameState::new(world, island, seed)` / `GameState::start(.., progress, voyage)` | Island generation runs off the seed. The coat of arms and look use `seed ^ 0x6c00_c0a7`. |
| One step | `GameState::tick(world, &Controls, &mut Vec<GameEvent>)` | Fixed step `state::TICK = 1/60 s`. It takes **no `dt`**, so simulation time can't be tied to frame time. |
| Input edge detection | `game::controls::ControlMapper::map(TICK, RawInput)` | Double-tap gallop is timed in ticks, not wall time. |
| Frame loop (both backends) | `scene::session::Session::step_ticks(raw, ticks)` | Runs `ticks` sim ticks per frame. `bag_tap` only counts on the first tick. The desktop calls `step` from an accumulator; the 3DS calls `step_ticks` with a catch-up count. |
| Saves | `game::save::{save_game, load_game}` | Header (`KNL1`, version 12, world fingerprint) plus the postcard-encoded `GameState`. It's a full snapshot, RNG included. |
| Scripted player | `game::autopilot::Autopilot` | Produces `RawInput` from the game state. It's used for desktop screenshots. |
| Profiling | `game::prof` (feature `profile`), `crates/game/tests/perf.rs` | This is the only place wall-clock time is read, and it's behind a feature. It never feeds back into the simulation. |

### Sources of nondeterminism (audited)

- **RNG.** `game::rng::Rng` (xorshift32) is the only randomness. It's seeded explicitly, and the purse has its own `bag_rng`. There's no `UnityEngine.Random` equivalent and no global RNG. The sequences are *not* the original's (see `docs/deviations.md`).
- **Wall-clock time.** None in gameplay. `game` is `no_std`, so it can't read a clock. `prof.rs` reads `Instant` only with `--features profile`, and only for reporting.
- **Iteration order.** There are no `HashMap`s or `HashSet`s in `game`. Collections are `Vec`s in insertion order. `bag.rs` keeps contacts sorted by key on purpose.
- **Floating point.** The simulation is `f32` throughout, with `libm` for transcendentals (software, same on every target). Rust doesn't contract `a*b+c` into FMA by default. So x86-64 and ARM11 *should* agree bit for bit, but this hasn't been shown. The 3DS VFP may run in flush-to-zero / default-NaN mode, and denormals would then differ. The framework doesn't assume cross-architecture bit equality: see "Comparisons" below.
- **Frame batching.** `Session::step_ticks` samples input once per frame. A script whose input changes mid-frame plays out differently at 2 ticks per frame than at 1. The runner models this explicitly (`ticks_per_frame`).
- **Dev cheats.** Desktop keys (`C` coins, `I` god mode, `--build`, `--greed-at`) change `GameState` outside `tick`. Recorded replays mark themselves *tainted* when one is used.

### What runs headlessly without proprietary assets

All of `game` does. `WorldData` derives `Default`, and the unit tests in `state.rs` and `tests/perf.rs` already build synthetic worlds (walls, shops, scaffolds, trees) by hand. The regression scenarios do the same, so **CI runs them without the game**. Scenarios on a real island (`assets/world.bin`, a v8 `KNLW` pack converted from the user's own copy) are opt-in, locally, through `regress soak --assets` and `regress replay`.

### How original behavior is documented today

- `docs/systems.md` maps C# classes to Rust modules and lists values read from the user's own scene/prefab data, marked **(scene)**.
- `docs/deviations.md` lists intentional differences.
- `// orig:` comments in the code name the original method each piece ports.
- `reference/decomp/` and `reference/ripped/` are local only and gitignored.

None of this is a *measurement of the running original game*. Values from scene data are what the original was configured with, not observations of its behavior. The reference system keeps the two apart (Phase 2).

### Publication safeguards

`tools/check_public_files.py` runs in CI. It rejects private paths (`assets/`, `reference/`, …), game asset extensions (including **`.json`**, `.png` and `.bin`), binaries, and decompiled-source signatures. The framework follows these rules:

- Fixtures and goldens are **TOML** text. No JSON, no binaries, no screenshots.
- Reference fixtures hold numbers and prose only. They never quote decompiled source or embed extracted media.
- Replays of real islands (`*.replay.toml`) hold only inputs, a seed and a world fingerprint, but they are useless without the user's `world.bin`. Keep them out of the repository unless reviewed; `regress replay` reads them from anywhere.

## Design

```
crates/regress/
  src/lib.rs          public API
  src/script.rs       input scripts: run-length RawInput, record + replay, TOML
  src/sim.rs          headless runner: GameState + ControlMapper + frame batching,
                      event log, coin ledger, invariants, checkpoints
  src/snapshot.rs     canonical, versioned state snapshot + comparison with tolerances
  src/meta.rs         replay/run metadata (commit, build profile, target, seed, fixture)
  src/worlds.rs       synthetic WorldData builders (no game assets)
  src/scenario.rs     scenario definition, checks, classification (internal vs parity)
  src/scenarios/      economy, movement, construction, time (day/night, lighting)
  src/tod.rs          time of day: light samples, phase onsets, random sample
                      times, screenshot metrics (`regress tod`)
  src/reference.rs    original-game reference fixtures: schema, loading, validation
  src/golden.rs       golden snapshot files, bless/compare
  src/replay.rs       recorded real-island runs (desktop --record) and their replay
  src/bin/regress.rs  CLI: list / run / soak / replay
  golden/*.toml       blessed snapshots (internal regression baselines)
  reference/*.toml    original-game reference fixtures
  tests/regress.rs    the CI gate
```

### Determinism model

A run is defined by **(world, island, seed, progress, setup, input script, ticks per frame)**. The runner:

1. Builds `GameState` from these, with no wall clock involved.
2. Feeds `RawInput` through `ControlMapper` exactly as `Session::step_ticks` does: one sample per frame, `bag_tap` on the first tick only. A dedicated test checks the runner against the real `Session` and requires identical state.
3. Logs every `GameEvent` as `tick kind detail`, in emission order.
4. Takes snapshots at checkpoint ticks.

Simulation time is `ticks × TICK`. The runner never reads a clock while simulating. It times the whole run separately, for the performance report only.

The timestep is fixed at `state::TICK` because the game is written against it: `tick` has no `dt`, and the tuning constants assume 60 Hz. The knob that *is* configurable and platform-relevant is **ticks per frame** (1 on desktop, 2–3 when an Old 3DS catches up). Changing `TICK` would change gameplay and is out of scope.

### Comparisons

| Comparison | Exactness | Used for |
|---|---|---|
| Full state hash (FNV-1a 64 over `save_game` bytes) | bitwise | Same binary, same platform: run twice; 1 vs 2 vs 3 ticks per frame; save → load → continue. |
| Event log hash | bitwise | Same as above. |
| Discrete snapshot fields (wallet, coin counts, roles, objects built, day, season, crown, event kind counts) | exact | Goldens across commits and platforms. |
| Numeric snapshot fields (clock time, monarch x/velocity) | absolute tolerance per field | Goldens across commits and platforms. |
| Reference metrics vs original-game fixtures | fixture's own tolerance | Parity checks. |

When the semantic snapshot still matches but a golden's state hash has changed, that's reported as a note ("trajectory changed, outcomes held"). `--strict` turns the note into a failure. That's the setting for optimizations that are meant to change nothing at all.

### Known issues

A scenario can carry `known_issue: Some("…")`. Its failure is then reported as **XFAIL** and doesn't fail the run. If it starts passing, it's reported as **XPASS**, which does fail, so the marker gets removed when the bug is fixed. Determinism, frame batching and golden checks are never masked.

### Adding a scenario

1. Add a `Scenario` to the right file in `src/scenarios/`. Give it a synthetic setup (`worlds::*`), a script whose segments are multiples of 6 ticks, and checks that hold for *every* seed.
2. `cargo run -p regress -- run <name> --seeds 50 --tpf 1,2,3` until it passes.
3. `cargo run -p regress -- run --bless <name>`, and commit the golden.
4. If it measures something about the original game, put the numbers in `metrics`, write a fixture in `reference/` from `TEMPLATE.toml.example`, and make it `Class::Parity`.

### Classification

Each scenario is either:

- **Internal regression**: it pins the port's own current behavior. A golden or check failing means *the behavior changed*, not that it's wrong. Re-bless only after deciding the change is intended.
- **Parity**: it computes metrics that are compared against a reference fixture in `reference/`. The report always prints the fixture's evidence type, confidence and verified flag. A parity pass against an unverified fixture is labelled `PARITY (unverified)`.

### Invariants checked on every tick

- Every position and velocity is finite: the monarch, units and coins.
- The wallet never exceeds `coins::MAX_COINS`.
- **Coin conservation**, in scenarios declared closed (no income sources): wallet + loose coins + coins held by citizens + coins that left play stays constant. A coin being sucked into a purse already counts for its new owner. Coins leave play by fading, and each fade is classified as it starts: spent on a payable (0.2 s fade), or lost to the water / expired on the ground (1 s fade). At the end of the run, "spent" is cross-checked against the `Pay.Complete` events.

## What the first run found

| Finding | Status |
|---|---|
| Two builders finishing a site in the same tick fired `Built` twice and ran `GameState::started` twice (a TowerKnight would spawn two shield shops). `many_builders` failed in 15 of 50 seeds. | **Fixed** in `state.rs` (dedupe per tick). Seeds 1–8 run in CI. |
| Nothing stops the monarch at the island's ends. On Land One (seed 1) the ground is x −162…146, but holding a direction reaches −857 / +700. | Known issue: `island_edge` is XFAIL. |
| `Bag::warm` (purse solver warm starting) is `serde(skip)`. Saving while purse coins are moving changes the outcome after load (`mass_coins`: wallet 51 vs 40). | Known issue: `SAVE_LOAD_KNOWN` in `tests/regress.rs`. |
| The full-state hash is computed from save bytes, so it can't see unsaved fields (`Bag::warm`, `god`, `Wallet::bottomless`). Two equal hashes therefore don't guarantee equal futures while the purse is moving. | Documented. Event hashes and outcome snapshots cover the gap in practice. |
| Purse capacity is set by overflow (about 55 coins, half of the spill lost to water), not by the 1000-coin wallet clamp. | Pinned by `purse_overflow` and `purse_full_pickup`. |
| The f32 clock makes a day 14 398 ticks (239.97 s), not 14 400. | Within the parity tolerance (0.25 s). |

## Measuring the original game

`tools/orig-harness/` (see its README) runs the same scripts on the user's own copy of the PC game through a BepInEx plugin we wrote. It fixes the frame rate at 60 (`Time.captureFramerate`), seeds `UnityEngine.Random`, replaces the gamepad at `Player.ReceiveInput`, and writes a trace of named state and method calls. `regress orig` (`src/orig.rs`) parses traces and computes each scenario's metrics with **one function for both games**: the port writes the same trace format (`orig::port_trace`) from the same script on the user's converted island. It then writes `trace-*` fixtures (`repeated-measurement`, unverified) holding only the numbers.

- Islands differ (Unity's RNG isn't the port's), so these scenarios measure layout-independent things or factor the layout out. Speeds count only frames outside forests, and both games start the monarch at the campfire, because their start positions differ (the port begins at the intro's start, the original after skipping it).
- A fixture's `scenarios = [...]` links it to synthetic port scenarios with the same metrics, which then compare against it on every `regress run` and in CI. Purse metrics are linked to the real island only (`regress orig check --assets`), because the synthetic purse isn't the real one's size: on Land One the port's purse settles at about 70 coins from 90, while the synthetic scenario settles at about 47.
- Nothing from the game is committed. `check_public_files.py` allows our two reviewed harness sources by exact path, and still checks them for decompiler and ripper markers.

## Time of day and lighting

The lighting is keyed to the clock alone: the director pushes a preset for each part of the day at fixed hours, and the blender fades it in. Two checks watch it:

- **`time_of_day_lighting`** (CI). It uses a synthetic world with one preset per part of the day and runs three days. A light *onset* is the first change in ambient brightness after at least 0.25 h of stillness. The check expects exactly the scheduled onsets every day (dawn 05:00, day 08:00, evening 16:00, night 21:15) and no others. It also checks that day 2 matches day 1 at every hour, and that random times of day (picked from the state hash, so they differ per seed) show their phase's preset once it has settled, or a value between the two presets during a blend. Shifting a transition or changing the clock speed fails it.
- **`regress tod --assets DIR`** (local). It picks `--samples` random times from a seed (printed, so a run can be repeated) spread over the four blends and the four stable stretches between them, on days 1 and later. Day 0 uses Land One's "First" presets and is skipped. It reports the real island's schedule against `time-of-day-lighting.toml` and each sample's blended light. `--shots` adds desktop screenshots (`--shots FILE --idle --camp --god`). `--orig` runs the original under the harness for the same clock times and puts the two games side by side. The harness samples `Managers.blender.state`, and the `[shots]` section of the config makes it take screenshots. Without `--weather`, each game rolls weather on its own, so the report says when the presets differ. `--weather NAME[,NAME...]` forces the same weather in both: the port through `Director::force_weather` (the random pick still runs, so the RNG and gameplay don't change), the original through the harness's `force_weather` key, which makes every `Day` asset offer only that weather before the director schedules its first cycle. Each sample then has to show that weather's presets in both games, or the run fails. `--weather all` runs every weather that has summer presets for all four parts of the day, one after another, into `target/tod/seed-N/<weather>/`, and ends with a summary: the mean ambient difference between the games, which samples look different and how, and any sample that showed the wrong weather. With `--orig`, each weather takes about 10 minutes. Screenshot comparisons only flag large differences, because the islands and cameras differ. Everything goes to `target/tod/seed-N/` and is never committed.

## Current limitations

- **No original-game measurements are committed yet.** The fixtures in `reference/` come from scene values already documented in `docs/systems.md` (`evidence = "documented-mechanic"`, `independently_verified = false`). The harness above produces real measurements; `trace-*` fixtures appear once someone runs it and reviews the result. Manual observations use `reference/TEMPLATE.toml.example`.
- **RNG sequences differ from Unity's.** Parity is only meaningful for distributions and deterministic mechanics, never for exact random outcomes.
- **On-device 3DS validation isn't automated.** The framework runs on desktop. The 3DS build compiles the same `game` crate, and the frame batching that the 3DS catch-up uses is tested. Running scenarios on hardware and reporting the hashes back (over the SD card) is the next step for cross-platform validation.
- **Synthetic worlds are small.** They exercise the mechanics, not real island layouts. `regress soak --assets` covers real islands locally.
- The user's prompt describing Phase 3 and later was cut off after "Farmer movement and income generation". The scenario set below stops at the systems that brief named in full.

## Implementation order

1. Phase 0: this audit (done).
2. Phase 1: `script`, `sim`, `snapshot`, `meta`, the determinism tests (repeat run, ticks per frame, save round trip, runner = `Session`).
3. Phase 2: `reference` schema and validator, the documented-mechanic fixtures, the template.
4. Phase 3 (first slice): economy (`coin_drop`, `purse_overflow`, `purse_full_pickup`, `pay_complete`, `pay_cancel`, `pay_unaffordable`, `mass_coins`), movement (`monarch_walk`, `monarch_gallop`, `citizen_walk`, `citizen_run`, `double_tap_gallop`, `island_edge`), construction (`beggar_to_builder`, `many_builders`), time (`day_night_cycle`, `time_of_day_lighting`).
5. Goldens, the CLI, the CI gate, desktop `--record` and `regress replay`.
6. Next: farmer income, greed waves, boat, and the 3DS on-device hash runner.

## Using it

```sh
cargo test -p regress                          # CI gate: determinism, goldens, invariants, fixtures
cargo run -p regress -- list
cargo run -p regress -- run                    # every scenario, default seeds, report
cargo run -p regress -- run --seeds 100        # 100 seeds each, checks + invariants (no goldens)
cargo run -p regress -- run --tpf 1,2,3        # also require identical state across frame batching
cargo run -p regress -- run --bless            # rewrite goldens after an intended change
cargo run -p regress -- run --strict           # fail on any trajectory change (optimizations)
cargo run -p regress -- run many_builders --seed 2 --events   # one seed, full event log
cargo run --release -p regress -- soak --assets assets --seeds 20 --days 8
cargo run -p desktop -- --record bug.replay.toml   # play, quit; then:
cargo run -p regress -- replay bug.replay.toml --assets assets
```
