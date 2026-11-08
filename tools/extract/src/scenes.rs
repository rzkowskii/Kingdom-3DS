//! Unity scenes, prefabs and data assets → JSON object trees.
//!
//! Each GameObject becomes a node with its transform, components and
//! children. Every component keeps all its serialized fields, with object
//! references rewritten as:
//! - `null` for `{fileID: 0}`,
//! - `{"local": id}` for objects in the same file (ids match node/component `id`s),
//! - `{"asset": "<path under Assets/>", "id": fileID}` for other assets.
//!
//! The game code picks out the fields it needs; the exporter stays generic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Unity class IDs we name; anything else is `Class<id>`.
fn class_name(id: u32) -> String {
    match id {
        1 => "GameObject",
        4 => "Transform",
        20 => "Camera",
        23 => "MeshRenderer",
        33 => "MeshFilter",
        50 => "Rigidbody2D",
        58 => "CircleCollider2D",
        60 => "PolygonCollider2D",
        61 => "BoxCollider2D",
        68 => "EdgeCollider2D",
        82 => "AudioSource",
        95 => "Animator",
        114 => "MonoBehaviour",
        198 => "ParticleSystem",
        199 => "ParticleSystemRenderer",
        212 => "SpriteRenderer",
        222 => "CanvasRenderer",
        223 => "Canvas",
        224 => "RectTransform",
        233 => "HingeJoint2D",
        _ => return format!("Class{id}"),
    }
    .to_owned()
}

pub struct Resolver<'a> {
    guids: &'a HashMap<String, PathBuf>,
    assets_root: &'a Path,
    /// Script GUID → class name (from `Scripts/**/<Class>.cs.meta`).
    scripts: HashMap<String, String>,
}

impl<'a> Resolver<'a> {
    pub fn new(guids: &'a HashMap<String, PathBuf>, assets_root: &'a Path) -> Self {
        let scripts = guids
            .iter()
            .filter(|(_, p)| p.extension().is_some_and(|x| x == "cs"))
            .filter_map(|(g, p)| Some((g.clone(), p.file_stem()?.to_string_lossy().into_owned())))
            .collect();
        Self {
            guids,
            assets_root,
            scripts,
        }
    }

    fn asset_path(&self, guid: &str) -> Option<String> {
        let p = self.guids.get(guid)?;
        Some(
            p.strip_prefix(self.assets_root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/"),
        )
    }

    /// YAML → JSON, rewriting `{fileID, guid?, type?}` references.
    fn convert(&self, v: &serde_yaml::Value) -> Value {
        use serde_yaml::Value as Y;
        match v {
            Y::Null => Value::Null,
            Y::Bool(b) => Value::Bool(*b),
            Y::Number(n) => n
                .as_i64()
                .map(Value::from)
                .or_else(|| n.as_f64().map(Value::from))
                .unwrap_or(Value::Null),
            Y::String(s) => Value::String(s.clone()),
            Y::Sequence(items) => Value::Array(items.iter().map(|i| self.convert(i)).collect()),
            Y::Mapping(m) => {
                if let Some(r) = self.reference(m) {
                    return r;
                }
                let mut out = Map::new();
                for (k, val) in m {
                    let key = match k {
                        Y::String(s) => s.clone(),
                        other => serde_yaml::to_string(other)
                            .unwrap_or_default()
                            .trim()
                            .to_owned(),
                    };
                    out.insert(key, self.convert(val));
                }
                Value::Object(out)
            }
            Y::Tagged(t) => self.convert(&t.value),
        }
    }

    fn reference(&self, m: &serde_yaml::Mapping) -> Option<Value> {
        let file_id = m.get("fileID")?;
        let allowed = m
            .keys()
            .all(|k| matches!(k.as_str(), Some("fileID" | "guid" | "type")));
        if !allowed {
            return None;
        }
        let id = file_id.as_i64().unwrap_or(0);
        Some(match m.get("guid").and_then(|g| g.as_str()) {
            None if id == 0 => Value::Null,
            None => json!({ "local": id }),
            Some(g) => match self.asset_path(g) {
                Some(path) => json!({ "asset": path, "id": id }),
                None => json!({ "guid": g, "id": id }),
            },
        })
    }
}

struct Doc {
    class: u32,
    id: i64,
    body: Map<String, Value>,
}

fn parse_docs(text: &str, r: &Resolver) -> Result<Vec<Doc>, String> {
    let mut docs = Vec::new();
    let mut header: Option<(u32, i64)> = None;
    let mut start = 0;
    let flush =
        |header: Option<(u32, i64)>, body: &str, docs: &mut Vec<Doc>| -> Result<(), String> {
            let Some((class, id)) = header else {
                return Ok(());
            };
            let y: serde_yaml::Value =
                serde_yaml::from_str(body).map_err(|e| format!("doc &{id}: {e}"))?;
            // A document is `ClassName: { fields }`.
            let body = match r.convert(&y) {
                Value::Object(o) => match o.into_iter().next() {
                    Some((_, Value::Object(fields))) => fields,
                    _ => Map::new(),
                },
                _ => Map::new(),
            };
            docs.push(Doc { class, id, body });
            Ok(())
        };
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("--- !u!") {
            flush(header, &text[start..pos], &mut docs)?;
            let mut parts = rest.split_whitespace();
            let class = parts.next().and_then(|c| c.parse().ok()).unwrap_or(0);
            let id = parts
                .next()
                .and_then(|a| a.trim_start_matches('&').parse().ok())
                .unwrap_or(0);
            header = Some((class, id));
            start = pos + line.len();
        }
        pos += line.len();
    }
    flush(header, &text[start..], &mut docs)?;
    Ok(docs)
}

