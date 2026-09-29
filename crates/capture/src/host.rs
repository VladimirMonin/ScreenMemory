//! The persistent native capture owner: one thread, one hidden message window, one
//! optional hotkey registration and a bounded command channel.
//!
//! * the hidden utility window exists for the whole adapter lifetime, so no window is
//!   created per capture and `PostQuitMessage` is never used to end a selection;
//! * the command channel is bounded (`CAPTURE_BACKPRESSURE`), so a third capture is
//!   refused before the freeze instead of queueing an unknown future scene;
//! * a hotkey reserves persistence inside the native dispatcher before any freeze;
//!   refusal never starts a native session and delivery must not block this thread.
use crate::{
    context,
    gdi::FrozenFrame,
    geometry::{self, DisplayTopology},
    hotkeys::{self, HotkeyBinding},
    overlay::{self, OverlayInputPolicy, OverlayObserver, Picked, SessionFlags},
    window::{self, ForegroundWindowProvider, TargetWindowProvider, WindowObservation},
    UNKNOWN_TEXT,
};
use sm_application::{
    capture::{
        CancelReason, CaptureError, CaptureMode, CaptureOutcome, CaptureRequest, CaptureResult,
        CaptureTrigger, GdiStep, NativeCaptureAdapter,
    },
    storage::PhysicalBounds,
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        GetWindowLongPtrW, MsgWaitForMultipleObjectsEx, PeekMessageW, PostThreadMessageW,
        RegisterClassExW, SetWindowLongPtrW, TranslateMessage, GWLP_USERDATA, MSG,
        MWMO_INPUTAVAILABLE, PM_NOREMOVE, QS_ALLINPUT, WM_APP, WM_DESTROY, WM_HOTKEY, WNDCLASSEXW,
    },
};

/// Thread message that wakes the idle native loop; it carries no data on purpose.
const WM_NATIVE_WAKE: u32 = WM_APP + 2;
/// Bounded wait of the idle loop that drains the command channel.
const POLL_MILLIS: u32 = 30;
/// Bounded command channel: one running command plus a small queue.
const COMMAND_CAPACITY: usize = 4;
const HOST_WINDOW_CLASS: &str = "ScreenMemoryCaptureHostV1";

/// One pre-freeze reservation. Dropping it without delivery releases its resources.
pub trait HotkeyReservation: Send {
    /// Transfer the owned outcome without waiting for persistence on the native thread.
    fn deliver(self: Box<Self>, outcome: Result<CaptureOutcome, CaptureError>);
}

/// Boundary consumer of captures started by the native hotkey path.
pub trait HotkeySink: Send + Sync {
    /// Nonblocking admission before a request, overlay, or frozen pixels exist.
    fn try_reserve(&self, mode: CaptureMode) -> Option<Box<dyn HotkeyReservation>>;
}

/// Sink of a host that only serves explicit commands.
pub struct NullHotkeySink;

impl HotkeySink for NullHotkeySink {
    fn try_reserve(&self, _mode: CaptureMode) -> Option<Box<dyn HotkeyReservation>> {
        None
    }
}

/// Trusted configuration of the native adapter. Every field is chosen by the host; no
/// field can be supplied by a client request.
pub struct CaptureHostConfig {
    /// Target provider of `window` mode (default: real foreground at execution).
    pub target_window: Arc<dyn TargetWindowProvider>,
    /// Optional overlay observer; production leaves this `None`.
    pub overlay: Option<Arc<dyn OverlayObserver>>,
    /// Optional global chord. `None` registers nothing and claims no system-wide key.
    pub hotkey: Option<HotkeyBinding>,
    /// Whether the configured chord is registered system-wide with `RegisterHotKey`.
    /// A host that only serves explicit commands leaves this `false` and claims no
    /// system-wide key; the `WM_HOTKEY` dispatch path is the same either way.
    pub register_hotkeys: bool,
    /// Boundary sink of the hotkey path.
    pub hotkey_sink: Option<Arc<dyn HotkeySink>>,
    /// Optional safety bound of one interactive session; production waits for the user.
    pub session_deadline: Option<Duration>,
    /// Input policy of the interactive overlay (S02 §7.5).
    pub input_policy: OverlayInputPolicy,
}

