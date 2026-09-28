//! S02 capture seam: the trusted capture request/result types, the pure selection
//! geometry and the native adapter port.
//!
//! Ownership rules encoded here:
//! * a client never supplies a trigger, an HWND or a path — the trusted host adapter
//!   decides the trigger and the target;
//! * GDI handles, DCs, bitmaps and the overlay window never cross this boundary: only
//!   owned pixel bytes and plain values are returned (S02 §7.2);
//! * all coordinates are physical pixels of the virtual desktop with half-open
//!   `[x, x + width) × [y, y + height)` semantics (S02 §6.1).
use crate::{
    storage::{CaptureContext, CaptureTarget, PhysicalBounds},
    RequestContext,
};
use async_trait::async_trait;
pub use sm_protocol::application::{CancelReason, CaptureMode};
use sm_protocol::{
    application::{
        CaptureCommandParams, CaptureOutcome as WireCaptureOutcome, LibraryAssetChunk,
        LibraryAssetRequest,
    },
    errors::AppError,
};
use std::fmt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Per-side ceiling of the canonical codec (S02 §6.2). An image that cannot be stored
/// canonically is refused before it is accepted, never silently downscaled.
pub const MAX_CAPTURE_SIDE: u32 = 16_383;
/// Decoded-pixel budget shared with the codec and the memory permits.
pub const MAX_CAPTURE_PIXELS: u64 = 96_000_000;

/// What started one capture. Only the trusted host adapter sets this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureTrigger {
    /// Native hotkey: the target and the pixels are fixed in the native dispatcher.
    Hotkey,
    /// An authorized client command (GUI button, tray, CLI). This is not a hotkey and
    /// never claims that a foreign menu survived until the freeze.
    ClientCommand,
}

impl CaptureTrigger {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hotkey => "hotkey",
            Self::ClientCommand => "client_command",
        }
    }
}

/// One authorized capture intent. The operation identity is a real UUID minted by the
/// trusted caller; nothing in the request names a window, a monitor or a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureRequest {
    operation_id: Uuid,
    mode: CaptureMode,
    trigger: CaptureTrigger,
}

impl CaptureRequest {
    pub fn new(operation_id: Uuid, mode: CaptureMode, trigger: CaptureTrigger) -> Self {
        Self {
            operation_id,
            mode,
            trigger,
        }
    }

    /// Trusted conversion of one wire command. The trigger is supplied by the host
    /// adapter, never decoded from the request (S02 §3.2).
    pub fn from_command(params: CaptureCommandParams, trigger: CaptureTrigger) -> Self {
        Self::new(params.operation_id.as_uuid(), params.mode, trigger)
    }

    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn mode(&self) -> CaptureMode {
        self.mode
    }
    pub fn trigger(&self) -> CaptureTrigger {
        self.trigger
    }
    /// Stored capture target of this mode.
    pub fn target(&self) -> CaptureTarget {
        match self.mode {
            CaptureMode::Region => CaptureTarget::Region,
            CaptureMode::Window => CaptureTarget::ActiveWindow,
            CaptureMode::Monitor => CaptureTarget::Monitor,
            CaptureMode::Desktop => CaptureTarget::Desktop,
        }
    }
}

/// Owned top-down, 32-bit BGRX pixels of one accepted crop.
///
/// The row stride of the frozen DIB is preserved only as long as the caller keeps the
/// source buffer; the accepted crop is copied tightly so no GDI memory is referenced.
#[derive(Clone, PartialEq, Eq)]
pub struct CapturedDib {
    width: u32,
    height: u32,
    stride: u32,
    bytes: Vec<u8>,
}

