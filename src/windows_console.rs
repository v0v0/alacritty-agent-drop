use std::io;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
    SetConsoleMode,
};

/// Enables Windows Virtual Terminal input while `agentdrop` owns the outer console.
///
/// `agentdrop` reads stdin as a byte stream so it can recognize bracketed paste markers.
/// Native Windows console input normally represents arrows and other navigation keys as
/// `KEY_EVENT` records instead of bytes. `ENABLE_VIRTUAL_TERMINAL_INPUT` asks ConPTY/the
/// console host to translate those events to the same VT byte sequences used on Unix
/// terminals (for example, Up becomes `ESC [ A`).
///
/// This guard is created after crossterm enables raw mode and is dropped before raw mode is
/// restored, so the complete console mode is returned to its previous state on exit.
pub struct VirtualTerminalInputGuard {
    handle: HANDLE,
    original_mode: CONSOLE_MODE,
}

impl VirtualTerminalInputGuard {
    pub fn enable() -> io::Result<Self> {
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }

            let mut original_mode: CONSOLE_MODE = 0;
            if GetConsoleMode(handle, &mut original_mode) == 0 {
                return Err(io::Error::last_os_error());
            }

            let desired_mode = original_mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if SetConsoleMode(handle, desired_mode) == 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(Self {
                handle,
                original_mode,
            })
        }
    }
}

impl Drop for VirtualTerminalInputGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SetConsoleMode(self.handle, self.original_mode);
        }
    }
}
