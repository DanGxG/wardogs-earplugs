use crate::model::Settings;
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
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
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
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
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;
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
