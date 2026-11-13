# Changelog

## 1.3.0 — 2026-11-08

### Added

- 3DS debug overlay (tap SELECT): the first line now shows FPS averaged
  over 30 frames, the lowest and highest readings, and the longest single
  frame since the overlay was opened. Reopen it to reset them.
- README: season comparison screenshots (Rust port on PC next to the
  original PC game), the 3DS bottom screen, a recap of how the regression
  runner checks lighting and movement speeds, and the first Old 3DS FPS
  numbers (57.9 idle, about 26–35 in busy play, low 24.6, high 59.9).
- `crates/regress`: a deterministic gameplay regression and parity
  framework (see `docs/regression/ARCHITECTURE.md`). It has a headless
  runner that drives `GameState` exactly as `Session` does, input scripts,
  versioned snapshots compared with tolerances, golden baselines, a coin
  conservation ledger, and original-game reference fixtures that record
  their evidence and whether anyone has verified them. The first 17
  scenarios cover coins, paying, the purse, movement, building and the
  day/night cycle. The CLI is `regress list|run|soak|replay`.
- `tools/orig-harness/` and `regress orig`: measure the original PC game
  (your own copy, never committed) with a BepInEx plugin that fixes the
  frame rate, seeds Unity's RNG, plays the port's input scripts and traces
  named state. The same metrics are computed from the original's trace and
  from the port on your island, and the results become `trace-*` reference
  fixtures.
- `citizen_walk` / `citizen_run` scenarios and the harness's `@drive`
  setup: a peasant with its AI off holds a `Mover` speed, in both games.
- `time_of_day_lighting` scenario and `time-of-day-lighting` fixture: the
  real director and blender run over synthetic presets for three days.
  Each lighting phase must start on schedule every day (dawn 05:00, day
  08:00, evening 16:00, night 21:15), nothing else may move the light, the
  same hour must look the same each day, and random times of day must show
  the light their window promises.
