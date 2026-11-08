//! Authored Unity cloud sprite geometry, kept separately from the v8 sheet.

use serde::{Deserialize, Serialize};

use crate::sheet::Sprite;

pub const FILE_NAME: &str = "cloud-meshes.bin";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloudMesh {
    /// Pixel offsets from the authored pivot, with Y down.
    /// Add the sheet sprite's pivot to obtain its local atlas coordinates.
    pub vertices: Vec<[f32; 2]>,
    /// Unity m_IndexBuffer triangle order.
    pub indices: Vec<u16>,
}

impl CloudMesh {
    pub(crate) fn valid_for(&self, sprite: &Sprite) -> bool {
        (3..=64).contains(&self.vertices.len())
            && (3..=192).contains(&self.indices.len())
            && self.indices.len().is_multiple_of(3)
            && self.vertices.iter().all(|[x, y]| {
                (0.0..=f32::from(sprite.w)).contains(&(x + sprite.px))
                    && (0.0..=f32::from(sprite.h)).contains(&(y + sprite.py))
            })
            && self.indices.as_chunks::<3>().0.iter().all(|&triangle| {
                let [Some(a), Some(b), Some(c)] =
                    triangle.map(|i| self.vertices.get(usize::from(i)))
                else {
                    return false;
                };
                (b[0] - a[0]) * (c[1] - a[1]) != (b[1] - a[1]) * (c[0] - a[0])
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudMeshes {
    /// World sprite IDs rather than atlas-specific sprite IDs.
    pub entries: Vec<(u16, CloudMesh)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Sheet;
    use game::assetfile::{self, Kind};
    use game::data::{CloudLayerDef, CloudsDef, WorldData};

    fn cloud_layer(id: u16) -> CloudLayerDef {
        CloudLayerDef {
            origin: [0.0; 3],
            depth: 1.0,
            width: 1.0,
            wind_multiplier: 0.0,
            lifetime: 8.0,
            fade_time: 1.0,
            vertical_motion: 0.0,
            max_clouds: 1,
            sprites: vec![id],
            prefab_active: true,
            allow_flip: false,
            y_curve: Default::default(),
            scale_curve: Default::default(),
            parallax_curve: Default::default(),
        }
    }

    #[test]
    fn validates_full_mesh_set_and_keeps_v8_sheet_encoding() {
        let world = WorldData {
            sprites: vec!["stratus".into(), "cumulus".into()],
            clouds: Some(CloudsDef {
                stratus: cloud_layer(0),
                cumulus: cloud_layer(1),
                wind_change_cooldown: 0.5,
            }),
            ..Default::default()
        };
        let mut sheet = Sheet::default();
        // The sheet's sprite order need not match the world's sprite IDs.
        sheet.names = vec!["cumulus".into(), "stratus".into()];
        sheet.sprites = vec![
            Sprite {
                page: 0,
                x: 10,
                y: 20,
                w: 10,
                h: 12,
                px: 3.0,
                py: 4.0,
                masks: [None; 2]
            };
            2
        ];
        sheet.link(&world.sprites);
        let original_sheet = assetfile::encode(Kind::Sheet, &sheet).unwrap();
        let stratus = CloudMesh {
            vertices: vec![[-3., 0.], [7., -4.], [5., 8.]],
            indices: vec![0, 2, 1],
        };
        let cumulus = CloudMesh {
            vertices: vec![[-2., 1.], [6., -3.], [5., 7.]],
            indices: vec![1, 2, 0],
        };
        let mut meshes = CloudMeshes {
            entries: vec![(0, stratus.clone()), (1, cumulus.clone())],
        };
        let bytes = |meshes: &CloudMeshes| assetfile::encode(Kind::CloudMeshes, meshes).unwrap();
        sheet.link_cloud_meshes(&world, &bytes(&meshes)).unwrap();
        assert_eq!(sheet.cloud_mesh(0), Some(&cumulus));
        assert_eq!(sheet.cloud_mesh(1), Some(&stratus));
        assert_eq!(
            assetfile::encode(Kind::Sheet, &sheet).unwrap(),
            original_sheet
        );

        let rejects = |sheet: &mut Sheet, meshes: &CloudMeshes| {
            assert!(sheet.link_cloud_meshes(&world, &bytes(meshes)).is_err());
            assert_eq!(sheet.cloud_mesh(0), Some(&cumulus));
            assert_eq!(sheet.cloud_mesh(1), Some(&stratus));
        };
        let (id, mesh) = meshes.entries.pop().unwrap();
        rejects(&mut sheet, &meshes);
        meshes.entries.push((0, mesh.clone()));
        rejects(&mut sheet, &meshes);
        meshes.entries[1].0 = id;
        meshes.entries[1].1.indices[0] = 3;
        rejects(&mut sheet, &meshes);
        meshes.entries[1].1.indices[0] = 2;
        rejects(&mut sheet, &meshes);
        meshes.entries[1].1 = mesh;
        meshes.entries[1].1.vertices[0][0] = -4.0;
        rejects(&mut sheet, &meshes);
        meshes.entries[1].1.vertices[0][0] = f32::NAN;
        rejects(&mut sheet, &meshes);
    }
}
