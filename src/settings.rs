use crate::model::Settings;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
use windows::{
    core::PCWSTR,
    Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
};

pub fn path() -> io::Result<PathBuf> {
    let directory = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::other("Windows LOCALAPPDATA is unavailable."))?;
    Ok(PathBuf::from(directory)
        .join("WarDogsEarplugs")
        .join("settings.txt"))
}

pub fn load(path: &Path) -> io::Result<Settings> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e),
    };
    let mut text = String::new();
    file.take(4097).read_to_string(&mut text)?;
    if text.len() > 4096 {
        return Err(io::Error::other("Settings file is too large."));
    }
    Settings::decode(&text).map_err(io::Error::other)
}

pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Invalid settings directory."))?;
    fs::create_dir_all(parent)?;
    // Login sessions share LOCALAPPDATA. Each writer needs its own staging file.
    let (temporary, mut file) = loop {
        let temporary = path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(settings.encode().as_bytes())?;
        file.sync_all()?;
        drop(file);
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Same-directory replacement: readers see the old complete file or the new complete file.
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(io::Error::other)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn another_writer_cannot_mutate_the_published_settings() {
        let directory = std::env::temp_dir().join(format!(
            "earplugs-settings-concurrent-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.txt");
        let mut competing_writer = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path.with_extension("tmp"))
            .unwrap();
        let settings = Settings::parse("60", "wardogs.exe").unwrap();
        save(&path, &settings).unwrap();
        competing_writer.write_all(b"broken").unwrap();
        competing_writer.sync_all().unwrap();
        let result = load(&path);
        drop(competing_writer);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("tmp"));
        fs::remove_dir(directory).unwrap();
        assert_eq!(
            result.unwrap(),
            settings,
            "Another writer changed the published configuration through a shared staging file"
        );
    }
    #[test]
    fn failed_replace_keeps_old_settings_and_cleans_staging_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory =
            std::env::temp_dir().join(format!("earplugs-settings-locked-{}", std::process::id()));
        let path = directory.join("settings.txt");
        save(&path, &Settings::default()).unwrap();
        let locked = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let result = save(&path, &Settings::parse("60", "wardogs.exe").unwrap());
        assert!(
            result.is_err(),
            "Replacing a destination locked against deletion should fail"
        );
        assert_eq!(load(&path).unwrap(), Settings::default());
        let leftovers = fs::read_dir(&directory).unwrap().count();
        drop(locked);
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
        assert_eq!(leftovers, 1, "Failed save left a staging file behind");
    }
    #[test]
    fn replaces_existing_settings_without_partial_content() {
        let directory =
            std::env::temp_dir().join(format!("earplugs-settings-test-{}", std::process::id()));
        let path = directory.join("settings.txt");
        assert_eq!(load(&path).unwrap(), Settings::default());
        save(&path, &Settings::default()).unwrap();
        let changed = Settings::parse("0", "wardogs.exe").unwrap();
        save(&path, &changed).unwrap();
        assert_eq!(load(&path).unwrap(), changed);
        assert!(!path.with_extension("tmp").exists());
        fs::write(&path, "corrupt").unwrap();
        assert!(load(&path).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