impl CaptureHostConfig {
    /// Production defaults: foreground target, no observer, no registered chord.
    pub fn new() -> Self {
        Self {
            target_window: Arc::new(ForegroundWindowProvider),
            overlay: None,
            hotkey: None,
            register_hotkeys: false,
            hotkey_sink: None,
            session_deadline: None,
            input_policy: OverlayInputPolicy::RequireForeground,
        }
    }
}

impl Default for CaptureHostConfig {
    fn default() -> Self {
        Self::new()
    }
}

enum Command {
    Run {
        request: CaptureRequest,
        reply: SyncSender<Result<CaptureOutcome, CaptureError>>,
    },
    Stop,
}

/// Command bookkeeping of the facade and the native thread, guarded by one mutex.
///
/// A command that is still queued behind a running session is a *known* operation of this
/// owner, so cancelling it is authorized; the request is remembered here and the queued
/// command is answered without ever freezing the screen (S02 §7.5).
struct Pending {
    /// Operation that owns the single native session right now, if any.
    current: Option<Uuid>,
    /// Operations this facade handed to the native thread and that have not answered yet.
    accepted: HashSet<Uuid>,
    /// Queued operations that were cancelled before their session started.
    cancelled: HashSet<Uuid>,
}

impl Pending {
    fn new() -> Self {
        Self {
            current: None,
            accepted: HashSet::new(),
            cancelled: HashSet::new(),
        }
    }
}

/// State shared between the facade and the native thread.
struct Shared {
    busy: AtomicBool,
    pending: Mutex<Pending>,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    failure: Mutex<Option<CaptureError>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            pending: Mutex::new(Pending::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            failure: Mutex::new(None),
        }
    }

    /// Register one command before the native thread can observe it.
    fn submit(&self, operation_id: Uuid) {
        self.pending
            .lock()
            .expect("pending lock")
            .accepted
            .insert(operation_id);
    }

    /// Drop the registration of a command that answered.
    fn retire(&self, operation_id: Uuid) {
        let mut pending = self.pending.lock().expect("pending lock");
        pending.accepted.remove(&operation_id);
        pending.cancelled.remove(&operation_id);
    }

    /// Signal exactly one known operation.
    ///
    /// * the running session is cancelled through the cooperative flag its overlay polls;
    /// * a command that is still queued behind another session is marked here, so it never
    ///   starts and therefore never freezes the screen;
    /// * an operation this owner never accepted is never signalled.
    fn cancel_request(&self, operation_id: Uuid) -> bool {
        let mut pending = self.pending.lock().expect("pending lock");
        if pending.current == Some(operation_id) {
            self.cancel.store(true, Ordering::Release);
            return true;
        }
        if pending.accepted.contains(&operation_id) {
            pending.cancelled.insert(operation_id);
            return true;
        }
        false
    }

    /// Claim the single session of the native thread.
    ///
    /// `Err` means this command was cancelled while it was still queued: the caller answers
    /// with its typed cancellation and freezes nothing. The cooperative flag is cleared
    /// while this operation is not yet published, so a cancel of an earlier session can
    /// never leak into this one and a cancel of this one is never lost.
    fn begin_session(&self, operation_id: Uuid) -> Result<(), CancelReason> {
        let mut pending = self.pending.lock().expect("pending lock");
        self.cancel.store(false, Ordering::Release);
        pending.current = Some(operation_id);
        if pending.cancelled.remove(&operation_id) {
            Err(CancelReason::ClientDisconnected)
        } else {
            Ok(())
        }
    }

    /// Release the single session of the native thread.
    fn end_session(&self) {
        self.pending.lock().expect("pending lock").current = None;
    }
}

/// Owner of one native capture controller thread.
pub struct CaptureHost {
    commands: SyncSender<Command>,
    thread_id: u32,
    window: isize,
    thread: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Shared>,
}

impl CaptureHost {
    /// Start the native thread, its hidden message window and the optional hotkey.
    pub fn start(config: CaptureHostConfig) -> Result<Self, CaptureError> {
        let (sender, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared::new());
        let thread_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("sm-capture-native".into())
            .spawn(move || run_native(config, receiver, thread_shared, ready_tx))
            .map_err(|_| CaptureError::Unavailable)?;
        let (thread_id, native_window) = match ready_rx.recv() {
            Ok(Ok(ready)) => ready,
            Ok(Err(error)) => {
                let _ = handle.join();
                return Err(error);
            }
            Err(_) => {
                let _ = handle.join();
                return Err(CaptureError::Unavailable);
            }
        };
        Ok(Self {
            commands: sender,
            thread_id,
            window: native_window,
            thread: Mutex::new(Some(handle)),
            shared,
        })
    }