impl CapturedDib {
    /// Adopt one top-down BGRX crop after strict length and geometry checks.
    pub fn from_bgra_topdown(
        width: u32,
        height: u32,
        stride: u32,
        bytes: Vec<u8>,
    ) -> Result<Self, CaptureError> {
        if width == 0 || height == 0 || width > MAX_CAPTURE_SIDE || height > MAX_CAPTURE_SIDE {
            return Err(CaptureError::LimitExceeded);
        }
        if u64::from(width) * u64::from(height) > MAX_CAPTURE_PIXELS {
            return Err(CaptureError::LimitExceeded);
        }
        let row = width.checked_mul(4).ok_or(CaptureError::LimitExceeded)?;
        if stride < row || !stride.is_multiple_of(4) {
            return Err(CaptureError::Internal);
        }
        let required = u64::from(stride) * u64::from(height);
        if required != bytes.len() as u64 {
            return Err(CaptureError::Internal);
        }
        Ok(Self {
            width,
            height,
            stride,
            bytes,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn stride(&self) -> u32 {
        self.stride
    }
    /// Raw top-down BGRX rows, including any padding up to `stride`.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Normalized tight RGBA8 rows: BGR becomes RGB and alpha is set opaque. The input is
    /// BGRX, not premultiplied RGBA, so no alpha channel of the screen is trusted.
    pub fn rgba8(&self) -> Vec<u8> {
        let row = (self.width as usize) * 4;
        let mut out = Vec::with_capacity(row * (self.height as usize));
        for line in self.bytes.chunks_exact(self.stride as usize) {
            for pixel in line[..row].as_chunks::<4>().0 {
                out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xff]);
            }
        }
        out
    }
}

/// Pixel content stays out of diagnostics: only geometry is printable.
impl fmt::Debug for CapturedDib {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturedDib")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// Observation of the target window at freeze time. `hwnd` is a diagnostic hex string
/// and is never an executable API parameter (S02 §5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetObservation {
    pub hwnd: String,
    pub pid: u32,
    pub executable: String,
    pub title: String,
    pub bounds: PhysicalBounds,
    pub dpi: u32,
}

/// One accepted capture: owned pixels plus the context that was observed with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureResult {
    pub request: CaptureRequest,
    pub context: CaptureContext,
    pub target: Option<TargetObservation>,
    /// Effective DPI of the display that owns the accepted crop center.
    pub frame_dpi: u32,
    /// Freeze instant; the time of a slow encode is never this value.
    pub frozen_at_ms: u64,
    /// Instant of the last accepted observation that is stored with the frame.
    pub observed_at_ms: u64,
    pub dib: CapturedDib,
    pub warnings: Vec<String>,
}

impl CaptureResult {
    /// Accepted geometry in physical pixels of the virtual desktop.
    pub fn bounds(&self) -> PhysicalBounds {
        self.context.bounds
    }
}

/// Terminal outcome of one native session. A cancel before an accepted crop never
/// deletes an image: there is nothing durable yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    Accepted(Box<CaptureResult>),
    Cancelled {
        operation_id: Uuid,
        reason: CancelReason,
    },
}

/// The native step that failed. A typed step plus the numeric OS code is reported;
/// a raw exception text with a window title or a path never is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GdiStep {
    Topology,
    DesktopDc,
    MemoryDc,
    DibSection,
    SelectObject,
    Blit,
    Flush,
    Crop,
    Overlay,
    Input,
    Cleanup,
}

impl GdiStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Topology => "topology",
            Self::DesktopDc => "desktop_dc",
            Self::MemoryDc => "memory_dc",
            Self::DibSection => "dib_section",
            Self::SelectObject => "select_object",
            Self::Blit => "blit",
            Self::Flush => "flush",
            Self::Crop => "crop",
            Self::Overlay => "overlay",
            Self::Input => "input",
            Self::Cleanup => "cleanup",
        }
    }
}

/// Typed capture failure. `safe_code` is the S02 §5.6 vocabulary used for diagnostics
/// and for mapping onto the stable S01 error envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// Another native session owns the single overlay.
    Busy,
    /// The bounded save slots are all taken; no pixels of an unknown future scene.
    Backpressure,
    /// The cached/foreground target disappeared or is not usable.
    TargetLost,
    /// Display geometry changed between freeze and acceptance.
    TopologyChanged,
    /// The overlay never obtained a controllable input state.
    FocusUnavailable,
    /// Environment refuses the capture (secure desktop, non-interactive session).
    Unavailable,
    /// The accepted image exceeds the canonical codec ceilings.
    LimitExceeded,
    /// One native step failed; the step and the numeric OS code are carried.
    Gdi { step: GdiStep, os_code: u32 },
    /// A zero-size or fully clipped selection.
    EmptySelection,
    /// The user or an authorized caller cancelled before acceptance.
    Cancelled,
    /// A boundary violation that must not be retried with the same inputs.
    Internal,
}

