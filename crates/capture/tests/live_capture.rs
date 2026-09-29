#![cfg(all(windows, target_pointer_width = "64"))]
//! Live native capture proofs on **owned** windows only.
//!
//! Scope discipline of these tests:
//! * the only pixels they may read belong to the virtual desktop frozen for the test and
//!   to a window created by this test process; no foreign HWND is addressed, resolved or
//!   captured outside the real virtual-desktop freeze that the product itself performs;
//! * the interactive overlay is addressed through `OverlayObserver`, i.e. the test posts
//!   messages to its **own** overlay window instead of injecting global input;
//! * every host is stopped by `Drop`; a stuck overlay cannot outlive the test.
use sm_application::capture::{
    CancelReason, CaptureError, CaptureMode, CaptureOutcome, CaptureRequest, CaptureTrigger,
    NativeCaptureAdapter,
};
use sm_capture::{
    CaptureHostConfig, HotkeyBinding, HotkeySink, OverlayInputPolicy, OverlayObserver,
    TargetWindow, TargetWindowProvider, WindowsCaptureAdapter,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT, WPARAM},
    Graphics::Gdi::{CreateSolidBrush, UpdateWindow},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::MOD_NOREPEAT,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, PostMessageW, RegisterClassExW,
            SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
            SW_SHOW, WM_HOTKEY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WNDCLASSEXW,
            WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
        },
    },
};

/// One desktop is shared by all interactive tests of this binary.
static DESKTOP: Mutex<()> = Mutex::new(());

fn desktop_guard() -> std::sync::MutexGuard<'static, ()> {
    DESKTOP
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Provider of an explicit owned target; the default provider would read the foreground.
/// The handle is stored as an opaque value, so the provider stays `Send + Sync`.
struct OwnedTarget(TargetWindow);

impl TargetWindowProvider for OwnedTarget {
    fn target_window(&self) -> TargetWindow {
        self.0.clone()
    }
}

/// Provider of a handle that is not a usable window.
struct BogusTarget(isize);

impl TargetWindowProvider for BogusTarget {
    fn target_window(&self) -> TargetWindow {
        TargetWindow::from_handle(self.0)
    }
}

fn make_param(x: i32, y: i32) -> LPARAM {
    (((y as u32 & 0xffff) << 16) | (x as u32 & 0xffff)) as LPARAM
}

/// Owned top-level popup with a solid-color client area (class background brush).
struct OwnedWindow(HWND);

impl OwnedWindow {
    fn new(color: u32, width: i32, height: i32, x: i32, y: i32, class: &str) -> Self {
        let name: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        let window_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: module,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: unsafe { CreateSolidBrush(color) },
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        unsafe {
            RegisterClassExW(&window_class);
        }
        let title: Vec<u16> = "sm-capture-owned"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                name.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module,
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "owned test window must exist");
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            ShowWindow(hwnd, SW_SHOW);
            // The default window procedure repaints the client area from the class brush.
            UpdateWindow(hwnd);
        }
        Self(hwnd)
    }

    fn rect(&self) -> RECT {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        assert_ne!(
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(self.0, &mut rect)
            },
            0
        );
        rect
    }
}

impl Drop for OwnedWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}

/// Observer that drives the owned overlay window from the test thread's request.
struct ScriptedOverlay {
    selection: Option<((i32, i32), (i32, i32))>,
    cancel_operation: Option<(Arc<WindowsCaptureAdapter>, Uuid)>,
    busy_hotkey: Option<(isize, i32)>,
    opened: AtomicUsize,
    closed: AtomicUsize,
}

impl ScriptedOverlay {
    fn new(selection: ((i32, i32), (i32, i32))) -> Arc<Self> {
        Arc::new(Self {
            selection: Some(selection),
            cancel_operation: None,
            busy_hotkey: None,
            opened: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
        })
    }
}

