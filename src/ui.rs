use crate::{
    audio::Audio,
    model::{F2Press, Settings},
    overlay::Overlay,
    settings, update,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::PathBuf,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT,
            WAIT_OBJECT_0, WPARAM,
        },
        Graphics::Gdi::{GetStockObject, COLOR_WINDOW, DEFAULT_GUI_FONT, HBRUSH},
        System::{
            Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
            RemoteDesktop::{WTSRegisterSessionNotification, WTSUnRegisterSessionNotification},
            Threading::{CreateEventW, CreateMutexW, ResetEvent, SetEvent, WaitForSingleObject},
        },
        UI::{
            Controls::EM_SETLIMITTEXT,
            Input::{
                GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
                RAWINPUTHEADER, RAWKEYBOARD, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDEV_REMOVE,
                RID_INPUT, RIM_TYPEKEYBOARD,
            },
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::*,
        },
    },
};

const CLASS: PCWSTR = w!("WarDogsEarplugsWindow");
const TRAY_MESSAGE: u32 = WM_APP + 1;
const ID_SAVE: usize = 101;
const ID_TOGGLE: usize = 102;
const ID_HIDE: usize = 103;
const ID_EXIT: usize = 104;
const ID_SHOW: usize = 105;
const ID_PERCENT: usize = 201;
const ID_EXECUTABLE: usize = 202;

struct App {
    window: HWND,
    percent: HWND,
    executable: HWND,
    status: HWND,
    settings: Settings,
    settings_path: PathBuf,
    audio: Audio,
    overlay: Overlay,
    tray: NOTIFYICONDATAW,
    tray_ready: bool,
    input_ready: bool,
    session_notifications: bool,
    quit_requested: bool,
    updater: Option<update::Updater>,
}

struct WindowState {
    app: RefCell<App>,
    pending: RefCell<VecDeque<Action>>,
    f2_press: RefCell<F2Press>,
    taskbar_created: u32,
    window_gone: Cell<bool>,
}

enum Action {
    Apply,
    Toggle,
    Hide,
    Exit,
    Show,
    Menu,
    RecreateTray,
    EndSession,
    Update,
}

impl WindowState {
    fn enqueue(&self, action: Action) {
        let mut pending = self.pending.borrow_mut();
        if matches!(action, Action::Menu)
            && pending
                .iter()
                .any(|queued| matches!(queued, Action::Hide | Action::Exit | Action::EndSession))
        {
            return;
        }
        if matches!(action, Action::Hide | Action::Exit | Action::EndSession) {
            pending.retain(|queued| !matches!(queued, Action::Menu));
        }
        if matches!(action, Action::EndSession) {
            pending.push_front(action);
        } else if matches!(action, Action::Hide | Action::Exit) {
            let index = usize::from(matches!(pending.front(), Some(Action::EndSession)));
            pending.insert(index, action);
        } else {
            pending.push_back(action);
        }
    }

