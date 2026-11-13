//! Gameplay regression runner. See `docs/regression/ARCHITECTURE.md`.
//!
//! ```text
//! regress list
//! regress run [NAME...] [--seed S] [--seeds N] [--seed-base S] [--tpf 1,2,3] [--bless] [--strict]
//!             [--events] [--report FILE]
//! regress soak --assets DIR [--seeds N] [--seed-base S] [--days D] [--island I]
//! regress replay FILE --assets DIR
//! regress orig list | export NAME [--seed S] [--out DIR]
//! regress orig run NAME [--runs N] [--seed-base S] [--timeout SECONDS]
//! regress orig fixture NAME [TRACE...] [--publish]
//! regress orig compare NAME --assets DIR [--seed S]
//! regress orig check --assets DIR [--seeds N]
//! regress tod --assets DIR [--seed S] [--samples N] [--days D] [--island I] [--shots] [--orig]
//!             [--weather NAME[,NAME...]|all] [--out DIR] [--timeout SECONDS]
//! ```

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use regress::golden;
use regress::reference;
use regress::scenario::{self, Class, Scenario};
use regress::scenarios;
use regress::sim::Sim;
use regress::snapshot::{Snapshot, Tolerance};

struct Args(Vec<String>);

impl Args {
    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.0
            .windows(2)
            .find(|w| w[0] == name)
            .map(|w| w[1].as_str())
    }
    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        self.value(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    /// Arguments that aren't flags or flag values.
    fn free(&self) -> Vec<&str> {
        let valued = [
            "--seed",
            "--seeds",
            "--seed-base",
            "--tpf",
            "--report",
            "--assets",
            "--days",
            "--island",
            "--runs",
            "--timeout",
            "--out",
            "--samples",
            "--weather",
        ];
        let mut out = Vec::new();
        let mut skip = false;
        for a in &self.0 {
            if skip {
                skip = false;
            } else if valued.contains(&a.as_str()) {
                skip = true;
            } else if !a.starts_with("--") {
                out.push(a.as_str());
            }
        }
        out
    }
}