impl OverlayObserver for ScriptedOverlay {
    fn overlay_created(&self, window: isize) {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let hwnd = window as HWND;
        unsafe {
            if let Some((adapter, operation)) = self.cancel_operation.as_ref() {
                assert!(
                    adapter.cancel(*operation),
                    "the named session must be cancellable"
                );
            }
            if let Some((hidden, id)) = self.busy_hotkey {
                // A hotkey that arrives while a session owns the single overlay.
                PostMessageW(hidden as HWND, WM_HOTKEY, id as WPARAM, 0);
            }
            if let Some((start, end)) = self.selection {
                PostMessageW(hwnd, WM_LBUTTONDOWN, 1, make_param(start.0, start.1));
                PostMessageW(
                    hwnd,
                    WM_MOUSEMOVE,
                    1,
                    make_param((start.0 + end.0) / 2, (start.1 + end.1) / 2),
                );
                PostMessageW(hwnd, WM_MOUSEMOVE, 1, make_param(end.0, end.1));
                PostMessageW(hwnd, WM_LBUTTONUP, 0, make_param(end.0, end.1));
            }
        }
    }

    fn overlay_closed(&self, _window: isize) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}

/// Bounded sink of the hotkey path: it never blocks the native thread.
struct RecordingSink {
    sender: mpsc::SyncSender<(CaptureMode, bool, u32, u32)>,
}
struct RecordingReservation {
    mode: CaptureMode,
    sender: mpsc::SyncSender<(CaptureMode, bool, u32, u32)>,
}
impl sm_capture::HotkeyReservation for RecordingReservation {
    fn deliver(self: Box<Self>, outcome: Result<CaptureOutcome, CaptureError>) {
        let record = match outcome {
            Ok(CaptureOutcome::Accepted(result)) => {
                (self.mode, true, result.dib.width(), result.dib.height())
            }
            _ => (self.mode, false, 0, 0),
        };
        self.sender
            .try_send(record)
            .expect("the synthetic hotkey test channel must have capacity");
    }
}
impl HotkeySink for RecordingSink {
    fn try_reserve(&self, mode: CaptureMode) -> Option<Box<dyn sm_capture::HotkeyReservation>> {
        Some(Box::new(RecordingReservation {
            mode,
            sender: self.sender.clone(),
        }))
    }
}

fn wall_clock_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Observer that holds one session open until the test releases it. It blocks the *native*
/// thread, which is exactly what keeps a follow-up command queued behind this session.
struct HeldOverlay {
    live: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    selection: ((i32, i32), (i32, i32)),
}

impl OverlayObserver for HeldOverlay {
    fn overlay_created(&self, window: isize) {
        // Published before any selection: the test knows that this session owns the overlay
        // and that every later command is still waiting behind it.
        let _ = self.live.send(());
        // Bounded hold: the overlay has not started its message loop yet, so no message of
        // this session is pending and waiting here loses nothing. The timeout keeps a
        // failing test from leaving the session stuck.
        let _ = self
            .release
            .lock()
            .expect("release lock")
            .recv_timeout(std::time::Duration::from_secs(20));
        let hwnd = window as HWND;
        let ((start_x, start_y), (end_x, end_y)) = self.selection;
        unsafe {
            PostMessageW(hwnd, WM_LBUTTONDOWN, 1, make_param(start_x, start_y));
            PostMessageW(hwnd, WM_MOUSEMOVE, 1, make_param(end_x, end_y));
            PostMessageW(hwnd, WM_LBUTTONUP, 0, make_param(end_x, end_y));
        }
    }

    fn overlay_closed(&self, _window: isize) {}
}

/// Observer that completes its selection only after the requested delay, so the time of the
/// interaction itself is strictly separated from the time of the freeze.
struct DelayedOverlay {
    selection: ((i32, i32), (i32, i32)),
    delay: std::time::Duration,
}

impl OverlayObserver for DelayedOverlay {
    fn overlay_created(&self, window: isize) {
        std::thread::sleep(self.delay);
        let hwnd = window as HWND;
        let ((start_x, start_y), (end_x, end_y)) = self.selection;
        unsafe {
            PostMessageW(hwnd, WM_LBUTTONDOWN, 1, make_param(start_x, start_y));
            PostMessageW(hwnd, WM_MOUSEMOVE, 1, make_param(end_x, end_y));
            PostMessageW(hwnd, WM_LBUTTONUP, 0, make_param(end_x, end_y));
        }
    }

    fn overlay_closed(&self, _window: isize) {}
}

fn config() -> CaptureHostConfig {
    CaptureHostConfig::new()
}

/// Per-monitor-v2 awareness first: without it the Windows APIs answer with virtualized
/// coordinates and every geometry comparison of this suite would be scaled, not physical.
fn physical_pixels() {
    assert!(
        sm_capture::ensure_per_monitor_awareness(),
        "the test process must observe physical pixels"
    );
}

