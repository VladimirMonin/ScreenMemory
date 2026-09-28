//! Region overlay: a native `WS_POPUP` window over the *already frozen* frame.
//!
//! The overlay paints the frozen DIB, dims everything outside the live selection and
//! draws the selection chrome (solid plus dashed border). It never re-reads screen
//! pixels while painting, so a repaint of another application cannot change the frame.
//!
//! Message-loop contract (S02 §2.2, §7.3):
//! * `GetMessageW` is decoded as three outcomes: positive dispatch, zero is a normal
//!   shutdown, `-1` is a queue error reported with the numeric OS code;
//! * a finished selection is signalled by its own `WM_APP` event, never by
//!   `PostQuitMessage`, which belongs to the whole native thread.
use crate::{gdi::FrozenFrame, geometry::DisplayTopology, window::is_usable_target};
use sm_application::capture::{selection_rect, CancelReason, CaptureError, GdiStep};
use sm_application::storage::PhysicalBounds;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, BitBlt, CreateBitmap, CreatePatternBrush, CreatePen, DeleteObject, EndPaint,
        FillRect, GetStockObject, InvalidateRect, Rectangle, SelectObject, SetBkMode, UpdateWindow,
        HBRUSH, HGDIOBJ, HPEN, NULL_BRUSH, PAINTSTRUCT, PS_DOT, PS_SOLID, SRCCOPY, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            GetActiveWindow, GetCapture, ReleaseCapture, SetActiveWindow, SetCapture, VK_ESCAPE,
        },
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
            GetMessageW, GetWindowLongPtrW, IsWindow, LoadCursorW, MsgWaitForMultipleObjectsEx,
            PostMessageW, RegisterClassExW, SendMessageTimeoutW, SetCursor, SetForegroundWindow,
            SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
            GWLP_USERDATA, HWND_TOPMOST, IDC_CROSS, MSG, QS_ALLINPUT, SMTO_ABORTIFHUNG,
            SMTO_ERRORONEXIT, SWP_SHOWWINDOW, SW_SHOW, WA_INACTIVE, WM_ACTIVATE, WM_APP,
            WM_CANCELMODE, WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE,
            WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
            WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

/// Session event that ends one selection; `WM_QUIT` is never used for this.
pub(crate) const WM_SESSION_DONE: u32 = WM_APP + 1;
const WINDOW_CLASS: &str = "ScreenMemoryCaptureOverlayV1";
/// Bounded wait of the loop that polls the cooperative cancel/stop flags.
const POLL_MILLIS: u32 = 30;
/// Bounded `WM_CANCELMODE` fallback timeout of S02 §7.5.
const CANCELMODE_TIMEOUT_MS: u32 = 50;

/// Observer of one live overlay. Production has none; a live test uses it to address the
/// owned overlay window instead of touching a foreign one.
pub trait OverlayObserver: Send + Sync {
    /// Called on the native thread right after the overlay window exists.
    fn overlay_created(&self, window: isize);
    /// Called on the native thread after the overlay stopped accepting input.
    fn overlay_closed(&self, window: isize);
}

/// Cooperative cancellation and stop flags shared with the owning host thread.
#[derive(Clone)]
pub(crate) struct SessionFlags {
    pub cancel: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    /// Optional safety bound of one session; production sessions wait for the user.
    pub deadline: Option<std::time::Instant>,
}

/// Result of one selection session.
pub(crate) enum Picked {
    Rect(PhysicalBounds),
    Cancelled(CancelReason),
}

/// Outcome of one selection session plus everything the adapter must record with it.
pub(crate) struct SessionOutcome {
    pub picked: Picked,
    pub warnings: Vec<String>,
}

/// How the overlay must obtain its input state (S02 §7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayInputPolicy {
    /// Production policy: the overlay must own the foreground, otherwise the session is
    /// refused with `CAPTURE_FOCUS_UNAVAILABLE`.
    RequireForeground,
    /// The trusted caller already established the input state of this session (an
    /// already-active host or an integration test driving its own overlay window). The
    /// overlay still becomes the active window of this thread and still refuses with
    /// `CAPTURE_FOCUS_UNAVAILABLE` when even that fails; the unverified foreground claim
    /// is recorded as a warning instead of being silently accepted.
    ThreadActive,
}

