# Original-game measurement harness

This harness runs the same scripted scenarios on **your own copy of Kingdom: New Lands (PC)** that `crates/regress` runs on the port, and turns what it measures into reference fixtures. It's Phase 2's "structured event traces where legally and technically obtainable".

Nothing from the game enters the repository:

| Where | What | In Git? |
|---|---|---|
| `tools/orig-harness/` | This README, the scripts, our plugin's source, the probe list (member *names* only) | yes |
| `gamefiles/harness/game/` | A private copy of your game (data symlinked, executable copied) with BepInEx | no (`/gamefiles/` is ignored) |
| `gamefiles/harness/traces/` | The configs and traces of each run, plus game logs | no |
| `gamefiles/harness/fixtures/` | Fixtures written from traces, before you review them | no |
| `crates/regress/reference/trace-*.toml` | Reviewed fixtures: numbers, setup, evidence description | yes, after review |

Your own installation and saves are never modified. Each run uses a throwaway `HOME`, so the game starts fresh and its saves and settings go to a temporary folder that is deleted afterwards.

## How it works

`KingdomTrace` is a BepInEx 5 plugin, written for this project. It looks up everything game-specific **by name at run time**, so it compiles without any game code. It references only `UnityEngine.dll` from your install and BepInEx's own DLLs, at build time.

- **Time.** `Time.captureFramerate = 60`, so every frame advances game time by exactly 1/60 s however fast it renders. One original frame equals one port tick.
- **Randomness.** `UnityEngine.Random` is seeded before the island is generated (`Level.GenerateCurrentConfig`), and seeded again when play starts, so the number of loading frames doesn't matter. The sequences are Unity's, not the port's, so **islands differ between the two games**. The scenarios measure things that don't depend on layout, or factor it out (speed is only measured outside forests).
- **Start.** The intro is skipped the way holding the skip button skips it (`Game.AbortIntroAndTutorial`). Frame 0 is the first frame with `Game.state == Playing`. Scenario setup (for example `@player_x` = the campfire, or 90 coins in the purse) is applied on that frame.
- **Input.** A Harmony prefix on `Player.ReceiveInput` replaces the gamepad with the scenario's script. The script is mapped to `UpdateActionState` / `UpdatePayState` exactly as the port's `ControlMapper` maps it (edges, 0.6 s double tap). The 3DS bag tap has no original equivalent, and scripts that use it are refused.
- **Trace.** Every N frames it samples the members in `probes.cfg`, and it logs calls to the methods listed there under the port's event names. The trace also records frames where the game didn't take input; such a trace is rejected as evidence.
- **Driven citizen.** Setup `@drive = Role speed dir` spawns that citizen one unit right of the campfire, disables its `Role` component (its state machine) and calls `Mover.SetSpeed(speed, dir)`. Only `Mover` and the physics body are left. Sample paths starting `@driven.` read it, and a component can be named by type (`@driven.Mover._moveSpeed`).
- **Lighting and screenshots.** `ambient`, `sky_top` and `sky_bottom` sample `Managers.blender.state` (written `r,g,b,a`), and `lights` lists the presets on the blender's stack. A `[shots]` section (`day hour path`, written by `regress tod --orig`) takes a screenshot with `ScreenCapture.CaptureScreenshot` the first frame the clock passes each time. Screenshots stay in `target/tod/` and are never committed.
- **Weather.** `force_weather = N` (a `BlenderState.Weather` value, written by `regress tod --weather`) sets every `Day` asset's `weatherOptions` to that weather when the island is generated, before the director schedules any day. `GetRandomWeatherOption` still draws from `Random`. Only the in-memory assets change. The trace header records `weather_forced N days K`.
- **Switches.** Long clock runs turn greed off with the original's own `Portal.DebugDisableSpawns`. Every setup value is written into the trace header and the fixture.

## Use

Needs Linux, the Steam Linux build of Kingdom: New Lands, `curl`, `unzip`, and the .NET SDK. The game runs in a 640×360 window.

```sh
tools/orig-harness/setup.sh                      # copy + BepInEx 5.4.23.2 (SHA-256 pinned) + plugin
cargo run --release -p regress -- orig list
cargo run --release -p regress -- orig run orig_walk --runs 3      # 3 seeds, traces + metrics
cargo run --release -p regress -- orig compare orig_walk --assets assets   # original vs the port on your island
cargo run --release -p regress -- orig fixture orig_walk           # writes gamefiles/harness/fixtures/
cargo run --release -p regress -- orig fixture orig_walk --publish # into crates/regress/reference/
cargo run --release -p regress -- orig check --assets assets       # port vs every published fixture
```

`regress orig export NAME` writes a config without running anything. `tools/orig-harness/run.sh CONFIG` runs one config by hand.

A published fixture is labelled `PARITY (unverified RepeatedMeasurement, medium)`. It names the port scenarios with the same metrics (`scenarios = [...]`), and `regress run` / `cargo test` compare those scenarios against it automatically. Before publishing, read the fixture. Check that the setup is what you intended and that the tolerances, which come from the spread across runs, look sane. Set `independently_verified = true` only once someone has checked the result another way, such as timing a normal play session.

## Scenarios

| Name | Measures | Compared with |
|---|---|---|
| `orig_walk` | walking speed outside forests, from the campfire | `monarch_walk`, port on your island |
| `orig_gallop` | gallop speed outside forests | `monarch_gallop`, port on your island |
| `orig_citizen_walk` | a peasant with its AI off, `Mover` held at 0.6 (`@drive`) | `citizen_walk` |
| `orig_citizen_run` | the same at 1.4 | `citizen_run` |
| `orig_day_cycle` | clock at start, first dawn, daylight, night, day length over 3 days | `day_night_cycle`, port on your island |
| `orig_time_of_day` | when each lighting phase starts, the lit day and night (blender sampled every 6 frames) | `time_of_day_lighting`; `regress tod --orig` adds screenshots |
| `orig_purse_overflow` | coins spilled and the purse's settled size from 90 coins | port on your island only (the synthetic purse isn't the real one's size) |

To add one, add an `OrigScenario` to `crates/regress/src/orig.rs`: a script, setup for both games, and a metrics function over a `Trace` (the same function serves the original's trace and the port's). If you need another probe, add its member path to `probes.cfg` and its port equivalent to `port_sample`.

## Legal and safety notes

- This modifies only a private copy of a game you own, locally, to measure it for an interoperable reimplementation. Nothing from the game is distributed. Check that this is acceptable where you live and under the game's terms before using it.
- BepInEx is downloaded from its official GitHub release, and its checksum is verified.
- Don't commit traces, game logs, screenshots or recordings. Fixtures carry numbers only.