fn main() -> ExitCode {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = all
        .split_first()
        .map_or(("help", &[][..]), |(c, r)| (c.as_str(), r));
    let args = Args(rest.to_vec());
    let ok = match cmd {
        "list" => list(),
        "run" => run(&args),
        "soak" => soak(&args),
        "replay" => replay(&args),
        "orig" => orig(&args),
        "tod" => tod(&args),
        _ => {
            eprintln!(
                "{}",
                include_str!("regress.rs")
                    .lines()
                    .skip(3)
                    .take(13)
                    .map(|l| l.trim_start_matches("//! "))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            true
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn class_label(sc: &Scenario) -> String {
    match sc.class {
        Class::Internal => "INTERNAL".into(),
        Class::Parity(ids) => format!("PARITY {}", ids.join(",")),
    }
}

fn list() -> bool {
    for sc in scenarios::all() {
        println!(
            "{:<20} {:<13} {:<40} {}",
            sc.name,
            format!("{:?}", sc.area),
            class_label(&sc),
            sc.summary
        );
    }
    let (refs, errors) = reference::load_all(&reference::dir());
    println!("\nreference fixtures:");
    for r in &refs {
        println!("  {:<28} {}", r.id, r.label());
    }
    for e in &errors {
        println!("  INVALID {e}");
    }
    errors.is_empty()
}

fn run(args: &Args) -> bool {
    let names = args.free();
    let bless = args.flag("--bless");
    let strict = args.flag("--strict");
    let tpfs: Vec<u32> = args
        .value("--tpf")
        .map(|v| v.split(',').filter_map(|t| t.parse().ok()).collect())
        .unwrap_or_default();
    let seeds: u32 = args.num("--seeds", 0);
    let seed_base: u32 = args.num("--seed-base", 1);
    let (refs, ref_errors) = reference::load_all(&reference::dir());
    let mut ok = ref_errors.is_empty();
    for e in &ref_errors {
        println!("reference fixture INVALID: {e}");
    }
    let gdir = golden::dir();
    let mut report = String::from("# regress run report\n");
    let _ = writeln!(
        report,
        "commit = {:?}\nplatform = {:?}\nprofile = {:?}\n",
        env!("REGRESS_COMMIT"),
        env!("REGRESS_TARGET"),
        env!("REGRESS_PROFILE")
    );

    let list: Vec<Scenario> = scenarios::all()
        .into_iter()
        .filter(|s| names.is_empty() || names.contains(&s.name))
        .collect();
    if list.is_empty() {
        println!("no scenario matches {names:?} (see `regress list`)");
        return false;
    }
    for sc in &list {
        let mut problems: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        // `--seed` replays one other seed (no golden for it).
        let seed: u32 = args.num("--seed", sc.seed);
        let out = scenario::run(sc, seed, 1, &refs);
        problems.extend(out.failures.iter().cloned());
        problems.extend(out.violations.iter().map(|(t, v)| format!("tick {t}: {v}")));
        for (id, _, fails) in &out.parity {
            problems.extend(fails.iter().map(|f| format!("parity {id}: {f}")));
        }

        // Checks above may be an expected failure (a known issue); what
        // follows (determinism, batching, goldens) never is.
        let mut hard: Vec<String> = Vec::new();

        // Determinism: the same run again must match bit for bit.
        let again = scenario::run(sc, seed, 1, &refs);
        if !out.final_snapshot().same_trajectory(again.final_snapshot()) {
            hard.push("not deterministic: a second identical run diverged".into());
        }

        // Frame batching (the Old 3DS catch-up path).
        for &tpf in tpfs.iter().filter(|&&t| t != 1) {
            let b = scenario::run(sc, seed, tpf, &refs);
            if !out.final_snapshot().same_trajectory(b.final_snapshot()) {
                let diffs = out
                    .final_snapshot()
                    .diff(b.final_snapshot(), Tolerance::default());
                hard.push(format!(
                    "{tpf} ticks per frame diverged from 1: {}",
                    if diffs.is_empty() {
                        "trajectory only".into()
                    } else {
                        diffs.join("; ")
                    }
                ));
            }
        }

        // Goldens.
        if seed != sc.seed {
            notes.push(format!("seed {seed} (goldens are for seed {})", sc.seed));
        } else if bless {
            let g = golden::Golden {
                meta: out.meta.clone(),
                checkpoints: out.checkpoints.clone(),
            };
            match golden::save(&gdir, sc.name, &g) {
                Ok(()) => notes.push("blessed".into()),
                Err(e) => hard.push(format!("could not write golden: {e}")),
            }
        } else {
            match golden::load(&gdir, sc.name) {
                None => notes.push("no golden (run with --bless)".into()),
                Some(Err(e)) => hard.push(format!("golden unreadable: {e}")),
                Some(Ok(g)) => {
                    let r = golden::compare(&g, &out.checkpoints, Tolerance::default());
                    hard.extend(r.diffs.iter().map(|d| format!("golden: {d}")));
                    if r.trajectory_changed && r.diffs.is_empty() {
                        let msg = format!(
                            "trajectory changed since {} but outcomes held",
                            g.meta.commit
                        );
                        if strict {
                            hard.push(msg);
                        } else {
                            notes.push(msg);
                        }
                    }
                }
            }
        }

        // Seed sweep: checks and invariants must hold for every seed.
        let mut swept = 0;
        let mut sweep_fail = 0;
        let sweep_start = Instant::now();
        for seed in seed_base..seed_base + seeds {
            let o = scenario::run(sc, seed, 1, &refs);
            swept += 1;
            if !o.passed() {
                sweep_fail += 1;
                if sweep_fail <= 3 {
                    let first = o
                        .failures
                        .first()
                        .cloned()
                        .or_else(|| o.violations.first().map(|(t, v)| format!("tick {t}: {v}")))
                        .or_else(|| o.parity.iter().find_map(|p| p.2.first().cloned()))
                        .unwrap_or_default();
                    problems.push(format!("seed {seed}: {first}"));
                }
            }
        }
        if sweep_fail > 3 {
            problems.push(format!("... {sweep_fail} of {swept} seeds failed"));
        }

        let ticks = out.meta.ticks;
        let tps = ticks as f64 / out.elapsed_s.max(1e-9);
        let (status, mut problems) = match (sc.known_issue, problems.is_empty()) {
            (None, true) => ("PASS", Vec::new()),
            (None, false) => ("FAIL", problems),
            (Some(issue), false) => {
                notes.insert(0, format!("known issue: {issue}"));
                notes.extend(problems.iter().map(|p| format!("(expected) {p}")));
                ("XFAIL", Vec::new())
            }
            (Some(_), true) => (
                "XPASS",
                vec![
                    "the known issue no longer reproduces: remove `known_issue` (and re-bless)"
                        .into(),
                ],
            ),
        };
        problems.extend(hard);
        let status = if status != "FAIL" && status != "XPASS" && !problems.is_empty() {
            "FAIL"
        } else {
            status
        };
        let label = if out.parity.is_empty() {
            class_label(sc)
        } else {
            out.parity
                .iter()
                .map(|p| p.1.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{status:<5} {:<20} {:>6} ticks {:>8.1} ms {:>9.0} ticks/s  {label}{}",
            sc.name,
            ticks,
            out.elapsed_s * 1e3,
            tps,
            if swept > 0 {
                format!(
                    "  [{} seeds, {:.1} s]",
                    swept,
                    sweep_start.elapsed().as_secs_f64()
                )
            } else {
                String::new()
            }
        );
        for p in &problems {
            println!("     ✗ {p}");
        }
        let problems_ok = problems.is_empty();
        for n in &notes {
            println!("     · {n}");
        }
        if args.flag("--events") {
            for l in &out.event_lines {
                println!("       {l}");
            }
        }
        if !out.metrics.is_empty() {
            let m: Vec<String> = out
                .metrics
                .iter()
                .map(|(k, v)| format!("{k} = {v:.4}"))
                .collect();
            println!("     metrics: {}", m.join(", "));
        }
        ok &= problems_ok;
        let _ = writeln!(
            report,
            "[scenario.{}]\nstatus = {status:?}\nclass = {:?}\nticks = {ticks}\nms = {:.3}\nticks_per_s = {tps:.0}\nstate_hash = {:?}\nproblems = {:?}\n",
            sc.name,
            label,
            out.elapsed_s * 1e3,
            out.final_snapshot().state_hash,
            problems
        );
    }
    if let Some(path) = args.value("--report") {
        if let Err(e) = std::fs::write(path, report) {
            println!("could not write report {path}: {e}");
            ok = false;
        }
    }
    ok
}

fn soak(args: &Args) -> bool {
    let Some(assets) = args.value("--assets") else {
        println!("soak needs --assets DIR (your converted assets/, with world.bin)");
        return false;
    };
    let (world, id) = match regress::load_world(&PathBuf::from(assets)) {
        Ok(w) => w,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    let seeds: u32 = args.num("--seeds", 10);
    let seed_base: u32 = args.num("--seed-base", 1);
    let days: u32 = args.num("--days", 6);
    let island: usize = args.num("--island", 0);
    let ticks = days * 240 * 60;
    println!(
        "soak: world {id:08x}, island {island}, {seeds} seeds, {days} days ({ticks} ticks) each"
    );
    let pilot = game::autopilot::Autopilot::default();
    let mut ok = true;
    let mut first_hash = None;
    for (i, seed) in (seed_base..seed_base + seeds).enumerate() {
        let run_once = || {
            let game = game::state::GameState::new(&world, island, seed);
            let mut sim = Sim::new(world.clone(), game);
            sim.log.keep = 0;
            let started = Instant::now();
            for f in 0..ticks {
                let raw = pilot.input(&sim.game, &sim.world, f);
                sim.frame(raw, 1);
                if sim.game.game_over || sim.game.won {
                    break;
                }
            }
            (
                Snapshot::of(&sim),
                sim.violations.clone(),
                started.elapsed().as_secs_f64(),
                sim.game.population.units.len(),
            )
        };
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_once));
        match res {
            Err(_) => {
                println!("FAIL seed {seed}: panicked");
                ok = false;
            }
            Ok((snap, violations, secs, units)) => {
                if i == 0 {
                    // Determinism on a real island: run the first seed again.
                    if let Ok((again, ..)) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_once))
                    {
                        if !again.same_trajectory(&snap) {
                            println!("FAIL seed {seed}: not deterministic on a real island");
                            ok = false;
                        }
                    }
                    first_hash = Some(snap.state_hash.clone());
                }
                let status = if violations.is_empty() {
                    "PASS"
                } else {
                    "FAIL"
                };
                ok &= violations.is_empty();
                println!(
                    "{status} seed {seed:<5} day {:<3} wallet {:<4} citizens {:<3} objects {:<4} {}{:>7.0} ticks/s  {}",
                    snap.day_shown,
                    snap.wallet,
                    units,
                    snap.objects,
                    if snap.game_over { "GAME OVER " } else if snap.won { "WON " } else { "" },
                    snap.tick as f64 / secs.max(1e-9),
                    snap.state_hash
                );
                for (t, v) in &violations {
                    println!("     ✗ tick {t}: {v}");
                }
            }
        }
    }
    let _ = first_hash;
    ok
}

fn replay(args: &Args) -> bool {
    let (Some(file), Some(assets)) = (args.free().first().copied(), args.value("--assets")) else {
        println!("usage: regress replay FILE --assets DIR");
        return false;
    };
    let rep = match std::fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|s| regress::replay::Replay::from_toml(&s))
    {
        Ok(r) => r,
        Err(e) => {
            println!("{file}: {e}");
            return false;
        }
    };
    let (world, id) = match regress::load_world(&PathBuf::from(assets)) {
        Ok(w) => w,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    if id != rep.world_fingerprint {
        println!(
            "world.bin fingerprint {id:08x} doesn't match the replay's {:08x}",
            rep.world_fingerprint
        );
        return false;
    }
    println!(
        "replay: recorded at {} on {}, island {}, seed {}, {} ticks",
        rep.meta.commit, rep.meta.platform, rep.meta.island, rep.meta.seed, rep.final_tick
    );
    for t in &rep.tainted {
        println!("  tainted: {t}");
    }
    let s = match rep.play(world) {
        Ok(s) => s,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    let hash = format!("{:016x}", regress::sim::state_hash(&s.game));
    let same = hash == rep.final_state_hash;
    println!(
        "{} final state {hash} (recorded {}); day {}, wallet {}, citizens {}",
        if same { "PASS" } else { "FAIL" },
        rep.final_state_hash,
        s.game.days.day,
        s.game.wallet.coins(),
        s.game.population.units.len()
    );
    if !same && !rep.tainted.is_empty() {
        println!("  (the recording was tainted, so a mismatch is expected)");
    }
    same
}

// ---- The original game (tools/orig-harness) ----------------------------

fn orig_dir(name: &str) -> PathBuf {
    regress::orig::repo_root()
        .join("gamefiles/harness/traces")
        .join(name)
}

fn orig_scenario(args: &Args) -> Option<regress::orig::OrigScenario> {
    let name = args.free().get(1).copied().unwrap_or("");
    let sc = regress::orig::find(name);
    if sc.is_none() {
        println!("no original-game scenario {name:?} (see `regress orig list`)");
    }
    sc
}

/// Local traces of `name`: `gamefiles/harness/traces/<name>/*/trace.txt`.
fn local_traces(name: &str) -> Vec<(PathBuf, regress::orig::Trace)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(orig_dir(name))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path().join("trace.txt"))
                .filter(|p| p.exists())
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            let t = regress::orig::Trace::parse(&text).ok()?;
            Some((p, t))
        })
        .collect()
}

