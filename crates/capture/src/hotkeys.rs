//! Optional global hotkeys owned by this native thread.
//!
//! Registration is explicit: `RegisterHotKey`/`UnregisterHotKey` always run on the thread
//! that owns the message window, the result is checked, and `MOD_NOREPEAT` keeps a held
//! chord from multiplying captures (S02 §3.4). A host that is not configured with a
//! binding registers nothing and therefore claims no system-wide chord.
use sm_application::capture::{CaptureError, CaptureMode, GdiStep};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_S,
    },
};

/// One requested global chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeyBinding {
    /// Capture mode this chord starts.
    pub mode: CaptureMode,
    /// Windows hotkey identifier of this registration.
    pub id: i32,
    /// Modifier bit set, `MOD_NOREPEAT` included.
    pub modifiers: u32,
    /// Virtual key code.
    pub virtual_key: u16,
}

impl HotkeyBinding {
    /// Proposed S02 default of the region action; nothing registers it implicitly.
    pub const fn region_default() -> Self {
        Self {
            mode: CaptureMode::Region,
            id: 1,
            modifiers: MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
            virtual_key: VK_S,
        }
    }
}

/// Registers one chord. A conflict with another application is reported, never hidden
/// behind a "saved" status.
pub(crate) fn register(window: HWND, binding: &HotkeyBinding) -> Result<(), CaptureError> {
    if unsafe {
        RegisterHotKey(
            window,
            binding.id,
            binding.modifiers,
            u32::from(binding.virtual_key),
        )
    } == 0
    {
        return Err(CaptureError::Gdi {
            step: GdiStep::Input,
            os_code: unsafe { windows_sys::Win32::Foundation::GetLastError() },
        });
    }
    Ok(())
}

/// Releases exactly the registration this owner created.
pub(crate) fn unregister(window: HWND, binding: &HotkeyBinding) {
    unsafe {
        UnregisterHotKey(window, binding.id);
    }
}