impl CaptureError {
    /// Closed S02 diagnostic code; never a path, title or provider string.
    pub const fn safe_code(self) -> &'static str {
        match self {
            Self::Busy => "CAPTURE_BUSY",
            Self::Backpressure => "CAPTURE_BACKPRESSURE",
            Self::TargetLost => "CAPTURE_TARGET_LOST",
            Self::TopologyChanged => "CAPTURE_TOPOLOGY_CHANGED",
            Self::FocusUnavailable => "CAPTURE_FOCUS_UNAVAILABLE",
            Self::Unavailable => "CAPTURE_UNAVAILABLE",
            Self::LimitExceeded => "IMAGE_LIMIT_EXCEEDED",
            Self::Gdi { .. } => "CAPTURE_GDI_FAILED",
            Self::EmptySelection => "CAPTURE_EMPTY_SELECTION",
            Self::Cancelled => "CAPTURE_CANCELLED",
            Self::Internal => "INTERNAL_FAILURE",
        }
    }
}

/// One native capture owner. The implementation is thread-confined internally and the
/// facade is `Send + Sync`: only owned results cross the boundary.
pub trait NativeCaptureAdapter: Send + Sync {
    /// Run one session. Blocking: a region session waits for the user.
    fn capture(&self, request: CaptureRequest) -> Result<CaptureOutcome, CaptureError>;
    /// Best-effort cancel of *this* operation only; `false` means no such session exists.
    fn cancel(&self, operation_id: Uuid) -> bool;
}

/// The capture port the authenticated session calls. The host owns the only
/// implementation; it never returns pixels, only the committed receipt (S02 §5.2).
#[async_trait]
pub trait CaptureApi: Send + Sync {
    async fn capture_command(
        &self,
        ctx: RequestContext,
        params: CaptureCommandParams,
        cancel: CancellationToken,
    ) -> Result<WireCaptureOutcome, AppError>;
    async fn read_asset(
        &self,
        ctx: RequestContext,
        request: LibraryAssetRequest,
    ) -> Result<LibraryAssetChunk, AppError>;
}

/// Pure selection geometry (S02 §6.1). Endpoints are sorted, the rectangle is intersected
/// with the frozen desktop and only then are the sizes computed; coordinates are never
/// clamped independently, so dragging past the left edge cannot widen the result.
///
/// All arithmetic runs in `i64`; the conversion back happens after the range check.
pub fn selection_rect(
    start: (i64, i64),
    end: (i64, i64),
    desktop: PhysicalBounds,
) -> Result<PhysicalBounds, CaptureError> {
    let left_edge = i64::from(desktop.x);
    let top_edge = i64::from(desktop.y);
    let right_edge = left_edge + i64::from(desktop.width);
    let bottom_edge = top_edge + i64::from(desktop.height);
    let left = start.0.min(end.0).max(left_edge);
    let top = start.1.min(end.1).max(top_edge);
    let right = start.0.max(end.0).min(right_edge);
    let bottom = start.1.max(end.1).min(bottom_edge);
    if right <= left || bottom <= top {
        return Err(CaptureError::EmptySelection);
    }
    let width = u32::try_from(right - left).map_err(|_| CaptureError::LimitExceeded)?;
    let height = u32::try_from(bottom - top).map_err(|_| CaptureError::LimitExceeded)?;
    let x = i32::try_from(left).map_err(|_| CaptureError::LimitExceeded)?;
    let y = i32::try_from(top).map_err(|_| CaptureError::LimitExceeded)?;
    Ok(PhysicalBounds {
        x,
        y,
        width,
        height,
    })
}