enum DragState {
    Idle,
    Dragging {
        start: (i64, i64),
        current: (i64, i64),
    },
}

/// Owner state addressed by the window procedure through `GWLP_USERDATA`.
struct OverlayState<'a> {
    frame: &'a FrozenFrame,
    desktop: PhysicalBounds,
    flags: SessionFlags,
    observer: Option<Arc<dyn OverlayObserver>>,
    dim_brush: HBRUSH,
    border_pen: HPEN,
    dash_pen: HPEN,
    drag: DragState,
    outcome: Option<Picked>,
    failure: Option<CaptureError>,
    finishing: bool,
    quit: bool,
    window: HWND,
    warnings: Vec<String>,
}

impl OverlayState<'_> {
    /// Frame-local rectangle of the current selection.
    fn selection_rect(&self) -> Option<PhysicalBounds> {
        let DragState::Dragging { start, current } = self.drag else {
            return None;
        };
        selection_rect(start, current, self.desktop).ok()
    }

    fn finish(&mut self, picked: Picked) {
        if self.outcome.is_none() && self.failure.is_none() {
            self.outcome = Some(picked);
        }
        self.finishing = true;
        unsafe {
            PostMessageW(self.window, WM_SESSION_DONE, 0, 0);
        }
    }

    fn fail(&mut self, error: CaptureError) {
        if self.failure.is_none() && self.outcome.is_none() {
            self.failure = Some(error);
        }
        self.finishing = true;
        unsafe {
            PostMessageW(self.window, WM_SESSION_DONE, 0, 0);
        }
    }
}

fn overlay_class() -> Result<u16, CaptureError> {
    static CLASS: OnceLock<Result<u16, u32>> = OnceLock::new();
    let registered = CLASS.get_or_init(|| {
        let name: Vec<u16> = WINDOW_CLASS
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: unsafe { GetModuleHandleW(std::ptr::null()) },
            hIcon: std::ptr::null_mut(),
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_CROSS) },
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        let atom = unsafe { RegisterClassExW(&class) };
        if atom == 0 {
            Err(unsafe { GetLastError() })
        } else {
            Ok(atom)
        }
    });
    match registered {
        Ok(atom) => Ok(*atom),
        Err(code) => Err(CaptureError::Gdi {
            step: GdiStep::Overlay,
            os_code: *code,
        }),
    }
}

/// One signed half of `LPARAM`. The unsigned `LOWORD`/`HIWORD` reading of the donor is
/// wrong for negative virtual-desktop coordinates (S02 §6.2).
fn signed_axis(value: isize, high: bool) -> i64 {
    let raw = value as u32;
    let half = if high {
        (raw >> 16) & 0xffff
    } else {
        raw & 0xffff
    };
    i64::from(half as u16 as i16)
}

