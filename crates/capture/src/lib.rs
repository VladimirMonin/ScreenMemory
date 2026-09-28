//! S02 native Windows capture: one frozen virtual-desktop DIB, four target modes and
//! RAII ownership of every native resource.
//!
//! Boundaries of this crate:
//! * no SQLite, no filesystem, no codec, no logging and no network — it returns owned
//!   pixel bytes plus observations, and the host owns persistence (S02 §4.1);
//! * `HDC`, `HBITMAP`, `HWND`, selection guards and overlay state live in this crate on
//!   the native thread. Nothing here is `Send` by accident: the frozen surface holds a
//!   raw DIB pointer, so the compiler refuses to move it across threads (S02 §7.2);
//! * the donor `DDB`/`GetDIBits` fallback is deliberately **not** ported. The single
//!   freeze path creates its own top-down 32-bit `CreateDIBSection`, so the documented
//!   "`GetDIBits` while the bitmap is still selected" defect cannot occur; the selected
//!   object guard restores the previous bitmap before the DIB is deleted and the memory
//!   DC is deleted last (S02 §2.2, §7.1);
//! * `GetMessageW` is decoded as three distinct outcomes everywhere: positive dispatch,
//!   zero is a normal shutdown, `-1` is a queue error. `PostQuitMessage` belongs to the
//!   whole native thread and is never used to finish one selection (S02 §2.2, §7.3).
//!
//! `region` shows a native overlay over the *already frozen* frame (pixels first, UI
//! second); `window`, `monitor` and `desktop` never create an overlay at all.
//!
//! S02 section 1.3 invariant: nothing here hides, shows or activates a host window before
//! the freeze. The only UI this crate creates is the selection overlay, and it exists only
//! after the frozen frame does. A host that is visible on screen therefore *is* part of the
//! frame, which is the documented behaviour rather than a defect to hide.
#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("sm-capture requires the audited Windows x64 native adapter");

mod context;
mod gdi;
mod geometry;
mod host;
mod hotkeys;
mod overlay;
mod window;

pub use gdi::FrozenFrame;
pub use geometry::{cursor_position as observe_cursor, snapshot as observe_topology};
pub use geometry::{
    ensure_per_monitor_awareness, process_is_per_monitor_aware, DisplayTopology, MonitorInfo,
};
pub use host::{CaptureHost, CaptureHostConfig, HotkeySink, NullHotkeySink, WindowsCaptureAdapter};
pub use hotkeys::HotkeyBinding;
pub use overlay::{OverlayInputPolicy, OverlayObserver};
pub use window::{ForegroundWindowProvider, TargetWindow, TargetWindowProvider, WindowObservation};

/// Largest virtual-desktop side the mouse message path can address (S02 §6.2).
///
/// Mouse coordinates arrive in a 16-bit pair inside `LPARAM`; a canvas wider or taller
/// than this limit is refused **before** the overlay instead of silently truncating input.
pub const MAX_MOUSE_CANVAS_SIDE: i32 = 32_767;

/// Unknown application/title sentinel of a partial context (S02 §6.3). A missing
/// observation becomes an explicit unknown value, never the last known application.
pub const UNKNOWN_TEXT: &str = "unknown";