/// Offset of one accepted rectangle inside the frozen frame buffer.
pub fn buffer_offset(
    rect: PhysicalBounds,
    frame: PhysicalBounds,
) -> Result<(i64, i64), CaptureError> {
    let dx = i64::from(rect.x) - i64::from(frame.x);
    let dy = i64::from(rect.y) - i64::from(frame.y);
    if dx < 0
        || dy < 0
        || dx + i64::from(rect.width) > i64::from(frame.width)
        || dy + i64::from(rect.height) > i64::from(frame.height)
    {
        return Err(CaptureError::Internal);
    }
    Ok((dx, dy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{CaptureTarget, MAX_TITLE_LENGTH};

    fn desktop() -> PhysicalBounds {
        PhysicalBounds {
            x: -3840,
            y: 0,
            width: 7680,
            height: 2160,
        }
    }

    #[test]
    fn s02_geometry_01_selection_uses_documented_order_and_clipping() {
        // Documented example of S02 §6.1: (-2400,200) → (-1400,800) inside the frozen
        // desktop must yield exactly 1000x600 with the global origin preserved.
        let rect = selection_rect((-2400, 200), (-1400, 800), desktop()).unwrap();
        assert_eq!(
            rect,
            PhysicalBounds {
                x: -2400,
                y: 200,
                width: 1000,
                height: 600
            }
        );
        // Reversed endpoints select the same rectangle.
        assert_eq!(
            rect,
            selection_rect((-1400, 800), (-2400, 200), desktop()).unwrap()
        );
        // Crossing the left edge clips instead of widening around the origin.
        let clipped = selection_rect((-5000, 100), (-3000, 500), desktop()).unwrap();
        assert_eq!(
            clipped,
            PhysicalBounds {
                x: -3840,
                y: 100,
                width: 840,
                height: 400
            }
        );
        // Fully outside the desktop is an empty selection, never a negative size.
        assert_eq!(
            selection_rect((-5000, 100), (-4000, 500), desktop()).unwrap_err(),
            CaptureError::EmptySelection
        );
        // Zero-size drag.
        assert_eq!(
            selection_rect((10, 10), (10, 10), desktop()).unwrap_err(),
            CaptureError::EmptySelection
        );
        // Extreme endpoint values must not panic and must not wrap.
        assert_eq!(
            selection_rect(
                (i64::from(i32::MIN), i64::from(i32::MIN)),
                (i64::from(i32::MAX), i64::from(i32::MAX)),
                desktop()
            )
            .unwrap(),
            desktop()
        );
    }

    #[test]
    fn s02_geometry_02_buffer_offset_never_signals_an_out_of_frame_crop() {
        let frame = desktop();
        let rect = PhysicalBounds {
            x: -2400,
            y: 200,
            width: 1000,
            height: 600,
        };
        assert_eq!(buffer_offset(rect, frame).unwrap(), (1440, 200));
        let outside = PhysicalBounds {
            x: 4000,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(
            buffer_offset(outside, frame).unwrap_err(),
            CaptureError::Internal
        );
    }

    #[test]
    fn s02_capture_01_command_conversion_keeps_the_trusted_trigger() {
        let operation = Uuid::from_u128(0x42);
        let params = CaptureCommandParams {
            operation_id: sm_domain::OperationId::new(operation),
            mode: CaptureMode::Desktop,
        };
        let request = CaptureRequest::from_command(params, CaptureTrigger::ClientCommand);
        assert_eq!(request.operation_id(), operation);
        assert_eq!(request.mode(), CaptureMode::Desktop);
        assert_eq!(request.trigger(), CaptureTrigger::ClientCommand);
        assert_eq!(request.target(), CaptureTarget::Desktop);
        assert_eq!(request.trigger().as_str(), "client_command");
        // The mode vocabulary maps onto distinct stored targets.
        for (mode, target) in [
            (CaptureMode::Region, CaptureTarget::Region),
            (CaptureMode::Window, CaptureTarget::ActiveWindow),
            (CaptureMode::Monitor, CaptureTarget::Monitor),
            (CaptureMode::Desktop, CaptureTarget::Desktop),
        ] {
            let request = CaptureRequest::new(operation, mode, CaptureTrigger::Hotkey);
            assert_eq!(request.target(), target);
            assert_eq!(request.target().as_str(), target.as_str());
            assert!(!request.target().as_str().is_empty());
        }
    }

    #[test]
    fn s02_capture_02_dib_is_adopted_only_with_valid_geometry() {
        // 2x1 BGRX with one padding pixel (stride 16 for a 2-pixel row of 8 bytes).
        let bytes = vec![
            0x01, 0x02, 0x03, 0xff, 0x04, 0x05, 0x06, 0xff, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
            0xaa, 0xaa,
        ];
        let dib = CapturedDib::from_bgra_topdown(2, 1, 16, bytes).unwrap();
        assert_eq!(dib.width(), 2);
        assert_eq!(dib.height(), 1);
        assert_eq!(dib.stride(), 16);
        assert_eq!(dib.bytes().len(), 16);
        // Padding is never promoted into a pixel.
        assert_eq!(
            dib.rgba8(),
            vec![0x03, 0x02, 0x01, 0xff, 0x06, 0x05, 0x04, 0xff]
        );
        // A stride smaller than one row is refused.
        assert_eq!(
            CapturedDib::from_bgra_topdown(2, 1, 4, vec![0; 4]).unwrap_err(),
            CaptureError::Internal
        );
        // A wrong byte length is refused instead of reading beyond the buffer.
        assert_eq!(
            CapturedDib::from_bgra_topdown(2, 1, 8, vec![0; 7]).unwrap_err(),
            CaptureError::Internal
        );
        // Zero geometry and oversized sides are refused before any allocation.
        assert_eq!(
            CapturedDib::from_bgra_topdown(0, 1, 0, vec![]).unwrap_err(),
            CaptureError::LimitExceeded
        );
        assert_eq!(
            CapturedDib::from_bgra_topdown(MAX_CAPTURE_SIDE + 1, 1, 0, vec![]).unwrap_err(),
            CaptureError::LimitExceeded
        );
        // The pixel budget, not only the side limit, is enforced.
        assert_eq!(
            CapturedDib::from_bgra_topdown(16_000, 16_000, 64_000, vec![0; 64_000 * 16_000])
                .unwrap_err(),
            CaptureError::LimitExceeded
        );
        // Debug never prints pixel content.
        let debug = format!("{:?}", dib);
        assert!(debug.contains("CapturedDib") && debug.contains("stride"));
        assert!(!debug.contains("0x") && !debug.contains("aa"));
    }

    #[test]
    fn s02_capture_03_every_error_carries_a_distinct_closed_code() {
        let errors = [
            CaptureError::Busy,
            CaptureError::Backpressure,
            CaptureError::TargetLost,
            CaptureError::TopologyChanged,
            CaptureError::FocusUnavailable,
            CaptureError::Unavailable,
            CaptureError::LimitExceeded,
            CaptureError::Gdi {
                step: GdiStep::Blit,
                os_code: 6,
            },
            CaptureError::EmptySelection,
            CaptureError::Cancelled,
            CaptureError::Internal,
        ];
        let mut codes: Vec<&str> = errors.iter().map(|error| error.safe_code()).collect();
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), count, "every failure must have its own code");
        for code in codes {
            assert!(
                code.starts_with("CAPTURE_")
                    || code == "IMAGE_LIMIT_EXCEEDED"
                    || code == "INTERNAL_FAILURE"
            );
        }
        // A failed native step reports the step and the numeric OS code, nothing else.
        assert_eq!(GdiStep::Blit.as_str(), "blit");
        assert_eq!(
            CaptureError::Gdi {
                step: GdiStep::DesktopDc,
                os_code: 87
            }
            .safe_code(),
            "CAPTURE_GDI_FAILED"
        );
    }

    #[test]
    fn s02_capture_04_context_of_an_accepted_frame_is_storable() {
        let bounds = PhysicalBounds {
            x: -2400,
            y: 200,
            width: 1000,
            height: 600,
        };
        let context = CaptureContext {
            device: r"\\.\DISPLAY1".to_string(),
            app: "screenmemory-sm-capture-test".to_string(),
            title: "owned".to_string(),
            target: CaptureTarget::Region,
            sources: vec![CaptureTarget::Region],
            bounds,
        };
        assert!(context.to_json().is_ok());
        assert!(context.device.len() <= MAX_TITLE_LENGTH);
        let result = CaptureResult {
            request: CaptureRequest::new(Uuid::nil(), CaptureMode::Region, CaptureTrigger::Hotkey),
            context,
            target: None,
            frame_dpi: 144,
            frozen_at_ms: 1,
            observed_at_ms: 2,
            dib: CapturedDib::from_bgra_topdown(2, 1, 8, vec![0; 8]).unwrap(),
            warnings: vec![],
        };
        assert_eq!(result.bounds(), bounds);
        assert_eq!(result.frame_dpi, 144);
    }

    #[test]
    fn s02_capture_05_outcome_and_request_are_value_types() {
        let operation = Uuid::from_u128(7);
        let cancelled = CaptureOutcome::Cancelled {
            operation_id: operation,
            reason: CancelReason::Escape,
        };
        assert_eq!(cancelled.clone(), cancelled);
        assert_ne!(
            cancelled,
            CaptureOutcome::Cancelled {
                operation_id: operation,
                reason: CancelReason::EmptySelection,
            }
        );
        let request = CaptureRequest::new(operation, CaptureMode::Monitor, CaptureTrigger::Hotkey);
        assert_ne!(
            request,
            CaptureRequest::new(
                operation,
                CaptureMode::Monitor,
                CaptureTrigger::ClientCommand
            )
        );
    }
}