/// Global physical point of a mouse message, from **signed** halves of `LPARAM`.
fn message_point(lparam: LPARAM, desktop: PhysicalBounds) -> (i64, i64) {
    let x = signed_axis(lparam, false) + i64::from(desktop.x);
    let y = signed_axis(lparam, true) + i64::from(desktop.y);
    (x, y)
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
    if pointer == 0 {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let state = unsafe { &mut *(pointer as *mut OverlayState) };
    match message {
        WM_PAINT => {
            unsafe {
                paint(state);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_SETCURSOR => {
            unsafe {
                SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_CROSS));
            }
            1
        }
        WM_LBUTTONDOWN => {
            if matches!(state.drag, DragState::Idle) {
                unsafe {
                    SetCapture(window);
                }
                // A capture this thread does not actually own would silently lose the
                // first drag; it is an explicit refusal instead.
                if unsafe { GetCapture() } != window {
                    state.fail(CaptureError::FocusUnavailable);
                } else {
                    let point = message_point(lparam, state.desktop);
                    state.drag = DragState::Dragging {
                        start: point,
                        current: point,
                    };
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            if let DragState::Dragging { start, .. } = state.drag {
                state.drag = DragState::Dragging {
                    start,
                    current: message_point(lparam, state.desktop),
                };
                unsafe {
                    InvalidateRect(window, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if let DragState::Dragging { start, current } = state.drag {
                // The session finishes *before* the capture is released, so the resulting
                // WM_CAPTURECHANGED cannot cancel an accepted frame a second time.
                state.finishing = true;
                unsafe {
                    ReleaseCapture();
                }
                let end = message_point(lparam, state.desktop);
                state.drag = DragState::Dragging { start, current };
                match selection_rect(start, end, state.desktop) {
                    Ok(rect) => state.finish(Picked::Rect(rect)),
                    Err(CaptureError::EmptySelection) => {
                        state.finish(Picked::Cancelled(CancelReason::EmptySelection))
                    }
                    Err(error) => state.fail(error),
                }
            }
            0
        }
        WM_RBUTTONUP => {
            state.finish(Picked::Cancelled(CancelReason::ExplicitRequest));
            0
        }
        WM_KEYDOWN => {
            if (wparam as u32 & 0xffff) as u16 == VK_ESCAPE {
                state.finish(Picked::Cancelled(CancelReason::Escape));
            }
            0
        }
        WM_CAPTURECHANGED => {
            if !state.finishing && matches!(state.drag, DragState::Dragging { .. }) {
                state.fail(CaptureError::FocusUnavailable);
            }
            0
        }
        WM_DISPLAYCHANGE => {
            state.fail(CaptureError::TopologyChanged);
            0
        }
        WM_ACTIVATE => {
            if (wparam as u32 & 0xffff) == WA_INACTIVE && !state.finishing {
                state.warnings.push("focus_lost".to_string());
                state.finish(Picked::Cancelled(CancelReason::ExplicitRequest));
            }
            0
        }
        WM_CLOSE => {
            state.finish(Picked::Cancelled(CancelReason::ExplicitRequest));
            0
        }
        WM_SESSION_DONE => {
            unsafe {
                DestroyWindow(window);
            }
            0
        }
        WM_DESTROY => {
            // The overlay never ends the whole thread: no PostQuitMessage here.
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            }
            state.quit = true;
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

unsafe fn paint(state: &OverlayState) {
    let mut paint_struct: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let dc = unsafe { BeginPaint(state.window, &mut paint_struct) };
    if dc.is_null() {
        return;
    }
    let desktop = state.desktop;
    let width = desktop.width as i32;
    let height = desktop.height as i32;
    // Base image is the frozen frame; no screen read happens during a repaint.
    unsafe {
        BitBlt(dc, 0, 0, width, height, state.frame.dc(), 0, 0, SRCCOPY);
    }
    let full = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    match state.selection_rect() {
        Some(rect) => {
            let left = rect.x - desktop.x;
            let top = rect.y - desktop.y;
            let right = left + rect.width as i32;
            let bottom = top + rect.height as i32;
            // Dim everything, then repaint exactly the accepted area undimmed.
            unsafe {
                FillRect(dc, &full, state.dim_brush);
                BitBlt(
                    dc,
                    left,
                    top,
                    rect.width as i32,
                    rect.height as i32,
                    state.frame.dc(),
                    left,
                    top,
                    SRCCOPY,
                );
                SelectObject(dc, GetStockObject(NULL_BRUSH));
                SelectObject(dc, state.border_pen as HGDIOBJ);
                Rectangle(dc, left - 1, top - 1, right + 1, bottom + 1);
                SelectObject(dc, state.dash_pen as HGDIOBJ);
                SetBkMode(dc, TRANSPARENT as i32);
                Rectangle(dc, left, top, right, bottom);
            }
        }
        None => unsafe {
            FillRect(dc, &full, state.dim_brush);
        },
    }
    unsafe {
        EndPaint(state.window, &paint_struct);
    }
}

fn create_pens() -> Result<(HBRUSH, HPEN, HPEN), CaptureError> {
    // An 8x8 monochrome checkerboard gives the documented 50% dim without layers.
    let pattern: [u8; 8] = [0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55];
    let bitmap = unsafe { CreateBitmap(8, 8, 1, 1, pattern.as_ptr() as *const std::ffi::c_void) };
    if bitmap.is_null() {
        return Err(CaptureError::Gdi {
            step: GdiStep::Overlay,
            os_code: unsafe { GetLastError() },
        });
    }
    let brush = unsafe { CreatePatternBrush(bitmap) };
    // The pattern brush keeps its own copy of the bitmap.
    unsafe {
        DeleteObject(bitmap as HGDIOBJ);
    }
    let border = unsafe { CreatePen(PS_SOLID, 1, 0x0000_ff00) };
    let dash = unsafe { CreatePen(PS_DOT, 1, 0x00ff_ffff) };
    if brush.is_null() || border.is_null() || dash.is_null() {
        if !brush.is_null() {
            unsafe {
                DeleteObject(brush as HGDIOBJ);
            }
        }
        if !border.is_null() {
            unsafe {
                DeleteObject(border as HGDIOBJ);
            }
        }
        if !dash.is_null() {
            unsafe {
                DeleteObject(dash as HGDIOBJ);
            }
        }
        return Err(CaptureError::Gdi {
            step: GdiStep::Overlay,
            os_code: unsafe { GetLastError() },
        });
    }
    Ok((brush, border, dash))
}

/// Show the overlay, wait for a selection and return the accepted rectangle.
pub(crate) fn pick(
    frame: &FrozenFrame,
    topology: &DisplayTopology,
    flags: SessionFlags,
    observer: Option<Arc<dyn OverlayObserver>>,
    policy: OverlayInputPolicy,
) -> Result<SessionOutcome, CaptureError> {
    overlay_class()?;
    let (dim_brush, border_pen, dash_pen) = create_pens()?;
    let class_name: Vec<u16> = WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = "ScreenMemory capture"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let desktop = topology.desktop;
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            desktop.x,
            desktop.y,
            desktop.width as i32,
            desktop.height as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        )
    };
    if window.is_null() {
        let error = CaptureError::Gdi {
            step: GdiStep::Overlay,
            os_code: unsafe { GetLastError() },
        };
        unsafe {
            DeleteObject(dim_brush as HGDIOBJ);
            DeleteObject(border_pen as HGDIOBJ);
            DeleteObject(dash_pen as HGDIOBJ);
        }
        return Err(error);
    }
    let mut state = Box::new(OverlayState {
        frame,
        desktop,
        flags,
        observer,
        dim_brush,
        border_pen,
        dash_pen,
        drag: DragState::Idle,
        outcome: None,
        failure: None,
        finishing: false,
        quit: false,
        window,
        warnings: Vec::new(),
    });
    unsafe {
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            (&mut *state as *mut OverlayState<'_>) as isize,
        );
        ShowWindow(window, SW_SHOW);
        SetWindowPos(
            window,
            HWND_TOPMOST,
            desktop.x,
            desktop.y,
            desktop.width as i32,
            desktop.height as i32,
            SWP_SHOWWINDOW,
        );
        UpdateWindow(window);
    }
    if let Some(observer) = state.observer.as_ref() {
        observer.overlay_created(window as isize);
    }
    // §7.5: one activation attempt plus exactly one documented WM_CANCELMODE fallback to
    // the previously foreground window. No synthetic input is injected.
    let previous = unsafe { GetForegroundWindow() };
    match activate(window, previous, policy) {
        Ok(Some(warning)) => state.warnings.push(warning),
        Ok(None) => {}
        Err(error) => {
            if let Some(observer) = state.observer.as_ref() {
                observer.overlay_closed(window as isize);
            }
            unsafe {
                DestroyWindow(window);
            }
            cleanup_state(state);
            return Err(error);
        }
    }
    let looped = message_loop(&mut state);
    if let Some(observer) = state.observer.as_ref() {
        observer.overlay_closed(window as isize);
    }
    // DestroyWindow ran through WM_SESSION_DONE; a still-live window is destroyed here.
    unsafe {
        if IsWindow(window) != 0 {
            DestroyWindow(window);
        }
    }
    let failure = state.failure;
    let outcome = state.outcome.take();
    let mut warnings = std::mem::take(&mut state.warnings);
    warnings.sort();
    warnings.dedup();
    let result = match (failure, outcome) {
        (Some(error), _) => Err(error),
        (None, Some(Picked::Rect(rect))) => Ok(SessionOutcome {
            picked: Picked::Rect(rect),
            warnings,
        }),
        (None, Some(Picked::Cancelled(reason))) => Ok(SessionOutcome {
            picked: Picked::Cancelled(reason),
            warnings,
        }),
        (None, None) => Err(looped.err().unwrap_or(CaptureError::Cancelled)),
    };
    cleanup_state(state);
    result
}

fn cleanup_state(state: Box<OverlayState>) {
    unsafe {
        DeleteObject(state.dim_brush as HGDIOBJ);
        DeleteObject(state.border_pen as HGDIOBJ);
        DeleteObject(state.dash_pen as HGDIOBJ);
    }
    drop(state);
}

/// Requests the foreground state for the overlay; `false` from `SetForegroundWindow` is
/// never ignored (S02 §7.5).
fn activate(
    window: HWND,
    previous: HWND,
    policy: OverlayInputPolicy,
) -> Result<Option<String>, CaptureError> {
    if unsafe { SetForegroundWindow(window) } != 0 {
        return Ok(None);
    }
    if !previous.is_null() && is_usable_target(previous as isize) {
        let mut outcome: usize = 0;
        // Only after the freeze, only to the observed previous window, bounded and hung-safe.
        unsafe {
            SendMessageTimeoutW(
                previous,
                WM_CANCELMODE,
                0,
                0,
                SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                CANCELMODE_TIMEOUT_MS,
                &mut outcome,
            );
        }
    }
    if unsafe { SetForegroundWindow(window) } != 0 {
        return Ok(None);
    }
    if policy == OverlayInputPolicy::ThreadActive {
        unsafe {
            SetActiveWindow(window);
        }
        if unsafe { GetActiveWindow() } == window {
            return Ok(Some("foreground_not_claimed".to_string()));
        }
    }
    Err(CaptureError::FocusUnavailable)
}

/// The overlay's own message pump: cooperative cancel/stop polling plus the three
/// distinct `GetMessageW` outcomes.
fn message_loop(state: &mut OverlayState) -> Result<(), CaptureError> {
    let mut message: MSG = unsafe { std::mem::zeroed() };
    loop {
        if state.quit || state.finishing {
            return Ok(());
        }
        if state.flags.stop.load(Ordering::Acquire) {
            return Err(CaptureError::Cancelled);
        }
        if state.flags.cancel.load(Ordering::Acquire) {
            state.warnings.push("cancel_requested".to_string());
            state.finish(Picked::Cancelled(CancelReason::ClientDisconnected));
            return Ok(());
        }
        if state
            .flags
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            state.warnings.push("session_deadline".to_string());
            state.finish(Picked::Cancelled(CancelReason::ExplicitRequest));
            return Ok(());
        }
        let waited = unsafe {
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), POLL_MILLIS, QS_ALLINPUT, 0)
        };
        if waited == windows_sys::Win32::Foundation::WAIT_FAILED {
            return Err(CaptureError::Gdi {
                step: GdiStep::Input,
                os_code: unsafe { GetLastError() },
            });
        }
        if waited != windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
            // Timeout: the loop polls the cooperative flags and waits again.
            continue;
        }
        // A message is queued, so GetMessageW cannot block here.
        let received = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if received > 0 {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        } else if received == 0 {
            // A queued WM_QUIT means the whole native thread is shutting down.
            return Ok(());
        } else {
            // -1: the queue itself is broken; this is never a normal completion.
            return Err(CaptureError::Gdi {
                step: GdiStep::Input,
                os_code: unsafe { GetLastError() },
            });
        }
    }
}
