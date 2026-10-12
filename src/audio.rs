use crate::model::{unchanged, Settings};
use std::{collections::HashSet, mem::size_of};
use windows::{
    core::Interface,
    Win32::{
        Foundation::CloseHandle,
        Media::Audio::{
            eRender, AudioSessionStateExpired, IAudioSessionControl, IAudioSessionControl2,
            IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
            AUDCLNT_E_DEVICE_INVALIDATED, DEVICE_STATE_ACTIVE,
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
    control: IAudioSessionControl,
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
                applied: settings.target_volume(),
                previous,
                control: volume.cast().map_err(|e| e.to_string())?,
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
            "Earplugs ON: {} game session(s), {}%. F2 restores.",
            self.changed.len(),
            settings.percent
        ))
    }

    pub fn reapply(&mut self, settings: &Settings) -> Result<String, String> {
        let target = settings.target_volume();
        let mut before = Vec::with_capacity(self.changed.len());
        // Read all current values before writing, and keep the original restore values.
        for session in &self.changed {
            before.push((unsafe { session.volume.GetMasterVolume() }.map_err(|e| format!("Cannot update game volume: {e}. Press F2 to restore or clear the old sessions."))?, session.applied));
        }
        for index in 0..self.changed.len() {
            if let Err(error) = unsafe {
                self.changed[index]
                    .volume
                    .SetMasterVolume(target, std::ptr::null())
            } {
                let mut rollback_failed = false;
                for (session, &(current, applied)) in self.changed[..index].iter_mut().zip(&before)
                {
                    if unsafe { session.volume.SetMasterVolume(current, std::ptr::null()) }.is_ok()
                    {
                        session.applied = applied;
                    } else {
                        rollback_failed = true;
                    }
                }
                return Err(format!(
                    "Cannot update game volume: {error}. {}",
                    if rollback_failed {
                        "Rollback was incomplete; press F2 to restore before exiting."
                    } else {
                        "Previous mixer levels preserved."
                    }
                ));
            }
            self.changed[index].applied = target;
        }
        Ok(format!(
            "Earplugs ON: {} game session(s), {}%. F2 restores the original volume.",
            self.changed.len(),
            settings.percent
        ))
    }

    pub fn restore(&mut self) -> Result<String, String> {
        let mut failed = Vec::new();
        let mut manual = 0;
        for session in self.changed.drain(..) {
            // Only expired sessions/removed devices can be safely dropped without restoration.
            match unsafe { session.control.GetState() } {
                Ok(state) if state == AudioSessionStateExpired => continue,
                Err(error) if error.code() == AUDCLNT_E_DEVICE_INVALIDATED => continue,
                _ => (),
            }
            match unsafe { session.volume.GetMasterVolume() } {
                Ok(current) if unchanged(current, session.applied) => {
                    if let Err(error) = unsafe {
                        session
                            .volume
                            .SetMasterVolume(session.previous, std::ptr::null())
                    } {
                        if error.code() != AUDCLNT_E_DEVICE_INVALIDATED {
                            failed.push(session);
                        }
                    }
                }
                Ok(_) => manual += 1,
                Err(error) if error.code() == AUDCLNT_E_DEVICE_INVALIDATED => (),
                Err(_) => failed.push(session),
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
            for (percent, expected) in [("60", 0.6), ("80", 0.8), ("0", 0.0), ("100", 1.0)] {
                audio
                    .reapply(&Settings::parse(percent, "wardogs-test-tone.exe").unwrap())
                    .unwrap();
                assert!(unchanged(
                    unsafe { volume.GetMasterVolume().unwrap() },
                    expected
                ));
            }
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
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4));
            audio.restore().unwrap();
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.2));
            let mute = Settings::parse("0", "wardogs-test-tone.exe").unwrap();
            audio.toggle(&mute).unwrap();
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.0));
            audio.restore().unwrap();
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.2));
            let missing = Settings::parse("40", "no-such-game-fixture.exe").unwrap();
            assert!(audio.toggle(&missing).is_err());
            assert!(!audio.is_lowered());
            if let Some(executable) = std::env::var_os("EARPLUGS_TEST_EXE") {
                verify_release_hotkey(&executable, &settings, volume);
            }
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

    fn verify_release_hotkey(
        executable: &std::ffi::OsStr,
        settings: &Settings,
        volume: &ISimpleAudioVolume,
    ) {
        use windows::{
            core::w,
            Win32::{
                Foundation::{LPARAM, WPARAM},
                UI::{
                    Input::KeyboardAndMouse::{
                        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
                        VK_F2,
                    },
                    WindowsAndMessaging::{FindWindowW, IsWindowVisible, PostMessageW, WM_COMMAND},
                },
            },
        };
        assert!(
            unsafe { FindWindowW(w!("WarDogsEarplugsWindow"), None) }.is_err(),
            "Close the running Earplugs app before the release hotkey test."
        );
        let profile = std::env::temp_dir().join(format!("earplugs-ui-test-{}", std::process::id()));
        let configuration = profile.join("WarDogsEarplugs").join("settings.txt");
        crate::settings::save(&configuration, settings).unwrap();
        let mut app = std::process::Command::new(executable)
            .arg("--background")
            .env("LOCALAPPDATA", &profile)
            .spawn()
            .unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let window = (0..50)
                .find_map(|_| {
                    let found = unsafe { FindWindowW(w!("WarDogsEarplugsWindow"), None) }.ok();
                    if found.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    found
                })
                .expect("Release EXE did not create a window");
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert!(
                !unsafe { IsWindowVisible(window) }.as_bool(),
                "Release must run in the background with F2 registered"
            );
            use windows::Win32::{
                Foundation::RECT,
                UI::WindowsAndMessaging::{
                    GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
                    GetWindowThreadProcessId, SendMessageW, GWL_EXSTYLE, GW_OWNER, HTTRANSPARENT,
                    MA_NOACTIVATE, WM_MOUSEACTIVATE, WM_NCHITTEST, WS_EX_TOPMOST,
                },
            };
            let overlay = unsafe { FindWindowW(w!("WarDogsEarplugsOverlay"), None) }
                .expect("Independent overlay window missing");
            assert!(
                !unsafe { IsWindowVisible(overlay) }.as_bool(),
                "Overlay must start hidden while earplugs are OFF"
            );
            assert!(
                unsafe { GetWindow(overlay, GW_OWNER) }.is_err(),
                "Overlay must not be owned by a game or settings window"
            );
            let mut overlay_pid = 0;
            unsafe {
                GetWindowThreadProcessId(overlay, Some(&mut overlay_pid));
            }
            assert_eq!(
                overlay_pid,
                app.id(),
                "Overlay must belong only to Earplugs"
            );
            assert_eq!(
                unsafe { SendMessageW(overlay, WM_NCHITTEST, None, None) }.0,
                HTTRANSPARENT as isize,
                "Overlay must pass hit testing through"
            );
            assert_eq!(
                unsafe { SendMessageW(overlay, WM_MOUSEACTIVATE, None, None) }.0,
                MA_NOACTIVATE as isize,
                "Overlay must not activate on mouse input"
            );
            unsafe {
                volume.SetMasterVolume(0.8, std::ptr::null()).unwrap();
            }
            let key_down = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_F2,
                        ..Default::default()
                    },
                },
            };
            let key_up = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_F2,
                        dwFlags: KEYEVENTF_KEYUP,
                        ..Default::default()
                    },
                },
            };
            for expected in [0.4, 0.8, 0.4] {
                let foreground = unsafe { GetForegroundWindow() };
                assert_eq!(
                    unsafe { SendInput(&[key_down, key_up], size_of::<INPUT>() as i32) },
                    2,
                    "Windows did not accept the test F2 input"
                );
                let changed = (0..30).any(|_| {
                    if unchanged(unsafe { volume.GetMasterVolume().unwrap() }, expected) {
                        true
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        false
                    }
                });
                assert!(
                    changed,
                    "Real F2 did not set expected mixer volume {expected}; actual={}, overlay_on={}, foreground={:?}",
                    unsafe { volume.GetMasterVolume().unwrap() },
                    unsafe { IsWindowVisible(overlay) }.as_bool(),
                    unsafe { GetForegroundWindow() },
                );
                assert!(
                    !unsafe { IsWindowVisible(window) }.as_bool(),
                    "Hotkey unexpectedly opened the UI"
                );
                assert_eq!(
                    unsafe { GetForegroundWindow() },
                    foreground,
                    "The overlay stole focus"
                );
                assert_eq!(
                    unsafe { IsWindowVisible(overlay) }.as_bool(),
                    expected == 0.4,
                    "Overlay visibility must follow F2 state"
                );
                if expected == 0.4 {
                    let mut bounds = RECT::default();
                    unsafe {
                        GetWindowRect(overlay, &mut bounds).unwrap();
                    }
                    assert_eq!(
                        (bounds.left, bounds.top),
                        (12, 12),
                        "Overlay must sit at the primary screen's top-left"
                    );
                    assert!(
                        unsafe { GetWindowLongPtrW(overlay, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST.0
                            != 0,
                        "Overlay must stay above normal windows"
                    );
                }
                if expected == 0.4 {
                    use windows::{
                        core::PCWSTR,
                        Win32::UI::WindowsAndMessaging::{GetDlgItem, SendMessageW, WM_SETTEXT},
                    };
                    let percent = unsafe { GetDlgItem(Some(window), 201) }.unwrap();
                    for (text, target) in [
                        ("60", 0.6),
                        ("80", 0.8),
                        ("10", 0.1),
                        ("0", 0.0),
                        ("100", 1.0),
                        ("101", 1.0),
                        ("40", 0.4),
                    ] {
                        let value: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                        unsafe {
                            SendMessageW(
                                percent,
                                WM_SETTEXT,
                                None,
                                Some(LPARAM(PCWSTR(value.as_ptr()).0 as isize)),
                            );
                            SendMessageW(window, WM_COMMAND, Some(WPARAM(101)), None);
                        }
                        assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, target), "Applying {text}% while ON did not change the actual Windows mixer volume");
                        assert!(
                            unsafe { IsWindowVisible(overlay) }.as_bool(),
                            "Applying a percentage should keep the overlay ON"
                        );
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // Reproduce a persisted 40% mixer session: a new 60% target must be exact.
            unsafe {
                SendMessageW(window, WM_COMMAND, Some(WPARAM(102)), None);
            }
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.8));
            unsafe {
                volume.SetMasterVolume(0.4, std::ptr::null()).unwrap();
            }
            let percent =
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(window), 201) }
                    .unwrap();
            unsafe {
                SendMessageW(
                    percent,
                    windows::Win32::UI::WindowsAndMessaging::WM_SETTEXT,
                    None,
                    Some(LPARAM(w!("60").0 as isize)),
                );
                SendMessageW(window, WM_COMMAND, Some(WPARAM(101)), None);
                SendMessageW(window, WM_COMMAND, Some(WPARAM(102)), None);
            }
            assert!(
                unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.6),
                "New 60% target was incorrectly capped by the old 40% mixer level"
            );
            assert!(unsafe { IsWindowVisible(overlay) }.as_bool());
            if let Ok(seconds) = std::env::var("EARPLUGS_TEST_OVERLAY_PAUSE") {
                println!("Release overlay is ON for visual/performance inspection.");
                std::thread::sleep(std::time::Duration::from_secs(seconds.parse().unwrap()));
            }
            unsafe {
                PostMessageW(Some(window), WM_COMMAND, WPARAM(104), LPARAM(0)).unwrap();
            }
            let exited = (0..30).any(|_| {
                if app.try_wait().unwrap().is_some() {
                    true
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    false
                }
            });
            assert!(exited, "Restore-and-exit did not close release EXE");
            assert!(
                unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4),
                "Exit did not restore previous mixer volume"
            );
            assert!(
                unsafe { FindWindowW(w!("WarDogsEarplugsOverlay"), None) }.is_err(),
                "Overlay remained after Exit"
            );
        }));
        let _ = app.kill();
        let _ = app.wait();
        let _ = std::fs::remove_file(&configuration);
        let _ = std::fs::remove_dir(configuration.parent().unwrap());
        let _ = std::fs::remove_dir(profile);
        result.unwrap();
    }
}