    unsafe fn drain_actions(&self) {
        let Ok(mut app) = self.app.try_borrow_mut() else {
            return;
        };
        loop {
            if self.window_gone.get() || app.quit_requested {
                self.pending.borrow_mut().clear();
                return;
            }
            // Drop the queue borrow before any action can pump nested messages.
            let action = { self.pending.borrow_mut().pop_front() };
            let Some(action) = action else {
                return;
            };
            match action {
                Action::Toggle => app.toggle(),
                Action::Apply => match app.save(true) {
                    Ok(status) => app.set_status(&status),
                    Err(error) => app.set_status(&error),
                },
                Action::Hide => app.hide(),
                Action::Exit => app.exit(),
                Action::Show => show_window(app.window),
                Action::Update => app.update(),
                Action::Menu => app.menu(),
                Action::RecreateTray => {
                    app.tray_ready = Shell_NotifyIconW(NIM_ADD, &app.tray).as_bool();
                    if !app.tray_ready {
                        show_window(app.window);
                    }
                }
                Action::EndSession => {
                    let _ = app.audio.restore();
                    app.overlay.set_on(false);
                    app.quit_requested = true;
                    PostQuitMessage(0);
                }
            }
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

unsafe fn application_icon(instance: HINSTANCE, small: bool) -> Result<HICON, String> {
    let (width, height) = if small {
        (SM_CXSMICON, SM_CYSMICON)
    } else {
        (SM_CXICON, SM_CYICON)
    };
    LoadImageW(
        Some(instance),
        PCWSTR(std::ptr::without_provenance(if small { 2 } else { 1 })),
        IMAGE_ICON,
        GetSystemMetrics(width),
        GetSystemMetrics(height),
        LR_SHARED,
    )
    .map(|icon| HICON(icon.0))
    .or_else(|_| LoadIconW(None, IDI_APPLICATION))
    .map_err(|e| e.to_string())
}

pub fn startup_error(error: &str) {
    let message = wide(error);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("WarDogs Earplugs"),
            MB_OK | MB_ICONERROR,
        );
    }
}

struct Handle(windows::Win32::Foundation::HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

pub fn run() -> Result<(), String> {
    unsafe {
        let mutex =
            CreateMutexW(None, false, w!("Local\\WarDogsEarplugs")).map_err(|e| e.to_string())?;
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        let _mutex = Handle(mutex);
        let ready = Handle(
            CreateEventW(None, true, false, w!("Local\\WarDogsEarplugsReady"))
                .map_err(|e| e.to_string())?,
        );
        if already_running {
            // Also support reopening an initialized older version without a ready event.
            if let Ok(window) = FindWindowW(CLASS, None) {
                if GetWindowLongPtrW(window, GWLP_USERDATA) != 0 {
                    show_window(window);
                    return Ok(());
                }
            }
            if WaitForSingleObject(ready.0, 5000) == WAIT_OBJECT_0 {
                let window = FindWindowW(CLASS, None).map_err(|_| {
                    "The running copy exited during startup. Start Earplugs again.".to_string()
                })?;
                show_window(window);
                return Ok(());
            }
            return Err("The running copy did not finish starting within five seconds. Try opening it again.".into());
        }
        ResetEvent(ready.0).map_err(|e| e.to_string())?;
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let _com = Com;
        let settings_path = settings::path().map_err(|e| e.to_string())?;
        let (settings, initial_status, settings_warning) = match settings::load(&settings_path) {
            Ok(settings) => (
                settings,
                "F2 keeps the chosen % of original volume. Apply updates it while ON.".to_string(),
                false,
            ),
            Err(error) => (
                Settings::default(),
                format!("Settings could not be loaded; using defaults. {error}"),
                true,
            ),
        };
        let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
            hIcon: application_icon(instance, false)?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_win32().to_string());
        }
        let overlay = Overlay::new(instance)?;
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("WarDogs Earplugs"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            464,
            344,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| e.to_string())?;
        let mut state = Box::new(WindowState {
            app: RefCell::new(App {
                window,
                percent: HWND::default(),
                executable: HWND::default(),
                status: HWND::default(),
                settings,
                settings_path,
                audio: Audio::default(),
                overlay,
                tray: NOTIFYICONDATAW::default(),
                tray_ready: false,
                input_ready: false,
                session_notifications: false,
                quit_requested: false,
                updater: None,
            }),
            pending: RefCell::new(VecDeque::new()),
            f2_press: RefCell::new(F2Press::default()),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            window_gone: Cell::new(false),
        });
        if let Err(error) = state.app.get_mut().initialize(instance, &initial_status) {
            let _ = DestroyWindow(window);
            return Err(error);
        }
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            (&*state as *const WindowState) as isize,
        );
        let _ = SetEvent(ready.0);
        let background = std::env::args_os().skip(1).any(|arg| arg == "--background");
        if !background
            || settings_warning
            || !state.app.borrow().tray_ready
            || !state.app.borrow().input_ready
        {
            show_window(window);
        }
        state.app.get_mut().updater = update::Updater::start(window).ok();
        let mut message = MSG::default();
        let result = loop {
            let received = GetMessageW(&mut message, None, 0, 0).0;
            if received == -1 {
                break Err(windows::core::Error::from_win32().to_string());
            }
            if received == 0 {
                break Ok(());
            }
            if !IsDialogMessageW(window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        };
        // State stays alive until the HWND can no longer dispatch callbacks.
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        let _ = state.app.get_mut().audio.restore();
        if state.app.get_mut().session_notifications {
            let _ = WTSUnRegisterSessionNotification(window);
        }
        if state.app.get_mut().tray_ready {
            let _ = Shell_NotifyIconW(NIM_DELETE, &state.app.get_mut().tray);
        }
        if state.app.get_mut().input_ready {
            let _ = RegisterRawInputDevices(
                &[RAWINPUTDEVICE {
                    usUsagePage: 1,
                    usUsage: 6,
                    dwFlags: RIDEV_REMOVE,
                    hwndTarget: HWND::default(),
                }],
                std::mem::size_of::<RAWINPUTDEVICE>() as u32,
            );
        }
        let _ = DestroyWindow(window);
        result
    }
}

unsafe fn show_window(window: HWND) {
    let _ = ShowWindow(window, SW_RESTORE);
    let _ = SetForegroundWindow(window);
}