    /// Run one session. Blocking: a region session waits for the user. The bounded channel
    /// refuses a new command with `Backpressure` instead of queueing unknown pixels.
    pub fn capture(&self, request: CaptureRequest) -> Result<CaptureOutcome, CaptureError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let operation_id = request.operation_id();
        // The operation becomes cancellable with its submission, not with the start of its
        // session: a request that waits behind another session is a known operation.
        self.shared.submit(operation_id);
        match self.commands.try_send(Command::Run {
            request,
            reply: reply_tx,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.retire(operation_id);
                return Err(CaptureError::Backpressure);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.retire(operation_id);
                return Err(CaptureError::Unavailable);
            }
        }
        // The thread owns a message queue from its first window creation on.
        unsafe {
            PostThreadMessageW(self.thread_id, WM_NATIVE_WAKE, 0, 0);
        }
        let outcome = reply_rx.recv().unwrap_or(Err(CaptureError::Unavailable));
        self.shared.retire(operation_id);
        outcome
    }

    /// Handle of this owner's hidden message window. It is an integration handle of the
    /// trusted host (tray/menu wiring); a client request can never carry or select it.
    pub fn native_window(&self) -> isize {
        self.window
    }

    /// Cancel exactly the named operation; a foreign operation id is never signalled. A
    /// command that is still queued behind the running session is cancelled too: its session
    /// must not start, so no freeze happens for it at all.
    pub fn cancel(&self, operation_id: Uuid) -> bool {
        self.shared.cancel_request(operation_id)
    }

    /// Typed failure of the native thread, if it already stopped.
    pub fn thread_failure(&self) -> Option<CaptureError> {
        *self.shared.failure.lock().expect("failure lock")
    }
}

impl Drop for CaptureHost {
    fn drop(&mut self) {
        // Stop first so a stuck interactive session polls the flag and unwinds.
        self.shared.stop.store(true, Ordering::Release);
        self.shared.cancel.store(true, Ordering::Release);
        let _ = self.commands.try_send(Command::Stop);
        unsafe {
            PostThreadMessageW(self.thread_id, WM_NATIVE_WAKE, 0, 0);
        }
        if let Some(handle) = self.thread.lock().expect("thread lock").take() {
            let _ = handle.join();
        }
    }
}

/// The production adapter: one native owner behind the application port.
pub struct WindowsCaptureAdapter {
    host: CaptureHost,
}

impl WindowsCaptureAdapter {
    pub fn start(config: CaptureHostConfig) -> Result<Self, CaptureError> {
        // Per-monitor-v2 before any window exists; a refused call is reported, not hidden.
        geometry::ensure_per_monitor_awareness();
        Ok(Self {
            host: CaptureHost::start(config)?,
        })
    }

    /// Native controller of this adapter (used by the host to observe thread failures).
    pub fn host(&self) -> &CaptureHost {
        &self.host
    }
}

impl NativeCaptureAdapter for WindowsCaptureAdapter {
    fn capture(&self, request: CaptureRequest) -> Result<CaptureOutcome, CaptureError> {
        self.host.capture(request)
    }

    fn cancel(&self, operation_id: Uuid) -> bool {
        self.host.cancel(operation_id)
    }
}

/// Native-thread state; its address is the `GWLP_USERDATA` of the hidden window.
struct ThreadState {
    config: CaptureHostConfig,
    shared: Arc<Shared>,
    binding: Option<HotkeyBinding>,
    quit: bool,
}

