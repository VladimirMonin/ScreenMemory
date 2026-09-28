//! GDI surface ownership: one top-down 32-bit DIB section per freeze and nothing else.
//!
//! Every native object is owned by a guard whose `Drop` restores or deletes exactly the
//! object it created. The frozen surface is `!Send` by construction (it stores the DIB
//! pointer), which is the compile-time proof that GDI memory stays on the native thread.
use crate::geometry::DisplayTopology;
use sm_application::{
    capture::{buffer_offset, CaptureError, CapturedDib, GdiStep},
    storage::PhysicalBounds,
};
use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HDC,
    HGDIOBJ, NOMIRRORBITMAP, SRCCOPY,
};

/// `SRCCOPY` plus the layered-window and no-mirroring flags fixed by S02 §7.1.
const FREEZE_ROP: u32 = SRCCOPY | CAPTUREBLT | NOMIRRORBITMAP;

/// Owned desktop DC (`GetDC(NULL)`), released with `ReleaseDC` and never `DeleteDC`.
struct ScreenDc {
    dc: HDC,
}

impl ScreenDc {
    fn acquire() -> Result<Self, CaptureError> {
        let hwnd = std::ptr::null_mut::<std::ffi::c_void>() as HWND;
        let dc = unsafe { GetDC(hwnd) };
        if dc.is_null() {
            return Err(CaptureError::Gdi {
                step: GdiStep::DesktopDc,
                os_code: unsafe { GetLastError() },
            });
        }
        Ok(Self { dc })
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(std::ptr::null_mut::<std::ffi::c_void>() as HWND, self.dc);
        }
    }
}

/// Owned memory DC, deleted with `DeleteDC` after its objects were restored.
struct MemoryDc {
    dc: HDC,
}

impl MemoryDc {
    fn create(screen: &ScreenDc) -> Result<Self, CaptureError> {
        let dc = unsafe { CreateCompatibleDC(screen.dc) };
        if dc.is_null() {
            return Err(CaptureError::Gdi {
                step: GdiStep::MemoryDc,
                os_code: unsafe { GetLastError() },
            });
        }
        Ok(Self { dc })
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.dc);
        }
    }
}

/// Owned top-down 32-bit DIB section with the pointer GDI handed out.
struct OwnedDib {
    bitmap: HGDIOBJ,
    /// Raw pointer into GDI-owned memory; valid until `DeleteObject` runs.
    bits: *mut u8,
    stride: u32,
}

impl OwnedDib {
    fn create(dc: HDC, width: u32, height: u32) -> Result<Self, CaptureError> {
        if width == 0 || height == 0 {
            return Err(CaptureError::Unavailable);
        }
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: i32::try_from(width).map_err(|_| CaptureError::Unavailable)?,
            // A negative height selects top-down rows: row 0 is the top scan line.
            biHeight: -i32::try_from(height).map_err(|_| CaptureError::Unavailable)?,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            bmiColors: [Default::default()],
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            return Err(CaptureError::Gdi {
                step: GdiStep::DibSection,
                os_code: unsafe { GetLastError() },
            });
        }
        Ok(Self {
            bitmap: bitmap as HGDIOBJ,
            bits: bits as *mut u8,
            stride: width.checked_mul(4).ok_or(CaptureError::Unavailable)?,
        })
    }
}

impl Drop for OwnedDib {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.bitmap);
        }
    }
}

/// Restores the bitmap that was selected in a DC before this guard existed.
///
/// The DDB/`GetDIBits` fallback of the donor is not ported, but the ordering defect it
/// exposed is still guarded here: the previous object is restored before the DIB is
/// deleted, and the memory DC is deleted after the bitmap.
struct SelectedBitmap {
    dc: HDC,
    previous: HGDIOBJ,
}

impl SelectedBitmap {
    fn select(dc: HDC, bitmap: HGDIOBJ) -> Result<Self, CaptureError> {
        let previous = unsafe { SelectObject(dc, bitmap) };
        if previous.is_null() {
            return Err(CaptureError::Gdi {
                step: GdiStep::SelectObject,
                os_code: unsafe { GetLastError() },
            });
        }
        Ok(Self { dc, previous })
    }
}

