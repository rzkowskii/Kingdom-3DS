//! Unity clips/controllers → typed curves, sprite keys and whitelisted markers.
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::yaml::{field, parse_float};
use game::animation::{
    self, AnimatorState, AnimatorTransition, Condition, Controller, Event, EventKind, Key, Library,
    Tangent, TransformTrack,
};
use serde::Serialize;
use serde_yaml::Value as Y;

pub const FORMAT_VERSION: u16 = 1;

#[derive(Clone, Debug, Serialize, Default)]
pub struct Clip {
    pub source_file: String,
    pub original_name: String,
    pub length: f32,
    pub looping: bool,
    /// None is an explicit SpriteRenderer null, not an unresolved reference.
    pub sprites: BTreeMap<String, Vec<(f32, Option<String>)>>,
    /// Compatibility for existing offset consumers; precise sampling uses animation.
    pub offsets: BTreeMap<String, Vec<(f32, f32, f32, bool)>>,
    pub animation: animation::Clip,
    pub unsupported: Vec<String>,
}

#[derive(Serialize)]
pub struct ClipFile<'a> {
    pub version: u16,
    pub clips: &'a BTreeMap<String, Clip>,
}

#[derive(Default, Serialize)]
pub struct Report {
    pub clips: usize,
    pub sprite_keys: usize,
    pub null_sprite_keys: usize,
    pub transform_tracks: usize,
    pub scalar_keys: usize,
    pub events: usize,
    pub controller_states: usize,
    pub controller_transitions: usize,
    pub duplicate_name_aliases: Vec<(String, String)>,
    pub unsupported: Vec<String>,
}