fn run_native(
    config: CaptureHostConfig,
    receiver: Receiver<Command>,
    shared: Arc<Shared>,
    ready: SyncSender<Result<(u32, isize), CaptureError>>,
) {
    let window = match create_host_window() {
        Ok(window) => window,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if let (Some(binding), true) = (config.hotkey, config.register_hotkeys) {
        if let Err(error) = hotkeys::register(window, &binding) {
            unsafe {
                DestroyWindow(window);
            }
            let _ = ready.send(Err(error));
            return;
        }
    }
    let mut state = Box::new(ThreadState {
        binding: config.hotkey,
        config,
        shared,
        quit: false,
    });
    unsafe {
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            (&mut *state as *mut ThreadState) as isize,
        );
    }
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok((thread_id, window as isize))).is_err() {
        // The facade is gone; release the registration and the window.
        unsafe {
            DestroyWindow(window);
        }
        return;
    }
    loop {
        while let Ok(command) = receiver.try_recv() {
            match command {
                Command::Run { request, reply } => {
                    let outcome = run_session(&mut state, request);
                    let _ = reply.send(outcome);
                }
                Command::Stop => state.quit = true,
            }
        }
        if state.quit {
            break;
        }
        let mut message: MSG = unsafe { std::mem::zeroed() };
        if unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) } != 0 {
            let received = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
            if received > 0 {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            } else if received == 0 {
                // A queued WM_QUIT is the normal end of this thread.
                break;
            } else {
                // -1: the queue is broken; this is a typed failure, not an empty loop.
                *state.shared.failure.lock().expect("failure lock") = Some(CaptureError::Gdi {
                    step: GdiStep::Input,
                    os_code: unsafe { GetLastError() },
                });
                break;
            }
            continue;
        }
        let waited = unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                POLL_MILLIS,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if waited == windows_sys::Win32::Foundation::WAIT_FAILED {
            *state.shared.failure.lock().expect("failure lock") = Some(CaptureError::Gdi {
                step: GdiStep::Input,
                os_code: unsafe { GetLastError() },
            });
            break;
        }
    }
    if let (Some(binding), true) = (state.binding, state.config.register_hotkeys) {
        hotkeys::unregister(window, &binding);
    }
    unsafe {
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        DestroyWindow(window);
    }
}

fn create_host_window() -> Result<HWND, CaptureError> {
    let name: Vec<u16> = HOST_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(host_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: module,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: name.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        // A repeated registration of the same class name in this process is not fatal.
        let existing = unsafe { GetLastError() };
        if existing != 1410 {
            return Err(CaptureError::Gdi {
                step: GdiStep::Overlay,
                os_code: existing,
            });
        }
    }
    // A real hidden top-level window: a message-only window would not receive every
    // broadcast/display notification (S02 §7.3).
    let window = unsafe {
        CreateWindowExW(
            0,
            name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        )
    };
    if window.is_null() {
        return Err(CaptureError::Gdi {
            step: GdiStep::Overlay,
            os_code: unsafe { GetLastError() },
        });
    }
    Ok(window)
}

