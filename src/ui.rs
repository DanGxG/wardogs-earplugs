use crate::{audio::Audio, model::Settings, settings};
use std::{cell::RefCell, path::PathBuf};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT,
            WPARAM,
        },
        Graphics::Gdi::{GetStockObject, COLOR_WINDOW, DEFAULT_GUI_FONT, HBRUSH},
        System::{
            Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            LibraryLoader::GetModuleHandleW,
            Threading::CreateMutexW,
        },
        UI::{
            Controls::EM_SETLIMITTEXT,
            Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, VK_F2},
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::*,
        },
    },
};

const CLASS: PCWSTR = w!("WarDogsEarplugsWindow");
const HOTKEY: i32 = 1;
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
    tray: NOTIFYICONDATAW,
    tray_ready: bool,
    hotkey_ready: bool,
    taskbar_created: u32,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
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
        if already_running {
            if let Ok(window) = FindWindowW(CLASS, None) {
                show_window(window);
            }
            return Ok(());
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let _com = Com;
        let settings_path = settings::path().map_err(|e| e.to_string())?;
        let (settings, initial_status) = match settings::load(&settings_path) {
            Ok(settings) => (
                settings,
                "F2 toggles game volume. Closing this window keeps it running.".to_string(),
            ),
            Err(error) => (
                Settings::default(),
                format!("Settings could not be loaded; using defaults. {error}"),
            ),
        };
        let instance = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
            hIcon: LoadIconW(None, IDI_APPLICATION).map_err(|e| e.to_string())?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_win32().to_string());
        }
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("WarDogs Earplugs"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            464,
            320,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| e.to_string())?;
        let mut app = Box::new(RefCell::new(App {
            window,
            percent: HWND::default(),
            executable: HWND::default(),
            status: HWND::default(),
            settings,
            settings_path,
            audio: Audio::default(),
            tray: NOTIFYICONDATAW::default(),
            tray_ready: false,
            hotkey_ready: false,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        }));
        if let Err(error) = app.get_mut().initialize(instance, &initial_status) {
            let _ = DestroyWindow(window);
            return Err(error);
        }
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            (&*app as *const RefCell<App>) as isize,
        );
        let background = std::env::args_os().skip(1).any(|arg| arg == "--background");
        if !background || !app.borrow().tray_ready || !app.borrow().hotkey_ready {
            show_window(window);
        }
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
        let _ = app.get_mut().audio.restore();
        if app.get_mut().tray_ready {
            let _ = Shell_NotifyIconW(NIM_DELETE, &app.get_mut().tray);
        }
        if app.get_mut().hotkey_ready {
            let _ = UnregisterHotKey(Some(window), HOTKEY);
        }
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        let _ = DestroyWindow(window);
        result
    }
}

unsafe fn show_window(window: HWND) {
    let _ = ShowWindow(window, SW_RESTORE);
    let _ = SetForegroundWindow(window);
}

impl App {
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
            "Game mixer volume (%)",
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
            "Save",
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
        self.hotkey_ready =
            RegisterHotKey(Some(self.window), HOTKEY, MOD_NOREPEAT, VK_F2.0 as u32).is_ok();
        self.tray.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        self.tray.hWnd = self.window;
        self.tray.uID = 1;
        self.tray.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        self.tray.uCallbackMessage = TRAY_MESSAGE;
        self.tray.hIcon = LoadIconW(None, IDI_APPLICATION).map_err(|e| e.to_string())?;
        let tip = wide("WarDogs Earplugs | F2 lower / restore");
        self.tray.szTip[..tip.len()].copy_from_slice(&tip);
        self.tray_ready = Shell_NotifyIconW(NIM_ADD, &self.tray).as_bool();
        if !self.hotkey_ready {
            self.set_status("F2 is already in use. Close the conflicting app, then restart Earplugs. The Toggle button still works.");
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

    unsafe fn save(&mut self, force: bool) -> Result<(), String> {
        let new_settings = Settings::parse(&self.input(self.percent), &self.input(self.executable))
            .map_err(str::to_string)?;
        if force || new_settings != self.settings {
            settings::save(&self.settings_path, &new_settings)
                .map_err(|e| format!("Could not save settings: {e}"))?;
        }
        self.settings = new_settings;
        Ok(())
    }

    unsafe fn toggle(&mut self) {
        let result = if self.audio.is_lowered() {
            self.audio.restore()
        } else {
            self.save(false)
                .and_then(|_| self.audio.toggle(&self.settings))
        };
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
        if let Err(error) = self.audio.restore() {
            self.set_status(&error);
            show_window(self.window);
            return;
        }
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
    let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *const RefCell<App>;
    if !pointer.is_null() {
        // Windows may dispatch nested control notifications. Never alias mutable state.
        if let Ok(mut app) = (*pointer).try_borrow_mut() {
            if message == app.taskbar_created && app.taskbar_created != 0 {
                app.tray_ready = Shell_NotifyIconW(NIM_ADD, &app.tray).as_bool();
                if !app.tray_ready {
                    show_window(window);
                }
                return LRESULT(0);
            }
            match message {
                WM_HOTKEY if wparam.0 == HOTKEY as usize => {
                    app.toggle();
                    return LRESULT(0);
                }
                WM_COMMAND if (wparam.0 >> 16) == BN_CLICKED as usize => {
                    match wparam.0 & 0xffff {
                        ID_SAVE => match app.save(true) {
                            Ok(()) => {
                                app.set_status("Settings saved. F2 uses them on the next lowering.")
                            }
                            Err(error) => app.set_status(&error),
                        },
                        ID_TOGGLE => app.toggle(),
                        ID_HIDE => app.hide(),
                        ID_EXIT => app.exit(),
                        _ => (),
                    }
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    app.hide();
                    return LRESULT(0);
                }
                WM_QUERYENDSESSION => {
                    return LRESULT(1);
                }
                WM_ENDSESSION if wparam.0 != 0 => {
                    let _ = app.audio.restore();
                    PostQuitMessage(0);
                    return LRESULT(0);
                }
                TRAY_MESSAGE => {
                    match lparam.0 as u32 {
                        WM_LBUTTONUP => show_window(window),
                        WM_RBUTTONUP | WM_CONTEXTMENU => app.menu(),
                        _ => (),
                    }
                    return LRESULT(0);
                }
                _ => (),
            }
        }
    }
    DefWindowProcW(window, message, wparam, lparam)
}
