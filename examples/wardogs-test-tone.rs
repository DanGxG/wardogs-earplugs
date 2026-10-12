//! Silent audio fixture for the opt-in real Windows mixer test. Not distributed.
#![windows_subsystem = "windows"]
use windows::{
    core::PCWSTR,
    Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_LOOP, SND_NODEFAULT},
};
fn main() {
    let path =
        std::env::temp_dir().join(format!("wardogs-silent-fixture-{}.wav", std::process::id()));
    let samples: u32 = 44100;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&44100u32.to_le_bytes());
    wav.extend_from_slice(&88200u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples * 2).to_le_bytes());
    wav.resize(44 + samples as usize * 2, 0);
    std::fs::write(&path, wav).unwrap();
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    assert!(unsafe {
        PlaySoundW(
            PCWSTR(name.as_ptr()),
            None,
            SND_FILENAME | SND_ASYNC | SND_LOOP | SND_NODEFAULT,
        )
    }
    .as_bool());
    std::thread::sleep(std::time::Duration::from_secs(30));
    unsafe {
        let _ = PlaySoundW(PCWSTR::null(), None, windows::Win32::Media::Audio::SND_SYNC);
    }
    let _ = std::fs::remove_file(path);
}