impl App {
    unsafe fn update(&mut self) {
        let event = self.updater.as_mut().and_then(update::Updater::receive);
        match event {
            Some(update::Event::Checked(Ok(Some(release)))) => {
                let prompt = wide(&format!("Hay una nueva versión de WarDogs Earplugs: {} (actual: {}).\n\n¿Quieres descargarla e instalarla ahora? La aplicación se reiniciará y conservará tus ajustes.", release.version, env!("CARGO_PKG_VERSION")));
                if MessageBoxW(
                    Some(self.window),
                    PCWSTR(prompt.as_ptr()),
                    w!("Actualización disponible"),
                    MB_YESNO | MB_ICONINFORMATION | MB_DEFBUTTON2,
                ) == IDYES
                {
                    self.set_status("Descargando actualización de GitHub...");
                    if let Some(updater) = &mut self.updater {
                        if let Err(error) = updater.download(release) {
                            self.update_error(&error);
                        }
                    }
                }
            }
            Some(update::Event::Downloaded(Ok(mut staged))) => {
                let restored = self.audio.restore();
                self.overlay.set_on(self.audio.is_lowered());
                if let Err(error) = restored {
                    self.update_error(&format!(
                        "Antes de actualizar, restaura el volumen con F2. {error}"
                    ));
                    return;
                }
                match staged.handoff(!IsWindowVisible(self.window).as_bool()) {
                    Ok(()) => {
                        self.quit_requested = true;
                        PostQuitMessage(0);
                    }
                    Err(error) => self.update_error(&error),
                }
            }
            Some(update::Event::Downloaded(Err(error))) => self.update_error(&error),
            Some(update::Event::Notice(notice)) => self.update_error(&notice),
            // Offline, missing releases and an up-to-date copy remain unobtrusive.
            Some(update::Event::Checked(_)) | None => (),
        }
    }

    unsafe fn update_error(&self, error: &str) {
        self.set_status(error);
        let text = wide(error);
        MessageBoxW(
            Some(self.window),
            PCWSTR(text.as_ptr()),
            w!("No se pudo actualizar"),
            MB_OK | MB_ICONERROR,
        );
    }

