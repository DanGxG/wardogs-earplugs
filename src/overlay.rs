use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, DrawTextW, EndPaint, GetStockObject, SelectObject, SetBkMode, SetTextColor,
            BLACK_BRUSH, DEFAULT_GUI_FONT, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBRUSH,
            PAINTSTRUCT, TRANSPARENT,
        },
        UI::WindowsAndMessaging::*,
    },
};

const CLASS: PCWSTR = w!("WarDogsEarplugsOverlay");

pub struct Overlay {
    window: HWND,
}

impl Overlay {
    pub unsafe fn new(instance: HINSTANCE) -> Result<Self, String> {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_win32().to_string());
        }
        // Unowned desktop window: never parented to the game or to its rendering surface.
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            CLASS,
            w!("Earplugs ON"),
            WS_POPUP,
            12,
            12,
            150,
            32,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| e.to_string())?;
        let overlay = Self { window };
        SetLayeredWindowAttributes(window, COLORREF(0), 220, LWA_ALPHA)
            .map_err(|e| e.to_string())?;
        Ok(overlay)
    }

    pub unsafe fn set_on(&self, on: bool) {
        if on {
            // No timer or foreground-window tracking; Windows keeps it above normal windows.
            let _ = SetWindowPos(
                self.window,
                Some(HWND_TOPMOST),
                12,
                12,
                150,
                32,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        } else {
            let _ = ShowWindow(self.window, SW_HIDE);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut paint);
            let mut rect = RECT::default();
            let _ = GetClientRect(window, &mut rect);
            let previous = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00C5FCA3));
            let mut text: Vec<u16> = "Earplugs ON".encode_utf16().collect();
            DrawTextW(
                dc,
                &mut text,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, previous);
            let _ = EndPaint(window, &paint);
            return LRESULT(0);
        }
        _ => (),
    }
    DefWindowProcW(window, message, wparam, lparam)
}
