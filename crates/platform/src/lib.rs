//! Traits implemented by each backend (desktop, n3ds).
#![no_std]

extern crate alloc;

pub mod diagnostics;

#[cfg(any(test, feature = "std"))]
extern crate std;
#[cfg(any(test, feature = "std"))]
pub mod savefile;

use alloc::vec::Vec;

/// Where saves live: files on desktop, the SD card on 3DS
/// (`sdmc:/3ds/kingdom-3ds/`). Slots are short names like `game::save::GAME_SLOT`.
pub trait Storage {
    /// Missing is distinct from an I/O failure. Never treat unreadable data as empty.
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, ReadError>;
    /// Read the previous save independently; the caller validates its contents.
    fn read_backup(&self, _slot: &str) -> Result<Option<Vec<u8>>, ReadError> {
        Ok(None)
    }
    /// Writes the whole slot; returns whether it worked.
    fn write(&mut self, slot: &str, data: &[u8]) -> bool;
    /// Install recovered state without rotating a rejected main file over the
    /// validated backup. Retain rejected primary bytes in a durable archive;
    /// backends must explicitly support both preservation guarantees.
    fn write_preserving_backup(&mut self, _slot: &str, _data: &[u8]) -> bool {
        false
    }
    fn delete(&mut self, slot: &str);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadError;