fn print_metrics(label: &str, m: &std::collections::BTreeMap<String, f64>) {
    let parts: Vec<String> = m.iter().map(|(k, v)| format!("{k} = {v:.4}")).collect();
    println!("  {label:<22} {}", parts.join(", "));
}

/// A run directory for `name`/`seed`, created and made absolute.
fn run_dir(name: &str, seed: u32) -> PathBuf {
    let dir = orig_dir(name).join(format!("seed-{seed}"));
    let _ = std::fs::create_dir_all(&dir);
    dir.canonicalize().unwrap_or(dir)
}

fn orig(args: &Args) -> bool {
    match args.free().first().copied() {
        Some("list") => {
            for sc in regress::orig::scenarios() {
                println!(
                    "{:<22} {:>6} frames  -> {:<24} {}",
                    sc.name, sc.frames, sc.fixture_id, sc.summary
                );
            }
            true
        }
        Some("export") => orig_export(args),
        Some("run") => orig_run(args),
        Some("fixture") => orig_fixture(args),
        Some("compare") => orig_compare(args),
        Some("check") => orig_check(args),
        _ => {
            println!(
                "regress orig list|export|run|fixture|compare (see tools/orig-harness/README.md)"
            );
            false
        }
    }
}

fn orig_export(args: &Args) -> bool {
    let Some(sc) = orig_scenario(args) else {
        return false;
    };
    let seed: u32 = args.num("--seed", 1);
    let dir = match args.value("--out") {
        Some(d) => {
            let _ = std::fs::create_dir_all(d);
            PathBuf::from(d)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(d))
        }
        None => run_dir(sc.name, seed),
    };
    let written = regress::orig::config(&sc, seed, &dir.join("trace.txt"))
        .and_then(|c| std::fs::write(dir.join("config.cfg"), c).map_err(|e| e.to_string()));
    match written {
        Ok(()) => {
            println!("{}", dir.join("config.cfg").display());
            true
        }
        Err(e) => {
            println!("{e}");
            false
        }
    }
}