fn number(v: &Y) -> Option<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .or_else(|| v.as_str().and_then(parse_float))
}
fn f(v: &Y) -> f32 {
    number(v).unwrap_or(0.0)
}
fn s(v: &Y) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
fn seq(v: &Y) -> &[Y] {
    v.as_sequence().map(Vec::as_slice).unwrap_or_default()
}
fn boolean(v: &Y) -> bool {
    v.as_bool().unwrap_or_else(|| v.as_i64() == Some(1))
}
fn tangent(v: &Y) -> Tangent {
    match number(v) {
        Some(x) if x.is_infinite() => Tangent::Step,
        Some(x) => Tangent::Slope(x),
        None => Tangent::Slope(0.0),
    }
}
fn clean_doc(text: &str) -> String {
    text.lines()
        .filter(|l| !l.starts_with('%') && !l.starts_with("---"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn curve_keys(curve: &Y, axis: Option<&str>) -> Vec<Key> {
    seq(&curve["m_Curve"])
        .iter()
        .map(|k| {
            let component = |name: &str| axis.map_or(&k[name], |axis| &k[name][axis]);
            Key {
                time: f(&k["time"]),
                value: f(component("value")),
                in_tangent: tangent(component("inSlope")),
                out_tangent: tangent(component("outSlope")),
            }
        })
        .collect()
}

fn curve_diagnostics(curve: &Y, path: &str, unsupported: &mut Vec<String>) {
    for property in ["m_PreInfinity", "m_PostInfinity"] {
        if curve[property].as_i64().is_some_and(|v| v != 2) {
            unsupported.push(format!("curve wrap {property} at {path}"));
        }
    }
    if seq(&curve["m_Curve"])
        .iter()
        .any(|k| k["weightedMode"].as_i64().is_some_and(|v| v != 0))
    {
        unsupported.push(format!("weighted curve tangents at {path}"));
    }
}

fn parse(text: &str, sprites_by_guid: &HashMap<String, String>) -> Result<Clip, String> {
    let doc: Y = serde_yaml::from_str(&clean_doc(text)).map_err(|e| e.to_string())?;
    let data = &doc["AnimationClip"];
    let length = field(text, "m_StopTime")
        .and_then(parse_float)
        .unwrap_or(0.0);
    let looping = field(text, "m_LoopTime") == Some("1");
    let mut clip = Clip {
        original_name: s(&data["m_Name"]),
        length,
        looping,
        animation: animation::Clip {
            length,
            looping,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut tracks: BTreeMap<String, TransformTrack> = BTreeMap::new();
    for item in seq(&data["m_PPtrCurves"]) {
        let path = s(&item["path"]);
        if item["attribute"].as_str() != Some("m_Sprite") {
            clip.unsupported
                .push(format!("object curve {} at {path}", s(&item["attribute"])));
            continue;
        }
        let mut keys = Vec::new();
        for key in seq(&item["curve"]) {
            let reference = &key["value"];
            let sprite = if reference["fileID"].as_i64() == Some(0) {
                None
            } else {
                let guid = reference["guid"]
                    .as_str()
                    .ok_or("sprite key lacks asset GUID")?;
                Some(
                    sprites_by_guid
                        .get(guid)
                        .ok_or_else(|| format!("unresolved sprite GUID {guid} at {path}"))?
                        .clone(),
                )
            };
            keys.push((f(&key["time"]), sprite));
        }
        clip.sprites.insert(path, keys);
    }
    for section in [
        "m_PositionCurves",
        "m_ScaleCurves",
        "m_RotationCurves",
        "m_EulerCurves",
    ] {
        for item in seq(&data[section]) {
            let path = s(&item["path"]);
            let track = tracks
                .entry(path.clone())
                .or_insert_with(|| TransformTrack {
                    path,
                    ..Default::default()
                });
            let curve = &item["curve"];
            curve_diagnostics(curve, &track.path, &mut clip.unsupported);
            for (i, axis) in ["x", "y", "z", "w"].iter().enumerate() {
                if i == 3 && section != "m_RotationCurves" {
                    continue;
                }
                let keys = curve_keys(curve, Some(axis));
                match section {
                    "m_PositionCurves" => track.position[i] = keys,
                    "m_ScaleCurves" => track.scale[i] = keys,
                    "m_RotationCurves" => track.rotation[i] = keys,
                    "m_EulerCurves" if i == 2 => track.euler_z = keys,
                    "m_EulerCurves"
                        if keys.iter().any(|k| {
                            k.value != 0.0
                                || k.in_tangent != Tangent::Slope(0.0)
                                || k.out_tangent != Tangent::Slope(0.0)
                        }) =>
                    {
                        clip.unsupported
                            .push(format!("nonzero Euler {axis} at {}", track.path));
                    }
                    _ => {}
                }
            }
        }
    }
    for item in seq(&data["m_FloatCurves"]) {
        let path = s(&item["path"]);
        let attr = s(&item["attribute"]);
        let class = item["classID"].as_i64().unwrap_or_default();
        let channel = match (class, attr.as_str()) {
            (212, "m_Color.r") => Some(0),
            (212, "m_Color.g") => Some(1),
            (212, "m_Color.b") => Some(2),
            (212, "m_Color.a") => Some(3),
            _ => None,
        };
        if channel.is_none() && !(class == 212 && attr == "m_Enabled") {
            clip.unsupported
                .push(format!("float curve {attr} class {class} at {path}"));
            continue;
        }
        let track = tracks
            .entry(path.clone())
            .or_insert_with(|| TransformTrack {
                path,
                ..Default::default()
            });
        curve_diagnostics(&item["curve"], &track.path, &mut clip.unsupported);
        let keys = curve_keys(&item["curve"], None);
        if let Some(i) = channel {
            track.color[i] = keys;
        } else {
            track.visible = keys;
        }
    }
    if !seq(&data["m_CompressedRotationCurves"]).is_empty() {
        clip.unsupported.push("compressed rotation curves".into());
    }
    for item in seq(&data["m_Events"]) {
        let name = s(&item["functionName"]);
        let Some(kind) = EventKind::from_function(&name) else {
            clip.unsupported.push(format!("event {name}"));
            continue;
        };
        if item["objectReferenceParameter"]["fileID"]
            .as_i64()
            .unwrap_or_default()
            != 0
        {
            clip.unsupported
                .push(format!("object-reference event parameter {name}"));
            continue;
        }
        clip.animation.events.push(Event {
            time: f(&item["time"]),
            kind,
            float_parameter: f(&item["floatParameter"]),
            int_parameter: item["intParameter"].as_i64().unwrap_or_default() as i32,
            string_parameter: s(&item["data"]),
        });
    }
    clip.animation
        .events
        .sort_by(|a, b| a.time.total_cmp(&b.time));
    clip.animation.tracks = tracks.into_values().collect();
    for track in &clip.animation.tracks {
        if track.position[0].is_empty() && track.position[1].is_empty() {
            continue;
        }
        let mut times: Vec<f32> = track.position[..2]
            .iter()
            .flatten()
            .map(|k| k.time)
            .collect();
        times.sort_by(f32::total_cmp);
        times.dedup();
        let keys = times
            .into_iter()
            .map(|t| {
                let stepped = track.position[..2].iter().any(|k| {
                    k.iter()
                        .find(|k| k.time == t)
                        .is_some_and(|k| k.out_tangent == Tangent::Step)
                });
                (
                    t,
                    animation::sample(&track.position[0], t, 0.0),
                    animation::sample(&track.position[1], t, 0.0),
                    stepped,
                )
            })
            .collect();
        clip.offsets.insert(track.path.clone(), keys);
    }
    if !clip.animation.validate() {
        return Err("invalid/nonfinite animation keys or markers".into());
    }
    Ok(clip)
}

pub fn extract_all(
    dir: &Path,
    sprites_by_guid: &HashMap<String, String>,
) -> Result<BTreeMap<String, Clip>, String> {
    let mut out = BTreeMap::new();
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "anim"))
        .collect();
    paths.sort();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let original = field(&text, "m_Name").unwrap_or_default().to_owned();
        let name = if out.contains_key(&original) {
            p.file_stem().unwrap().to_string_lossy().into_owned()
        } else {
            original
        };
        let mut clip =
            parse(&text, sprites_by_guid).map_err(|e| format!("{}: {e}", p.display()))?;
        clip.source_file = p.file_name().unwrap().to_string_lossy().into_owned();
        if out.insert(name.clone(), clip).is_some() {
            return Err(format!("ambiguous duplicate clip key {name}"));
        }
    }
    Ok(out)
}

fn docs(text: &str) -> Result<Vec<(i64, Y)>, String> {
    let mut out = Vec::new();
    for part in text.split("--- !u!").skip(1) {
        let (head, body) = part.split_once('\n').ok_or("invalid Unity document")?;
        let id = head
            .split('&')
            .nth(1)
            .and_then(|s| s.trim().parse().ok())
            .ok_or("invalid Unity object ID")?;
        let doc: Y = serde_yaml::from_str(body).map_err(|e| e.to_string())?;
        out.push((id, doc));
    }
    Ok(out)
}

pub fn library(
    assets: &Path,
    guids: &HashMap<String, PathBuf>,
    clips: &BTreeMap<String, Clip>,
) -> Result<(Library, Report), String> {
    let mut library = Library {
        clips: clips
            .iter()
            .map(|(n, c)| (n.clone(), c.animation.clone()))
            .collect(),
        ..Default::default()
    };
    let mut report = Report {
        clips: clips.len(),
        ..Default::default()
    };
    for (name, clip) in clips {
        if name != &clip.original_name {
            report
                .duplicate_name_aliases
                .push((clip.source_file.clone(), name.clone()));
        }
        report.sprite_keys += clip.sprites.values().map(Vec::len).sum::<usize>();
        report.null_sprite_keys += clip
            .sprites
            .values()
            .flatten()
            .filter(|k| k.1.is_none())
            .count();
        report.transform_tracks += clip.animation.tracks.len();
        report.scalar_keys += clip
            .animation
            .tracks
            .iter()
            .map(|t| {
                t.position
                    .iter()
                    .chain(&t.rotation)
                    .chain(&t.scale)
                    .chain(&t.color)
                    .chain(core::iter::once(&t.euler_z))
                    .chain(core::iter::once(&t.visible))
                    .map(Vec::len)
                    .sum::<usize>()
            })
            .sum::<usize>();
        report.events += clip.animation.events.len();
        report
            .unsupported
            .extend(clip.unsupported.iter().map(|s| format!("{name}: {s}")));
    }
    let clip_name = |v: &Y| -> Option<String> {
        let path = guids.get(v["guid"].as_str()?)?;
        let file = path.file_name()?.to_str()?;
        if let Some((name, _)) = clips.iter().find(|(_, c)| c.source_file == file) {
            return Some(name.clone());
        }
        let text = std::fs::read_to_string(path).ok()?;
        Some(field(&text, "m_Name")?.to_owned())
    };
    let mut paths: Vec<_> = std::fs::read_dir(assets.join("AnimatorController"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "controller"))
        .collect();
    paths.sort();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let objects = docs(&text)?;
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let states: HashMap<i64, String> = objects
            .iter()
            .filter(|(_, v)| !v["AnimatorState"].is_null())
            .map(|(id, v)| (*id, s(&v["AnimatorState"]["m_Name"])))
            .collect();
        let default = objects
            .iter()
            .find_map(|(_, v)| v["AnimatorStateMachine"]["m_DefaultState"]["fileID"].as_i64())
            .and_then(|id| states.get(&id).cloned());
        library.controllers.push(Controller {
            name: name.clone(),
            default_state: default,
        });
        let mut transition_sources: HashMap<i64, String> = HashMap::new();
        for (_, v) in &objects {
            let state = &v["AnimatorState"];
            if state.is_null() {
                continue;
            }
            let state_name = s(&state["m_Name"]);
            for t in seq(&state["m_Transitions"]) {
                if let Some(id) = t["fileID"].as_i64() {
                    transition_sources.insert(id, state_name.clone());
                }
            }
            let clip = clip_name(&state["m_Motion"]);
            if state["m_Motion"]["fileID"]
                .as_i64()
                .is_some_and(|id| id != 0)
                && clip.is_none()
            {
                report
                    .unsupported
                    .push(format!("{name}/{state_name}: unresolved/BlendTree motion"));
            }
            library.states.push(AnimatorState {
                controller: name.clone(),
                name: state_name,
                clip,
                speed: number(&state["m_Speed"]).unwrap_or(1.0),
                speed_parameter: boolean(&state["m_SpeedParameterActive"])
                    .then(|| s(&state["m_SpeedParameter"])),
                cycle_offset: f(&state["m_CycleOffset"]),
            });
        }
        for (id, v) in &objects {
            let t = &v["AnimatorStateTransition"];
            if t.is_null() {
                continue;
            }
            library.transitions.push(AnimatorTransition {
                controller: name.clone(),
                from: transition_sources
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| "*".into()),
                to: t["m_DstState"]["fileID"]
                    .as_i64()
                    .and_then(|id| states.get(&id).cloned()),
                conditions: seq(&t["m_Conditions"])
                    .iter()
                    .map(|c| Condition {
                        parameter: s(&c["m_ConditionEvent"]),
                        mode: c["m_ConditionMode"].as_i64().unwrap_or_default() as i32,
                        threshold: f(&c["m_EventTreshold"]),
                    })
                    .collect(),
                exit_time: boolean(&t["m_HasExitTime"]).then(|| f(&t["m_ExitTime"])),
                duration: f(&t["m_TransitionDuration"]),
                fixed_duration: boolean(&t["m_HasFixedDuration"]),
                offset: f(&t["m_TransitionOffset"]),
            });
        }
    }
    let mut paths: Vec<_> = std::fs::read_dir(assets.join("AnimatorOverrideController"))
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "overrideController"))
        .collect();
    paths.sort();
    for p in paths {
        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        let parsed = docs(&text)?;
        let data =
            &parsed.first().ok_or("empty override controller")?.1["AnimatorOverrideController"];
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let base = guids
            .get(data["m_Controller"]["guid"].as_str().unwrap_or_default())
            .and_then(|p| p.file_stem())
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{name}: unresolved base controller"))?;
        let replacements: HashMap<_, _> = seq(&data["m_Clips"])
            .iter()
            .filter_map(|v| {
                Some((
                    clip_name(&v["m_OriginalClip"])?,
                    clip_name(&v["m_OverrideClip"])?,
                ))
            })
            .collect();
        let Some(controller) = library.controllers.iter().find(|c| c.name == base).cloned() else {
            return Err(format!("{name}: missing base controller {base}"));
        };
        library.controllers.push(Controller {
            name: name.clone(),
            ..controller
        });
        let states: Vec<_> = library
            .states
            .iter()
            .filter(|s| s.controller == base)
            .map(|s| {
                let mut s = s.clone();
                s.controller = name.clone();
                if let Some(replacement) = s.clip.as_ref().and_then(|c| replacements.get(c)) {
                    s.clip = Some(replacement.clone());
                }
                s
            })
            .collect();
        library.states.extend(states);
        let transitions: Vec<_> = library
            .transitions
            .iter()
            .filter(|t| t.controller == base)
            .map(|t| {
                let mut t = t.clone();
                t.controller = name.clone();
                t
            })
            .collect();
        library.transitions.extend(transitions);
    }
    report.controller_states = library.states.len();
    report.controller_transitions = library.transitions.len();
    Ok((library, report))
}

