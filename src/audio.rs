use crate::model::{unchanged, Settings};
use std::{
    collections::{HashMap, HashSet},
    mem::size_of,
};
use windows::{
    core::{Interface, PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES},
        Media::Audio::{
            eRender, AudioSessionStateExpired, IAudioSessionControl, IAudioSessionControl2,
            IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
            AUDCLNT_E_DEVICE_INVALIDATED, DEVICE_STATE_ACTIVE,
        },
        System::{
            Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL},
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
        },
    },
};

struct ChangedSession {
    volume: ISimpleAudioVolume,
    identity: Option<SessionIdentity>,
    previous: f32,
    applied: f32,
    rollback: Option<f32>,
}

struct SessionIdentity {
    endpoint: String,
    session: String,
    instance: String,
}

struct Session {
    volume: ISimpleAudioVolume,
    identity: SessionIdentity,
}

#[derive(Default)]
pub struct Audio {
    changed: Vec<ChangedSession>,
    restoring: bool,
    // Keep completed and unwritten original instances excluded until recovery ends.
    known_instances: HashSet<String>,
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
        for session in sessions {
            let previous =
                unsafe { session.volume.GetMasterVolume() }.map_err(|e| e.to_string())?;
            pending.push(ChangedSession {
                applied: settings.target_volume(),
                previous,
                rollback: None,
                identity: Some(session.identity),
                volume: session.volume,
            });
        }
        self.known_instances = pending
            .iter()
            .filter_map(|session| session.identity.as_ref().map(|id| id.instance.clone()))
            .collect();
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
        if self.restoring
            || self
                .changed
                .iter()
                .any(|session| session.rollback.is_some())
        {
            return Err("An earlier volume update still needs recovery. Press F2 to restore before applying again.".into());
        }
        let target = settings.target_volume();
        let (known, conflicts) = self.identities();
        for session in &mut self.changed {
            refresh_session(session, &known, &conflicts, false)?;
            if let Some(identity) = &session.identity {
                self.known_instances.insert(identity.instance.clone());
            }
        }
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
                    let recovery = if unchanged(current, applied) {
                        session.previous
                    } else {
                        current
                    };
                    match unsafe { session.volume.GetMasterVolume() } {
                        Ok(latest) if !unchanged(latest, session.applied) => (),
                        Ok(_) => {
                            if unsafe { session.volume.SetMasterVolume(current, std::ptr::null()) }
                                .is_ok()
                            {
                                session.applied = applied;
                            } else {
                                session.rollback = Some(recovery);
                                rollback_failed = true;
                            }
                        }
                        Err(_) => {
                            session.rollback = Some(recovery);
                            rollback_failed = true;
                        }
                    }
                }
                return Err(format!(
                    "Cannot update game volume: {error}. {}",
                    if rollback_failed {
                        "Rollback was incomplete; press F2 to restore before exiting."
                    } else {
                        "Mixer levels rolled back; newer manual edits preserved."
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
        let (known, conflicts) = self.identities();
        let mut failed = Vec::new();
        let mut manual = 0;
        let mut reason = None;
        for mut session in self.changed.drain(..) {
            if let Err(error) = refresh_session(&mut session, &known, &conflicts, true) {
                reason.get_or_insert(error);
                failed.push(session);
                continue;
            }
            if let Some(identity) = &session.identity {
                self.known_instances.insert(identity.instance.clone());
            }
            match unsafe { session.volume.GetMasterVolume() } {
                Ok(current) if unchanged(current, session.applied) => {
                    let target = session.rollback.unwrap_or(session.previous);
                    if unchanged(current, target) {
                        continue;
                    }
                    let result =
                        unsafe { session.volume.SetMasterVolume(target, std::ptr::null()) };
                    if result.is_err() || !session_is_live(&session) {
                        failed.push(session);
                    }
                }
                Ok(_) => manual += 1,
                Err(_) => failed.push(session),
            }
        }
        self.changed = failed;
        self.restoring = !self.changed.is_empty();
        if !self.changed.is_empty() {
            return Err(reason.unwrap_or_else(|| "Windows could not restore every game session. Press F2 to retry before exiting.".into()));
        }
        self.known_instances.clear();
        Ok(if manual == 0 {
            "Earplugs OFF: previous game volume restored.".into()
        } else {
            "Earplugs OFF: preserved your manual mixer changes.".into()
        })
    }

    fn identities(&self) -> (HashSet<String>, HashSet<(String, String)>) {
        let mut known = self.known_instances.clone();
        let mut targets = HashMap::new();
        let mut conflicts = HashSet::new();
        for session in &self.changed {
            if let Some(identity) = &session.identity {
                known.insert(identity.instance.clone());
                let key = (identity.endpoint.clone(), identity.session.clone());
                let target = session.rollback.unwrap_or(session.previous);
                if targets
                    .insert(key.clone(), target)
                    .is_some_and(|old| !unchanged(old, target))
                {
                    conflicts.insert(key);
                }
            }
        }
        (known, conflicts)
    }
}

fn session_is_live(session: &ChangedSession) -> bool {
    session.identity.is_none()
        || session
            .volume
            .cast::<IAudioSessionControl>()
            .is_ok_and(|control| {
                unsafe { control.GetState() }.is_ok_and(|state| state != AudioSessionStateExpired)
            })
}

fn refresh_session(
    session: &mut ChangedSession,
    known: &HashSet<String>,
    conflicts: &HashSet<(String, String)>,
    restoring: bool,
) -> Result<(), String> {
    let Some(identity) = &session.identity else {
        return Ok(());
    };
    let control: IAudioSessionControl = session.volume.cast().map_err(|error| error.to_string())?;
    match unsafe { control.GetState() } {
        Ok(state) if state != AudioSessionStateExpired => return Ok(()),
        Err(error) if error.code() != AUDCLNT_E_DEVICE_INVALIDATED => return Err(error.to_string()),
        _ => (),
    }
    let replacement = replacement_for(identity, known)?.ok_or_else(|| "Game audio ended or its output is unavailable. Restart the game or reconnect its output, then press F2 to restore. Earplugs keeps the original volume until then.".to_string())?;
    let current =
        unsafe { replacement.volume.GetMasterVolume() }.map_err(|error| error.to_string())?;
    if conflicts.contains(&(identity.endpoint.clone(), identity.session.clone()))
        && (!restoring || unchanged(current, session.applied))
    {
        return Err("Multiple ended game instances had different original levels. Set the new game's mixer level manually, then press F2 to preserve that change.".into());
    }
    session.volume = replacement.volume;
    session.identity = Some(replacement.identity);
    Ok(())
}

unsafe fn take_string(value: PWSTR) -> Result<String, String> {
    if value.is_null() {
        return Err("Windows returned a missing audio session identifier.".into());
    }
    let result = value.to_string().map_err(|error| error.to_string());
    CoTaskMemFree(Some(value.0.cast()));
    result
}

unsafe fn session_identity(
    endpoint: &str,
    control: &IAudioSessionControl2,
) -> Result<SessionIdentity, String> {
    Ok(SessionIdentity {
        endpoint: endpoint.into(),
        session: take_string(
            control
                .GetSessionIdentifier()
                .map_err(|error| error.to_string())?,
        )?,
        instance: take_string(
            control
                .GetSessionInstanceIdentifier()
                .map_err(|error| error.to_string())?,
        )?,
    })
}

fn replacement_for(
    identity: &SessionIdentity,
    known: &HashSet<String>,
) -> Result<Option<Session>, String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|error| error.to_string())?;
        let endpoint: Vec<u16> = identity.endpoint.encode_utf16().chain(Some(0)).collect();
        let device = enumerator
            .GetDevice(PCWSTR(endpoint.as_ptr()))
            .map_err(|error| error.to_string())?;
        let manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|error| error.to_string())?;
        let list = manager
            .GetSessionEnumerator()
            .map_err(|error| error.to_string())?;
        let mut replacement = None;
        for index in 0..list.GetCount().map_err(|error| error.to_string())? {
            let control = list.GetSession(index).map_err(|error| error.to_string())?;
            if control.GetState().map_err(|error| error.to_string())? == AudioSessionStateExpired {
                continue;
            }
            let control2: IAudioSessionControl2 =
                control.cast().map_err(|error| error.to_string())?;
            let current = session_identity(&identity.endpoint, &control2)?;
            if current.session == identity.session && !known.contains(&current.instance) {
                if replacement.is_some() {
                    return Err("Multiple replacement game sessions exist. Close extra game instances, then press F2 to restore.".into());
                }
                replacement = Some(Session {
                    volume: control.cast().map_err(|error| error.to_string())?,
                    identity: current,
                });
            }
        }
        Ok(replacement)
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
        if let Err(error) = result {
            if error.code() != windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                return Err(format!("Windows process enumeration failed: {error}"));
            }
        }
        Ok(ids)
    }
}