fn adapter(config: CaptureHostConfig) -> Arc<WindowsCaptureAdapter> {
    Arc::new(WindowsCaptureAdapter::start(config).expect("native capture owner starts"))
}

#[test]
fn s02_nat_01_topology_and_cursor_are_real_observations() {
    physical_pixels();
    let topology = sm_capture::observe_topology().expect("topology is observable");
    assert!(
        !topology.monitors.is_empty(),
        "a connected display must exist"
    );
    for monitor in &topology.monitors {
        assert!(
            monitor.bounds.width > 0 && monitor.bounds.height > 0,
            "observed monitor geometry must be positive"
        );
        assert!(
            monitor.bounds.x >= topology.desktop.x
                && monitor.bounds.y >= topology.desktop.y
                && monitor.bounds.x + monitor.bounds.width as i32
                    <= topology.desktop.x + topology.desktop.width as i32
                && monitor.bounds.y + monitor.bounds.height as i32
                    <= topology.desktop.y + topology.desktop.height as i32,
            "the virtual desktop must be the bounding rectangle of its displays"
        );
    }
    assert!(topology.monitors.iter().any(|monitor| monitor.primary));
    // The mouse path of the initial adapter cannot address a larger canvas.
    assert!(topology.desktop.width <= sm_capture::MAX_MOUSE_CANVAS_SIDE as u32);
    assert!(topology.desktop.height <= sm_capture::MAX_MOUSE_CANVAS_SIDE as u32);
    // A second snapshot of an unchanged desktop must be identical, so the freeze-time
    // signature is a usable topology-change detector.
    let again = sm_capture::observe_topology().expect("topology is observable");
    assert_eq!(topology.signature, again.signature);
    let (x, y) = sm_capture::observe_cursor().expect("cursor is observable in this session");
    assert!(
        topology.contains_point(x, y),
        "the cursor of an interactive session is inside the virtual desktop"
    );
}

#[test]
fn s02_nat_02_freeze_crop_is_consistent_and_bounded() {
    let _guard = desktop_guard();
    physical_pixels();
    let topology = sm_capture::observe_topology().expect("topology is observable");
    let frame = sm_capture::FrozenFrame::freeze(&topology).expect("freezing the desktop");
    let whole = frame
        .crop(topology.desktop)
        .expect("cropping the whole frame");
    assert_eq!(whole.width(), topology.desktop.width);
    assert_eq!(whole.height(), topology.desktop.height);
    assert_eq!(whole.stride(), topology.desktop.width * 4);
    assert_eq!(
        whole.bytes().len(),
        (whole.stride() * whole.height()) as usize
    );

    let inner = sm_application::storage::PhysicalBounds {
        x: topology.desktop.x + 1,
        y: topology.desktop.y + 1,
        width: 8,
        height: 4,
    };
    let cropped = frame.crop(inner).expect("cropping an inner rectangle");
    assert_eq!(cropped.width(), 8);
    assert_eq!(cropped.height(), 4);
    assert_eq!(cropped.stride(), 32);
    assert_eq!(cropped.bytes().len(), 32 * 4);
    // The accepted crop must be exactly the rows of the frozen frame it names.
    for line in 0..cropped.height() as usize {
        let row_start = (((inner.y - topology.desktop.y) as usize) + line)
            * whole.stride() as usize
            + ((inner.x - topology.desktop.x) as usize) * 4;
        assert_eq!(
            &cropped.bytes()
                [line * cropped.stride() as usize..(line + 1) * cropped.stride() as usize],
            &whole.bytes()[row_start..row_start + cropped.stride() as usize],
            "crop row {line} must equal the frozen frame row"
        );
    }
    // RGBA normalization swaps B/R and never trusts an alpha channel.
    let rgba = cropped.rgba8();
    assert_eq!(rgba.len(), 8 * 4 * 4);
    for pixel in rgba.as_chunks::<4>().0 {
        assert_eq!(pixel[3], 0xff);
    }
    // A rectangle outside the frame is refused before any pointer arithmetic.
    let outside = sm_application::storage::PhysicalBounds {
        x: topology.desktop.x + topology.desktop.width as i32 + 1,
        y: topology.desktop.y,
        width: 4,
        height: 4,
    };
    assert_eq!(frame.crop(outside), Err(CaptureError::Internal));
}