    unsafe fn initialize(&mut self, instance: HINSTANCE, status: &str) -> Result<(), String> {
        self.control(
            instance,
            "STATIC",
            "F2: lower / restore WarDogs audio",
            20,
            16,
            410,
            24,
            0,
            0,
        )?;
        self.control(
            instance,
            "STATIC",
            "Original volume kept (%)",
            20,
            56,
            230,
            24,
            0,
            0,
        )?;
        self.percent = self.control(
            instance,
            "EDIT",
            &self.settings.percent.to_string(),
            270,
            52,
            140,
            26,
            ID_PERCENT,
            ES_NUMBER as u32 | WS_BORDER.0 | WS_TABSTOP.0,
        )?;
        SendMessageW(self.percent, EM_SETLIMITTEXT, Some(WPARAM(3)), None);
        self.control(
            instance,
            "STATIC",
            "Game executable contains",
            20,
            96,
            230,
            24,
            0,
            0,
        )?;
        self.executable = self.control(
            instance,
            "EDIT",
            &self.settings.executable,
            270,
            92,
            140,
            26,
            ID_EXECUTABLE,
            ES_AUTOHSCROLL as u32 | WS_BORDER.0 | WS_TABSTOP.0,
        )?;
        SendMessageW(self.executable, EM_SETLIMITTEXT, Some(WPARAM(100)), None);
        self.control(
            instance,
            "STATIC",
            "Use a full .exe name for an exact match. Default: wardogs",
            20,
            130,
            410,
            22,
            0,
            0,
        )?;
        self.control(
            instance,
            "BUTTON",
            "Apply",
            20,
            166,
            80,
            30,
            ID_SAVE,
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
        )?;
        self.control(
            instance,
            "BUTTON",
            "Toggle (F2)",
            110,
            166,
            112,
            30,
            ID_TOGGLE,
            WS_TABSTOP.0,
        )?;
        self.control(
            instance,
            "BUTTON",
            "Hide",
            232,
            166,
            80,
            30,
            ID_HIDE,
            WS_TABSTOP.0,
        )?;
        self.control(
            instance,
            "BUTTON",
            "Exit",
            322,
            166,
            88,
            30,
            ID_EXIT,
            WS_TABSTOP.0,
        )?;
        self.status = self.control(instance, "STATIC", status, 20, 212, 410, 60, 0, 0)?;
        self.control(
            instance,
            "STATIC",
            &format!("v{} | MIT", env!("CARGO_PKG_VERSION")),
            20,
            282,
            410,
            20,
            0,
            0,
        )?;
        // Complete every fallible resource lookup before registering input.
        self.tray.hIcon = application_icon(instance, true)?;
        SendMessageW(
            self.window,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(self.tray.hIcon.0 as isize)),
        );
        // A background input sink observes press/release events without taking
        // exclusive ownership of a shortcut, hooking input or disabling legacy keys.
        self.input_ready = RegisterRawInputDevices(
            &[RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: 6,
                dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                hwndTarget: self.window,
            }],
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        )
        .is_ok();
        self.session_notifications = WTSRegisterSessionNotification(self.window, 0).is_ok();
        self.tray.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        self.tray.hWnd = self.window;
        self.tray.uID = 1;
        self.tray.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        self.tray.uCallbackMessage = TRAY_MESSAGE;
        let tip = wide("WarDogs Earplugs | F2 lower / restore");
        self.tray.szTip[..tip.len()].copy_from_slice(&tip);
        self.tray_ready = Shell_NotifyIconW(NIM_ADD, &self.tray).as_bool();
        if !self.input_ready {
            self.set_status("Windows keyboard input registration failed. Restart Earplugs. The Toggle button still works.");
        } else if !self.tray_ready {
            self.set_status(
                "Windows could not create the tray icon. Keep this window open; F2 still works.",
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn control(
        &self,
        instance: HINSTANCE,
        class: &str,
        text: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        id: usize,
        extra_style: u32,
    ) -> Result<HWND, String> {
        let class = wide(class);
        let text = wide(text);
        let control = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR(text.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | extra_style),
            x,
            y,
            width,
            height,
            Some(self.window),
            Some(HMENU(id as *mut _)),
            Some(instance),
            None,
        )
        .map_err(|e| e.to_string())?;
        SendMessageW(
            control,
            WM_SETFONT,
            Some(WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize)),
            Some(LPARAM(1)),
        );
        Ok(control)
    }

    unsafe fn set_status(&self, text: &str) {
        let text = wide(text);
        let _ = SetWindowTextW(self.status, PCWSTR(text.as_ptr()));
    }

    unsafe fn input(&self, window: HWND) -> String {
        let mut buffer = [0u16; 256];
        let length = GetWindowTextW(window, &mut buffer);
        String::from_utf16_lossy(&buffer[..length as usize])
    }

    unsafe fn save(&mut self, force: bool) -> Result<String, String> {
        let new_settings = Settings::parse(&self.input(self.percent), &self.input(self.executable))
            .map_err(str::to_string)?;
        let preferences_saved = force || new_settings != self.settings;
        if preferences_saved {
            settings::save(&self.settings_path, &new_settings)
                .map_err(|e| format!("Could not save settings: {e}"))?;
        }
        let result = if self.audio.is_lowered() {
            if new_settings.executable != self.settings.executable {
                self.audio
                    .restore()
                    .and_then(|_| self.audio.toggle(&new_settings))
            } else if force || new_settings.percent != self.settings.percent {
                self.audio.reapply(&new_settings)
            } else {
                Ok("Settings saved.".into())
            }
        } else {
            Ok("Settings saved. F2 keeps this percentage of the original game volume.".into())
        };
        if result.is_ok() {
            self.settings = new_settings;
        }
        self.overlay.set_on(self.audio.is_lowered());
        result.map_err(|error| {
            if preferences_saved {
                format!("Preferences saved; {error}")
            } else {
                error
            }
        })
    }

    unsafe fn toggle(&mut self) {
        let result = if self.audio.is_lowered() {
            self.audio.restore()
        } else {
            self.save(false)
                .and_then(|_| self.audio.toggle(&self.settings))
        };
        self.overlay.set_on(self.audio.is_lowered());
        match result {
            Ok(status) => self.set_status(&status),
            Err(error) => {
                self.set_status(&error);
                show_window(self.window);
            }
        }
    }

    unsafe fn hide(&self) {
        if self.tray_ready {
            let _ = ShowWindow(self.window, SW_HIDE);
        } else {
            self.set_status("Cannot hide without a tray icon. Use Minimize or Exit.");
        }
    }

    unsafe fn exit(&mut self) {
        let result = self.audio.restore();
        self.overlay.set_on(self.audio.is_lowered());
        if let Err(error) = result {
            self.set_status(&error);
            show_window(self.window);
            return;
        }
        self.quit_requested = true;
        PostQuitMessage(0);
    }

    unsafe fn menu(&mut self) {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING, ID_SHOW, w!("Show settings"));
        let _ = AppendMenuW(menu, MF_STRING, ID_TOGGLE, w!("Toggle earplugs (F2)"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Restore and exit"));
        let mut point = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(self.window);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            self.window,
            None,
        )
        .0 as usize;
        let _ = DestroyMenu(menu);
        let _ = PostMessageW(Some(self.window), WM_NULL, WPARAM(0), LPARAM(0));
        match selected {
            ID_SHOW => show_window(self.window),
            ID_TOGGLE => self.toggle(),
            ID_EXIT => self.exit(),
            _ => (),
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *const WindowState;
    if pointer.is_null() {
        return DefWindowProcW(window, message, wparam, lparam);
    }
    let state = &*pointer;
    let result = dispatch_message(state, window, message, wparam, lparam);
    // A tray menu or COM call can pump nested messages while App is borrowed.
    // Drain after the outer handler releases it, without reposting or polling.
    state.drain_actions();
    result
}

unsafe fn dispatch_message(
    state: &WindowState,
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_INPUT {
        if let Some((device, key, down)) = keyboard_event(lparam) {
            let toggle = state.f2_press.borrow_mut().event(device, key, down);
            if toggle {
                state.enqueue(Action::Toggle);
            }
        }
        // Foreground raw-input messages require Windows' default cleanup.
        return DefWindowProcW(window, message, wparam, lparam);
    }
    if message == WM_INPUT_DEVICE_CHANGE && wparam.0 == GIDC_REMOVAL as usize {
        state.f2_press.borrow_mut().remove(lparam.0 as usize);
        return LRESULT(0);
    }
    if (message == WM_POWERBROADCAST
        && matches!(
            wparam.0 as u32,
            PBT_APMSUSPEND | PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL
        ))
        || (message == WM_WTSSESSION_CHANGE
            && matches!(
                wparam.0 as u32,
                WTS_SESSION_LOCK
                    | WTS_SESSION_UNLOCK
                    | WTS_CONSOLE_DISCONNECT
                    | WTS_REMOTE_DISCONNECT
            ))
    {
        state.f2_press.borrow_mut().reset();
        return LRESULT(1);
    }
    if message == WM_QUERYENDSESSION {
        return LRESULT(1);
    }
    if message == WM_NCDESTROY {
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        state.window_gone.set(true);
        PostQuitMessage(0);
        return DefWindowProcW(window, message, wparam, lparam);
    }
    // Queue actions independently of App, including during menus and COM callbacks.
    let action = if message == update::MESSAGE {
        Some(Action::Update)
    } else if message == state.taskbar_created && state.taskbar_created != 0 {
        Some(Action::RecreateTray)
    } else {
        match message {
            WM_COMMAND if (wparam.0 >> 16) == BN_CLICKED as usize => match wparam.0 & 0xffff {
                ID_SAVE => Some(Action::Apply),
                ID_TOGGLE => Some(Action::Toggle),
                ID_HIDE => Some(Action::Hide),
                ID_EXIT => Some(Action::Exit),
                _ => None,
            },
            WM_CLOSE => Some(Action::Hide),
            WM_ENDSESSION if wparam.0 != 0 => Some(Action::EndSession),
            WM_ENDSESSION => return LRESULT(0),
            TRAY_MESSAGE => match lparam.0 as u32 {
                WM_LBUTTONUP => Some(Action::Show),
                WM_RBUTTONUP | WM_CONTEXTMENU => Some(Action::Menu),
                _ => None,
            },
            _ => None,
        }
    };
    if let Some(action) = action {
        let end_menu = matches!(action, Action::Hide | Action::Exit | Action::EndSession);
        state.enqueue(action);
        if end_menu {
            let _ = EndMenu();
        }
        return LRESULT(0);
    }
    DefWindowProcW(window, message, wparam, lparam)
}

unsafe fn keyboard_event(handle: LPARAM) -> Option<(usize, u16, bool)> {
    let mut input = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let read = GetRawInputData(
        HRAWINPUT(handle.0 as *mut _),
        RID_INPUT,
        Some((&mut input as *mut RAWINPUT).cast()),
        &mut size,
        std::mem::size_of::<RAWINPUTHEADER>() as u32,
    );
    if read == u32::MAX
        || read
            < (std::mem::size_of::<RAWINPUTHEADER>() + std::mem::size_of::<RAWKEYBOARD>()) as u32
        || input.header.dwType != RIM_TYPEKEYBOARD.0
    {
        return None;
    }
    let keyboard = input.data.keyboard;
    Some((
        input.header.hDevice.0 as usize,
        keyboard.VKey,
        keyboard.Flags & 1 == 0,
    ))
}
