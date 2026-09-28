//! Display topology, cursor and DPI observations in physical virtual-desktop pixels.
//!
//! An `HMONITOR` is a short-lived observation, never a stored library identity: the
//! snapshot keeps the device name plus the observed geometry and DPI, and a normalized
//! signature so a topology change can be detected without a second enumeration.
use crate::{MAX_MOUSE_CANVAS_SIDE, UNKNOWN_TEXT};
use sm_application::{
    capture::{CaptureError, GdiStep},
    storage::PhysicalBounds,
};
use std::hash::{Hash, Hasher};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{LPARAM, POINT, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows_sys::Win32::UI::HiDpi::{
    GetAwarenessFromDpiAwarenessContext, GetDpiForMonitor, GetThreadDpiAwarenessContext,
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    DPI_AWARENESS_PER_MONITOR_AWARE, MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

/// One observed display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Device name as reported by the display (`\\.\DISPLAY1`).
    pub device: String,
    /// Full physical rectangle, taskbar included.
    pub bounds: PhysicalBounds,
    /// Work area of the same display.
    pub work: PhysicalBounds,
    /// Effective DPI of the display.
    pub dpi: u32,
    pub primary: bool,
}

/// Frozen display snapshot of one capture operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayTopology {
    pub monitors: Vec<MonitorInfo>,
    /// Bounding rectangle of every connected display; gaps stay part of the frame.
    pub desktop: PhysicalBounds,
    /// Hash of the normalized geometry, used to detect a topology change.
    pub signature: u64,
}

impl DisplayTopology {
    /// Monitor containing a point, without any automatic primary fallback.
    pub fn monitor_for_point(&self, x: i64, y: i64) -> Option<&MonitorInfo> {
        self.monitors
            .iter()
            .find(|monitor| contains_point(monitor.bounds, x, y))
    }

    /// Monitor that owns the center of an accepted rectangle.
    pub fn monitor_for_rect(&self, rect: PhysicalBounds) -> Option<&MonitorInfo> {
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        let center_x = i64::from(rect.x) + i64::from(rect.width) / 2;
        let center_y = i64::from(rect.y) + i64::from(rect.height) / 2;
        self.monitor_for_point(center_x, center_y)
    }

    /// Half-open containment test of one physical point.
    pub fn contains_point(&self, x: i64, y: i64) -> bool {
        contains_point(self.desktop, x, y)
    }

    /// Device name of the monitor that owns the rectangle center, or an explicit unknown.
    pub fn device_for_rect(&self, rect: PhysicalBounds) -> (String, u32) {
        match self.monitor_for_rect(rect) {
            Some(monitor) => (monitor.device.clone(), monitor.dpi),
            None => (UNKNOWN_TEXT.to_string(), 0),
        }
    }
}

fn contains_point(bounds: PhysicalBounds, x: i64, y: i64) -> bool {
    x >= i64::from(bounds.x)
        && y >= i64::from(bounds.y)
        && x < i64::from(bounds.x) + i64::from(bounds.width)
        && y < i64::from(bounds.y) + i64::from(bounds.height)
}

/// Physical rectangle of one `RECT`, refusing a non-positive extent.
fn bounds_from_rect(rect: RECT) -> Option<PhysicalBounds> {
    let width = i64::from(rect.right) - i64::from(rect.left);
    let height = i64::from(rect.bottom) - i64::from(rect.top);
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(PhysicalBounds {
        x: rect.left,
        y: rect.top,
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
    })
}