- `regress tod --assets DIR`: picks random times of day from a seed (or a
  given `--seed`), measures the port's lighting on the real island, and
  reports the schedule, the lit day and night lengths, and each sample's
  light. `--shots` takes desktop screenshots at those clock times. `--orig`
  runs the original under the harness, which now samples the blender and
  takes screenshots at clock times. The report puts the two side by side
  and flags where the port looks darker, brighter, warmer, colder or
  flatter.
  `--weather NAME[,NAME...]` forces that weather in both games (the port's
  director and the original's `Day` assets) so their lighting can be
  compared like for like; `--weather all` runs every weather with a full
  set of summer presets in turn and ends with a summary across them.
- Desktop dev flags for that: `--shots FILE`, `--idle`, `--camp`, `--god`,
  `--force-weather NAME` (every day gets that weather and blends on the
  normal schedule; the random pick still happens, so gameplay is unchanged).
- CI checks formatting (`cargo fmt --check`, the 3DS crate too).
- Desktop `--record FILE` records a fresh run's input as a replay that
  `regress replay` plays back and checks.

### Fixed

- The monarch moved about 5% too fast. In the original, ground friction
  slows the steed on every physics step before `Mover` restores the
  speed setting, so it always moves 0.19 units/s slower than the setting.
  Walking is now 1.81 units/s and galloping 3.41, which matches what the
  original measures (`orig_walk` / `orig_gallop`).
- Citizens moved too fast, by about 22% at walking speed. Their bodies
  have drag 1 as well as friction, so a held speed `s` actually moves at
  (s − 0.12)/1.02: a peasant walks at 0.47 instead of 0.6 and runs at
  1.25 instead of 1.4, matching the original (`orig_citizen_walk` /
  `orig_citizen_run`). Braking includes the drag too.
- Two builders landing the last stroke on the same site in the same tick
  reported it built twice and ran its "just built" effects twice (a
  TowerKnight would get two shield shops). Found by the `many_builders`
  seed sweep.

### Known issues (found by the regression runner)

- Nothing stops the monarch at the ends of the island (`island_edge`,
  XFAIL).
- Saving while coins are still tumbling in the purse changes how many
  spill after loading: the purse solver's warm-start impulses aren't saved.

## 1.2.0 — 2026-11-05

### Added

- `build.sh`: an automated install for Debian, Fedora and Arch based
  distros. It installs missing system packages, Rust (nightly, `cargo-3ds`)
  and devkitPro, builds the chosen target through `tools/setup.sh`, and lays
  out the SD card in `dist/sdcard/` (optionally copying it to `SDCARD`).
  Settings live in `.env` (template: `.example.env`).
  With no `TARGET` set, it asks whether to build the `.3dsx` (Homebrew
  Launcher) or the `.cia` (HOME Menu); without a terminal it builds the
  `.3dsx`.

### Fixed (purse)

- Coins in the purse jittered on the touch screen and never settled. The
  pile had been switched to one 1/30 s step every other tick to save time,
  but at that step a resting pile keeps rocking, so it never falls asleep.

### Changed

- The fires' light on the ground costs at most one quad per 3 px column
  (~134) instead of up to ~255 quads per fire: every light is summed into
  one row of columns, and each column fades with a vertical gradient
  (`Renderer::add_gradient`) instead of three stepped bands, which also
  removes the visible banding. In the frame bench's busiest frame, quads
  drop from 1173 to 607 (additive fills 784 → 96).
- 3DS renderer: translucent quads are written straight into the GPU's
  vertex buffer in painter's order instead of being collected, regrouped
  into a per-frame temporary list and copied again (one to two fewer
  copies of most of the scene per frame, and no per-frame allocation).
- 3DS: the tick profiler only reads the clock while the SELECT overlay is
  shown (`game::prof::set_enabled`), and release builds use one codegen
  unit.

### Fixed

- The sun and moon's glows flooded the sky, washing night out to a warm haze
  and bleeding above the horizon after sunset. Every glow child drew the first
  child's sprite and scale in its own place, each child's `SpriteRenderer`
  colour was ignored, and the glows stayed drawn while their body was below
  the horizon. Each child now draws with its own sprite, scale and colour
  under the preset's glow colour, and fades out as its body sets
  (`CelestialDef::glows`, `celestial_up`). Clouds remain a known deviation
  (docs/deviations.md).
- The night sky had no stars. A procedural Heaven-layer field now fades with
  the lighting presets' blended `stars` level and twinkles gently
  (`scene::top::draw_stars`).
- Fire light was a halo painted over the scene, which showed as an oval
  over the night sky, and wall torches' low-quality glow washed the sky and
  water grey. As on the PC's high quality setting, every fire is now a
  point light that falls only on lit game-layer sprites: each sprite's tint
  takes ambient + Σ light × falloff at its centre (wide sprites excepted),
  and the ground strip under each light gets warm columns fading toward the
  water. The sky, haze, water and far layers stay unlit
  (`scene::top::fire_light`, `Ctx::point_lit`, `draw_ground_light`).
- Fires had no glint on the water. Each now hangs the original's
  `Reflected Light Small` streak below the bank, fading in as the light
  drops (`scene::top::draw_fire_reflection`).

### Performance


- The draw list sorts packed integer keys instead of stable-sorting tuples
- Coins pick up in one fused pass over the grabbers instead of two scans,
  flying coins skip the pickup checks when nothing can reach them, and
  coins being sucked into a purse find it through a sorted index rather
  than a linear scan, flattening the spike while a crowd gathers a pile.
- A builder's hammer stroke no longer rebuilds the payable list; it
  refreshes when a site finishes or is paid for instead of at up to 60 Hz.
- Worker reassignment counts busy workers in one pass rather than scanning
  every unit per site, and the night-shelter check finds a worker's job by
  binary search rather than a scan of every object.
- A stress benchmark (`cargo test -p game --features profile --release
  --test perf`) and bag solver profiler marks document the work in
  [docs/perf.md](docs/perf.md).

### Fixed

- The fill and haze gradients skipped the ambient light, which kept nights
  bright and warm. They use the lit `Haze Material` in the original, so they
  now go through the same multiply as the sprites. The sky quad's
  `Custom/Sky` shader takes no lighting, so the sky still shows the presets'
  colours as they are, and the night sky is navy, not black
  (`scene::top::draw_world`).
- Water was the mirrored scene times a lifted `waterColor`, which read
  flat green at dusk and pale at dawn. It now follows the original's
  `Custom/Water` (read from the PC build's GLSL): the reflection fades into
  `waterColor` with depth, by `(v + 0.2) × waterColor.a`, so the shore
  mirrors the scene and the deep water takes the preset's colour; rain and
  snow mix in the ambient light (`scene::top::draw_water`). Both backends'
  `reflect` now add.

### Changed

- Desktop dev flags: `--weather "Clear Summer"` holds the lighting to one
  weather's presets and `--hour H` starts the clock at hour H, for comparing
  looks against the original; `KINGDOM_LIGHTS=1` lists the blender's presets
  when a screenshot is taken.
- `world.bin`'s layout changed: celestial glow children are stored per child
  (sprite, depth, scale, colour). Re-run the asset conversion to rebuild
  `world.bin`; saves made against the previous world data are rejected by its
  fingerprint and start fresh.

## 1.0.0 — 2026-11-03

Initial versioned release of the Kingdom: New Lands fan port for Nintendo 3DS,
with a shared desktop backend and local asset conversion tools.

### Fixed

- Saving over existing SD-card slots now handles FAT rename restrictions. Writes
  flush and close before replacement, retain a backup on the SD filesystem, and
  attempt rollback on failure. Missing primary saves can recover from backups.
  The user confirmed the save fix works on hardware.
- Tree chopping and builder sounds use the worker position and distance rolloff,
  preventing work across the map from playing at full volume.
- Free archers rebalance every second after recruitment and losses. Tower guards
  and knight followers keep their assignments; odd free counts differ by one.
- Save migration and world-index validation reject incompatible or malformed data.
- Rendering submission, queued audio effects, audio decoding and mixing, and
  platform initialization have additional validation and failure handling.
- Simulation edge cases, asset parsing, and conversion tools have additional
  bounds checks and regression coverage.

### Build and distribution

- Workspace and 3DS package versions are now 1.0.0.
- Packaging fallback sources are pinned, publication checks are expanded, and
  Rust 1.88 is the declared minimum supported version.
- Converted game assets and asset-containing CIA/3DSX bundles remain local;
  releases of this repository contain source only. Supply your own game files.

### Validation and remaining checks

Workspace tests, Clippy with warnings denied, standalone save replacement tests,
and the 3DS release build pass. Hardware save behavior is confirmed by the user;
audio range and archer balancing still need in-game confirmation. Broader GPU/DSP
soak testing and other follow-ups are tracked in
[the review status](docs/fix-review-status.md) and
[the hardware follow-up](docs/hardware-followup-2026-11-03.md).