/// Required gameplay timelines must not silently degrade to hardcoded guesses.
pub fn validate_gameplay(library: &Library) -> Result<(), String> {
    for (clip, events) in [
        ("player_bear_gallop", &[EventKind::TriggerAbility][..]),
        ("worker_hammer", &[EventKind::HammerHit][..]),
        (
            "catapult_fire",
            &[
                EventKind::ProjectileRelease,
                EventKind::ArmImpact,
                EventKind::FiringDone,
            ][..],
        ),
        ("squid_attack", &[EventKind::GrabComplete][..]),
    ] {
        let c = library
            .clip(clip)
            .ok_or_else(|| format!("required animation clip {clip} is missing"))?;
        for kind in events {
            if c.first_event_time(*kind).is_none() {
                return Err(format!("required marker {kind:?} in {clip} is missing"));
            }
        }
    }
    for (controller, state, clip) in [
        ("player_bear", "Run", "player_bear_gallop"),
        ("worker", "Hammer", "worker_hammer"),
        ("catapult", "Fire", "catapult_fire"),
        ("squid", "Attack", "squid_attack"),
        ("boss", "Throw", "boss_throw"),
        ("portal", "Close", "portal_close"),
        ("portal", "Open", "portal_open"),
        ("portal", "Idle", "portal_idle"),
        ("portal", "Destroy", "portal_destroy"),
    ] {
        let s = library
            .state(controller, state)
            .ok_or_else(|| format!("required animator state {controller}/{state} is missing"))?;
        if s.clip.as_deref() != Some(clip)
            || s.speed_parameter.is_some()
            || !s.speed.is_finite()
            || s.speed <= 0.0
            || s.cycle_offset != 0.0
        {
            return Err(format!(
                "unsupported gameplay animator timing {controller}/{state}"
            ));
        }
    }
    validate_portal_graph(library)?;
    validate_boss_throw(library)
}

