//! Shared, host-tested filesystem implementation (including FAT replacement).
pub use platform::savefile::{read, read_backup, write, write_preserving_backup};