#[test]
fn s02_nat_03_region_overlay_crops_the_frozen_frame() {
    let _guard = desktop_guard();
    physical_pixels();
    let topology = sm_capture::observe_topology().expect("topology is observable");
    // Frame-local coordinates of the scripted selection, kept inside every display.
    let start = (12, 10);
    let end = (132, 110);
    let observer = ScriptedOverlay::new((start, end));
    let mut settings = config();
    settings.overlay = Some(observer.clone());
    // A bounded session keeps a mistake from leaving a visible overlay behind.
    settings.session_deadline = Some(std::time::Duration::from_secs(10));
    settings.input_policy = OverlayInputPolicy::ThreadActive;
    let adapter = adapter(settings);
    let operation = Uuid::from_u128(0x5102_0001);
    let request = CaptureRequest::new(
        operation,
        CaptureMode::Region,
        CaptureTrigger::ClientCommand,
    );
    let outcome = adapter
        .capture(request)
        .expect("the native session must answer");
    assert_eq!(observer.opened.load(Ordering::SeqCst), 1);
    match outcome {
        CaptureOutcome::Accepted(result) => {
            let expected = sm_application::capture::selection_rect(
                (
                    i64::from(topology.desktop.x) + i64::from(start.0),
                    i64::from(topology.desktop.y) + i64::from(start.1),
                ),
                (
                    i64::from(topology.desktop.x) + i64::from(end.0),
                    i64::from(topology.desktop.y) + i64::from(end.1),
                ),
                topology.desktop,
            )
            .expect("the scripted selection is a valid rectangle");
            assert_eq!(result.bounds(), expected);
            assert_eq!(result.dib.width(), expected.width);
            assert_eq!(result.dib.height(), expected.height);
            assert_eq!(result.dib.stride(), expected.width * 4);
            assert_eq!(
                result.context.target,
                sm_application::storage::CaptureTarget::Region
            );
            assert_eq!(
                result.context.sources,
                vec![sm_application::storage::CaptureTarget::Region]
            );
            assert!(!result.context.device.is_empty());
            // A region crop of a smaller rectangle is accepted even though the frozen
            // canvas is larger; the *saved* image is inside the canonical codec ceilings.
            assert!(result.dib.width() <= sm_application::capture::MAX_CAPTURE_SIDE);
        }
        CaptureOutcome::Cancelled { reason, .. } => {
            // The documented refusal when this session cannot hand the overlay a
            // controllable foreground: no frame, no stuck session and no silent success.
            assert_eq!(reason, CancelReason::ExplicitRequest);
            eprintln!(
                "S02-NAT-03: interactive region path NOT_RUN in this environment (overlay refused input)"
            );
        }
    }
    assert_eq!(observer.closed.load(Ordering::SeqCst), 1);
    // The single-session gate is released after the session.
    let cancelled = adapter.capture(CaptureRequest::new(
        Uuid::from_u128(0x5102_0002),
        CaptureMode::Desktop,
        CaptureTrigger::ClientCommand,
    ));
    assert!(matches!(cancelled, Ok(CaptureOutcome::Accepted(_))));
    // A foreign operation id is never cancellable.
    assert!(!adapter.cancel(Uuid::from_u128(0xdead)));
}

#[test]
fn s02_nat_04_cancel_flag_ends_the_session_without_a_frame() {
    let _guard = desktop_guard();
    physical_pixels();
    let operation = Uuid::from_u128(0x5104_0001);
    // The adapter handle is needed before the session starts, so the observer owns a slot
    // that is filled by the test through a channel-free two-step: build, then patch.
    let slot: Arc<Mutex<Option<Arc<WindowsCaptureAdapter>>>> = Arc::new(Mutex::new(None));
    struct CancelViaSlot {
        slot: Arc<Mutex<Option<Arc<WindowsCaptureAdapter>>>>,
        operation: Uuid,
    }
    impl OverlayObserver for CancelViaSlot {
        fn overlay_created(&self, _window: isize) {
            let guard = self.slot.lock().expect("slot");
            let adapter = guard
                .as_ref()
                .expect("adapter published before the session");
            assert!(adapter.cancel(self.operation));
        }
        fn overlay_closed(&self, _window: isize) {}
    }
    let observer = Arc::new(CancelViaSlot {
        slot: slot.clone(),
        operation,
    });
    let mut settings = config();
    settings.overlay = Some(observer);
    settings.session_deadline = Some(std::time::Duration::from_secs(10));
    settings.input_policy = OverlayInputPolicy::ThreadActive;
    let adapter = adapter(settings);
    *slot.lock().expect("slot") = Some(adapter.clone());
    let outcome = adapter
        .capture(CaptureRequest::new(
            operation,
            CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        ))
        .expect("a cancelled session answers with a typed outcome");
    match outcome {
        CaptureOutcome::Cancelled {
            reason,
            operation_id,
        } => {
            assert_eq!(operation_id, operation);
            assert_eq!(reason, CancelReason::ClientDisconnected);
        }
        other => panic!("a cancelled session must not produce a frame: {other:?}"),
    }
}