impl Drop for SelectedBitmap {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
        }
    }
}

/// One frozen virtual desktop. Field order is the cleanup order: the selected object is
/// restored first, then the DIB is deleted, then the memory DC is deleted.
pub struct FrozenFrame {
    _selected: SelectedBitmap,
    dib: OwnedDib,
    _dc: MemoryDc,
    pub bounds: PhysicalBounds,
    pub dpi: u32,
    pub topology_signature: u64,
}

impl FrozenFrame {
    /// Single freeze: blit the whole virtual desktop into a top-down DIB and flush GDI
    /// before any CPU access. The desktop DC is released before this returns.
    pub fn freeze(topology: &DisplayTopology) -> Result<Self, CaptureError> {
        let screen = ScreenDc::acquire()?;
        let dc = MemoryDc::create(&screen)?;
        let dib = OwnedDib::create(dc.dc, topology.desktop.width, topology.desktop.height)?;
        let selected = SelectedBitmap::select(dc.dc, dib.bitmap)?;
        let width = i32::try_from(topology.desktop.width).map_err(|_| CaptureError::Unavailable)?;
        let height =
            i32::try_from(topology.desktop.height).map_err(|_| CaptureError::Unavailable)?;
        let blitted = unsafe {
            BitBlt(
                dc.dc,
                0,
                0,
                width,
                height,
                screen.dc,
                topology.desktop.x,
                topology.desktop.y,
                FREEZE_ROP,
            )
        };
        if blitted == 0 {
            return Err(CaptureError::Gdi {
                step: GdiStep::Blit,
                os_code: unsafe { GetLastError() },
            });
        }
        if unsafe { GdiFlush() } == 0 {
            return Err(CaptureError::Gdi {
                step: GdiStep::Flush,
                os_code: unsafe { GetLastError() },
            });
        }
        let dpi = topology
            .monitor_for_rect(topology.desktop)
            .map(|monitor| monitor.dpi)
            .unwrap_or(0);
        Ok(Self {
            _selected: selected,
            dib,
            _dc: dc,
            bounds: topology.desktop,
            dpi,
            topology_signature: topology.signature,
        })
    }

    /// Copy exactly the accepted rectangle out of the frozen frame. The DIB is flushed
    /// again before the CPU reads it, and only these rows are copied.
    pub fn crop(&self, rect: PhysicalBounds) -> Result<CapturedDib, CaptureError> {
        let (offset_x, offset_y) = buffer_offset(rect, self.bounds)?;
        if unsafe { GdiFlush() } == 0 {
            return Err(CaptureError::Gdi {
                step: GdiStep::Flush,
                os_code: unsafe { GetLastError() },
            });
        }
        let row = rect.width.checked_mul(4).ok_or(CaptureError::Unavailable)?;
        let rows = usize::try_from(rect.height).map_err(|_| CaptureError::Unavailable)?;
        let mut bytes = Vec::with_capacity(row as usize * rows);
        let stride = self.dib.stride as usize;
        for line in 0..rows {
            let start = (offset_y as usize + line) * stride + offset_x as usize * 4;
            // The offset was validated against the frozen frame; the read stays inside it.
            let source =
                unsafe { std::slice::from_raw_parts(self.dib.bits.add(start), row as usize) };
            bytes.extend_from_slice(source);
        }
        CapturedDib::from_bgra_topdown(rect.width, rect.height, row, bytes)
    }

    /// Raw frame DC used by the overlay for its single base blit.
    pub(crate) fn dc(&self) -> HDC {
        self._dc.dc
    }
}

/// Only geometry is printable; the pointer is never rendered.
impl std::fmt::Debug for FrozenFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrozenFrame")
            .field("bounds", &self.bounds)
            .field("dpi", &self.dpi)
            .field("stride", &self.dib.stride)
            .finish()
    }
}