fn local_id(v: &Value) -> Option<i64> {
    v.get("local").and_then(Value::as_i64)
}

fn vec_of(v: Option<&Value>, keys: &[&str], default: f64) -> Value {
    Value::Array(
        keys.iter()
            .map(|k| {
                json!(v
                    .and_then(|o| o.get(*k))
                    .and_then(Value::as_f64)
                    .unwrap_or(default))
            })
            .collect(),
    )
}

/// Builds the GameObject trees of one scene or prefab file.
pub fn export_file(path: &Path, r: &Resolver) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let docs = parse_docs(&text, r).map_err(|e| format!("{}: {e}", path.display()))?;
    let by_id: HashMap<i64, &Doc> = docs.iter().map(|d| (d.id, d)).collect();

    // GameObject id → its Transform doc.
    let mut transform_of: HashMap<i64, &Doc> = HashMap::new();
    for d in &docs {
        if matches!(d.class, 4 | 224) {
            if let Some(go) = d.body.get("m_GameObject").and_then(local_id) {
                transform_of.insert(go, d);
            }
        }
    }

    fn build(
        go_id: i64,
        by_id: &HashMap<i64, &Doc>,
        transform_of: &HashMap<i64, &Doc>,
        r: &Resolver,
    ) -> Value {
        let go = by_id[&go_id];
        let tf = transform_of.get(&go_id);
        let mut comps = Vec::new();
        for c in go
            .body
            .get("m_Component")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            // 2017 format: `- component: {fileID: N}`.
            let Some(cid) = c
                .as_object()
                .and_then(|o| o.values().next())
                .and_then(local_id)
            else {
                continue;
            };
            let Some(cd) = by_id.get(&cid) else { continue };
            if matches!(cd.class, 4 | 224) {
                continue;
            }
            let mut fields = cd.body.clone();
            let ty = if cd.class == 114 {
                let script = fields
                    .get("m_Script")
                    .and_then(|s| s.get("asset"))
                    .and_then(Value::as_str);
                script
                    .and_then(|p| Path::new(p).file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "MonoBehaviour".to_owned())
            } else {
                class_name(cd.class)
            };
            for noise in [
                "m_ObjectHideFlags",
                "m_PrefabParentObject",
                "m_PrefabInternal",
                "m_GameObject",
                "m_Script",
                "m_EditorHideFlags",
                "m_EditorClassIdentifier",
                "m_Name",
            ] {
                fields.remove(noise);
            }
            comps.push(json!({ "id": cid, "type": ty, "fields": fields }));
        }
        let _ = r;
        let tb = tf.map(|t| &t.body);
        let children: Vec<Value> = tb
            .and_then(|b| b.get("m_Children"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(local_id)
            .filter_map(|tid| by_id.get(&tid))
            .filter_map(|t| t.body.get("m_GameObject").and_then(local_id))
            .filter(|g| by_id.contains_key(g))
            .map(|g| build(g, by_id, transform_of, r))
            .collect();
        json!({
            "id": go_id,
            "transform_id": tf.map(|t| t.id),
            "name": go.body.get("m_Name").cloned().unwrap_or(Value::Null),
            "active": go.body.get("m_IsActive").and_then(Value::as_i64) != Some(0),
            "tag": go.body.get("m_TagString").cloned().unwrap_or(Value::Null),
            "layer": go.body.get("m_Layer").and_then(Value::as_i64).unwrap_or(0),
            "pos": vec_of(tb.and_then(|b| b.get("m_LocalPosition")), &["x", "y", "z"], 0.0),
            "scale": vec_of(tb.and_then(|b| b.get("m_LocalScale")), &["x", "y", "z"], 1.0),
            "rot": vec_of(tb.and_then(|b| b.get("m_LocalRotation")), &["x", "y", "z", "w"], 0.0),
            "components": comps,
            "children": children,
        })
    }

    let roots: Vec<Value> = transform_of
        .iter()
        .filter(|(_, t)| t.body.get("m_Father").is_none_or(|f| local_id(f).is_none()))
        .map(|(&go, t)| {
            let order = t
                .body
                .get("m_RootOrder")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            (order, go)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|(_, go)| build(go, &by_id, &transform_of, r))
        .collect();
    Ok(json!({ "roots": roots }))
}

/// ScriptableObject `.asset` files (LevelConfig, Day, Wave, BlenderState, …):
/// `{type, name, fields}` for the first MonoBehaviour document in the file.
pub fn export_data_asset(path: &Path, r: &Resolver) -> Result<Option<Value>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !text.contains("--- !u!114 ") {
        return Ok(None);
    }
    let docs = parse_docs(&text, r).map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(d) = docs.into_iter().find(|d| d.class == 114) else {
        return Ok(None);
    };
    let mut fields = d.body;
    let ty = fields
        .get("m_Script")
        .and_then(|s| s.get("asset"))
        .and_then(Value::as_str)
        .and_then(|p| Path::new(p).file_stem())
        .map(|s| s.to_string_lossy().into_owned());
    let script_guid = fields
        .get("m_Script")
        .and_then(|s| s.get("guid"))
        .and_then(Value::as_str);
    let ty = ty.or_else(|| script_guid.and_then(|g| r.scripts.get(g).cloned()));
    let name = fields.remove("m_Name");
    for noise in [
        "m_ObjectHideFlags",
        "m_PrefabParentObject",
        "m_PrefabInternal",
        "m_GameObject",
        "m_Script",
        "m_EditorHideFlags",
        "m_EditorClassIdentifier",
        "m_Enabled",
    ] {
        fields.remove(noise);
    }
    Ok(Some(json!({ "type": ty, "name": name, "fields": fields })))
}