fn sessions_for(settings: &Settings) -> Result<Vec<Session>, String> {
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
            let endpoint = take_string(device.GetId().map_err(|error| error.to_string())?)?;
            let manager = match device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                Ok(manager) => manager,
                Err(error) if error.code() == AUDCLNT_E_DEVICE_INVALIDATED => continue,
                Err(error) => {
                    return Err(format!("Cannot inspect a Windows output device: {error}"))
                }
            };
            let list = manager.GetSessionEnumerator().map_err(|e| e.to_string())?;
            for index in 0..list.GetCount().map_err(|e| e.to_string())? {
                let control = list.GetSession(index).map_err(|e| e.to_string())?;
                let control2: IAudioSessionControl2 = control.cast().map_err(|e| e.to_string())?;
                if ids.contains(&control2.GetProcessId().map_err(|e| e.to_string())?)
                    && control.GetState().map_err(|e| e.to_string())? != AudioSessionStateExpired
                {
                    sessions.push(Session {
                        volume: control.cast().map_err(|e| e.to_string())?,
                        identity: session_identity(&endpoint, &control2)?,
                    });
                }
            }
        }
        Ok(sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    use windows::Win32::Media::Audio::{IAudioSessionControl, ISimpleAudioVolume_Impl};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    static AUDIO_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct MockState {
        current: Cell<f32>,
        fail_value: Cell<Option<f32>>,
        on_failure: RefCell<Option<Box<dyn FnOnce()>>>,
    }

    #[windows::core::implement(ISimpleAudioVolume)]
    struct MockVolume {
        state: Rc<MockState>,
    }

    impl ISimpleAudioVolume_Impl for MockVolume_Impl {
        fn SetMasterVolume(
            &self,
            value: f32,
            _: *const windows::core::GUID,
        ) -> windows::core::Result<()> {
            if self
                .state
                .fail_value
                .get()
                .is_some_and(|fail| unchanged(fail, value))
            {
                self.state.fail_value.set(None);
                if let Some(callback) = self.state.on_failure.borrow_mut().take() {
                    callback();
                }
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
            self.state.current.set(value);
            Ok(())
        }
        fn GetMasterVolume(&self) -> windows::core::Result<f32> {
            Ok(self.state.current.get())
        }
        fn SetMute(
            &self,
            _: windows::core::BOOL,
            _: *const windows::core::GUID,
        ) -> windows::core::Result<()> {
            Ok(())
        }
        fn GetMute(&self) -> windows::core::Result<windows::core::BOOL> {
            Ok(false.into())
        }
    }

    fn mock_session(current: f32, previous: f32, applied: f32) -> (ChangedSession, Rc<MockState>) {
        let state = Rc::new(MockState {
            current: Cell::new(current),
            fail_value: Cell::new(None),
            on_failure: RefCell::new(None),
        });
        let volume: ISimpleAudioVolume = MockVolume {
            state: Rc::clone(&state),
        }
        .into();
        (
            ChangedSession {
                volume,
                identity: None,
                previous,
                applied,
                rollback: None,
            },
            state,
        )
    }

    #[test]
    fn failed_apply_does_not_overwrite_a_new_manual_change() {
        let (first, first_state) = mock_session(0.4, 0.8, 0.4);
        let (second, second_state) = mock_session(0.4, 0.8, 0.4);
        second_state.fail_value.set(Some(0.6));
        let changed_manually = Rc::clone(&first_state);
        *second_state.on_failure.borrow_mut() =
            Some(Box::new(move || changed_manually.current.set(0.25)));
        let mut audio = Audio {
            changed: vec![first, second],
            ..Audio::default()
        };
        assert!(audio
            .reapply(&Settings::parse("60", "wardogs").unwrap())
            .is_err());
        assert!(
            unchanged(first_state.current.get(), 0.25),
            "Apply rollback overwrote a newer manual mixer change; actual={}",
            first_state.current.get()
        );
        audio.restore().unwrap();
        assert!(unchanged(first_state.current.get(), 0.25));
    }

    #[test]
    fn failed_rollback_remembers_its_manual_recovery_volume() {
        let (first, first_state) = mock_session(0.25, 0.8, 0.4);
        let (second, second_state) = mock_session(0.4, 0.8, 0.4);
        first_state.fail_value.set(Some(0.25));
        second_state.fail_value.set(Some(0.6));
        let mut audio = Audio {
            changed: vec![first, second],
            ..Audio::default()
        };
        assert!(audio
            .reapply(&Settings::parse("60", "wardogs").unwrap())
            .is_err());
        assert!(unchanged(first_state.current.get(), 0.6));
        audio.restore().unwrap();
        assert!(
            unchanged(first_state.current.get(), 0.25),
            "Retry lost the pre-Apply manual mixer level; actual={}",
            first_state.current.get()
        );
    }

    #[test]
    fn failed_rollback_of_owned_volume_restores_original() {
        let (first, first_state) = mock_session(0.4, 0.8, 0.4);
        let (second, second_state) = mock_session(0.4, 0.8, 0.4);
        first_state.fail_value.set(Some(0.4));
        second_state.fail_value.set(Some(0.6));
        let mut audio = Audio {
            changed: vec![first, second],
            ..Audio::default()
        };
        assert!(audio
            .reapply(&Settings::parse("60", "wardogs").unwrap())
            .is_err());
        audio.restore().unwrap();
        assert!(
            unchanged(first_state.current.get(), 0.8),
            "Recovery reported OFF but left the owned volume lowered; actual={}",
            first_state.current.get()
        );
    }
    #[test]
    fn partial_restoration_cannot_be_overwritten_by_apply() {
        let (first, first_state) = mock_session(0.4, 0.8, 0.4);
        let (second, second_state) = mock_session(0.4, 0.8, 0.4);
        first_state.fail_value.set(Some(0.8));
        let mut audio = Audio {
            changed: vec![first, second],
            ..Audio::default()
        };
        assert!(audio.restore().is_err());
        assert!(unchanged(second_state.current.get(), 0.8));
        let update = audio.reapply(&Settings::parse("60", "wardogs").unwrap());
        assert!(
            update.is_err(),
            "Apply overwrote a pending partial restoration"
        );
        assert!(unchanged(first_state.current.get(), 0.4));
        audio.restore().unwrap();
        assert!(unchanged(first_state.current.get(), 0.8));
    }

    #[test]
    #[ignore = "Requires an active Windows audio output; first build the wardogs-test-tone example"]
    fn expired_session_restores_persisted_volume() {
        let _serial = AUDIO_TEST.lock().unwrap();
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
        let mut child = std::process::Command::new(&fixture).spawn().unwrap();
        let settings = Settings::parse("40", "wardogs-test-tone.exe").unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let original = (0..50)
                .find_map(|_| {
                    let volume = sessions_for(&settings).unwrap().into_iter().next();
                    if volume.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    volume
                })
                .expect("Silent fixture did not create an audio session");
            let original_identity = original.identity;
            let volume = original.volume;
            unsafe {
                volume.SetMasterVolume(0.8, std::ptr::null()).unwrap();
            }
            let mut audio = Audio::default();
            audio.toggle(&settings).unwrap();
            child.kill().unwrap();
            child.wait().unwrap();
            let control: IAudioSessionControl = volume.cast().unwrap();
            let expired = (0..50).any(|_| {
                if unsafe { control.GetState().unwrap() } == AudioSessionStateExpired {
                    true
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    false
                }
            });
            assert!(expired, "Fixture session did not expire after process exit");
            assert!(
                audio.restore().is_err(),
                "Ended session must remain pending until a live replacement exists"
            );
            assert!(audio.is_lowered());
            drop(control);
            drop(volume);
            child = std::process::Command::new(&fixture).spawn().unwrap();
            let restarted = (0..50)
                .find_map(|_| {
                    let volume = sessions_for(&settings).unwrap().into_iter().next();
                    if volume.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    volume
                })
                .expect("Restarted fixture did not create a mixer session");
            assert_eq!(restarted.identity.endpoint, original_identity.endpoint);
            assert_eq!(restarted.identity.session, original_identity.session);
            assert_ne!(restarted.identity.instance, original_identity.instance);
            assert!(unchanged(
                unsafe { restarted.volume.GetMasterVolume().unwrap() },
                0.4
            ));
            audio.restore().unwrap();
            assert!(!audio.is_lowered());
            assert!(
                unchanged(unsafe { restarted.volume.GetMasterVolume().unwrap() }, 0.8),
                "Live replacement did not restore the original volume"
            );
            child.kill().unwrap();
            child.wait().unwrap();
            drop(restarted);
            child = std::process::Command::new(&fixture).spawn().unwrap();
            let inherited = (0..50)
                .find_map(|_| {
                    let session = sessions_for(&settings).unwrap().into_iter().next();
                    if session.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    session
                })
                .expect("Final fixture did not create a session");
            assert!(
                unchanged(unsafe { inherited.volume.GetMasterVolume().unwrap() }, 0.8),
                "The final restart did not inherit the restored 80% level"
            );
            audio.toggle(&settings).unwrap();
            child.kill().unwrap();
            child.wait().unwrap();
            drop(inherited);
            child = std::process::Command::new(&fixture).spawn().unwrap();
            let replacement = (0..50)
                .find_map(|_| {
                    let session = sessions_for(&settings).unwrap().into_iter().next();
                    if session.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    session
                })
                .expect("Replacement fixture did not create a session");
            audio
                .reapply(&Settings::parse("60", "wardogs-test-tone.exe").unwrap())
                .unwrap();
            assert!(
                unchanged(
                    unsafe { replacement.volume.GetMasterVolume().unwrap() },
                    0.6
                ),
                "Apply did not update the restarted game's live session"
            );
            unsafe {
                replacement
                    .volume
                    .SetMasterVolume(0.25, std::ptr::null())
                    .unwrap();
            }
            audio.restore().unwrap();
            assert!(
                unchanged(
                    unsafe { replacement.volume.GetMasterVolume().unwrap() },
                    0.25
                ),
                "Restoration overwrote a manual change on a replacement session"
            );
        }));
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

    #[test]
    #[ignore = "Requires an active Windows audio output; first build the wardogs-test-tone example"]
    fn restored_instance_is_not_reused_for_another_session() {
        let _serial = AUDIO_TEST.lock().unwrap();
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        let fixture = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples/wardogs-test-tone.exe");
        let settings = Settings::parse("40", "wardogs-test-tone.exe").unwrap();
        let mut children = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            children.push(std::process::Command::new(&fixture).spawn().unwrap());
            let first = wait_for_fixture(&settings, children[0].id());
            unsafe { first.volume.SetMasterVolume(0.4, std::ptr::null()).unwrap() };
            children.push(std::process::Command::new(&fixture).spawn().unwrap());
            let second = wait_for_fixture(&settings, children[1].id());
            assert_eq!(first.identity.endpoint, second.identity.endpoint);
            assert_eq!(first.identity.session, second.identity.session);
            assert_ne!(first.identity.instance, second.identity.instance);
            unsafe {
                second
                    .volume
                    .SetMasterVolume(0.8, std::ptr::null())
                    .unwrap()
            };
            let mut audio = Audio::default();
            audio.toggle(&settings).unwrap();
            children[1].kill().unwrap();
            children[1].wait().unwrap();
            let control: IAudioSessionControl = second.volume.cast().unwrap();
            assert!((0..50).any(|_| {
                if unsafe { control.GetState().unwrap() } == AudioSessionStateExpired {
                    true
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    false
                }
            }));
            assert!(audio.restore().is_err());
            assert!(
                audio.restore().is_err(),
                "A completed original instance was mistaken for another instance's replacement"
            );
            assert!(audio.is_lowered());
            assert!(unchanged(
                unsafe { first.volume.GetMasterVolume().unwrap() },
                0.4
            ));
            children.push(std::process::Command::new(&fixture).spawn().unwrap());
            let replacement = wait_for_fixture(&settings, children[2].id());
            audio.restore().unwrap();
            assert!(!audio.is_lowered());
            assert!(unchanged(
                unsafe { replacement.volume.GetMasterVolume().unwrap() },
                0.8
            ));
            assert!(unchanged(
                unsafe { first.volume.GetMasterVolume().unwrap() },
                0.4
            ));
            audio.toggle(&settings).unwrap();
            for index in [0, 2] {
                children[index].kill().unwrap();
                children[index].wait().unwrap();
            }
            for volume in [&first.volume, &replacement.volume] {
                let control: IAudioSessionControl = volume.cast().unwrap();
                assert!((0..50).any(|_| {
                    if unsafe { control.GetState().unwrap() } == AudioSessionStateExpired {
                        true
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        false
                    }
                }));
            }
            children.push(std::process::Command::new(&fixture).spawn().unwrap());
            let manual = wait_for_fixture(&settings, children[3].id());
            unsafe {
                manual
                    .volume
                    .SetMasterVolume(0.25, std::ptr::null())
                    .unwrap()
            };
            assert!(
                audio
                    .reapply(&Settings::parse("60", "wardogs-test-tone.exe").unwrap())
                    .is_err(),
                "Apply assigned conflicting original volumes to one replacement instance"
            );
            assert!(unchanged(
                unsafe { manual.volume.GetMasterVolume().unwrap() },
                0.25
            ));
            audio.restore().unwrap();
            assert!(!audio.is_lowered());
            assert!(unchanged(
                unsafe { manual.volume.GetMasterVolume().unwrap() },
                0.25
            ));
        }));
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(
                std::env::temp_dir().join(format!("wardogs-silent-fixture-{}.wav", child.id())),
            );
        }
        unsafe { CoUninitialize() };
        result.unwrap();
    }

    fn wait_for_fixture(settings: &Settings, pid: u32) -> Session {
        (0..50)
            .find_map(|_| {
                let session = sessions_for(settings).unwrap().into_iter().find(|session| {
                    let control: IAudioSessionControl2 = session.volume.cast().unwrap();
                    (unsafe { control.GetProcessId().unwrap() }) == pid
                });
                if session.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                session
            })
            .expect("Silent fixture did not create its Windows audio session")
    }

    #[test]
    #[ignore = "Requires an active Windows audio output; first build the wardogs-test-tone example"]
    fn real_windows_mixer_roundtrip() {
        let _serial = AUDIO_TEST.lock().unwrap();
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
            let volume = &sessions[0].volume;
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
            // Closing while a tray menu holds App used to destroy the HWND
            // through DefWindowProc, stranding the process and its audio state.
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
                "Tray menu did not open for nested-close regression"
            );
            unsafe {
                SendMessageW(
                    window,
                    windows::Win32::UI::WindowsAndMessaging::WM_APP + 1,
                    Some(WPARAM(0)),
                    Some(LPARAM(
                        windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP as isize,
                    )),
                );
                SendMessageW(
                    window,
                    windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    None,
                    None,
                );
            }
            assert!(unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(window)) }.as_bool(), "Closing during the tray menu destroyed the app window and stranded the background process");
            assert!(app.try_wait().unwrap().is_none());
            let dismissed = (0..20).any(|_| {
                let open = unsafe { FindWindowW(w!("#32768"), None) }
                    .ok()
                    .is_some_and(|menu| {
                        let mut menu_pid = 0;
                        unsafe {
                            GetWindowThreadProcessId(menu, Some(&mut menu_pid));
                        }
                        menu_pid == app.id() && unsafe { IsWindowVisible(menu) }.as_bool()
                    });
                if !open {
                    true
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    false
                }
            });
            assert!(
                dismissed,
                "A queued tray menu reopened ahead of Close, blocking the close request"
            );
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
