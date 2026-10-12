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
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
        };
        let no_input = std::env::var_os("EARPLUGS_TEST_NO_INPUT").is_some();
        let mut available = if no_input { u16::MAX } else { 0 };
        for modifiers in if no_input { 0..0 } else { 0..16 } {
            if unsafe {
                RegisterHotKey(
                    None,
                    1000 + modifiers as i32,
                    HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT,
                    VK_F2.0 as u32,
                )
            }
            .is_ok()
            {
                available |= 1 << modifiers;
                unsafe {
                    UnregisterHotKey(None, 1000 + modifiers as i32).unwrap();
                }
            } else {
                println!("Desktop already owns F2 modifier mask {modifiers}; skipping that shortcut to avoid launching an unrelated app during this test.");
            }
        }
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
            if no_input {
                println!("Keyboard injection disabled by EARPLUGS_TEST_NO_INPUT; testing release controls, mixer and overlay only.");
            } else {
                verify_held_keys(volume, window, overlay, available);
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
                let key_state = [0x10, 0x11, 0x12, 0x5b, 0x5c, 0x71].map(|key| {
                    (key, unsafe {
                        windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(key)
                    })
                });
                if no_input {
                    unsafe {
                        SendMessageW(window, WM_COMMAND, Some(WPARAM(102)), None);
                    }
                } else {
                    assert_eq!(
                        unsafe { SendInput(&[key_down, key_up], size_of::<INPUT>() as i32) },
                        2,
                        "Windows did not accept the test F2 input"
                    );
                }
                let changed = (0..30).any(|_| {
                    if unchanged(unsafe { volume.GetMasterVolume().unwrap() }, expected)
                        && unsafe { IsWindowVisible(overlay) }.as_bool() == (expected == 0.4)
                    {
                        true
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        false
                    }
                });
                assert!(
                    changed,
                    "Real F2 did not set expected mixer volume {expected}; actual={}, overlay_on={}, foreground={:?}, keys_before={key_state:?}",
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
            // Tray menus run a nested Windows message loop. Input received while
            // App is borrowed must be deferred and then applied after dismissal.
            if !no_input {
                unsafe {
                    PostMessageW(
                        Some(window),
                        windows::Win32::UI::WindowsAndMessaging::WM_APP + 1,
                        WPARAM(0),
                        LPARAM(windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP as isize),
                    )
                    .unwrap();
                }
                let menu_open = (0..30).any(|_| {
                    if let Ok(menu) = unsafe { FindWindowW(w!("#32768"), None) } {
                        let mut menu_pid = 0;
                        unsafe {
                            GetWindowThreadProcessId(menu, Some(&mut menu_pid));
                        }
                        if menu_pid == app.id() && unsafe { IsWindowVisible(menu) }.as_bool() {
                            return true;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    false
                });
                assert!(
                    menu_open,
                    "Tray popup did not open for reentrant hotkey regression"
                );
                unsafe {
                    assert_eq!(SendInput(&[key_down, key_up], size_of::<INPUT>() as i32), 2);
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
                assert!(
                    unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.6),
                    "Input was not deferred during the tray menu"
                );
                unsafe {
                    SendMessageW(
                        window,
                        windows::Win32::UI::WindowsAndMessaging::WM_CANCELMODE,
                        None,
                        None,
                    );
                }
            } else {
                unsafe {
                    SendMessageW(window, WM_COMMAND, Some(WPARAM(102)), None);
                }
            }
            let restored = (0..30).any(|_| {
                if unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4) {
                    true
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    false
                }
            });
            assert!(
                restored,
                "F2 delivered during a tray menu was lost; expected 40%, actual={}",
                unsafe { volume.GetMasterVolume().unwrap() }
            );
            assert!(!unsafe { IsWindowVisible(overlay) }.as_bool());
            unsafe {
                SendMessageW(window, WM_COMMAND, Some(WPARAM(102)), None);
            }
            assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.6));
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

    fn verify_held_keys(
        volume: &ISimpleAudioVolume,
        window: windows::Win32::Foundation::HWND,
        overlay: windows::Win32::Foundation::HWND,
        available: u16,
    ) {
        use windows::Win32::{
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            System::Threading::GetCurrentThreadId,
            UI::{
                Input::KeyboardAndMouse::{
                    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
                    VIRTUAL_KEY,
                },
                WindowsAndMessaging::*,
            },
        };
        fn keys(keys: &[u16], up: bool) {
            let inputs: Vec<INPUT> = keys
                .iter()
                .map(|key| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(*key),
                            dwFlags: if up {
                                KEYEVENTF_KEYUP
                            } else {
                                Default::default()
                            },
                            ..Default::default()
                        },
                    },
                })
                .collect();
            assert_eq!(
                unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize,
                inputs.len(),
                "Windows rejected held-key test input"
            );
        }
        struct HeldKeys(Vec<u16>);
        impl Drop for HeldKeys {
            fn drop(&mut self) {
                keys(&self.0, true);
            }
        }
        // Consume test movement/modifier input in a harmless native window.
        unsafe extern "system" fn fixture_proc(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(window, message, wparam, lparam)
        }
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || unsafe {
            let class = windows::core::w!("EarplugsKeyboardFixture");
            assert_ne!(
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(fixture_proc),
                    lpszClassName: class,
                    ..Default::default()
                }),
                0
            );
            let fixture = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                windows::core::w!("Earplugs keyboard test"),
                WS_POPUP | WS_VISIBLE | WS_CAPTION,
                100,
                100,
                260,
                40,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            send.send((fixture.0 as usize, GetCurrentThreadId()))
                .unwrap();
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = DestroyWindow(fixture);
            let _ = UnregisterClassW(class, None);
        });
        let (fixture, thread_id) = receive.recv().unwrap();
        let fixture = HWND(fixture as *mut _);
        struct Fixture(u32, Option<std::thread::JoinHandle<()>>, HWND);
        impl Drop for Fixture {
            fn drop(&mut self) {
                unsafe {
                    let _ = PostThreadMessageW(self.0, WM_QUIT, WPARAM(0), LPARAM(0));
                }
                self.1.take().unwrap().join().unwrap();
                unsafe {
                    let _ = SetForegroundWindow(self.2);
                }
            }
        }
        let _fixture = Fixture(thread_id, Some(thread), unsafe { GetForegroundWindow() });
        // Test-only input-queue attachment gives the fixture foreground focus;
        // modifier/letter combinations must never be sent to the user's app.
        let foreground_thread = unsafe { GetWindowThreadProcessId(GetForegroundWindow(), None) };
        let current_thread = unsafe { GetCurrentThreadId() };
        let mut message = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        }
        let attached = unsafe {
            windows::Win32::System::Threading::AttachThreadInput(
                current_thread,
                foreground_thread,
                true,
            )
        }
        .as_bool();
        let fixture_attached = unsafe {
            windows::Win32::System::Threading::AttachThreadInput(current_thread, thread_id, true)
        }
        .as_bool();
        unsafe {
            let _ = BringWindowToTop(fixture);
            let _ = SetForegroundWindow(fixture);
        }
        let focused = unsafe { GetForegroundWindow() } == fixture;
        if fixture_attached {
            unsafe {
                let _ = windows::Win32::System::Threading::AttachThreadInput(
                    current_thread,
                    thread_id,
                    false,
                );
            }
        }
        if attached {
            unsafe {
                let _ = windows::Win32::System::Threading::AttachThreadInput(
                    current_thread,
                    foreground_thread,
                    false,
                );
            }
        }
        assert!(focused, "Could not focus safe keyboard fixture: foreground_thread={foreground_thread}, current_thread={current_thread}, attached={attached}, fixture_attached={fixture_attached}");
        std::thread::sleep(std::time::Duration::from_millis(100));
        // The first case deterministically reproduces bare-F2-only registration.
        // Skip only desktop-owned shortcuts to avoid opening unrelated programs;
        // the application input sink itself does not depend on this probe.
        let combinations: &[&[u16]] = &[
            &[0xa0, 0x57], // left Shift + W
            &[0x57, 0x41], // W + A
            &[0xa1, 0x57], // right Shift + W
            &[0xa2, 0x57], // left Ctrl + W
            &[0xa3, 0x57], // right Ctrl + W
            &[0xa4, 0x57], // left Alt + W
            &[0xa5, 0x57], // right Alt + W (AltGr on some layouts)
            &[0xa0, 0xa2, 0x57, 0x41],
            &[0xa0, 0xa4, 0x57],
            &[0xa2, 0xa4, 0x57],
            &[0xa0, 0xa2, 0xa4, 0x57],
            &[0x5b], // Win combinations omit W to avoid the OS Win+W shortcut.
            &[0x5c],
            &[0x5b, 0xa0],
            &[0x5b, 0xa2],
            &[0x5b, 0xa4],
            &[0x5b, 0xa0, 0xa2],
            &[0x5b, 0xa0, 0xa4],
            &[0x5b, 0xa2, 0xa4],
            &[0x5b, 0xa0, 0xa2, 0xa4],
        ];
        for combination in combinations {
            let modifiers = combination.iter().fold(0, |mask, key| {
                mask | match key {
                    0xa4 | 0xa5 => 1,
                    0xa2 | 0xa3 => 2,
                    0xa0 | 0xa1 => 4,
                    0x5b | 0x5c => 8,
                    _ => 0,
                }
            });
            if available & (1 << modifiers) == 0 {
                continue;
            }
            if modifiers & 8 == 0 {
                assert_eq!(
                    unsafe { GetForegroundWindow() },
                    fixture,
                    "Keyboard fixture lost focus before sending {combination:x?}"
                );
            }
            let _held = HeldKeys([*combination, &[0x71]].concat());
            keys(combination, false);
            for expected in [0.4, 0.8] {
                keys(&[0x71], false);
                keys(&[0x71], true);
                let changed = (0..30).any(|_| {
                    if unchanged(unsafe { volume.GetMasterVolume().unwrap() }, expected)
                        && unsafe { IsWindowVisible(overlay) }.as_bool() == (expected == 0.4)
                    {
                        true
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        false
                    }
                });
                assert!(
                    changed,
                    "F2 with held keys {combination:x?} failed; expected={expected}, actual={}",
                    unsafe { volume.GetMasterVolume().unwrap() }
                );
                assert_eq!(
                    unsafe { IsWindowVisible(overlay) }.as_bool(),
                    expected == 0.4
                );
                assert!(!unsafe { IsWindowVisible(window) }.as_bool());
                let foreground = unsafe { GetForegroundWindow() };
                assert!(
                    foreground != window && foreground != overlay,
                    "Earplugs took focus for held keys {combination:x?}"
                );
                // A Win-key release can open the Windows Start menu. The input
                // sink intentionally leaves OS shortcuts and key delivery alone.
                if modifiers & 8 == 0 {
                    assert_eq!(
                        foreground, fixture,
                        "Focus changed for held keys {combination:x?}"
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        // Repeated key-down events and modifier changes during a held F2 must
        // not turn one press into multiple volume changes.
        let _held = HeldKeys(vec![0x71, 0xa0, 0xa2, 0xa4, 0x5b]);
        keys(&[0x71], false);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4));
        for modifier in [0xa0, 0xa2, 0xa4, 0x5b] {
            let mask = match modifier {
                0xa0 => 4,
                0xa2 => 2,
                0xa4 => 1,
                _ => 8,
            };
            if available & (1 << mask) == 0 {
                continue;
            }
            keys(&[modifier], false);
            for _ in 0..5 {
                keys(&[0x71], false);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(
                unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4),
                "Held F2 repeated while adding modifier {modifier:x}"
            );
            keys(&[modifier], true);
            keys(&[0x71], false);
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(
                unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.4),
                "Held F2 repeated while releasing modifier {modifier:x}"
            );
        }
        keys(&[0x71], true);
        keys(&[0x71], false);
        keys(&[0x71], true);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            unchanged(unsafe { volume.GetMasterVolume().unwrap() }, 0.8),
            "A fresh F2 press did not restore after holding"
        );
        println!("Held movement keys, available modifier combinations (mask {available:04x}), left/right modifiers and F2 no-repeat passed.");
    }
}