/// Recursively lists files under `dir` with extension `ext`.
pub fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &10
GameObject:
  m_Component:
  - component: {fileID: 11}
  - component: {fileID: 12}
  - component: {fileID: 13}
  m_Name: Root
  m_TagString: Untagged
  m_IsActive: 1
--- !u!4 &11
Transform:
  m_GameObject: {fileID: 10}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 1.5, y: 2, z: 0}
  m_LocalScale: {x: -1, y: 1, z: 1}
  m_Children:
  - {fileID: 21}
  m_Father: {fileID: 0}
  m_RootOrder: 0
--- !u!212 &12
SpriteRenderer:
  m_GameObject: {fileID: 10}
  m_Enabled: 1
  m_Sprite: {fileID: 21300000, guid: abc, type: 2}
--- !u!233 &13
HingeJoint2D:
  m_GameObject: {fileID: 10}
  m_ConnectedRigidBody: {fileID: 88}
  m_ConnectedAnchor: {x: 0.421875, y: 0.171875}
  m_UseMotor: 0
--- !u!1 &20
GameObject:
  m_Component:
  - component: {fileID: 21}
  m_Name: Child
  m_IsActive: 0
--- !u!4 &21
Transform:
  m_GameObject: {fileID: 20}
  m_LocalPosition: {x: 0, y: 1, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 11}
";

    #[test]
    fn builds_tree_and_resolves_refs() {
        let dir = std::env::temp_dir().join(format!("extract-scene-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.unity");
        std::fs::write(&file, SCENE).unwrap();
        let guids = HashMap::from([("abc".to_owned(), dir.join("Sprite/coin.asset"))]);
        let r = Resolver::new(&guids, &dir);
        let v = export_file(&file, &r).unwrap();
        let root = &v["roots"][0];
        assert_eq!(root["name"], "Root");
        assert_eq!(root["transform_id"], 11);
        assert_eq!(root["pos"], json!([1.5, 2.0, 0.0]));
        assert_eq!(root["scale"][0], -1.0);
        assert_eq!(root["components"][0]["type"], "SpriteRenderer");
        assert_eq!(root["components"][1]["type"], "HingeJoint2D");
        assert_eq!(
            root["components"][1]["fields"]["m_ConnectedRigidBody"]["local"],
            88
        );
        assert_eq!(
            root["components"][1]["fields"]["m_ConnectedAnchor"]["x"],
            0.421875
        );
        assert_eq!(
            root["components"][0]["fields"]["m_Sprite"]["asset"],
            "Sprite/coin.asset"
        );
        assert_eq!(root["children"][0]["name"], "Child");
        assert_eq!(root["children"][0]["transform_id"], 21);
        assert_eq!(root["children"][0]["active"], false);
        std::fs::remove_dir_all(dir).ok();
    }
}