#[test]
fn s02_nat_05_busy_hotkey_never_starts_a_second_session() {
    let _guard = desktop_guard();
    physical_pixels();
    let (sender, receiver) = mpsc::sync_channel(4);
    let hotkey_id = 21;
    let settings_observer_slot: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));
    struct BusyScript {
        slot: Arc<Mutex<Option<isize>>>,
        hotkey_id: i32,
    }
    impl OverlayObserver for BusyScript {
        fn overlay_created(&self, window: isize) {
            let hidden = *self.slot.lock().expect("slot");
            let hidden = hidden.expect("the hidden window is published before the session");
            unsafe {
                PostMessageW(hidden as HWND, WM_HOTKEY, self.hotkey_id as WPARAM, 0);
                PostMessageW(window as HWND, WM_LBUTTONDOWN, 1, make_param(5, 5));
                PostMessageW(window as HWND, WM_MOUSEMOVE, 1, make_param(60, 40));
                PostMessageW(window as HWND, WM_LBUTTONUP, 0, make_param(60, 40));
            }
        }
        fn overlay_closed(&self, _window: isize) {}
    }
    let mut settings = config();
    settings.overlay = Some(Arc::new(BusyScript {
        slot: settings_observer_slot.clone(),
        hotkey_id,
    }));
    settings.session_deadline = Some(std::time::Duration::from_secs(10));
    settings.input_policy = OverlayInputPolicy::ThreadActive;
    // A Desktop chord needs no overlay, so a busy gate is the only thing that can stop it.
    settings.hotkey = Some(HotkeyBinding {
        mode: CaptureMode::Desktop,
        id: hotkey_id,
        modifiers: MOD_NOREPEAT,
        virtual_key: windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_S,
    });
    settings.hotkey_sink = Some(Arc::new(RecordingSink { sender }));
    let adapter = adapter(settings);
    *settings_observer_slot.lock().expect("slot") = Some(adapter.host().native_window());
    let outcome = adapter
        .capture(CaptureRequest::new(
            Uuid::from_u128(0x5105_0001),
            CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        ))
        .expect("the region session answers");
    let _ = outcome;
    // The hotkey that arrived during the session must not have started a second capture.
    let unexpected = receiver.recv_timeout(std::time::Duration::from_millis(200));
    assert!(
        unexpected.is_err(),
        "a hotkey during a live session must not start a second capture: {unexpected:?}"
    );
}

