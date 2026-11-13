//! The CI gate for the regression framework. Runs without the game's files.
//! See docs/regression/ARCHITECTURE.md.

use game::controls::RawInput;
use game::progress::Progress;
use regress::golden;
use regress::reference::{self, Reference};
use regress::scenario::{self, Scenario};
use regress::scenarios;
use regress::script::Script;
use regress::sim::{state_hash, Sim};
use regress::snapshot::Tolerance;
use scene::session::Session;

fn refs() -> Vec<Reference> {
    let (refs, errors) = reference::load_all(&reference::dir());
    assert!(
        errors.is_empty(),
        "invalid reference fixtures:\n{}",
        errors.join("\n")
    );
    refs
}

fn final_hashes(sc: &Scenario, seed: u32, tpf: u32, refs: &[Reference]) -> (String, String) {
    let o = scenario::run(sc, seed, tpf, refs);
    let f = o.final_snapshot();
    (f.state_hash.clone(), f.event_hash.clone())
}

/// A scenario passes, or fails as its known issue says it does.
fn assert_passes(sc: &Scenario, seed: u32, refs: &[Reference]) {
    let o = scenario::run(sc, seed, 1, refs);
    let mut problems = o.failures.clone();
    problems.extend(o.violations.iter().map(|(t, v)| format!("tick {t}: {v}")));
    for (id, _, f) in &o.parity {
        problems.extend(f.iter().map(|f| format!("parity {id}: {f}")));
    }
    match sc.known_issue {
        None => assert!(
            problems.is_empty(),
            "{} (seed {seed}) failed:\n  {}",
            sc.name,
            problems.join("\n  ")
        ),
        Some(issue) => assert!(
            !problems.is_empty(),
            "{} (seed {seed}) passes: the known issue looks fixed ({issue}); remove `known_issue`",
            sc.name
        ),
    }
}

#[test]
fn scenario_names_are_unique() {
    let all = scenarios::all();
    let mut names: Vec<&str> = all.iter().map(|s| s.name).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), all.len());
}

#[test]
fn scenarios_pass_on_their_seed() {
    let refs = refs();
    for sc in scenarios::all() {
        assert_passes(&sc, sc.seed, &refs);
    }
}

/// Checks must hold for any seed, not just the blessed one. Seed 2 of
/// `many_builders` caught two builders finishing one site in the same tick.
#[test]
fn scenarios_pass_across_seeds() {
    let refs = refs();
    for sc in scenarios::all() {
        for seed in 1..=8 {
            assert_passes(&sc, seed, &refs);
        }
    }
}

#[test]
fn runs_are_deterministic() {
    let refs = refs();
    for sc in scenarios::all() {
        assert_eq!(
            final_hashes(&sc, sc.seed, 1, &refs),
            final_hashes(&sc, sc.seed, 1, &refs),
            "{} diverged between two identical runs",
            sc.name
        );
    }
}

/// The Old 3DS catches up by running 2-3 ticks per frame; the outcome must
/// be identical to 1 tick per frame for frame-aligned scripts.
#[test]
fn frame_batching_does_not_change_the_simulation() {
    let refs = refs();
    for sc in scenarios::all() {
        let one = final_hashes(&sc, sc.seed, 1, &refs);
        for tpf in [2, 3] {
            assert_eq!(
                one,
                final_hashes(&sc, sc.seed, tpf, &refs),
                "{} at {tpf} ticks per frame",
                sc.name
            );
        }
    }
}

/// The runner must drive the game exactly as the backends' `Session` does.
#[test]
fn runner_matches_session() {
    for sc in scenarios::all() {
        for tpf in [1u32, 3] {
            let mut sim = (sc.setup)(sc.seed);
            let script = (sc.script)();
            let (world, game) = (sim.world.clone(), sim.game.clone());
            sim.run(&script, tpf);

            let mut session = Session::new(world.clone(), 0, sc.seed, Progress::everything());
            session.game = game;
            let inputs: Vec<RawInput> = script.inputs().collect();
            for chunk in inputs.chunks(tpf as usize) {
                session.step_ticks(chunk[0], chunk.len() as u32);
            }
            assert_eq!(
                sim.state_hash(),
                state_hash(&session.game),
                "{} at {tpf} ticks per frame: runner and Session differ",
                sc.name
            );
        }
    }
}

