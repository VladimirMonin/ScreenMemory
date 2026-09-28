//! Target window resolution and observation: HWND identity, title, executable and
//! geometry in physical pixels.
//!
//! The trusted host adapter decides which window is the target. `window` mode reads the
//! real foreground window exactly once at execution unless the host configured another
//! provider; no client ever supplies an HWND (S02 §3.2, §6.5).
use crate::{geometry::DisplayTopology, UNKNOWN_TEXT};
use sm_application::{
    capture::{CaptureError, GdiStep, TargetObservation},
    storage::PhysicalBounds,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, HWND, RECT},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITOR_DEFAULTTONEAREST},
    },
    System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible, GWL_STYLE, WS_CHILD,
    },
};

/// Longest accepted window title in UTF-16 units before truncation is reported.
const MAX_TITLE_UNITS: usize = 4_096;
/// Upper bound of the executable path buffer (S02 §6.3).
const MAX_PATH_UNITS: usize = 32_768;
/// Initial executable path buffer; the query grows this once on `ERROR_INSUFFICIENT_BUFFER`.
const INITIAL_PATH_UNITS: usize = 1_024;

/// One trusted target: an opaque handle plus, for a window of *this* process, the caption
/// known to the application registry.
///
/// A caption is only supplied by the trusted host. Reading the caption of a window in our
/// own process with `GetWindowTextW` would send a message to our own thread and can block
/// it (S02 §6.3), so an own window without a registered caption is reported as unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetWindow {
    /// Window identity as an opaque handle value; `0` means "no usable target".
    pub handle: isize,
    /// Registered caption of an own-process window, if the host has one.
    pub caption: Option<String>,
}

impl TargetWindow {
    pub fn from_handle(handle: isize) -> Self {
        Self {
            handle,
            caption: None,
        }
    }
}

/// Trusted source of the `window` mode target. Production reads the real foreground
/// window at execution; the host may inject another provider for a deterministic test.
pub trait TargetWindowProvider: Send + Sync {
    fn target_window(&self) -> TargetWindow;
}

/// Foreground window at the moment of execution (S02 §3.2, hotkey path).
pub struct ForegroundWindowProvider;

impl TargetWindowProvider for ForegroundWindowProvider {
    fn target_window(&self) -> TargetWindow {
        let hwnd = unsafe { GetForegroundWindow() };
        TargetWindow::from_handle(hwnd as isize)
    }
}

/// One observed target window. The handle is a diagnostic value; paths stay out of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowObservation {
    pub hwnd: isize,
    pub pid: u32,
    pub executable: String,
    /// Visible screen rectangle clipped to the virtual desktop.
    pub bounds: PhysicalBounds,
    pub title: String,
    pub dpi: u32,
    /// `true` when `DwmGetWindowAttribute` answered; `false` means `GetWindowRect`.
    pub dwm_bounds: bool,
    pub truncated_title: bool,
    pub warnings: Vec<String>,
}

impl WindowObservation {
    /// Diagnostic hex form of the handle; never an API parameter (S02 §5.1).
    pub fn hwnd_hex(&self) -> String {
        format!("{:x}", self.hwnd as usize)
    }

    /// Convert into the observation DTO that travels with an accepted frame.
    pub fn to_target(&self) -> TargetObservation {
        TargetObservation {
            hwnd: self.hwnd_hex(),
            pid: self.pid,
            executable: self.executable.clone(),
            title: self.title.clone(),
            bounds: self.bounds,
            dpi: self.dpi,
        }
    }
}

fn wide_to_string(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// A usable target is a real, visible top-level window of this session.
pub fn is_usable_target(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let handle = hwnd as HWND;
    if unsafe { IsWindow(handle) } == 0 || unsafe { IsWindowVisible(handle) } == 0 {
        return false;
    }
    // A child/control window is not a capture target; the top-level owner is.
    (unsafe { GetWindowLongW(handle, GWL_STYLE) } as u32) & WS_CHILD == 0
}

/// Window rectangle: `DWMWA_EXTENDED_FRAME_BOUNDS` first (invisible resize borders and
/// DPI virtualization excluded), `GetWindowRect` as the reported fallback.
fn window_rect(hwnd: HWND) -> Result<(RECT, bool), CaptureError> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut extended = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let size = std::mem::size_of::<RECT>() as u32;
    let dwm = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut extended as *mut RECT as *mut std::ffi::c_void,
            size,
        )
    };
    if dwm == 0 && extended.right > extended.left && extended.bottom > extended.top {
        return Ok((extended, true));
    }
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(CaptureError::Gdi {
            step: GdiStep::Topology,
            os_code: unsafe { GetLastError() },
        });
    }
    Ok((rect, false))
}

/// Clips a rectangle to the frozen desktop; `None` when nothing remains.
pub fn clip_to_desktop(rect: PhysicalBounds, desktop: PhysicalBounds) -> Option<PhysicalBounds> {
    let left = i64::from(rect.x).max(i64::from(desktop.x));
    let top = i64::from(rect.y).max(i64::from(desktop.y));
    let right = (i64::from(rect.x) + i64::from(rect.width))
        .min(i64::from(desktop.x) + i64::from(desktop.width));
    let bottom = (i64::from(rect.y) + i64::from(rect.height))
        .min(i64::from(desktop.y) + i64::from(desktop.height));
    if right <= left || bottom <= top {
        return None;
    }
    let width = u32::try_from(right - left).ok()?;
    let height = u32::try_from(bottom - top).ok()?;
    let x = i32::try_from(left).ok()?;
    let y = i32::try_from(top).ok()?;
    Some(PhysicalBounds {
        x,
        y,
        width,
        height,
    })
}