fn wide_to_string(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Per-monitor-v2 awareness for this process. It is applied before any window exists;
/// a manifest is the documented production route and this call is the same policy stated
/// at runtime. The returned value is the *resulting* state of this thread, so a repeated
/// call of an already-aware process still reports the truth.
pub fn ensure_per_monitor_awareness() -> bool {
    if process_is_per_monitor_aware() {
        return true;
    }
    // ERROR_ACCESS_DENIED (5) means another component already set the awareness.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    process_is_per_monitor_aware()
}

/// Whether this thread observes per-monitor-aware physical pixels.
pub fn process_is_per_monitor_aware() -> bool {
    let context = unsafe { GetThreadDpiAwarenessContext() };
    if context.is_null() {
        return false;
    }
    (unsafe { GetAwarenessFromDpiAwarenessContext(context) }) == DPI_AWARENESS_PER_MONITOR_AWARE
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // The callback only appends owned values; no GDI object is retained.
    let collected = unsafe { &mut *(data as *mut Vec<MonitorInfo>) };
    if let Some(info) = unsafe { read_monitor(monitor) } {
        collected.push(info);
    }
    TRUE
}

unsafe fn read_monitor(monitor: HMONITOR) -> Option<MonitorInfo> {
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } == 0 {
        return None;
    }
    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    // A failed DPI query is reported as unknown (0), never as an assumed 96.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    Some(MonitorInfo {
        device: wide_to_string(&info.szDevice),
        bounds: bounds_from_rect(info.monitorInfo.rcMonitor)?,
        work: bounds_from_rect(info.monitorInfo.rcWork)?,
        dpi: dpi_x,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}

/// Enumerate every connected display and compute the frozen virtual-desktop rectangle.
pub fn snapshot() -> Result<DisplayTopology, CaptureError> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    let collected = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut monitors as *mut Vec<MonitorInfo> as LPARAM,
        )
    };
    if collected == 0 || monitors.is_empty() {
        return Err(CaptureError::Unavailable);
    }
    let left = monitors
        .iter()
        .map(|monitor| i64::from(monitor.bounds.x))
        .min()
        .ok_or(CaptureError::Unavailable)?;
    let top = monitors
        .iter()
        .map(|monitor| i64::from(monitor.bounds.y))
        .min()
        .ok_or(CaptureError::Unavailable)?;
    let right = monitors
        .iter()
        .map(|monitor| i64::from(monitor.bounds.x) + i64::from(monitor.bounds.width))
        .max()
        .ok_or(CaptureError::Unavailable)?;
    let bottom = monitors
        .iter()
        .map(|monitor| i64::from(monitor.bounds.y) + i64::from(monitor.bounds.height))
        .max()
        .ok_or(CaptureError::Unavailable)?;
    let width = u32::try_from(right - left).map_err(|_| CaptureError::Unavailable)?;
    let height = u32::try_from(bottom - top).map_err(|_| CaptureError::Unavailable)?;
    let desktop = PhysicalBounds {
        x: i32::try_from(left).map_err(|_| CaptureError::Unavailable)?,
        y: i32::try_from(top).map_err(|_| CaptureError::Unavailable)?,
        width,
        height,
    };
    // A canvas that the 16-bit mouse coordinates cannot address is refused before the
    // overlay exists; a region crop of a smaller rectangle is a separate decision.
    if i64::from(desktop.width) > i64::from(MAX_MOUSE_CANVAS_SIDE)
        || i64::from(desktop.height) > i64::from(MAX_MOUSE_CANVAS_SIDE)
    {
        return Err(CaptureError::Unavailable);
    }
    let signature = signature_of(&monitors, desktop);
    Ok(DisplayTopology {
        monitors,
        desktop,
        signature,
    })
}

fn signature_of(monitors: &[MonitorInfo], desktop: PhysicalBounds) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    desktop.x.hash(&mut hasher);
    desktop.y.hash(&mut hasher);
    desktop.width.hash(&mut hasher);
    desktop.height.hash(&mut hasher);
    for monitor in monitors {
        monitor.device.hash(&mut hasher);
        monitor.bounds.x.hash(&mut hasher);
        monitor.bounds.y.hash(&mut hasher);
        monitor.bounds.width.hash(&mut hasher);
        monitor.bounds.height.hash(&mut hasher);
        monitor.dpi.hash(&mut hasher);
        monitor.primary.hash(&mut hasher);
    }
    hasher.finish()
}

/// Current cursor position in physical pixels.
pub fn cursor_position() -> Result<(i64, i64), CaptureError> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err(CaptureError::Gdi {
            step: GdiStep::Topology,
            os_code: unsafe { windows_sys::Win32::Foundation::GetLastError() },
        });
    }
    Ok((i64::from(point.x), i64::from(point.y)))
}