fn orig_run(args: &Args) -> bool {
    use regress::orig::{aggregate, config, repo_root, Trace};
    let Some(sc) = orig_scenario(args) else {
        return false;
    };
    let runs: u32 = args.num("--runs", 1);
    let base: u32 = args.num("--seed-base", 1);
    let timeout: u32 = args.num("--timeout", 1800);
    let script = repo_root().join("tools/orig-harness/run.sh");
    let mut ok = true;
    let mut all = Vec::new();
    for seed in base..base + runs {
        let dir = run_dir(sc.name, seed);
        let trace = dir.join("trace.txt");
        let _ = std::fs::remove_file(&trace);
        let cfg = match config(&sc, seed, &trace) {
            Ok(c) => c,
            Err(e) => {
                println!("{e}");
                return false;
            }
        };
        if let Err(e) = std::fs::write(dir.join("config.cfg"), cfg) {
            println!("{e}");
            return false;
        }
        println!("{}: seed {seed}, {} frames ...", sc.name, sc.frames);
        let status = std::process::Command::new(&script)
            .arg(dir.join("config.cfg"))
            .arg(timeout.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            println!(
                "  the game exited with {status:?} (logs in {})",
                dir.display()
            );
        }
        let parsed = std::fs::read_to_string(&trace)
            .map_err(|e| e.to_string())
            .and_then(|s| Trace::parse(&s));
        match parsed {
            Ok(t) => {
                let problems = t.problems();
                for p in &problems {
                    println!("  ✗ {p}");
                }
                ok &= problems.is_empty();
                let m = (sc.metrics)(&t);
                print_metrics(&format!("original seed {seed}"), &m);
                all.push(m);
            }
            Err(e) => {
                println!("  ✗ no usable trace: {e}");
                ok = false;
            }
        }
    }
    if all.len() > 1 {
        for (k, (mean, min, max)) in aggregate(&all) {
            println!("  {k:<22} mean {mean:.4}  range {min:.4} .. {max:.4}");
        }
    }
    println!(
        "next: regress orig fixture {} (writes the measured numbers as a fixture)",
        sc.name
    );
    ok
}