unsafe extern "system" fn host_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
    if pointer == 0 {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    }
    let state = unsafe { &mut *(pointer as *mut ThreadState) };
    match message {
        WM_HOTKEY => {
            let id = wparam as i32;
            if let Some(binding) = state.binding {
                if binding.id == id {
                    hotkey_capture(state, binding.mode);
                }
            }
            0
        }
        WM_DESTROY => {
            state.quit = true;
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Hotkey path: the session runs on this thread, so the pixels exist before any sink or
/// UI work. A held chord never multiplies sessions (`MOD_NOREPEAT` plus the busy gate).
fn hotkey_capture(state: &mut ThreadState, mode: CaptureMode) {
    if state.shared.busy.load(Ordering::Acquire) {
        return;
    }
    let Some(reservation) = state
        .config
        .hotkey_sink
        .as_ref()
        .and_then(|sink| sink.try_reserve(mode))
    else {
        return;
    };
    let request = CaptureRequest::new(Uuid::new_v4(), mode, CaptureTrigger::Hotkey);
    let outcome = run_session(state, request);
    reservation.deliver(outcome);
}

/// One native session behind the single-session gate.
fn run_session(
    state: &mut ThreadState,
    request: CaptureRequest,
) -> Result<CaptureOutcome, CaptureError> {
    if state.shared.busy.swap(true, Ordering::AcqRel) {
        return Err(CaptureError::Busy);
    }
    let operation_id = request.operation_id();
    // A queued command that was cancelled while an earlier session owned the overlay is
    // excluded here, before any topology snapshot or freeze of this command.
    let result = match state.shared.begin_session(operation_id) {
        Err(reason) => Ok(CaptureOutcome::Cancelled {
            operation_id,
            reason,
        }),
        Ok(()) => session_body(state, &request),
    };
    state.shared.end_session();
    state.shared.busy.store(false, Ordering::Release);
    result
}

fn session_body(
    state: &ThreadState,
    request: &CaptureRequest,
) -> Result<CaptureOutcome, CaptureError> {
    let topology = geometry::snapshot()?;
    if request.mode() == CaptureMode::Region {
        // The capture instant of this frame: measured with the freeze, so neither the later
        // selection nor the crop/encode can stand in for it (S02 §6.3).
        let frozen_at_ms = unix_ms();
        let frame = FrozenFrame::freeze(&topology)?;
        let flags = SessionFlags {
            cancel: state.shared.cancel.clone(),
            stop: state.shared.stop.clone(),
            deadline: state
                .config
                .session_deadline
                .map(|bound| std::time::Instant::now() + bound),
        };
        let observer = state.config.overlay.clone();
        let session = overlay::pick(
            &frame,
            &topology,
            flags,
            observer,
            state.config.input_policy,
        )?;
        match session.picked {
            Picked::Rect(rect) => {
                // A display rebuilt between freeze and acceptance invalidates the
                // geometry; the operation ends with a typed topology refusal instead of
                // saving a crop that no longer matches the frame (S02 §6.2).
                if geometry::snapshot()?.signature != frame.topology_signature {
                    return Err(CaptureError::TopologyChanged);
                }
                let dib = frame.crop(rect)?;
                let result = assemble(
                    request,
                    &topology,
                    rect,
                    None,
                    dib,
                    session.warnings,
                    frozen_at_ms,
                );
                Ok(CaptureOutcome::Accepted(Box::new(result)))
            }
            Picked::Cancelled(reason) => Ok(CaptureOutcome::Cancelled {
                operation_id: request.operation_id(),
                reason,
            }),
        }
    } else {
        let (rect, observation) = fixed_target(state, &topology, request.mode())?;
        let frozen_at_ms = unix_ms();
        let frame = FrozenFrame::freeze(&topology)?;
        let dib = frame.crop(rect)?;
        let result = assemble(
            request,
            &topology,
            rect,
            observation.as_ref(),
            dib,
            Vec::new(),
            frozen_at_ms,
        );
        Ok(CaptureOutcome::Accepted(Box::new(result)))
    }
}

/// Target of the three non-interactive modes: no overlay exists for them.
fn fixed_target(
    state: &ThreadState,
    topology: &DisplayTopology,
    mode: CaptureMode,
) -> Result<(PhysicalBounds, Option<WindowObservation>), CaptureError> {
    match mode {
        CaptureMode::Desktop => Ok((topology.desktop, None)),
        CaptureMode::Monitor => {
            let (x, y) = geometry::cursor_position()?;
            // The monitor under the cursor at execution; no silent fallback to primary.
            let monitor = topology
                .monitor_for_point(x, y)
                .ok_or(CaptureError::Unavailable)?;
            Ok((monitor.bounds, None))
        }
        CaptureMode::Window => {
            let target = state.config.target_window.target_window();
            let observation = window::observe(&target, topology, &own_executable())?;
            Ok((observation.bounds, Some(observation)))
        }
        CaptureMode::Region => Err(CaptureError::Internal),
    }
}

/// One accepted crop with the instant of its own freeze. `frozen_at_ms` is passed in
/// because it belongs to the freeze of this frame, not to this later assembly step.
#[allow(clippy::too_many_arguments)]
fn assemble(
    request: &CaptureRequest,
    topology: &DisplayTopology,
    rect: PhysicalBounds,
    observation: Option<&WindowObservation>,
    dib: sm_application::capture::CapturedDib,
    mut warnings: Vec<String>,
    frozen_at_ms: u64,
) -> CaptureResult {
    let built = context::build(topology, request.target(), rect, observation);
    warnings.extend(built.warnings);
    warnings.sort();
    warnings.dedup();
    let (_, dpi) = topology.device_for_rect(rect);
    CaptureResult {
        request: request.clone(),
        context: built.context,
        target: observation.map(|value| value.to_target()),
        frame_dpi: dpi,
        frozen_at_ms,
        observed_at_ms: frozen_at_ms,
        dib,
        warnings,
    }
}

fn own_executable() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| UNKNOWN_TEXT.to_string())
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod hotkey_sink_tests {
    use super::*;

    #[test]
    fn null_sink_refuses_pre_freeze() {
        assert!(NullHotkeySink.try_reserve(CaptureMode::Region).is_none());
    }
}
