//! Saves as files: `$KINGDOM_SAVES`, else `$XDG_DATA_HOME/kingdom-3ds`, else
//! `~/.local/share/kingdom-3ds`.

use std::path::PathBuf;

pub struct Files {
    dir: PathBuf,
}

impl Files {
    pub fn new() -> Self {
        let dir = std::env::var_os("KINGDOM_SAVES")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .filter(|d| d.is_absolute())
                    .map(|d| d.join("kingdom-3ds"))
            })
            .or_else(|| std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("kingdom-3ds")))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/kingdom-3ds"))
            })
            .unwrap_or_else(|| PathBuf::from("saves"));
        Files { dir }
    }

    fn path(&self, slot: &str) -> PathBuf {
        self.dir.join(format!("{slot}.sav"))
    }
}

impl platform::Storage for Files {
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
        platform::savefile::read(&self.path(slot))
    }

    fn read_backup(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
        platform::savefile::read_backup(&self.path(slot))
    }

    fn write(&mut self, slot: &str, data: &[u8]) -> bool {
        std::fs::create_dir_all(&self.dir).is_ok()
            && platform::savefile::write(&self.path(slot), data).is_ok()
    }

    fn write_preserving_backup(&mut self, slot: &str, data: &[u8]) -> bool {
        std::fs::create_dir_all(&self.dir).is_ok()
            && platform::savefile::write_preserving_backup(&self.path(slot), data).is_ok()
    }

    fn delete(&mut self, slot: &str) {
        let path = self.path(slot);
        for p in [
            path.clone(),
            path.with_extension("sav.bak"),
            path.with_extension("sav.tmp"),
            path.with_extension("sav.old"),
        ] {
            let _ = std::fs::remove_file(p);
        }
    }
}