/// Scenarios whose outcome changes across a mid-run save and load, and why.
/// They must keep diverging (so a fix is noticed and the entry removed).
const SAVE_LOAD_KNOWN: &[(&str, &str)] = &[
    // `bag::Bag::warm` (the purse solver's warm-start impulses) is
    // `serde(skip)`: after loading, coins still tumbling in the purse settle
    // differently and spill a different number of coins.
    ("purse_overflow", "purse warm-start impulses aren't saved"),
    (
        "purse_full_pickup",
        "purse warm-start impulses aren't saved",
    ),
    ("mass_coins", "purse warm-start impulses aren't saved"),
];

/// Saving and loading mid-run must not change where the run ends up.
#[test]
fn save_and_load_mid_run_continues_identically() {
    let mut problems = Vec::new();
    for sc in scenarios::all() {
        let script = (sc.script)();
        let inputs: Vec<RawInput> = script.inputs().collect();
        // Split on a frame boundary of 6 ticks.
        let half = inputs.len() / 2 / 6 * 6;

        let mut whole = (sc.setup)(sc.seed);
        whole.run_inputs(&inputs, 1);

        let mut split = (sc.setup)(sc.seed);
        split.run_inputs(&inputs[..half], 1);
        let bytes = game::save::save_game(&split.game, 0);
        split.game = game::save::load_game(&bytes, 0, &split.world)
            .unwrap_or_else(|e| panic!("{}: load failed: {e:?}", sc.name));
        split.run_inputs(&inputs[half..], 1);

        let same = whole.state_hash() == split.state_hash() && whole.log.hash() == split.log.hash();
        let known = SAVE_LOAD_KNOWN.iter().find(|k| k.0 == sc.name);
        match (same, known) {
            (true, None) | (false, Some(_)) => {}
            (false, None) => problems.push(format!(
                "{}: save/load at tick {half} changed the outcome: {:?}",
                sc.name,
                regress::snapshot::Snapshot::of(&whole).diff(
                    &regress::snapshot::Snapshot::of(&split),
                    Tolerance::default()
                )
            )),
            (true, Some((_, why))) => problems.push(format!(
                "{}: now survives save/load; remove it from SAVE_LOAD_KNOWN ({why})",
                sc.name
            )),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// What the runner records replays to the same state, through TOML.
#[test]
fn recorded_input_replays_identically() {
    for sc in scenarios::all() {
        let mut a = (sc.setup)(sc.seed);
        a.run(&(sc.script)(), 1);
        let text = a.recorded.to_toml();
        let script = Script::from_toml(&text).expect("recorded script parses");
        let mut b = (sc.setup)(sc.seed);
        b.run(&script, 1);
        assert_eq!(a.state_hash(), b.state_hash(), "{}", sc.name);
    }
}

#[test]
fn positioned_coin_impacts_keep_the_replay_event_identity() {
    use game::{coins::CoinEvent, state::GameEvent};
    use regress::sim::EventLog;

    let mut legacy = EventLog::new(1);
    legacy.push(14, &GameEvent::Coin(CoinEvent::HitGround));
    let mut positioned = EventLog::new(1);
    positioned.push(14, &GameEvent::Coin(CoinEvent::HitGroundAt { x: 25.0 }));

    assert_eq!(positioned.lines, legacy.lines);
    assert_eq!(positioned.hash(), legacy.hash());
    assert_eq!(positioned.count("Coin.HitGround"), 1);
}

/// Desktop-style recording through `Recorder` and playback through
/// `Replay::play` (synthetic world, so it runs in CI).
#[test]
fn replay_file_round_trips() {
    use regress::replay::{Recorder, Replay};
    let world = regress::worlds::world(5);
    let progress = Progress::everything();
    let mut session = Session::new(world.clone(), 0, 9, progress.clone());
    let mut rec = Recorder::new("test", 0xdead_beef, 0, 9, &progress, None);
    let frames = Script::new()
        .walk(1, 2.0)
        .tap(6)
        .idle(1.0)
        .gallop(-1, 3.0)
        .hold_pay(1.0)
        .idle(2.0);
    let inputs: Vec<RawInput> = frames.inputs().collect();
    for (i, chunk) in inputs.chunks(2).enumerate() {
        let ticks = if i % 5 == 0 { 1 } else { chunk.len() as u32 };
        rec.frame(chunk[0], ticks);
        session.step_ticks(chunk[0], ticks);
    }
    let replay = rec.finish(&session);
    let back = Replay::from_toml(&replay.to_toml()).expect("replay parses");
    let played = back.play(world).expect("replay plays");
    assert_eq!(
        format!("{:016x}", state_hash(&played.game)),
        replay.final_state_hash
    );
}

/// Every scenario has a blessed golden and still matches it.
#[test]
fn goldens_match() {
    let refs = refs();
    let dir = golden::dir();
    let mut problems = Vec::new();
    for sc in scenarios::all() {
        let o = scenario::run(&sc, sc.seed, 1, &refs);
        match golden::load(&dir, sc.name) {
            None => problems.push(format!(
                "{}: no golden; run `cargo run -p regress -- run --bless {}`",
                sc.name, sc.name
            )),
            Some(Err(e)) => problems.push(format!("{}: {e}", sc.name)),
            Some(Ok(g)) => {
                let r = golden::compare(&g, &o.checkpoints, Tolerance::default());
                problems.extend(r.diffs.iter().map(|d| format!("{}: {d}", sc.name)));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "golden mismatches (if intended: `cargo run -p regress -- run --bless`):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn parity_scenarios_name_existing_fixtures() {
    let refs = refs();
    for sc in scenarios::all() {
        if let scenario::Class::Parity(ids) = sc.class {
            for id in ids {
                assert!(
                    refs.iter().any(|r| r.id == *id),
                    "{}: no fixture {id}",
                    sc.name
                );
            }
        }
    }
}

fn fixture(evidence: &str) -> String {
    format!(
        r#"
id = "x"
title = "t"
[original]
game_version = "v"
platform = "pc-linux"
[setup]
description = "d"
[[expected]]
metric = "m"
value = 1.0
tolerance = 0.1
[evidence]
{evidence}
"#
    )
}

#[test]
fn fixtures_cannot_overclaim() {
    let check = |ev: &str| -> Vec<String> {
        let r: Reference = toml::from_str(&fixture(ev)).expect("parses");
        r.validate(Some("x"))
    };
    let base = r#"source = "s"
confidence = "medium""#;
    assert!(check(&format!(
        "kind = \"documented-mechanic\"\n{base}\nindependently_verified = false"
    ))
    .is_empty());
    // Verified without saying who verified it.
    assert!(!check(&format!(
        "kind = \"manual-observation\"\n{base}\nindependently_verified = true"
    ))
    .is_empty());
    // High confidence without verification.
    assert!(!check("kind = \"manual-observation\"\nsource = \"s\"\nconfidence = \"high\"\nindependently_verified = false").is_empty());
    // A repeated measurement of one run.
    assert!(!check(&format!(
        "kind = \"repeated-measurement\"\n{base}\nindependently_verified = false\nsamples = 1"
    ))
    .is_empty());
    // A screenshot as the evidence source.
    assert!(!check("kind = \"recording\"\nsource = \"shots/day1.png\"\nconfidence = \"low\"\nindependently_verified = false").is_empty());
}

#[test]
fn unverified_parity_is_labelled_unverified() {
    for r in refs() {
        if !r.evidence.independently_verified {
            assert!(r.label().contains("unverified"), "{}", r.id);
        }
    }
}

#[test]
fn loads_versioned_world_and_fingerprints_the_full_file() {
    use game::assetfile::{self, Kind};

    let dir = std::env::temp_dir().join(format!("kingdom-regress-world-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("world.bin");
    let world = regress::worlds::lit_world();
    let bytes = assetfile::encode(Kind::World, &world).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let loaded = regress::load_world(&dir);

    std::fs::write(&path, postcard::to_allocvec(&world).unwrap()).unwrap();
    let rejected = regress::load_world(&dir);
    std::fs::remove_dir_all(dir).unwrap();

    let (decoded, id) = loaded.unwrap();
    assert_eq!(decoded, world);
    assert_eq!(id, game::save::fingerprint(&bytes));
    let error = rejected.unwrap_err();
    assert!(
        error.contains("world.bin") && error.contains("re-extract"),
        "{error}"
    );
}

/// Real islands from the user's own converted game. Not in CI:
/// `cargo test -p regress --release -- --ignored`.
#[test]
#[ignore]
fn real_island_soak_is_deterministic_and_sane() {
    let configured = std::env::var_os("KINGDOM_ASSETS");
    let dir = configured.as_deref().map_or_else(
        || std::path::Path::new("../../assets"),
        std::path::Path::new,
    );
    let path = dir.join("world.bin");
    if configured.is_none()
        && !path
            .try_exists()
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    {
        eprintln!("no {}; skipping", path.display());
        return;
    }
    let (world, _) = regress::load_world(dir).unwrap_or_else(|e| panic!("{e}"));
    let pilot = game::autopilot::Autopilot::default();
    let run = |seed| {
        let g = game::state::GameState::new(&world, 0, seed);
        let mut s = Sim::new(world.clone(), g);
        s.log.keep = 0;
        for f in 0..(3 * 240 * 60) {
            let raw = pilot.input(&s.game, &s.world, f);
            s.frame(raw, 1);
        }
        s
    };
    for seed in 1..=3 {
        let (a, b) = (run(seed), run(seed));
        assert!(a.violations.is_empty(), "seed {seed}: {:?}", a.violations);
        assert_eq!(
            a.state_hash(),
            b.state_hash(),
            "seed {seed} not deterministic"
        );
    }
}

/// The original-game pipeline without the game: a stand-in trace (the port
/// on a synthetic island, relabelled) becomes a valid fixture that the
/// port's numbers then satisfy.
#[test]
fn orig_fixture_pipeline_round_trips() {
    use regress::orig;
    // world(0) plus lighting presets, for orig_time_of_day.
    let world = regress::worlds::lit_world();
    for sc in orig::scenarios() {
        if sc.name == "orig_day_cycle" {
            continue; // 43 800 frames: covered by day_night_cycle
        }
        let traces: Vec<_> = (1..=2)
            .map(|seed| {
                let mut t = orig::port_trace(&sc, &world, seed);
                t.header = vec![
                    ("harness".into(), "KingdomTrace 1.0.0".into()),
                    ("game_version".into(), "test".into()),
                    ("seed".into(), seed.to_string()),
                ];
                (std::path::PathBuf::from(format!("seed-{seed}")), t)
            })
            .collect();
        let r = orig::fixture(&sc, &traces).unwrap_or_else(|e| panic!("{}: {e}", sc.name));
        assert!(
            r.validate(Some(sc.fixture_id)).is_empty(),
            "{}: {:?}",
            sc.name,
            r.validate(None)
        );
        assert_eq!(
            r.evidence.kind,
            regress::reference::EvidenceKind::RepeatedMeasurement
        );
        assert!(r.label().contains("unverified"));
        let back: Reference = toml::from_str(&orig::fixture_toml(&r)).expect("fixture parses");
        let m = (sc.metrics)(&traces[0].1);
        assert!(
            back.compare(&m).is_empty(),
            "{}: {:?}",
            sc.name,
            back.compare(&m)
        );
    }
}
