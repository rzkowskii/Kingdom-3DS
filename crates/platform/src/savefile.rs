//! File operations shared by desktop and SD saves. Content validation belongs
//! to the session, so primary and backup reads are deliberately separate.
use crate::ReadError;
use alloc::{format, vec::Vec};
use std::{fs, io, path::Path};

pub fn read(path: &Path) -> Result<Option<Vec<u8>>, ReadError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ReadError),
    }
}

pub fn read_backup(path: &Path) -> Result<Option<Vec<u8>>, ReadError> {
    read(Path::new(&format!("{}.bak", path.display())))
}

pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    replace(path, data, false, |a, b| fs::rename(a, b))
}

pub fn write_preserving_backup(path: &Path, data: &[u8]) -> io::Result<()> {
    replace(path, data, true, |a, b| fs::rename(a, b))
}

/// Retain rejected bytes independently of the rotating .bak. A future game
/// version or repaired asset pack may still be able to load them. Exclusive
/// creation means repeated recoveries never replace an earlier archive.
fn archive_rejected(path: &Path) -> io::Result<()> {
    let mut source = fs::File::open(path)?;
    for n in 0..=u32::MAX {
        let archive = path.with_extension(if n == 0 {
            "sav.rejected".into()
        } else {
            format!("sav.rejected.{n}")
        });
        let mut dest = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result = io::copy(&mut source, &mut dest).and_then(|_| dest.sync_all());
        if result.is_err() {
            drop(dest);
            let _ = fs::remove_file(archive);
        }
        return result;
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn replace(
    path: &Path,
    data: &[u8],
    preserve_backup: bool,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    use io::Write;
    let tmp = path.with_extension("sav.tmp");
    // A recovered write must leave the validated .bak untouched, even on FAT
    // where rename cannot replace a destination. The rejected main is archived
    // durably first; .old is only the rollback file during installation.
    let previous = path.with_extension(if preserve_backup {
        "sav.old"
    } else {
        "sav.bak"
    });
    let mut file = fs::File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    let had_main = match fs::metadata(path) {
        Ok(_) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    if had_main {
        if preserve_backup {
            archive_rejected(path)?;
        }
        remove_if_present(&previous)?;
        rename(path, &previous)?;
    }
    if let Err(e) = rename(&tmp, path) {
        if had_main {
            let _ = rename(&previous, path);
        }
        return Err(e);
    }
    if preserve_backup && had_main {
        let _ = remove_if_present(&previous);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "kingdom-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn fat_rename(a: &Path, b: &Path) -> io::Result<()> {
        if b.exists() {
            Err(io::ErrorKind::AlreadyExists.into())
        } else {
            fs::rename(a, b)
        }
    }
    #[test]
    fn failed_install_restores_main_and_keeps_validated_backup() {
        let d = dir();
        let p = d.join("game.sav");
        let b = p.with_extension("sav.bak");
        fs::write(&p, b"corrupt main").unwrap();
        fs::write(&b, b"valid backup").unwrap();
        let fail = |a: &Path, b: &Path| {
            if a.extension().is_some_and(|x| x == "tmp") {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                fat_rename(a, b)
            }
        };
        assert!(replace(&p, b"recovered", true, fail).is_err());
        assert_eq!(read(&p).unwrap().unwrap(), b"corrupt main");
        assert_eq!(read_backup(&p).unwrap().unwrap(), b"valid backup");
        replace(&p, b"recovered", true, fat_rename).unwrap();
        assert_eq!(read(&p).unwrap().unwrap(), b"recovered");
        assert_eq!(read_backup(&p).unwrap().unwrap(), b"valid backup");
        assert_eq!(
            fs::read(p.with_extension("sav.rejected")).unwrap(),
            b"corrupt main"
        );
        assert_eq!(
            fs::read(p.with_extension("sav.rejected.1")).unwrap(),
            b"corrupt main"
        );
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn repeated_recovery_archives_never_replace_rejected_bytes() {
        let d = dir();
        let p = d.join("game.sav");
        fs::write(&p, b"different game fingerprint").unwrap();
        fs::write(p.with_extension("sav.bak"), b"valid backup").unwrap();
        replace(&p, b"recovered", true, fat_rename).unwrap();
        fs::write(&p, b"second incompatible primary").unwrap();
        replace(&p, b"recovered again", true, fat_rename).unwrap();
        assert_eq!(
            fs::read(p.with_extension("sav.rejected")).unwrap(),
            b"different game fingerprint"
        );
        assert_eq!(
            fs::read(p.with_extension("sav.rejected.1")).unwrap(),
            b"second incompatible primary"
        );
        assert_eq!(read_backup(&p).unwrap().unwrap(), b"valid backup");
        fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn normal_replacement_keeps_previous_save_and_reads_are_independent() {
        let d = dir();
        let p = d.join("game.sav");
        assert_eq!(read(&p), Ok(None));
        replace(&p, b"first", false, fat_rename).unwrap();
        replace(&p, b"second", false, fat_rename).unwrap();
        assert_eq!(read_backup(&p).unwrap().unwrap(), b"first");
        fs::remove_file(&p).unwrap();
        assert_eq!(read(&p), Ok(None));
        assert_eq!(read_backup(&p).unwrap().unwrap(), b"first");
        fs::create_dir(&p).unwrap();
        assert_eq!(read(&p), Err(ReadError));
        fs::remove_dir_all(d).unwrap();
    }
}
