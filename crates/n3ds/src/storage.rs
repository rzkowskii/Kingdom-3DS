//! Saves on the SD card, `sdmc:/3ds/kingdom-3ds/<slot>.sav`.

const DIR: &str = "sdmc:/3ds/kingdom-3ds";

pub struct SdCard;

impl SdCard {
    pub fn new() -> SdCard {
        let _ = std::fs::create_dir_all(DIR);
        SdCard
    }

    fn path(slot: &str) -> String {
        format!("{DIR}/{slot}.sav")
    }
}

impl platform::Storage for SdCard {
    fn read(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
        crate::savefile::read(std::path::Path::new(&Self::path(slot)))
    }

    fn read_backup(&self, slot: &str) -> Result<Option<Vec<u8>>, platform::ReadError> {
        crate::savefile::read_backup(std::path::Path::new(&Self::path(slot)))
    }

    fn write_preserving_backup(&mut self, slot: &str, data: &[u8]) -> bool {
        crate::savefile::write_preserving_backup(std::path::Path::new(&Self::path(slot)), data)
            .is_ok()
    }

    fn write(&mut self, slot: &str, data: &[u8]) -> bool {
        crate::savefile::write(std::path::Path::new(&Self::path(slot)), data).is_ok()
    }

    fn delete(&mut self, slot: &str) {
        let path = Self::path(slot);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{path}.bak"));
        let _ = std::fs::remove_file(format!("{path}.tmp"));
        let _ = std::fs::remove_file(format!("{path}.old"));
    }
}
