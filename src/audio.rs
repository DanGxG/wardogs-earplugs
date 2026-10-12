use crate::model::{unchanged, Settings};
use std::{collections::HashSet, mem::size_of};
use windows::{
    core::Interface,
    Win32::{
        Foundation::CloseHandle,
        Media::Audio::{
            eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2,
            IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        },
        System::{
            Com::{CoCreateInstance, CLSCTX_ALL},
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
        },
    },
};

struct ChangedSession {
    volume: ISimpleAudioVolume,
    previous: f32,
    applied: f32,
}

#[derive(Default)]
pub struct Audio {
    changed: Vec<ChangedSession>,
}

impl Audio {
    pub fn is_lowered(&self) -> bool {
        !self.changed.is_empty()
    }

    pub fn toggle(&mut self, settings: &Settings) -> Result<String, String> {
        if self.is_lowered() {
            return self.restore();
        }
        let sessions = sessions_for(settings)?;
        if sessions.is_empty() {
            return Err("No matching game audio. Start WarDogs and check the executable name in Task Manager's Details tab.".into());
        }
        // Read every original value before changing any session.
        let mut pending = Vec::with_capacity(sessions.len());
        for volume in sessions {
            let previous = unsafe { volume.GetMasterVolume() }.map_err(|e| e.to_string())?;
            pending.push(ChangedSession {
                applied: settings.lowered_volume(previous),
                previous,
                volume,
            });
        }
        for session in pending {
            if let Err(error) = unsafe {
                session
                    .volume
                    .SetMasterVolume(session.applied, std::ptr::null())
            } {
                let rollback = self.restore();
                return Err(format!(
                    "Volume change failed: {error}. {}",
                    rollback.unwrap_or_else(|e| e)
                ));
            }
            self.changed.push(session);
        }
        Ok(format!(
            "Earplugs ON: {} game session(s), at most {}%. F2 restores.",
            self.changed.len(),
            settings.percent
        ))
    }

    pub fn restore(&mut self) -> Result<String, String> {
        let mut failed = Vec::new();
        let mut manual = 0;
        for session in self.changed.drain(..) {
            // Closed games/devices may disappear; keep failed sessions for a retry.
            match unsafe { session.volume.GetMasterVolume() } {
                Ok(current) if unchanged(current, session.applied) => {
                    if unsafe {
                        session
                            .volume
                            .SetMasterVolume(session.previous, std::ptr::null())
                    }
                    .is_err()
                    {
                        failed.push(session);
                    }
                }
                Ok(_) => manual += 1,
                Err(_) => { /* The audio session is no longer available. */ }
            }
        }
        self.changed = failed;
        if !self.changed.is_empty() {
            return Err(
                "Windows could not restore every game session. Press F2 to retry before exiting."
                    .into(),
            );
        }
        Ok(if manual == 0 {
            "Earplugs OFF: previous game volume restored.".into()
        } else {
            "Earplugs OFF: preserved your manual mixer changes.".into()
        })
    }
}

fn process_ids(settings: &Settings) -> Result<HashSet<u32>, String> {
    // Windows process-list metadata only: no game process handles or memory access.
    unsafe {
        let snapshot =
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ids = HashSet::new();
        let mut result = Process32FirstW(snapshot, &mut entry);
        while result.is_ok() {
            let length = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            if settings.matches(&String::from_utf16_lossy(&entry.szExeFile[..length])) {
                ids.insert(entry.th32ProcessID);
            }
            result = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        Ok(ids)
    }
}

fn sessions_for(settings: &Settings) -> Result<Vec<ISimpleAudioVolume>, String> {
    let ids = process_ids(settings)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| e.to_string())?;
        let mut sessions = Vec::new();
        for index in 0..devices.GetCount().map_err(|e| e.to_string())? {
            let device = devices.Item(index).map_err(|e| e.to_string())?;
            let manager = match device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                Ok(manager) => manager,
                Err(_) => continue,
            };
            let list = manager.GetSessionEnumerator().map_err(|e| e.to_string())?;
            for index in 0..list.GetCount().map_err(|e| e.to_string())? {
                let control = list.GetSession(index).map_err(|e| e.to_string())?;
                let control2: IAudioSessionControl2 = control.cast().map_err(|e| e.to_string())?;
                if ids.contains(&control2.GetProcessId().map_err(|e| e.to_string())?)
                    && control.GetState().map_err(|e| e.to_string())? != AudioSessionStateExpired
                {
                    sessions.push(control.cast().map_err(|e| e.to_string())?);
                }
            }
        }
        Ok(sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    #[test]
    #[ignore = "Requires an active Windows audio output; first build the wardogs-test-tone example"]
    fn real_windows_mixer_roundtrip() {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        }
        let fixture = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/wardogs-test-tone.exe");
        let mut child = std::process::Command::new(fixture).spawn().unwrap();
        let settings = Settings::parse("40", "wardogs-test-tone.exe").unwrap();
        let result = std::panic::catch_unwind(|| {
            let mut sessions = Vec::new();
            for _ in 0..50 {
                sessions = sessions_for(&settings).unwrap();
                if !sessions.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(
                !sessions.is_empty(),
                "Silent fixture did not get a Windows mixer session"
            );
            let volume = &sessions[0];
            unsafe {
                volume.SetMasterVolume(0.8, std::ptr::null()).unwrap();
            }
            let mut audio = Audio::default();
            audio.toggle(&settings).unwrap();
            assert!(audio.is_lowered());
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4));
            audio.toggle(&settings).unwrap();
            assert!(!audio.is_lowered());
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.8));
            audio.toggle(&settings).unwrap();
            unsafe {
                volume.SetMasterVolume(0.25, std::ptr::null()).unwrap();
            }
            audio.restore().unwrap();
            assert!(unchanged(
                unsafe { volume.GetMasterVolume().unwrap() },
                0.25
            ));
            unsafe {
                volume.SetMasterVolume(0.2, std::ptr::null()).unwrap();
            }
            audio.toggle(&settings).unwrap();
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.2));
            audio.restore().unwrap();
            let mute = Settings::parse("0", "wardogs-test-tone.exe").unwrap();
            audio.toggle(&mute).unwrap();
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.0));
            audio.restore().unwrap();
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.2));
            let missing = Settings::parse("40", "no-such-game-fixture.exe").unwrap();
            assert!(audio.toggle(&missing).is_err());
            assert!(!audio.is_lowered());
        });
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(
            std::env::temp_dir().join(format!("wardogs-silent-fixture-{}.wav", child.id())),
        );
        unsafe {
            CoUninitialize();
        }
        result.unwrap();
    }
}