/// Bounded executable file name of one process. A denied or protected process yields an
/// explicit unknown plus a warning; the capture itself is not destroyed.
fn executable_name(pid: u32) -> (String, Option<String>) {
    let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return (
            UNKNOWN_TEXT.to_string(),
            Some("process_not_readable".to_string()),
        );
    }
    let mut units = vec![0u16; INITIAL_PATH_UNITS];
    let mut length = INITIAL_PATH_UNITS as u32;
    let mut ok = unsafe {
        windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
            handle,
            0,
            units.as_mut_ptr(),
            &mut length,
        )
    };
    let mut warning = None;
    if ok == 0 && length > INITIAL_PATH_UNITS as u32 {
        units = vec![0u16; MAX_PATH_UNITS];
        length = MAX_PATH_UNITS as u32;
        ok = unsafe {
            windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
                handle,
                0,
                units.as_mut_ptr(),
                &mut length,
            )
        };
    }
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return (
            UNKNOWN_TEXT.to_string(),
            Some("executable_unavailable".to_string()),
        );
    }
    let full = wide_to_string(&units[..(length as usize).min(units.len())]);
    if full.len() > MAX_PATH_UNITS / 2 {
        warning = Some("executable_truncated".to_string());
    }
    let name = full
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(UNKNOWN_TEXT)
        .to_string();
    (name, warning)
}

/// Bounded window title. Inside the owning process `GetWindowTextW` may block, so own
/// windows are described from their configured name instead (S02 §6.3).
fn window_title(hwnd: HWND) -> (String, bool) {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return (UNKNOWN_TEXT.to_string(), false);
    }
    let units = (length as usize + 1).min(MAX_TITLE_UNITS + 1);
    let mut buffer = vec![0u16; units];
    let written = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), units as i32) };
    if written <= 0 {
        return (UNKNOWN_TEXT.to_string(), true);
    }
    let truncated = (written as usize) >= MAX_TITLE_UNITS;
    let title = wide_to_string(&buffer[..(written as usize).min(buffer.len())]);
    if title.is_empty() {
        (UNKNOWN_TEXT.to_string(), truncated)
    } else {
        (title, truncated)
    }
}

/// Observe one target window; geometry is clipped to the frozen desktop.
pub fn observe(
    target: &TargetWindow,
    topology: &DisplayTopology,
    own_executable: &str,
) -> Result<WindowObservation, CaptureError> {
    let hwnd = target.handle;
    if !is_usable_target(hwnd) {
        return Err(CaptureError::TargetLost);
    }
    let handle = hwnd as HWND;
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(handle, &mut pid);
    }
    let (rect, dwm_bounds) = window_rect(handle)?;
    let raw = PhysicalBounds {
        x: rect.left,
        y: rect.top,
        width: u32::try_from(i64::from(rect.right) - i64::from(rect.left))
            .map_err(|_| CaptureError::TargetLost)?,
        height: u32::try_from(i64::from(rect.bottom) - i64::from(rect.top))
            .map_err(|_| CaptureError::TargetLost)?,
    };
    let bounds = clip_to_desktop(raw, topology.desktop).ok_or(CaptureError::TargetLost)?;
    let mut warnings = Vec::new();
    if !dwm_bounds {
        warnings.push("dwm_frame_bounds_unavailable".to_string());
    }
    if bounds != raw {
        warnings.push("target_clipped_to_desktop".to_string());
    }
    let own_process = pid == std::process::id();
    let (title, truncated_title) = match (own_process, target.caption.as_deref()) {
        // A registered caption is the only safe source inside our own process.
        (true, Some(caption)) => (
            caption.chars().take(MAX_TITLE_UNITS).collect::<String>(),
            caption.chars().count() > MAX_TITLE_UNITS,
        ),
        (true, None) => {
            warnings.push("own_window_caption_from_registry".to_string());
            (UNKNOWN_TEXT.to_string(), false)
        }
        (false, _) => window_title(handle),
    };
    // The window belongs to this process: its identity is the observed executable name.
    let (executable, executable_warning) = if own_process {
        (own_executable.to_string(), None)
    } else {
        executable_name(pid)
    };
    if let Some(warning) = executable_warning {
        warnings.push(warning);
    }
    if truncated_title {
        warnings.push("title_truncated".to_string());
    }
    let monitor = unsafe { MonitorFromWindow(handle, MONITOR_DEFAULTTONEAREST) };
    let mut info: windows_sys::Win32::Graphics::Gdi::MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of_val(&info) as u32;
    let mut dpi = 0u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } != 0 {
        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        if unsafe {
            windows_sys::Win32::UI::HiDpi::GetDpiForMonitor(
                monitor,
                windows_sys::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
                &mut dpi_x,
                &mut dpi_y,
            )
        } == 0
        {
            dpi = dpi_x;
        }
    }
    Ok(WindowObservation {
        hwnd,
        pid,
        executable,
        bounds,
        title,
        dpi,
        dwm_bounds,
        truncated_title,
        warnings,
    })
}
