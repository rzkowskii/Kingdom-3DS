# Contributing

Thanks for helping. This port aims to play like the original Kingdom: New Lands, so the main rule for any change is that it shouldn't change gameplay by accident. The parity and regression runner (`crates/regress`) is how we check that.

## Before you start

- You need your own copy of the game to build a playable version (see the [README](README.md#installation)). The checks below run without any game files.
- Never commit game files, extracted assets, decompiled C#, traces, screenshots or recordings from the original. Keep them in the ignored folders (`gamefiles/`, `reference/`, `assets/`, `dist/`). See [the publication audit](docs/publication-audit.md).
- Don't commit absolute paths, mount points or usernames. Use relative paths or placeholders.

## Run the parity and regression tests before and after your change

**We recommend running these tests before and after every fix or optimization**, so you can be sure the change didn't break or shift anything else.

1. **Before you change anything**, on a clean checkout, record a baseline:

   ```sh
   cargo test -p regress
   cargo run --release -p regress -- run --seeds 24 --tpf 1,2,3 --report target/before.toml
   ```

   If something already fails here, it isn't caused by your change. Note it in your pull request.

2. **Make your change.**

3. **Run the same tests again**:

   ```sh
   cargo test -p regress
   cargo run --release -p regress -- run --seeds 24 --tpf 1,2,3 --report target/after.toml
   ```

   Compare `target/before.toml` and `target/after.toml` (inside the ignored `target/` folder, so they never get committed). CI runs the same sweep and uploads its report.

What a failure means:

| Report | Meaning | What to do |
|---|---|---|
| `PARITY` check fails | The port no longer matches a value measured in, or documented from, the original game (`crates/regress/reference/`). | Treat it as a bug in your change unless you have evidence that the fixture is wrong. |
| Golden mismatch | The port's own behavior changed compared with the blessed snapshot in `crates/regress/golden/`. | If the change is intended (a gameplay fix), re-bless with `cargo run -p regress -- run --bless <name>` and explain why in your pull request. If it isn't, fix it. |
| Determinism or `--tpf` failure | Results depend on run order or on how many ticks are batched per frame. | Always a bug: never re-bless these. |
| `XPASS` | A scenario marked as a known issue started passing. | Remove its `known_issue` marker. |

**Optimizations** must not change gameplay at all. For a pure performance change, also run:

```sh
cargo run --release -p regress -- run --strict
```

`--strict` fails on any change to a trajectory, not just ones outside the tolerances.

If you have your own converted assets, you can also soak-test real islands:

```sh
cargo run --release -p regress -- soak --assets assets --seeds 20 --days 8
```

## Other checks

CI runs these, so run them before you open a pull request:

```sh
cargo fmt --all --check
(cd crates/n3ds && cargo fmt --check)
cargo build --workspace --all-targets --locked
cargo test --workspace --locked
cargo run --release --locked -p regress -- run --seeds 24 --tpf 1,2,3
cargo clippy --workspace --all-targets --locked -- -D warnings
shellcheck build.sh tools/setup.sh tools/cia/build.sh
python3 tools/check_public_files.py
```

`cargo fmt --all` (and `cargo fmt` inside `crates/n3ds`, which is outside the workspace) fixes formatting. Run it before you commit so the format check passes.

For changes to the 3DS backend or anything performance-sensitive, please also test on hardware if you can, ideally on an Old 3DS, and say which model you used.

## Parity with the original

- Gameplay values come from the original game. Comments name the original class or method (`orig: Mover.Update()`) so others can check them.
- Known differences are listed in [docs/deviations.md](docs/deviations.md). If your change adds or removes one, update that file.
- To measure something in the original, use the local harness in [`tools/orig-harness/`](tools/orig-harness/README.md). It turns measurements into reference fixtures that contain numbers only. To add a scenario, see [docs/regression/ARCHITECTURE.md](docs/regression/ARCHITECTURE.md).

## Pull requests

- Keep each pull request to one fix or feature.
- Add an entry under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md).
- In the description, say what changed and why, give the before/after test results, and explain any goldens you re-blessed.