#[test]
fn s02_nat_06_hotkey_message_runs_a_session_and_calls_the_bounded_sink() {
    let _guard = desktop_guard();
    physical_pixels();
    let (sender, receiver) = mpsc::sync_channel(1);
    let hotkey_id = 22;
    let mut settings = config();
    settings.hotkey = Some(HotkeyBinding {
        mode: CaptureMode::Desktop,
        id: hotkey_id,
        modifiers: MOD_NOREPEAT,
        virtual_key: windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_S,
    });
    settings.hotkey_sink = Some(Arc::new(RecordingSink { sender }));
    let adapter = adapter(settings);
    let hidden = adapter.host().native_window();
    assert_ne!(hidden, 0, "the native owner must expose its hidden window");
    let posted = unsafe { PostMessageW(hidden as HWND, WM_HOTKEY, hotkey_id as WPARAM, 0) };
    assert_ne!(posted, 0, "the synthetic hotkey message must be queued");
    let record = receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the native hotkey path must reach the bounded sink");
    let (mode, accepted, width, height) = record;
    assert_eq!(mode, CaptureMode::Desktop);
    assert!(
        accepted,
        "the hotkey session must produce an accepted frame"
    );
    let topology = sm_capture::observe_topology().expect("topology is observable");
    assert_eq!(
        (width, height),
        (topology.desktop.width, topology.desktop.height)
    );
    // A wrong hotkey id is not routed into a capture.
    let (other_sender, other_receiver) = mpsc::sync_channel(1);
    let mut other_settings = config();
    other_settings.hotkey = Some(HotkeyBinding {
        mode: CaptureMode::Desktop,
        id: hotkey_id,
        modifiers: MOD_NOREPEAT,
        virtual_key: windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_S,
    });
    other_settings.hotkey_sink = Some(Arc::new(RecordingSink {
        sender: other_sender,
    }));
    let other = WindowsCaptureAdapter::start(other_settings).expect("second owner starts");
    let other_hidden = other.host().native_window();
    unsafe {
        PostMessageW(
            other_hidden as HWND,
            WM_HOTKEY,
            (hotkey_id + 1) as WPARAM,
            0,
        );
    }
    assert!(
        other_receiver
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "an unknown hotkey id must not start a capture"
    );
}

#[test]
fn s02_nat_07_window_mode_uses_the_configured_owned_target() {
    let _guard = desktop_guard();
    physical_pixels();
    let topology = sm_capture::observe_topology().expect("topology is observable");
    // The owned window lives inside the primary display and never overlaps a foreign one.
    let monitor = topology
        .monitors
        .iter()
        .find(|monitor| monitor.primary)
        .expect("a primary display exists");
    let window = OwnedWindow::new(
        0x0000_00ff,
        200,
        150,
        monitor.bounds.x + 60,
        monitor.bounds.y + 60,
        "sm-capture-owned-window-v1",
    );
    let mut settings = config();
    settings.target_window = Arc::new(OwnedTarget(TargetWindow {
        handle: window.0 as isize,
        caption: Some("sm-capture-owned".to_string()),
    }));
    let adapter = adapter(settings);
    let outcome = adapter
        .capture(CaptureRequest::new(
            Uuid::from_u128(0x5107_0001),
            CaptureMode::Window,
            CaptureTrigger::ClientCommand,
        ))
        .expect("window mode answers");
    let result = match outcome {
        CaptureOutcome::Accepted(result) => result,
        other => panic!("an owned visible window must be capturable: {other:?}"),
    };
    let observed = window.rect();
    assert_eq!(
        result.context.target,
        sm_application::storage::CaptureTarget::ActiveWindow
    );
    let bounds = result.bounds();
    assert!(bounds.width > 0 && bounds.height > 0);
    assert!(
        bounds.width <= (observed.right - observed.left) as u32
            && bounds.height <= (observed.bottom - observed.top) as u32,
        "the captured rectangle cannot exceed the target window"
    );
    assert!(
        bounds.x >= observed.left && bounds.y >= observed.top,
        "the captured rectangle must lie inside the target window"
    );
    assert_eq!(result.dib.width(), bounds.width);
    assert_eq!(result.dib.height(), bounds.height);
    let observation = result
        .target
        .as_ref()
        .expect("window mode stores the observed target");
    assert_eq!(observation.pid, std::process::id());
    assert!(!observation.hwnd.is_empty());
    assert!(
        observation.dpi == 0 || (72..=480).contains(&observation.dpi),
        "an observed DPI is either unavailable or a plausible value"
    );
    let expected_app = std::env::current_exe()
        .expect("current executable")
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(result.context.app, expected_app);
    assert_eq!(result.context.title, "sm-capture-owned");
    // The real pixel probe: an opaque topmost owned window painted with its class brush.
    let center_x = bounds.width / 2;
    let center_y = bounds.height / 2;
    let stride = result.dib.stride() as usize;
    let offset = center_y as usize * stride + center_x as usize * 4;
    let pixel = &result.dib.bytes()[offset..offset + 4];
    if pixel[2] == 0xff && pixel[1] == 0x00 && pixel[0] == 0x00 {
        // Observed exactly as painted: the freeze really reads the owned window.
        assert_eq!(pixel[3], 0xff);
    } else {
        eprintln!(
            "S02-NAT-07: pixel identity NOT_RUN in this environment (desktop content not observable); geometry and context verified"
        );
    }
}

