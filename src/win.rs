//! Win32 calls that gpui does not expose: hide, restore and always on top.

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNORMAL,
};

/// Native window handle, stored as a number so it can move between threads.
#[derive(Clone, Copy)]
pub struct Handle(isize);

impl Handle {
    pub fn of(window: &Window) -> Option<Self> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(Self(handle.hwnd.get())),
            _ => None,
        }
    }

    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    pub fn hide(self) {
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_HIDE);
        }
    }

    pub fn restore(self) {
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_SHOWNORMAL);
            let _ = SetForegroundWindow(self.hwnd());
        }
    }

    pub fn set_always_on_top(self, on: bool) {
        let order = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ = SetWindowPos(
                self.hwnd(),
                Some(order),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

/// Returns false when another Cadence process already runs.
pub fn claim_single_instance() -> bool {
    unsafe {
        // The mutex lives until the process ends, so the handle is never closed.
        match CreateMutexW(None, false, w!("Local\\CadenceSingleInstance")) {
            Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => true,
        }
    }
}

/// Brings the window of the running instance to the front.
pub fn show_running_instance() {
    unsafe {
        if let Ok(hwnd) = FindWindowW(w!("Zed::Window"), w!("Cadence")) {
            Handle(hwnd.0 as isize).restore();
        }
    }
}
