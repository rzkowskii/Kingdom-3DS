//! Scene-local fog and mist definitions. This presentation asset keeps its
//! own sprite IDs, separate from the save-compatible world sprite table.

use std::io::ErrorKind;
use std::path::Path;

use game::assetfile::{self, Kind};
use game::data::CloudLayerDef;
use serde::{Deserialize, Serialize};

use crate::cloud_mesh::CloudMesh;
use crate::sheet::Sheet;

pub const FILE_NAME: &str = "atmosphere.bin";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AtmosphereDef {
    pub fog: CloudLayerDef,
    pub mist: CloudLayerDef,
    /// Both layer definitions index this name table.
    pub sprites: Vec<String>,
    /// The authored tight mesh at each local sprite ID.
    pub meshes: Vec<CloudMesh>,
}

impl AtmosphereDef {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let def: Self = assetfile::decode(Kind::Atmosphere, bytes).map_err(|e| e.to_string())?;
        let count = def.sprites.len();
        let referenced = |index| {
            def.fog
                .sprites
                .iter()
                .chain(&def.mist.sprites)
                .any(|&id| usize::from(id) == index)
        };
        if count != def.meshes.len()
            || !def.fog.valid_with_parallax(count, 1.25)
            || !def.mist.valid(count)
            || !def.sprites.iter().enumerate().all(|(index, name)| {
                !name.is_empty() && !def.sprites[..index].contains(name) && referenced(index)
            })
            // fog_03 has an authored collinear triangle, which draws no pixels.
            || !def.meshes.iter().all(|mesh| mesh.valid_geometry(true))
        {
            return Err("invalid fog or mist definitions".into());
        }
        Ok(def)
    }

    pub fn validate_sheet(&self, sheet: &Sheet) -> Result<(), String> {
        for (name, mesh) in self.sprites.iter().zip(&self.meshes) {
            let id = sheet
                .id(name)
                .ok_or_else(|| format!("missing atmosphere sprite {name}"))?;
            if !mesh.fits(sheet.get(id)) {
                return Err(format!("atmosphere mesh lies outside sprite {name}"));
            }
        }
        Ok(())
    }

    /// Older v8 asset packs have no atmospheric controller data.
    pub fn load(path: &Path, sheet: &Sheet) -> Result<Option<Self>, String> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let def = Self::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        def.validate_sheet(sheet)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(def))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::data::Curve;

    #[test]
    fn sidecar_checks_local_sprite_ids_and_mesh_crop_bounds() {
        let layer = |id, parallax| CloudLayerDef {
            origin: [0.0; 3],
            depth: 3.0,
            width: 16.0,
            wind_multiplier: 1.0,
            lifetime: 12.0,
            fade_time: 2.0,
            vertical_motion: 0.1,
            max_clouds: 25,
            sprites: vec![id],
            prefab_active: true,
            allow_flip: true,
            y_curve: Curve {
                keys: vec![[0.0, 0.0, 0.0, 0.0]],
            },
            scale_curve: Curve {
                keys: vec![[0.0, 1.0, 0.0, 0.0]],
            },
            parallax_curve: Curve {
                keys: vec![[0.0, parallax, 0.0, 0.0]],
            },
        };
        let mesh = CloudMesh {
            vertices: vec![[-1.0, -1.0], [1.0, -1.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
        };
        let mut def = AtmosphereDef {
            fog: layer(0, 1.25),
            mist: layer(1, 0.75),
            sprites: vec!["fog".into(), "mist".into()],
            meshes: vec![mesh; 2],
        };
        let encode = |def: &AtmosphereDef| assetfile::encode(Kind::Atmosphere, def).unwrap();
        let loaded = AtmosphereDef::decode(&encode(&def)).unwrap();
        assert_eq!(loaded, def);

        let mut sheet = Sheet::default();
        sheet.names = def.sprites.clone();
        sheet.sprites = vec![
            crate::sheet::Sprite {
                page: 0,
                x: 0,
                y: 0,
                w: 2,
                h: 2,
                px: 1.0,
                py: 1.0,
                masks: [None, None],
            };
            2
        ];
        sheet.link(&[]);
        loaded.validate_sheet(&sheet).unwrap();
        sheet.sprites[1].w = 1;
        assert!(loaded.validate_sheet(&sheet).is_err());

        def.mist.sprites[0] = 2;
        assert!(AtmosphereDef::decode(&encode(&def)).is_err());
        def.mist.sprites[0] = 1;
        def.meshes[1].indices[2] = 3;
        assert!(AtmosphereDef::decode(&encode(&def)).is_err());
    }
}