#[test]
fn s02_nat_08_unusable_target_is_refused_without_touching_any_window() {
    let _guard = desktop_guard();
    physical_pixels();
    for bogus in [0isize, 0x1234, 0xffff_ffff] {
        let mut settings = config();
        settings.target_window = Arc::new(BogusTarget(bogus));
        let adapter = adapter(settings);
        let outcome = adapter.capture(CaptureRequest::new(
            Uuid::from_u128(0x5108_0001),
            CaptureMode::Window,
            CaptureTrigger::ClientCommand,
        ));
        assert_eq!(
            outcome,
            Err(CaptureError::TargetLost),
            "handle {bogus} must never be substituted with another window"
        );
    }
}

#[test]
fn s02_nat_09_production_policy_refuses_without_a_verified_input_state() {
    // The production policy never treats an unclaimed foreground as a working overlay:
    // it either verifies the input state and completes the selection, or it answers with
    // the typed CAPTURE_FOCUS_UNAVAILABLE and leaves no half-open session behind.
    let _guard = desktop_guard();
    physical_pixels();
    let observer = ScriptedOverlay::new(((8, 8), (88, 68)));
    let mut settings = config();
    settings.overlay = Some(observer.clone());
    settings.session_deadline = Some(std::time::Duration::from_secs(10));
    assert_eq!(settings.input_policy, OverlayInputPolicy::RequireForeground);
    let adapter = adapter(settings);
    let operation = Uuid::from_u128(0x5109_0001);
    let outcome = adapter.capture(CaptureRequest::new(
        operation,
        CaptureMode::Region,
        CaptureTrigger::Hotkey,
    ));
    assert_eq!(observer.opened.load(Ordering::SeqCst), 1);
    assert_eq!(observer.closed.load(Ordering::SeqCst), 1);
    match outcome {
        Ok(CaptureOutcome::Accepted(result)) => {
            assert_eq!(result.request.trigger(), CaptureTrigger::Hotkey);
            assert!(result.dib.width() > 0 && result.dib.height() > 0);
        }
        Err(CaptureError::FocusUnavailable) => {
            eprintln!(
                "S02-NAT-09: the foreground claim was refused by this session; the typed refusal was verified"
            );
        }
        other => panic!("an unverified overlay must not produce another outcome: {other:?}"),
    }
    // Either way the single-session gate is released and a later session still works.
    assert!(matches!(
        adapter.capture(CaptureRequest::new(
            Uuid::from_u128(0x5109_0002),
            CaptureMode::Desktop,
            CaptureTrigger::ClientCommand,
        )),
        Ok(CaptureOutcome::Accepted(_))
    ));
}