fn orig_fixture(args: &Args) -> bool {
    use regress::orig::{fixture, fixture_toml, repo_root, Trace};
    let Some(sc) = orig_scenario(args) else {
        return false;
    };
    let given: Vec<&str> = args.free().into_iter().skip(2).collect();
    let traces: Vec<(PathBuf, Trace)> = if given.is_empty() {
        local_traces(sc.name)
    } else {
        given
            .iter()
            .filter_map(|p| {
                let t = Trace::parse(&std::fs::read_to_string(p).ok()?).ok()?;
                Some((PathBuf::from(p), t))
            })
            .collect()
    };
    let r = match fixture(&sc, &traces) {
        Ok(r) => r,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    let problems = r.validate(Some(&r.id));
    if !problems.is_empty() {
        println!("fixture invalid: {problems:?}");
        return false;
    }
    let publish = args.flag("--publish");
    let dir = if publish {
        reference::dir()
    } else {
        repo_root().join("gamefiles/harness/fixtures")
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.toml", r.id));
    if let Err(e) = std::fs::write(&path, fixture_toml(&r)) {
        println!("{e}");
        return false;
    }
    println!(
        "wrote {} ({} runs, {})",
        path.display(),
        traces.len(),
        r.label()
    );
    for e in &r.expected {
        println!(
            "  {:<22} {} ± {} {}",
            e.metric, e.value, e.tolerance, e.unit
        );
    }
    if !publish {
        println!("review it, then add --publish to put it in crates/regress/reference/");
    }
    true
}

fn orig_compare(args: &Args) -> bool {
    use regress::orig::{aggregate, port_trace};
    let Some(sc) = orig_scenario(args) else {
        return false;
    };
    let Some(assets) = args.value("--assets") else {
        println!("compare needs --assets DIR (your converted assets/)");
        return false;
    };
    let (world, _) = match regress::load_world(&PathBuf::from(assets)) {
        Ok(w) => w,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    let seed: u32 = args.num("--seed", 1);
    let port = (sc.metrics)(&port_trace(&sc, &world, seed));
    let traces = local_traces(sc.name);
    let runs: Vec<_> = traces.iter().map(|(_, t)| (sc.metrics)(t)).collect();
    let agg = aggregate(&runs);
    println!(
        "{}: original ({} local traces) vs port (your island, seed {seed})",
        sc.name,
        traces.len()
    );
    println!(
        "  {:<22} {:>24} {:>12} {:>10}",
        "metric", "original (range)", "port", "diff"
    );
    for (k, v) in &port {
        match agg.get(k) {
            Some((mean, min, max)) => println!(
                "  {k:<22} {:>24} {v:>12.4} {:>+10.4}",
                format!("{mean:.4} ({min:.3}..{max:.3})"),
                v - mean
            ),
            None => println!("  {k:<22} {:>24} {v:>12.4}", "-"),
        }
    }
    if traces.is_empty() {
        println!(
            "no traces of the original yet: regress orig run {}",
            sc.name
        );
    }
    true
}

/// Every original-game scenario with a committed fixture, run on the port
/// with the user's real island and checked against the original's numbers.
fn orig_check(args: &Args) -> bool {
    use regress::orig::{port_trace, scenarios};
    let Some(assets) = args.value("--assets") else {
        println!("check needs --assets DIR (your converted assets/)");
        return false;
    };
    let (world, _) = match regress::load_world(&PathBuf::from(assets)) {
        Ok(w) => w,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    let (refs, errors) = reference::load_all(&reference::dir());
    for e in &errors {
        println!("reference fixture INVALID: {e}");
    }
    let seeds: u32 = args.num("--seeds", 3);
    let mut ok = errors.is_empty();
    let mut checked = 0;
    for sc in scenarios() {
        let Some(r) = refs.iter().find(|r| r.id == sc.fixture_id) else {
            println!("SKIP  {:<22} no fixture {} yet", sc.name, sc.fixture_id);
            continue;
        };
        checked += 1;
        let mut fails = Vec::new();
        for seed in 1..=seeds {
            let m = (sc.metrics)(&port_trace(&sc, &world, seed));
            fails.extend(
                r.compare(&m)
                    .into_iter()
                    .map(|f| format!("seed {seed}: {f}")),
            );
        }
        println!(
            "{} {:<22} {} ({seeds} island seeds)",
            if fails.is_empty() { "PASS " } else { "FAIL " },
            sc.name,
            r.label()
        );
        for f in &fails {
            println!("     ✗ {f}");
        }
        ok &= fails.is_empty();
    }
    if checked == 0 {
        println!("no original-game fixtures yet: regress orig run NAME, then regress orig fixture NAME --publish");
    }
    ok
}

// ---- Time of day on the real island (local) ------------------------------

/// What one game showed at one sample time.
#[derive(Default)]
struct Seen {
    /// Clock when sampled (hours since day 0 00:00).
    at: f64,
    ambient: f64,
    sky: f64,
    stars: f64,
    /// Presets on the blender's stack, `name:weight|...`.
    lights: String,
    image: Option<regress::tod::ImageStats>,
}

/// The preset on top of a `name:weight|...` stack, the one blending in or
/// holding ("DefaultState" sits at the bottom and is skipped, and so are
/// presets at weight 0).
fn top_light(stack: &str) -> (&str, f32) {
    stack
        .split('|')
        .filter_map(|e| e.rsplit_once(':'))
        .map(|(n, w)| (n, w.parse().unwrap_or(0.0)))
        // Presets parked at weight 0 (the original keeps "Forest" there).
        .rfind(|&(n, w)| n != "DefaultState" && w > 0.0)
        .unwrap_or(("?", 0.0))
}

/// Picks random times of day from a seed, measures the port's lighting at
/// each on the real island, optionally takes screenshots of the port
/// (`--shots`) and runs the original under the harness for the same times
/// (`--orig`), then reports the lighting schedule against the documented
/// one and how the two games differ at each time.
fn tod(args: &Args) -> bool {
    use regress::orig::repo_root;
    use regress::tod::{pick_times, summer_weathers, weather_flag};

    let Some(assets) = args.value("--assets") else {
        println!("tod needs --assets DIR (your converted assets/)");
        return false;
    };
    let assets = PathBuf::from(assets);
    let (world, _) = match regress::load_world(&assets) {
        Ok(w) => w,
        Err(e) => {
            println!("{e}");
            return false;
        }
    };
    // A random seed unless given; printed so a run can be repeated.
    let seed: u32 = args
        .value("--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| (d.subsec_nanos() ^ d.as_secs() as u32) % 100_000 + 1)
        });
    let n: usize = args.num("--samples", 8);
    let days: u32 = args.num("--days", 2).max(2);
    let island: usize = args.num("--island", 0);
    // --weather NAME[,NAME...] or all: one run per weather, in turn.
    let weathers: Vec<Option<(&'static str, u16)>> = match args.value("--weather") {
        None => vec![None],
        Some("all") => summer_weathers(&world).into_iter().map(Some).collect(),
        Some(list) => {
            let mut v = Vec::new();
            for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match (
                    weather_flag(name),
                    regress::tod::WEATHERS
                        .iter()
                        .find(|w| w.0.eq_ignore_ascii_case(name)),
                ) {
                    (Some(flag), Some(w)) => v.push(Some((w.0, flag))),
                    _ => {
                        let known: Vec<&str> =
                            summer_weathers(&world).iter().map(|w| w.0).collect();
                        println!(
                            "unknown weather {name:?}; this island has: {}",
                            known.join(", ")
                        );
                        return false;
                    }
                }
            }
            v
        }
    };
    if weathers.is_empty() {
        println!("no weather has a full set of summer presets in this world.bin");
        return false;
    }
    let out = args.value("--out").map_or_else(
        || repo_root().join(format!("target/tod/seed-{seed}")),
        PathBuf::from,
    );
    let times = pick_times(seed, n, days);
    println!(
        "time of day: seed {seed} (repeat with --seed {seed}), {} samples on days 1..={days}, island {island}",
        times.len()
    );
    if weathers.len() > 1 {
        let names: Vec<&str> = weathers.iter().flatten().map(|w| w.0).collect();
        println!("weathers, one run each: {}", names.join(", "));
    }

    let mut ok = true;
    let mut summaries = Vec::new();
    for weather in weathers {
        let dir = weather.map_or_else(|| out.clone(), |w| out.join(w.0));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            println!("{}: {e}", dir.display());
            return false;
        }
        let dir = dir.canonicalize().unwrap_or(dir);
        if let Some((name, _)) = weather {
            println!("\n===== weather {name} =====");
        }
        let run = TodRun {
            world: &world,
            assets: &assets,
            seed,
            island,
            days,
            times: &times,
            weather,
            out: &dir,
        };
        let (run_ok, summary) = tod_once(args, &run);
        ok &= run_ok;
        summaries.push(summary);
    }
    if summaries.len() > 1 {
        tod_summary(&summaries, &out);
    }
    ok
}

/// One `regress tod` pass: everything but the choice of weathers.
struct TodRun<'a> {
    world: &'a game::data::WorldData,
    assets: &'a std::path::Path,
    seed: u32,
    island: usize,
    days: u32,
    times: &'a [regress::tod::SampleTime],
    /// Forced on both games: (name, `BlenderState.Weather` flag).
    weather: Option<(&'static str, u16)>,
    out: &'a std::path::Path,
}

/// What a pass found, for the summary across weathers.
struct TodSummary {
    weather: String,
    ok: bool,
    /// Mean |port − original| of the blended ambient luma over the samples.
    ambient_diff: f64,
    /// Samples whose screenshots differ, and how.
    flagged: Vec<String>,
    /// Samples where a game didn't show the forced weather.
    wrong_weather: usize,
    samples: usize,
}

fn tod_summary(all: &[TodSummary], out: &std::path::Path) {
    println!("\n===== summary over {} weathers =====", all.len());
    println!(
        "  {:<20} {:>6} {:>14} {:>10} {:>14}",
        "weather", "result", "ambient |Δ|", "flagged", "wrong weather"
    );
    let mut toml = String::from("# regress tod: one entry per weather. Numbers only.\n");
    for s in all {
        println!(
            "  {:<20} {:>6} {:>14} {:>10} {:>14}",
            s.weather,
            if s.ok { "ok" } else { "FAIL" },
            if s.ambient_diff.is_finite() {
                format!("{:.3}", s.ambient_diff)
            } else {
                "-".into()
            },
            format!("{}/{}", s.flagged.len(), s.samples),
            s.wrong_weather
        );
        let _ = writeln!(
            toml,
            "\n[[weather]]\nname = \"{}\"\nok = {}\nambient_diff = {}\nwrong_weather = {}\nflagged = {:?}",
            s.weather,
            s.ok,
            toml_num(s.ambient_diff),
            s.wrong_weather,
            s.flagged
        );
    }
    for s in all.iter().filter(|s| !s.flagged.is_empty()) {
        println!("  {}:", s.weather);
        for f in &s.flagged {
            println!("     {f}");
        }
    }
    let _ = std::fs::write(out.join("summary.toml"), toml);
    println!("\nsummary in {}", out.join("summary.toml").display());
}

fn tod_once(args: &Args, run: &TodRun) -> (bool, TodSummary) {
    use regress::orig::{config_with_shots, find, port_light_stack, repo_root, Trace};
    use regress::tod::{
        daytime_lengths, desktop_top_screen, differences, light_metrics, luma, Image, ImageStats,
        LightSample,
    };
    let TodRun {
        world,
        assets,
        seed,
        island,
        days,
        times,
        weather,
        out,
    } = *run;
    let world = world.clone();

    // The port, headless: the island the desktop would start with this
    // seed, the monarch at the campfire, greed unable to take the crown.
    let mut g = game::state::GameState::start(
        &world,
        island,
        seed,
        &game::progress::Progress::default(),
        None,
    );
    g.monarch.x = g.campfire;
    g.monarch.velocity = 0.0;
    g.god = true;
    if let Some((_, flag)) = weather {
        g.director.force_weather(&world, flag);
    }
    let mut sim = Sim::new(world.clone(), g);
    sim.light_every = Some(6);
    let end = times
        .last()
        .map_or(0.0, |t| t.hours())
        .max(days as f64 * 24.0 + 24.0 + 6.0);
    let mut port: Vec<Seen> = Vec::new();
    let mut next = 0;
    while next < times.len() || sim.lights.last().is_none_or(|l| l.hours() < end) {
        sim.frame(Default::default(), 1);
        let now = LightSample::of(sim.tick, &sim.game);
        while next < times.len() && now.hours() >= times[next].hours() {
            port.push(Seen {
                at: now.hours(),
                ambient: luma(now.ambient),
                sky: 0.5 * (luma(now.sky_top) + luma(now.sky_bottom)),
                stars: now.stars as f64,
                lights: port_light_stack(&sim.game, &sim.world),
                image: None,
            });
            next += 1;
        }
    }
    let series: Vec<(f64, f64)> = sim
        .lights
        .iter()
        .map(|l| (l.hours(), luma(l.ambient)))
        .collect();
    let mut port_m = light_metrics(&series);
    let edges: Vec<(f64, bool)> = sim
        .lights
        .windows(2)
        .filter(|w| w[0].daytime != w[1].daytime)
        .map(|w| (w[1].hours(), w[1].daytime))
        .collect();
    let (day_s, night_s) = daytime_lengths(&edges);
    port_m.insert("daytime_s".into(), day_s);
    port_m.insert("night_s".into(), night_s);

    let mut ok = true;

    // Screenshots of the port, from the desktop build.
    let shot_name = |who: &str, i: usize| out.join(format!("{who}-{i:02}.png"));
    if args.flag("--shots") {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|e| Some(e.parent()?.join("desktop")))
            .filter(|p| p.exists());
        match exe {
            None => {
                println!("--shots needs the desktop build next to regress: cargo build --release -p desktop");
                ok = false;
            }
            Some(exe) => {
                let list: String = times
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        format!("{} {} {}\n", t.day, t.hour, shot_name("port", i).display())
                    })
                    .collect();
                let file = out.join("port-shots.txt");
                let _ = std::fs::write(&file, list);
                println!("port: taking {} screenshots ...", times.len());
                let status = std::process::Command::new(exe)
                    .env("KINGDOM_ASSETS", assets)
                    .args(["--shots", &file.to_string_lossy()])
                    .args(["--new", "--idle", "--camp", "--god", "--speed", "6"])
                    .args(["--seed", &seed.to_string(), "--island", &island.to_string()])
                    .args(weather.map_or(vec![], |w| vec!["--force-weather", w.0]))
                    .stdout(std::process::Stdio::null())
                    .status();
                if !matches!(status, Ok(s) if s.success()) {
                    println!("  the desktop exited with {status:?}");
                    ok = false;
                }
                for (i, p) in port.iter_mut().enumerate() {
                    match Image::load(&shot_name("port", i)) {
                        Ok(img) => {
                            let top = img.crop(desktop_top_screen(img.w, img.h));
                            p.image = Some(ImageStats::of(&top));
                            let _ = top.save(&shot_name("port", i));
                        }
                        Err(e) => {
                            println!("  {e}");
                            ok = false;
                        }
                    }
                }
            }
        }
    }

    // The original, under the harness, with screenshots at the same times.
    let mut orig: Vec<Seen> = Vec::new();
    let mut orig_m = None;
    if args.flag("--orig") {
        let mut sc = find("orig_time_of_day").expect("orig_time_of_day exists");
        // The original's clock starts at 04:00 on day 0 on frame 0.
        sc.frames = ((end - 4.0) * 600.0) as u32 + 600;
        let trace = out.join("orig-trace.txt");
        let _ = std::fs::remove_file(&trace);
        let shots: Vec<(u32, f32, PathBuf)> = times
            .iter()
            .enumerate()
            .map(|(i, t)| (t.day, t.hour, shot_name("orig", i)))
            .collect();
        let cfg = match config_with_shots(&sc, seed, &trace, &shots) {
            // Top-level keys go before the first [section].
            Ok(c) => weather.map_or(c.clone(), |w| format!("force_weather = {}\n{c}", w.1)),
            Err(e) => {
                println!("{e}");
                return (false, summary_fail(weather));
            }
        };
        let cfg_path = out.join("orig-config.cfg");
        let _ = std::fs::write(&cfg_path, cfg);
        println!(
            "original: running {} frames under the harness (a game window opens) ...",
            sc.frames
        );
        let timeout: u32 = args.num("--timeout", 1800);
        let status = std::process::Command::new(repo_root().join("tools/orig-harness/run.sh"))
            .arg(&cfg_path)
            .arg(timeout.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            println!(
                "  the game exited with {status:?} (logs in {})",
                out.display()
            );
        }
        match std::fs::read_to_string(&trace)
            .map_err(|e| e.to_string())
            .and_then(|t| Trace::parse(&t))
        {
            Ok(t) => {
                for p in t.problems() {
                    println!("  ✗ {p}");
                    ok = false;
                }
                let at = |h: f64| {
                    t.samples.iter().find(|(_, kv)| {
                        let day: f64 = kv.get("day").and_then(|v| v.parse().ok()).unwrap_or(-1.0);
                        let time: f64 = kv.get("time").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                        day * 24.0 + time >= h
                    })
                };
                let col = |kv: &std::collections::BTreeMap<String, String>, k: &str| {
                    kv.get(k)
                        .and_then(|v| regress::tod::parse_color(v))
                        .map_or(f64::NAN, luma)
                };
                for (i, time) in times.iter().enumerate() {
                    let mut seen = Seen {
                        at: f64::NAN,
                        ambient: f64::NAN,
                        sky: f64::NAN,
                        stars: f64::NAN,
                        ..Default::default()
                    };
                    if let Some((_, kv)) = at(time.hours()) {
                        let day: f64 = kv.get("day").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                        let tm: f64 = kv.get("time").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                        seen.at = day * 24.0 + tm;
                        seen.ambient = col(kv, "ambient");
                        seen.sky = 0.5 * (col(kv, "sky_top") + col(kv, "sky_bottom"));
                        seen.stars = kv
                            .get("stars")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(f64::NAN);
                        seen.lights = kv.get("lights").cloned().unwrap_or_default();
                    }
                    // Unity writes the file at the end of the frame; the
                    // game has quit since, so it's there if it was taken.
                    if let Ok(img) = Image::load(&shot_name("orig", i)) {
                        seen.image = Some(ImageStats::of(&img));
                    }
                    orig.push(seen);
                }
                orig_m = Some((sc.metrics)(&t));
            }
            Err(e) => {
                println!("  ✗ no usable trace of the original: {e}");
                ok = false;
            }
        }
    }

    // Side-by-side pictures: port | original, at the original's height.
    let pairs = port.iter().zip(&orig).count();
    for i in 0..pairs {
        if let (Ok(p), Ok(o)) = (
            Image::load(&shot_name("port", i)),
            Image::load(&shot_name("orig", i)),
        ) {
            let o = o.crop((0, 0, o.w, o.h));
            let both = Image::side_by_side(&[&p.to_height(o.h), &o]);
            let _ = both.save(&out.join(format!("compare-{i:02}.png")));
        }
    }

    // ---- Report ----
    let mut report = String::new();
    let _ = writeln!(
        report,
        "# regress tod: seed {seed}, island {island}. Numbers only."
    );
    let _ = writeln!(report, "seed = {seed}\nisland = {island}\n");
    println!("\nschedule (day 1; the lit day is when ambient is above halfway):");
    let (refs, _) = reference::load_all(&reference::dir());
    let fixture = refs.iter().find(|r| r.id == "time-of-day-lighting");
    println!(
        "  {:<26} {:>10} {:>10} {:>10}",
        "metric", "port", "original", "documented"
    );
    let _ = writeln!(report, "[metrics]");
    for (k, v) in &port_m {
        let o = orig_m.as_ref().and_then(|m| m.get(k)).copied();
        let doc = fixture
            .and_then(|r| r.expected.iter().find(|e| &e.metric == k))
            .map(|e| e.value);
        let f = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.3}"));
        println!("  {k:<26} {:>10} {:>10} {:>10}", f(Some(*v)), f(o), f(doc));
        let _ = writeln!(
            report,
            "{k} = {{ port = {}, original = {} }}",
            toml_num(*v),
            o.map_or("nan".into(), toml_num)
        );
    }
    if let Some(r) = fixture {
        let fails = r.compare(&port_m);
        for f in &fails {
            println!("  ✗ port: {f}");
        }
        if let Some(m) = &orig_m {
            for f in r.compare(m) {
                println!("  ✗ original: {f} (the documented values may be wrong)");
            }
        }
        ok &= fails.is_empty();
    }
    // The schedule must match the original's to the sample (0.01 h).
    if let Some(m) = &orig_m {
        for k in [
            "dawn_light_start_h",
            "day_light_start_h",
            "evening_light_start_h",
            "night_light_start_h",
        ] {
            let (a, b) = (
                port_m.get(k).copied().unwrap_or(f64::NAN),
                m.get(k).copied().unwrap_or(f64::NAN),
            );
            if (a - b).is_nan() || (a - b).abs() > 0.03 {
                println!("  ✗ {k}: port {a:.3} vs original {b:.3}");
                ok = false;
            }
        }
    }

    println!(
        "\nsamples (ambient / sky are blended light luma; image numbers are screenshot luma):"
    );
    let mut flagged = Vec::new();
    let mut wrong_weather = 0;
    let mut diffs = Vec::new();
    // With a forced weather, both games must show its presets.
    let shows_weather =
        |s: &Seen| weather.is_none_or(|w| top_light(&s.lights).0.starts_with(&format!("{}_", w.0)));
    for (i, t) in times.iter().enumerate() {
        let p = &port[i];
        let o = orig.get(i);
        println!("  {i:02} {} ({})", t.label(), t.window);
        let line = |who: &str, s: &Seen| {
            let img = s.image.map_or(String::new(), |m| {
                format!(
                    "  image {:.2} (sky {:.2} mid {:.2} low {:.2}) warmth {:+.2} contrast {:.2}",
                    m.luma, m.sky, m.middle, m.bottom, m.warmth, m.contrast
                )
            });
            let (name, w) = top_light(&s.lights);
            println!(
                "     {who:<5} ambient {:.3} sky {:.3} stars {:.2}  {name} {w:.2}{img}",
                s.ambient, s.sky, s.stars,
            );
        };
        line("port", p);
        if !shows_weather(p) {
            println!("     ✗ the port isn't showing the forced weather");
            wrong_weather += 1;
        }
        let _ = writeln!(
            report,
            "\n[[sample]]\nindex = {i}\nlabel = \"{}\"\nwindow = \"{}\"\nhours = {}",
            t.label(),
            t.window,
            toml_num(t.hours())
        );
        let entry = |who: &str, s: &Seen, report: &mut String| {
            let _ = writeln!(
                report,
                "{who} = {{ at = {}, ambient = {}, sky = {}, stars = {}, light = \"{}\"{} }}",
                toml_num(s.at),
                toml_num(s.ambient),
                toml_num(s.sky),
                toml_num(s.stars),
                top_light(&s.lights).0,
                s.image.map_or(String::new(), |m| format!(
                    ", image = {}, image_sky = {}, image_middle = {}, image_bottom = {}, warmth = {}, contrast = {}",
                    toml_num(m.luma),
                    toml_num(m.sky),
                    toml_num(m.middle),
                    toml_num(m.bottom),
                    toml_num(m.warmth),
                    toml_num(m.contrast)
                ))
            );
        };
        entry("port", p, &mut report);
        if let Some(o) = o {
            line("orig", o);
            entry("original", o, &mut report);
            if !shows_weather(o) {
                println!("     ✗ the original isn't showing the forced weather");
                wrong_weather += 1;
            }
            if (p.ambient - o.ambient).is_finite() {
                diffs.push((p.ambient - o.ambient).abs());
            }
            let same = top_light(&p.lights).0 == top_light(&o.lights).0;
            if same && (p.ambient - o.ambient).abs() > 0.03 {
                println!(
                    "     ! same preset, but the blended ambient differs by {:+.3}",
                    p.ambient - o.ambient
                );
            } else if !same {
                println!("     · different presets (without --weather the games roll weather differently)");
            }
            if let (Some(a), Some(b)) = (p.image, o.image) {
                let d = differences(&a, &b);
                if !d.is_empty() {
                    println!("     ! the port looks {}", d.join(", "));
                    flagged.push(format!(
                        "{i:02} {} ({}): {}",
                        t.label(),
                        t.window,
                        d.join(", ")
                    ));
                }
                let _ = writeln!(report, "differences = {:?}", d);
            }
        }
    }
    let _ = std::fs::write(out.join("report.toml"), report);
    println!(
        "\nreport, screenshots and side-by-sides in {}",
        out.display()
    );
    ok &= wrong_weather == 0;
    let summary = TodSummary {
        weather: weather.map_or("random".into(), |w| w.0.into()),
        ok,
        ambient_diff: if diffs.is_empty() {
            f64::NAN
        } else {
            diffs.iter().sum::<f64>() / diffs.len() as f64
        },
        flagged,
        wrong_weather,
        samples: times.len(),
    };
    (ok, summary)
}

fn summary_fail(weather: Option<(&'static str, u16)>) -> TodSummary {
    TodSummary {
        weather: weather.map_or("random".into(), |w| w.0.into()),
        ok: false,
        ambient_diff: f64::NAN,
        flagged: Vec::new(),
        wrong_weather: 0,
        samples: 0,
    }
}

fn toml_num(v: f64) -> String {
    if v.is_finite() {
        format!("{:.4}", v)
    } else {
        "nan".into()
    }
}