fn validate_boss_throw(library: &Library) -> Result<(), String> {
    let clip = library
        .state_clip("boss", "Throw")
        .ok_or("missing boss/Throw clip")?;
    let mut releases = clip
        .events
        .iter()
        .filter(|e| e.kind == EventKind::BoulderRelease);
    if !clip.validate()
        || clip.looping
        || releases
            .next()
            .is_none_or(|event| event.time <= 0.0 || event.time >= clip.length)
        || releases.next().is_some()
        || !clip.tracks.iter().any(|track| {
            track.path == "Hand" && track.position.iter().take(2).all(|keys| !keys.is_empty())
        })
    {
        return Err("unsupported boss/Throw release or Hand track".into());
    }
    let mut entry = library
        .transitions
        .iter()
        .filter(|t| t.controller == "boss" && t.to.as_deref() == Some("Throw"));
    if entry.next().is_none_or(|t| {
        t.from != "*"
            || t.exit_time.is_some()
            || t.duration != 0.0
            || t.offset != 0.0
            || t.conditions.len() != 1
            || t.conditions[0].parameter != "Throw"
            || t.conditions[0].mode != 1
    }) || entry.next().is_some()
    {
        return Err("unsupported boss/Throw entry transition".into());
    }
    let mut exit = library
        .transitions
        .iter()
        .filter(|t| t.controller == "boss" && t.from == "Throw");
    if exit.next().is_none_or(|t| {
        t.to.as_deref() != Some("Stand")
            || t.exit_time != Some(1.0)
            || t.duration != 0.0
            || t.offset != 0.0
            || !t.conditions.is_empty()
    }) || exit.next().is_some()
    {
        return Err("unsupported boss/Throw exit transition".into());
    }
    Ok(())
}