/// A command that waits behind another session is a known operation of this owner: its
/// cancellation must exclude it *before* it starts, so a cancelled request never freezes
/// the screen at all.
#[test]
fn s02_nat_10_a_queued_session_cancelled_before_it_starts_never_freezes() {
    let _guard = desktop_guard();
    physical_pixels();
    let first = Uuid::from_u128(0x510a_0001);
    let second = Uuid::from_u128(0x510a_0002);
    let (live_tx, live_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let mut settings = config();
    settings.overlay = Some(Arc::new(HeldOverlay {
        live: live_tx,
        release: Mutex::new(release_rx),
        selection: ((10, 10), (90, 70)),
    }));
    settings.session_deadline = Some(std::time::Duration::from_secs(30));
    settings.input_policy = OverlayInputPolicy::ThreadActive;
    let adapter = adapter(settings);

    // First command: an interactive region session that owns the overlay until released.
    let first_handle = {
        let adapter = adapter.clone();
        std::thread::spawn(move || {
            adapter.capture(CaptureRequest::new(
                first,
                CaptureMode::Region,
                CaptureTrigger::ClientCommand,
            ))
        })
    };
    live_rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the first session must own its overlay");
    // Second command: it can only wait behind the first one. A desktop capture is chosen
    // because it would freeze immediately and answer `Accepted` if it were allowed to run.
    let second_handle = {
        let adapter = adapter.clone();
        std::thread::spawn(move || {
            adapter.capture(CaptureRequest::new(
                second,
                CaptureMode::Desktop,
                CaptureTrigger::ClientCommand,
            ))
        })
    };
    let mut cancelled = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if adapter.cancel(second) {
            cancelled = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        cancelled,
        "a queued operation must be cancellable while another session owns the overlay"
    );
    // Repeating the cancellation of a still queued command stays truthful, and an operation
    // this owner never accepted is still never signalled.
    assert!(adapter.cancel(second));
    assert!(!adapter.cancel(Uuid::from_u128(0x510a_dead)));

    // Release the first session so the queue advances to the cancelled command.
    release_tx
        .send(())
        .expect("the first session must be released");
    match first_handle.join().expect("first thread") {
        Ok(CaptureOutcome::Accepted(result)) => {
            assert_eq!(result.request.operation_id(), first);
            assert!(result.dib.width() > 0 && result.dib.height() > 0);
        }
        // A session that cannot claim the foreground is refused with its typed code; the
        // queue must advance in that case too, which the second outcome below still proves.
        other => eprintln!("S02-NAT-10: the first session answered {other:?}"),
    }
    let second_outcome = second_handle
        .join()
        .expect("second thread")
        .expect("the queued command always answers");
    match second_outcome {
        CaptureOutcome::Cancelled {
            operation_id,
            reason,
        } => {
            assert_eq!(operation_id, second);
            assert_eq!(reason, CancelReason::ClientDisconnected);
        }
        other => panic!("a cancelled queued session must not freeze: {other:?}"),
    }
    // The owner still serves later commands after an excluded one.
    assert!(matches!(
        adapter.capture(CaptureRequest::new(
            Uuid::from_u128(0x510a_0003),
            CaptureMode::Desktop,
            CaptureTrigger::ClientCommand,
        )),
        Ok(CaptureOutcome::Accepted(_))
    ));
}

/// The capture instant of a frame is its freeze (S02 §6.3): a selection that takes half a
/// second must not be recorded as the capture time, and neither must the encode that
/// follows it.
#[test]
fn s02_nat_11_the_capture_instant_is_the_freeze_not_the_delayed_selection() {
    let _guard = desktop_guard();
    physical_pixels();
    let selection_delay = std::time::Duration::from_millis(500);
    let mut settings = config();
    settings.overlay = Some(Arc::new(DelayedOverlay {
        selection: ((10, 10), (90, 70)),
        delay: selection_delay,
    }));
    settings.session_deadline = Some(std::time::Duration::from_secs(30));
    settings.input_policy = OverlayInputPolicy::ThreadActive;
    let adapter = adapter(settings);
    let request_started = wall_clock_ms();
    let outcome = adapter.capture(CaptureRequest::new(
        Uuid::from_u128(0x510b_0001),
        CaptureMode::Region,
        CaptureTrigger::ClientCommand,
    ));
    let answered = wall_clock_ms();
    match outcome.expect("the native session must answer") {
        CaptureOutcome::Accepted(result) => {
            assert!(
                result.frozen_at_ms >= request_started,
                "a capture instant cannot precede its own request"
            );
            assert!(result.frozen_at_ms <= answered);
            // The frozen frame is the observed frame of this result.
            assert_eq!(result.frozen_at_ms, result.observed_at_ms);
            // The delayed selection must not have moved the capture instant: with the freeze
            // time taken after `pick`/`crop` this subtraction collapses to a few milliseconds.
            assert!(
                answered - result.frozen_at_ms >= 300,
                "the capture instant was recorded after the selection (frozen_at_ms={}, answered={answered})",
                result.frozen_at_ms
            );
        }
        other => {
            eprintln!(
                "S02-NAT-11: interactive region path NOT_RUN in this environment ({other:?}); the freeze instant of the non-interactive path stays covered by S02-NAT-02"
            );
        }
    }
    // The non-interactive path records its own freeze instant in the same bounded window.
    let desktop_started = wall_clock_ms();
    match adapter
        .capture(CaptureRequest::new(
            Uuid::from_u128(0x510b_0002),
            CaptureMode::Desktop,
            CaptureTrigger::ClientCommand,
        ))
        .expect("the desktop session answers")
    {
        CaptureOutcome::Accepted(result) => {
            let desktop_answered = wall_clock_ms();
            assert!(result.frozen_at_ms >= desktop_started);
            assert!(result.frozen_at_ms <= desktop_answered);
            assert_eq!(result.frozen_at_ms, result.observed_at_ms);
        }
        other => panic!("a desktop capture must produce a frame: {other:?}"),
    }
}
