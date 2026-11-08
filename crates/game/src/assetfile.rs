//! Versioned envelopes for converted assets. Reject an old or mixed layout
//! before it reaches a renderer or a save slot. Re-extraction is intentional:
//! the old flattened format cannot recover discarded transforms or events.

use alloc::vec::Vec;
use core::fmt;
use serde::{de::DeserializeOwned, Serialize};

pub const VERSION: u16 = 8;
const HEADER: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    World,
    Sheet,
}

impl Kind {
    fn magic(self) -> [u8; 4] {
        match self {
            Self::World => *b"KNLW",
            Self::Sheet => *b"KNLS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Format,
    Version(u16),
    Corrupt,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format => f.write_str("old or wrong asset format; re-extract and repack assets"),
            Self::Version(v) => write!(
                f,
                "unsupported asset version {v}; re-extract and repack assets"
            ),
            Self::Corrupt => f.write_str("damaged asset data; re-extract and repack assets"),
        }
    }
}

pub fn encode<T: Serialize>(kind: Kind, value: &T) -> Result<Vec<u8>, Error> {
    let body = postcard::to_allocvec(value).map_err(|_| Error::Corrupt)?;
    let len = u32::try_from(body.len()).map_err(|_| Error::Corrupt)?;
    let mut bytes = Vec::with_capacity(HEADER + body.len());
    bytes.extend_from_slice(&kind.magic());
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend(body);
    Ok(bytes)
}

pub fn decode<T: DeserializeOwned>(kind: Kind, bytes: &[u8]) -> Result<T, Error> {
    if bytes.len() < HEADER || bytes[..4] != kind.magic() {
        return Err(Error::Format);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(Error::Version(version));
    }
    let len = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    if len != bytes.len() - HEADER {
        return Err(Error::Corrupt);
    }
    let (value, tail) = postcard::take_from_bytes(&bytes[HEADER..]).map_err(|_| Error::Corrupt)?;
    if !tail.is_empty() {
        return Err(Error::Corrupt);
    }
    Ok(value)
}

/// Validate hierarchy indices and numeric poses before rendering converted data.
pub fn decode_world(bytes: &[u8]) -> Result<crate::data::WorldData, Error> {
    let world: crate::data::WorldData = decode(Kind::World, bytes)?;
    if !world.transforms_valid()
        || !world.animations.clips.iter().all(|(_, c)| c.validate())
        || !world.camera.valid()
        || !world.squid.valid()
        || !world.boss_throw.valid()
        || world.red_moon.is_some_and(|id| {
            !world
                .lights
                .get(usize::from(id))
                .is_some_and(crate::data::LightPreset::is_red_moon_overlay)
        })
        || world
            .boulder_fx
            .as_ref()
            .is_some_and(|fx| !fx.valid(world.sprites.len()))
        || world
            .clouds
            .as_ref()
            .is_some_and(|clouds| !clouds.valid(world.sprites.len()))
        || !world.protos.iter().flat_map(|p| &p.obstacles).all(|o| {
            (3..=1024).contains(&o.points.len())
                && o.origin
                    .iter()
                    .chain(o.points.iter().flatten())
                    .all(|v| v.is_finite() && v.abs() <= 10000.0)
        })
        || !world
            .protos
            .iter()
            .all(|p| p.catapult.and_then(|c| c.recoil).is_none_or(|d| d.valid()))
    {
        return Err(Error::Corrupt);
    }
    Ok(world)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_moon_reference_requires_an_existing_overlay() {
        let world = crate::data::WorldData {
            red_moon: Some(0),
            ..Default::default()
        };
        let bytes = encode(Kind::World, &world).unwrap();
        assert!(matches!(decode_world(&bytes), Err(Error::Corrupt)));
    }

    #[test]
    fn converted_world_roundtrips_and_old_or_mixed_formats_are_rejected() {
        let world = crate::data::WorldData::default();
        let bytes = encode(Kind::World, &world).unwrap();
        assert_eq!(
            decode::<crate::data::WorldData>(Kind::World, &bytes).unwrap(),
            world
        );
        let old = postcard::to_allocvec(&world).unwrap();
        assert_eq!(
            decode::<crate::data::WorldData>(Kind::World, &old),
            Err(Error::Format)
        );
        assert_eq!(
            decode::<crate::data::WorldData>(Kind::Sheet, &bytes),
            Err(Error::Format)
        );
        let mut future = bytes.clone();
        future[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(
            decode::<crate::data::WorldData>(Kind::World, &future),
            Err(Error::Version(VERSION + 1))
        );
    }

    #[test]
    fn squid_physics_and_obstacle_numeric_data_are_validated() {
        let mut w = crate::data::WorldData::default();
        let encoded = encode(Kind::World, &w).unwrap();
        assert!(decode_world(&encoded).is_ok());
        let mut previous = encoded;
        previous[4..6].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(decode_world(&previous), Err(Error::Version(3)));
        w.squid.fly_duration = f32::NAN;
        assert_eq!(
            decode_world(&encode(Kind::World, &w).unwrap()),
            Err(Error::Corrupt)
        );
        w.squid = Default::default();
        w.protos.push(crate::data::Proto {
            obstacles: alloc::vec![crate::data::ObstacleDef {
                origin: [0., 0.],
                points: alloc::vec![[0., 0.], [1., 0.]]
            }],
            ..Default::default()
        });
        assert_eq!(
            decode_world(&encode(Kind::World, &w).unwrap()),
            Err(Error::Corrupt)
        );
        w.protos[0].obstacles[0].points.push([0., 1.]);
        assert!(decode_world(&encode(Kind::World, &w).unwrap()).is_ok());
        w.protos[0].obstacles[0].origin[0] = f32::INFINITY;
        assert_eq!(
            decode_world(&encode(Kind::World, &w).unwrap()),
            Err(Error::Corrupt)
        );
    }

    #[test]
    fn incomplete_and_extra_payloads_do_not_decode_as_valid_assets() {
        let bytes = encode(Kind::World, &42u32).unwrap();
        for cut in 0..bytes.len() {
            assert!(decode::<u32>(Kind::World, &bytes[..cut]).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(decode::<u32>(Kind::World, &extra), Err(Error::Corrupt));
        // Even a matching length must not admit trailing serialized garbage.
        let len = (extra.len() - HEADER) as u32;
        extra[6..10].copy_from_slice(&len.to_le_bytes());
        assert_eq!(decode::<u32>(Kind::World, &extra), Err(Error::Corrupt));
    }
}