/// The port implements this authored zero-blend graph, rather than silently
/// treating every future Unity controller as having the same transition rules.
fn validate_portal_graph(library: &Library) -> Result<(), String> {
    if library
        .default_state("portal")
        .is_none_or(|s| s.name != "Close")
    {
        return Err("unsupported portal default state".into());
    }
    for state in ["Close", "Open", "Idle", "Destroy"] {
        let clip = library
            .state_clip("portal", state)
            .ok_or_else(|| format!("missing portal/{state} clip"))?;
        if !clip.validate() || clip.length <= 0.0 || clip.looping != (state == "Idle") {
            return Err(format!("unsupported portal/{state} clip timing"));
        }
    }
    let transitions: Vec<_> = library
        .transitions
        .iter()
        .filter(|t| t.controller == "portal")
        .collect();
    let expected = [
        ("Close", "Open", Some("Open"), None),
        ("Open", "Idle", None, Some(1.0)),
        ("Destroy", "Open", Some("Open"), None),
        ("Idle", "Destroy", Some("Destroy"), None),
        ("Idle", "Close", Some("Close"), None),
    ];
    if transitions.len() != expected.len() {
        return Err("unsupported portal transition graph".into());
    }
    for (from, to, trigger, exit) in expected {
        let matches = transitions
            .iter()
            .filter(|t| t.from == from && t.to.as_deref() == Some(to))
            .filter(|t| t.duration == 0.0 && t.offset == 0.0 && t.exit_time == exit)
            .filter(|t| match trigger {
                None => t.conditions.is_empty(),
                Some(trigger) => {
                    t.conditions.len() == 1
                        && t.conditions[0].parameter == trigger
                        && t.conditions[0].mode == 1
                }
            })
            .count();
        if matches != 1 {
            return Err(format!("unsupported portal transition {from}->{to}"));
        }
    }
    let idle: Vec<_> = transitions.iter().filter(|t| t.from == "Idle").collect();
    if idle[0].to.as_deref() != Some("Destroy") {
        return Err("unsupported portal trigger priority".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::animation::Pose;
    use scene::sheet::Sheet;

    #[test]
    fn portal_graph_gate_rejects_missing_clips_blends_and_changed_trigger_rules() {
        let mut library = Library::default();
        library.controllers.push(Controller {
            name: "portal".into(),
            default_state: Some("Close".into()),
        });
        for state in ["Close", "Open", "Idle", "Destroy"] {
            library.clips.push((
                state.into(),
                animation::Clip {
                    length: 1.0,
                    looping: state == "Idle",
                    ..Default::default()
                },
            ));
            library.states.push(AnimatorState {
                controller: "portal".into(),
                name: state.into(),
                clip: Some(state.into()),
                speed: 1.0,
                ..Default::default()
            });
        }
        for (from, to, trigger, exit) in [
            ("Close", "Open", Some("Open"), None),
            ("Open", "Idle", None, Some(1.0)),
            ("Destroy", "Open", Some("Open"), None),
            ("Idle", "Destroy", Some("Destroy"), None),
            ("Idle", "Close", Some("Close"), None),
        ] {
            library.transitions.push(AnimatorTransition {
                controller: "portal".into(),
                from: from.into(),
                to: Some(to.into()),
                conditions: trigger
                    .into_iter()
                    .map(|p| Condition {
                        parameter: p.into(),
                        mode: 1,
                        threshold: 0.0,
                    })
                    .collect(),
                exit_time: exit,
                duration: 0.0,
                fixed_duration: true,
                offset: 0.0,
            });
        }
        assert!(validate_portal_graph(&library).is_ok());
        let mut bad = library.clone();
        bad.clips.remove(0);
        assert!(validate_portal_graph(&bad).is_err());
        let mut bad = library.clone();
        bad.transitions[0].duration = 0.25;
        assert!(validate_portal_graph(&bad).is_err());
        let mut bad = library.clone();
        bad.transitions[0].conditions[0].parameter = "Destroy".into();
        assert!(validate_portal_graph(&bad).is_err());
        let mut bad = library.clone();
        bad.transitions.swap(3, 4);
        assert!(validate_portal_graph(&bad).is_err());
        let mut bad = library;
        bad.transitions[1].offset = 0.5;
        assert!(validate_portal_graph(&bad).is_err());
    }

    const CLIP: &str = "AnimationClip:
  m_Name: test
  m_PositionCurves:
  - curve:
      m_Curve:
      - time: 0
        value: {x: 0, y: 3, z: 0}
        inSlope: {x: 0, y: 0, z: 0}
        outSlope: {x: 8, y: Infinity, z: 0}
      - time: 2
        value: {x: 4, y: 7, z: 0}
        inSlope: {x: 0, y: Infinity, z: 0}
        outSlope: {x: 0, y: 0, z: 0}
    path: Arm
  m_ScaleCurves:
  - curve:
      m_Curve:
      - time: 0
        value: {x: -1.3, y: 1.1, z: 1}
        inSlope: {x: 0, y: 0, z: 0}
        outSlope: {x: 0, y: 0, z: 0}
    path: Arm
  m_RotationCurves:
  - curve:
      m_Curve:
      - time: 0
        value: {x: 0, y: 0, z: 0.36361992, w: 0.9315474}
        inSlope: {x: 0, y: 0, z: 0, w: 0}
        outSlope: {x: 0, y: 0, z: 0, w: 0}
    path: Arm
  m_FloatCurves:
  - curve:
      m_Curve:
      - time: 0
        value: 0.353
        inSlope: 0
        outSlope: 9.705
      - time: 0.06666667
        value: 1
        inSlope: 4.852496
        outSlope: 4.8525
    attribute: m_Color.a
    classID: 212
    path: Arm
  - curve:
      m_Curve: []
    attribute: m_AnchoredPosition.x
    classID: 224
    path: Menu
  m_PPtrCurves:
  - curve:
    - time: 0
      value: {fileID: 21300000, guid: aaa, type: 2}
    - time: 0.75
      value: {fileID: 0}
    - time: 1
      value: {fileID: 21300000, guid: aaa, type: 2}
    attribute: m_Sprite
    path: Arm
  m_MuscleClip:
    m_StopTime: 2
    m_LoopTime: 1
  m_Events:
  - time: 0.15
    functionName: ReleaseProjectileFromAnimation
    data: safe-data
    objectReferenceParameter: {fileID: 0}
    floatParameter: 0.5
    intParameter: 2
  - time: 0.16666667
    functionName: ArmImpactFromAnimation
    objectReferenceParameter: {fileID: 0}
  - time: 1.1666666
    functionName: FiringDoneFromAnimation
    objectReferenceParameter: {fileID: 0}
  - time: 0.4
    functionName: ArbitraryMethod
";
    fn names() -> HashMap<String, String> {
        HashMap::from([("aaa".into(), "sprite".into())])
    }
    #[test]
    fn typed_tracks_events_and_null_frames_survive_json_and_packed_sheet() {
        let c = parse(CLIP, &names()).unwrap();
        let p = c.animation.pose("Arm", 1.0, Pose::default());
        assert_eq!(p.position, [4.0, 3.0, 0.0]);
        assert_eq!(p.scale, [-1.3, 1.1, 1.0]);
        assert!((p.rotation[2] - 0.36361992).abs() < 1e-6);
        let p = c.animation.pose("Arm", 1.0 / 30.0, Pose::default());
        assert!(
            (p.color[3] - 0.7169375).abs() < 1e-5,
            "independent Hermite value {}",
            p.color[3]
        );
        assert_eq!(c.animation.events.len(), 3);
        assert_eq!(c.animation.events[0].kind, EventKind::ProjectileRelease);
        assert_eq!(c.animation.events[0].string_parameter, "safe-data");
        assert_eq!(c.animation.events[0].int_parameter, 2);
        assert_eq!(c.unsupported.len(), 2);
        let clips = BTreeMap::from([("test".into(), c)]);
        let json = serde_json::to_string(&ClipFile {
            version: FORMAT_VERSION,
            clips: &clips,
        })
        .unwrap();
        assert!(json.contains("Step")); // no Infinity->null loss
        let sprites = r#"{"pages":["page.png"],"sprites":{"sprite":{"page":0,"x":0,"y":0,"w":2,"h":2,"px":1,"py":1}}}"#;
        let font = r#"{"size":8,"line":8,"glyphs":[]}"#;
        let sheet = Sheet::from_json(sprites, &json, font).unwrap();
        sheet.validate().unwrap();
        let bytes = game::assetfile::encode(game::assetfile::Kind::Sheet, &sheet).unwrap();
        let mut restored: Sheet =
            game::assetfile::decode(game::assetfile::Kind::Sheet, &bytes).unwrap();
        restored.link(&[]);
        let clip = restored.clip("test").unwrap();
        assert_eq!(clip.offset("Arm", 1.0), (4.0, 3.0));
        assert_eq!(clip.sprite_frame("Arm", 0.8), Some(None));
        assert_eq!(clip.sprite_frame("Arm", 1.1), Some(Some(0)));
        assert_eq!(clip.animation.events, clips["test"].animation.events);
    }
    #[test]
    fn euler_and_renderer_enabled_tracks_are_applied_and_version_is_required() {
        let text = CLIP.replace("  m_ScaleCurves:", "  m_EulerCurves:\n  - curve:\n      m_Curve:\n      - time: 0\n        value: {x: 0, y: 0, z: -180}\n        inSlope: {x: 0, y: 0, z: 0}\n        outSlope: {x: 0, y: 0, z: 0}\n    path: Arm\n  m_ScaleCurves:")
            .replace("  m_FloatCurves:", "  m_FloatCurves:\n  - curve:\n      m_Curve:\n      - time: 0\n        value: 0\n        inSlope: Infinity\n        outSlope: Infinity\n    attribute: m_Enabled\n    classID: 212\n    path: Arm");
        let clip = parse(&text, &names()).unwrap();
        let pose = clip.animation.pose("Arm", 0.0, Pose::default());
        assert!(!pose.visible);
        assert!((pose.rotation[2] + 1.0).abs() < 1e-6);
        assert!(Sheet::from_json(
            r#"{"pages":[],"sprites":{}}"#,
            "{}",
            r#"{"size":8,"line":8,"glyphs":[]}"#
        )
        .unwrap_err()
        .contains("regenerate assets"));
    }

    #[test]
    fn unknown_sprites_and_missing_required_markers_are_explicit_errors() {
        assert!(parse(CLIP, &HashMap::new())
            .unwrap_err()
            .contains("unresolved sprite"));
        assert!(validate_gameplay(&Library::default())
            .unwrap_err()
            .contains("player_bear_gallop"));
    }

    #[test]
    fn duplicate_embedded_names_keep_both_clip_assets() {
        let dir = std::env::temp_dir().join(format!(
            "kingdom-animation-duplicates-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.anim"), CLIP).unwrap();
        std::fs::write(
            dir.join("test_1.anim"),
            CLIP.replace("time: 0.15", "time: 0.25"),
        )
        .unwrap();
        let clips = extract_all(&dir, &names()).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(
            clips["test"]
                .animation
                .first_event_time(EventKind::ProjectileRelease),
            Some(0.15)
        );
        assert_eq!(
            clips["test_1"]
                .animation
                .first_event_time(EventKind::ProjectileRelease),
            Some(0.25)
        );
        assert_eq!(clips["test_1"].source_file, "test_1.anim");
        assert_eq!(clips["test_1"].original_name, "test");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Optional evidence gate against the user's locally exported PC assets.
    #[test]
    #[ignore = "requires KINGDOM_REFERENCE_ASSETS pointing at a local AssetRipper Assets directory"]
    fn local_reference_animation_inventory() {
        let assets = PathBuf::from(
            std::env::var_os("KINGDOM_REFERENCE_ASSETS").expect("KINGDOM_REFERENCE_ASSETS"),
        );
        let guids = crate::guid_map(&assets);
        let names = guids
            .iter()
            .filter(|(_, path)| path.starts_with(assets.join("Sprite")))
            .filter_map(|(guid, path)| {
                let text = std::fs::read_to_string(path).ok()?;
                Some((guid.clone(), field(&text, "m_Name")?.to_owned()))
            })
            .collect();
        let clips = extract_all(&assets.join("AnimationClip"), &names).unwrap();
        let (library, report) = library(&assets, &guids, &clips).unwrap();
        validate_gameplay(&library).unwrap();
        assert_eq!(clips.len(), 244);
        assert_eq!(library.state_speed("player_bear", "Run"), Some(1.0));
        assert_eq!(
            library
                .clip("catapult_fire")
                .unwrap()
                .first_event_time(EventKind::ProjectileRelease),
            Some(0.15)
        );
        assert_eq!(
            library
                .clip("worker_hammer")
                .unwrap()
                .first_event_time(EventKind::HammerHit),
            Some(0.41666666)
        );
        assert_eq!(
            library
                .clip("squid_attack")
                .unwrap()
                .first_event_time(EventKind::GrabComplete),
            Some(0.5)
        );
        let throw = library.state_clip("boss", "Throw").unwrap();
        assert_eq!(library.state_speed("boss", "Throw"), Some(1.0));
        assert_eq!(throw.length, 2.0);
        let release = throw.first_event_time(EventKind::BoulderRelease).unwrap();
        assert_eq!(release, 0.8333333);
        assert_eq!(
            throw.pose("Hand", 0.8, Pose::default()).position,
            [0.758, 0.441, 0.0]
        );
        assert_eq!(
            throw.pose("Hand", release, Pose::default()).position,
            [0.963, 0.876, 0.0]
        );
        assert_eq!(clips["boss_throw"].sprites[""].len(), 10);
        let mut bad = library.clone();
        bad.clips
            .iter_mut()
            .find(|c| c.0 == "boss_throw")
            .unwrap()
            .1
            .events
            .clear();
        assert!(validate_boss_throw(&bad).unwrap_err().contains("release"));
        let mut bad = library.clone();
        bad.transitions
            .iter_mut()
            .find(|t| t.controller == "boss" && t.from == "Throw")
            .unwrap()
            .duration = 0.1;
        assert!(validate_boss_throw(&bad)
            .unwrap_err()
            .contains("exit transition"));
        let receipt = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/p2-validation");
        std::fs::create_dir_all(&receipt).unwrap();
        std::fs::write(
            receipt.join("animation-reference-report.json"),
            serde_json::to_string_pretty(&report).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn controller_speed_transition_and_override_resolve_to_authored_clip() {
        let dir = std::env::temp_dir().join(format!(
            "kingdom-animation-controller-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("AnimatorController")).unwrap();
        std::fs::create_dir_all(dir.join("AnimatorOverrideController")).unwrap();
        let base = dir.join("AnimatorController/base.controller");
        std::fs::write(&base,"--- !u!91 &91\nAnimatorController:\n  m_Name: base\n--- !u!1107 &7\nAnimatorStateMachine:\n  m_DefaultState: {fileID: 2}\n--- !u!1102 &2\nAnimatorState:\n  m_Name: Run\n  m_Speed: 1.5\n  m_CycleOffset: 0\n  m_SpeedParameterActive: 0\n  m_Motion: {fileID: 7400000, guid: old}\n  m_Transitions:\n  - {fileID: 3}\n--- !u!1101 &3\nAnimatorStateTransition:\n  m_DstState: {fileID: 2}\n  m_TransitionDuration: 0.1\n  m_HasFixedDuration: 1\n  m_HasExitTime: 1\n  m_ExitTime: 0.8\n  m_Conditions:\n  - m_ConditionMode: 3\n    m_ConditionEvent: Speed\n    m_EventTreshold: 2\n").unwrap();
        std::fs::write(dir.join("old.anim"), "AnimationClip:\n  m_Name: old\n").unwrap();
        std::fs::write(dir.join("new.anim"), "AnimationClip:\n  m_Name: new\n").unwrap();
        std::fs::write(dir.join("AnimatorOverrideController/skin.overrideController"),"--- !u!221 &221\nAnimatorOverrideController:\n  m_Controller: {fileID: 91, guid: base}\n  m_Clips:\n  - m_OriginalClip: {fileID: 7400000, guid: old}\n    m_OverrideClip: {fileID: 7400000, guid: new}\n").unwrap();
        let guids = HashMap::from([
            ("base".into(), base),
            ("old".into(), dir.join("old.anim")),
            ("new".into(), dir.join("new.anim")),
        ]);
        let (library, report) = library(&dir, &guids, &BTreeMap::new()).unwrap();
        assert!(report.unsupported.is_empty());
        assert_eq!(library.state_speed("skin", "Run"), Some(1.5));
        assert_eq!(
            library
                .default_state("AnimatorOverrideController/skin.overrideController")
                .unwrap()
                .clip
                .as_deref(),
            Some("new")
        );
        let t = library
            .transitions
            .iter()
            .find(|t| t.controller == "skin")
            .unwrap();
        assert_eq!(t.exit_time, Some(0.8));
        assert_eq!(t.duration, 0.1);
        assert_eq!(t.conditions[0].threshold, 2.0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
