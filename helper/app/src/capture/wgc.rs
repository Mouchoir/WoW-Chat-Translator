//! Windows.Graphics.Capture on one window (reused from the exe-tech capture probe).
//! Must run on an MTA thread; the frame pool is free-threaded and polled, no event handlers.
//!
//! CPU cost (P4b): only the requested sub-rectangles leave the GPU. Each tick issues every region's
//! copy into its own reusable staging texture first and then maps them, so the thread waits for the
//! GPU once per tick. The decoder and the tag scan read the mapped staging memory directly (no
//! copy, no allocation per tick; only the pixels they look at are touched) until `release`. The
//! frame rate WGC delivers follows the capture rate (`set_min_interval`), because every delivered
//! frame costs CPU in this process even when nobody reads it. Each region index has two staging
//! textures used in turn, so the previous capture of a region stays readable
//! ([`Grabber::previous`]) without any copy.
//!
//! Measured (RTX 3080, game running): a blocking `Map` on the staging copy spin-waits until the
//! GPU has done the copy, and with the driver's threading optimizations a driver worker thread
//! spins as well; at 30 Hz that alone cost about 7% of a core. The device is therefore created
//! with `PREVENT_INTERNAL_THREADING_OPTIMIZATIONS`, and the thread sleeps on an `ID3D11Fence`
//! event until the copies are done, so `Map` never waits.
//!
//! Creating the D3D11 device costs about 0.25 s of CPU (measured: the driver starts), so a
//! [`WgcDevice`] outlives its capture session: the pipeline gets it back with
//! [`Grabber::into_wgc_device`] and reuses it when the game window comes back.

use std::time::Instant;

use tp_bridge::image::{Bgra, Rect};
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use super::{clip, GrabStatus, GrabTiming, Grabber, Method};
use crate::wow::{hwnd, WowInfo};

/// The D3D11 device (and its GPU fence) a capture session runs on.
pub struct WgcDevice {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    d3d: IDirect3DDevice,
    gpu_wait: Option<GpuWait>,
}

impl WgcDevice {
    pub fn create() -> windows::core::Result<WgcDevice> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                // No driver worker thread (it spins after each submission); see the module notes.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let device = device.ok_or_else(windows::core::Error::empty)?;
            let ctx = ctx.ok_or_else(windows::core::Error::empty)?;
            let dxgi: IDXGIDevice = device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let gpu_wait = GpuWait::new(&device, &ctx);
            Ok(WgcDevice { device, ctx, d3d, gpu_wait })
        }
    }
}

pub struct WgcGrabber {
    target: isize,
    /// Always Some, except while `into_wgc_device` hands it back.
    dev: Option<WgcDevice>,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    /// Two staging textures per region index, used in turn.
    slots: Vec<Slot>,
    /// The regions of the last fresh grab, mapped until `release` (one entry per region).
    mapped: Vec<Option<Mapped>>,
    /// The previous capture of each of those regions (same rectangle), with its grab number.
    prev_mapped: Vec<Option<(Mapped, u64)>>,
    interval_ms: u32,
    frames: u64,
    timing: GrabTiming,
}

/// Sleeps (no spinning) until the GPU has executed the commands issued so far.
struct GpuWait {
    ctx4: ID3D11DeviceContext4,
    fence: ID3D11Fence,
    value: u64,
    event: HANDLE,
}

impl GpuWait {
    fn new(device: &ID3D11Device, ctx: &ID3D11DeviceContext) -> Option<GpuWait> {
        unsafe {
            let dev5: ID3D11Device5 = device.cast().ok()?;
            let ctx4: ID3D11DeviceContext4 = ctx.cast().ok()?;
            let mut fence: Option<ID3D11Fence> = None;
            dev5.CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence).ok()?;
            let event = CreateEventW(None, false, false, None).ok()?;
            Some(GpuWait { ctx4, fence: fence?, value: 0, event })
        }
    }

    /// True when the GPU finished within `timeout_ms`.
    fn wait(&mut self, timeout_ms: u32) -> bool {
        self.value += 1;
        unsafe {
            if self.ctx4.Signal(&self.fence, self.value).is_err() {
                return false;
            }
            self.ctx4.Flush();
            if self.fence.GetCompletedValue() >= self.value {
                return true;
            }
            if self.fence.SetEventOnCompletion(self.value, self.event).is_err() {
                return false;
            }
            WaitForSingleObject(self.event, timeout_ms) == WAIT_OBJECT_0
        }
    }
}

impl Drop for GpuWait {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

struct Staging {
    tex: ID3D11Texture2D,
    w: u32,
    h: u32,
}

/// The two staging textures of one region index.
#[derive(Default)]
struct Slot {
    buf: [Option<Staging>; 2],
    /// What each texture holds: the region's rectangle and the grab number; None while unknown.
    held: [Option<([i32; 4], u64)>; 2],
    /// The texture with the newest capture.
    cur: usize,
}

fn rect4(r: Rect) -> [i32; 4] {
    [r.x, r.y, r.w, r.h]
}

/// A mapped staging texture holding one region (client coordinates `origin` + `w` x `h`).
struct Mapped {
    tex: ID3D11Texture2D,
    ptr: *const u8,
    pitch: u32,
    w: u32,
    h: u32,
    origin: (i32, i32),
}

/// True when Windows.Graphics.Capture is available on this system.
pub fn supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

fn timespan_ms(ms: u32) -> windows::Foundation::TimeSpan {
    windows::Foundation::TimeSpan { Duration: i64::from(ms) * 10_000 }
}

impl WgcGrabber {
    /// Starts capturing `target` on a new device. `min_interval_ms` caps the frame rate WGC
    /// delivers (0 = none).
    pub fn open(target: isize, min_interval_ms: u32) -> windows::core::Result<WgcGrabber> {
        WgcGrabber::open_with(target, min_interval_ms, None)
    }

    /// Starts capturing `target` on `dev` (a device kept from an earlier session), or on a new
    /// device when None. A device whose open fails is dropped.
    pub fn open_with(target: isize, min_interval_ms: u32, dev: Option<WgcDevice>) -> windows::core::Result<WgcGrabber> {
        unsafe {
            // S_FALSE when the thread is already in the MTA; an STA caller gets an error we ignore.
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            // Borderless capture: allowed silently for unpackaged apps on Windows 11.
            if let Ok(op) = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless) {
                let _ = op.join();
            }
            let dev = match dev {
                Some(d) => d,
                None => WgcDevice::create()?,
            };
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd(target))?;
            let size = item.Size()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&dev.d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);
            if min_interval_ms > 0 {
                let _ = session.SetMinUpdateInterval(timespan_ms(min_interval_ms));
            }
            session.StartCapture()?;
            Ok(WgcGrabber {
                target,
                dev: Some(dev),
                pool,
                session,
                pool_size: size,
                slots: Vec::new(),
                mapped: Vec::new(),
                prev_mapped: Vec::new(),
                interval_ms: min_interval_ms,
                frames: 0,
                timing: GrabTiming::default(),
            })
        }
    }

    pub fn target(&self) -> isize {
        self.target
    }

    fn dev(&self) -> &WgcDevice {
        self.dev.as_ref().expect("device present until into_wgc_device")
    }

    /// Frames read since `open`.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The current cap on the delivered frame rate (ms between frames).
    pub fn min_interval_ms(&self) -> u32 {
        self.interval_ms
    }

    /// Staging texture `b` of region `i`, at least `w` x `h`. A texture is replaced only when the
    /// region outgrows it, or shrinks to less than half its area.
    fn staging(&mut self, i: usize, b: usize, w: u32, h: u32) -> Option<ID3D11Texture2D> {
        if self.slots.len() <= i {
            self.slots.resize_with(i + 1, Slot::default);
        }
        if let Some(st) = &self.slots[i].buf[b] {
            let fits = st.w >= w && st.h >= h;
            let wasteful = u64::from(st.w) * u64::from(st.h) > 2 * u64::from(w) * u64::from(h) + 4096;
            if fits && !wasteful {
                return Some(st.tex.clone());
            }
        }
        self.slots[i].buf[b] = None;
        self.slots[i].held[b] = None;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe { self.dev().device.CreateTexture2D(&desc, None, Some(&mut tex)) }.ok()?;
        let tex = tex?;
        self.slots[i].buf[b] = Some(Staging { tex: tex.clone(), w, h });
        Some(tex)
    }

    /// The newest pending frame, closing older ones.
    fn newest_frame(&mut self) -> Option<Direct3D11CaptureFrame> {
        let mut newest: Option<Direct3D11CaptureFrame> = None;
        for _ in 0..8 {
            match self.pool.TryGetNextFrame() {
                Ok(f) => {
                    if let Some(old) = newest.replace(f) {
                        let _ = old.Close();
                    }
                }
                Err(_) => break,
            }
        }
        newest
    }

    /// Unmaps the regions of the last grab (and their previous captures).
    fn unmap(&mut self) {
        let Some(dev) = self.dev.as_ref() else { return };
        for m in self.mapped.drain(..).flatten() {
            unsafe { dev.ctx.Unmap(&m.tex, 0) };
        }
        for (m, _) in self.prev_mapped.drain(..).flatten() {
            unsafe { dev.ctx.Unmap(&m.tex, 0) };
        }
    }

    fn map(&self, tex: &ID3D11Texture2D, c: [i32; 4]) -> Option<Mapped> {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        if unsafe { self.dev().ctx.Map(tex, 0, D3D11_MAP_READ, 0, Some(&mut m)) }.is_err() || m.pData.is_null() {
            return None;
        }
        Some(Mapped { tex: tex.clone(), ptr: m.pData as *const u8, pitch: m.RowPitch, w: c[2] as u32, h: c[3] as u32, origin: (c[0], c[1]) })
    }

    fn view(m: &Mapped) -> Bgra<'_> {
        let len = m.pitch as usize * (m.h as usize - 1) + m.w as usize * 4;
        // SAFETY: the texture stays mapped until `unmap`, which needs `&mut self`, so the slice
        // cannot outlive the mapping; the mapping holds `h` rows of `pitch` bytes.
        let px = unsafe { std::slice::from_raw_parts(m.ptr, len) };
        Bgra { px, width: m.w, height: m.h, stride: m.pitch, origin_x: m.origin.0, origin_y: m.origin.1 }
    }

    /// Copies the regions of one frame to their staging textures and maps them.
    fn read_frame(&mut self, frame: &Direct3D11CaptureFrame, wow: &WowInfo, regions: &[Rect]) -> Option<GrabStatus> {
        let content = frame.ContentSize().ok()?;
        if content.Width != self.pool_size.Width || content.Height != self.pool_size.Height {
            // The window was resized: new buffers, and skip this frame.
            self.pool.Recreate(&self.dev().d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, content).ok()?;
            self.pool_size = content;
            return Some(GrabStatus::NoNewFrame);
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface().ok()?.cast().ok()?;
        let tex: ID3D11Texture2D = unsafe { access.GetInterface() }.ok()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        let fw = (desc.Width as i32).min(content.Width);
        let fh = (desc.Height as i32).min(content.Height);
        // Pass 1: every region's GPU copy (client region to frame coordinates, clipped to the
        // client and to the frame).
        let t_copy = Instant::now();
        let mut jobs: Vec<Option<(Rect, ID3D11Texture2D, usize)>> = Vec::with_capacity(regions.len());
        for (i, r) in regions.iter().enumerate() {
            let c = clip(*r, wow.client_w, wow.client_h).and_then(|c| {
                let fr = Rect { x: c.x + wow.frame_dx, y: c.y + wow.frame_dy, w: c.w, h: c.h };
                clip(fr, fw, fh).map(|f| Rect { x: f.x - wow.frame_dx, y: f.y - wow.frame_dy, w: f.w, h: f.h })
            });
            let Some(c) = c else {
                jobs.push(None);
                continue;
            };
            let (w, h) = (c.w as u32, c.h as u32);
            // Write the texture that does not hold the newest capture of this region.
            if self.slots.len() <= i {
                self.slots.resize_with(i + 1, Slot::default);
            }
            let slot = &self.slots[i];
            let wb = if slot.held[slot.cur].is_some() { 1 - slot.cur } else { slot.cur };
            self.slots[i].held[wb] = None;
            let staging = self.staging(i, wb, w, h)?;
            let left = (c.x + wow.frame_dx) as u32;
            let top = (c.y + wow.frame_dy) as u32;
            let b = D3D11_BOX { left, top, front: 0, right: left + w, bottom: top + h, back: 1 };
            unsafe { self.dev().ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &tex, 0, Some(&b)) };
            jobs.push(Some((c, staging, wb)));
        }
        // Pass 2: sleep until the copies are done (no spin-wait in Map), then map and read back.
        let t_wait = Instant::now();
        self.timing.copy = t_wait - t_copy;
        if jobs.iter().any(|j| j.is_some()) {
            if let Some(g) = self.dev.as_mut().and_then(|d| d.gpu_wait.as_mut()) {
                g.wait(250);
            }
        }
        let t_read = Instant::now();
        self.timing.wait = t_read - t_wait;
        // A grab number is never used twice, even when this grab fails half-way.
        self.frames += 1;
        let number = self.frames;
        for (i, job) in jobs.into_iter().enumerate() {
            let Some((c, staging, wb)) = job else {
                self.mapped.push(None);
                self.prev_mapped.push(None);
                continue;
            };
            let Some(m) = self.map(&staging, rect4(c)) else {
                self.unmap();
                return None;
            };
            self.mapped.push(Some(m));
            // The other texture, when it holds the same rectangle (no GPU work is pending on it).
            let prev = match self.slots[i].held[1 - wb] {
                Some((r, n)) if r == rect4(c) => self.slots[i].buf[1 - wb].as_ref().map(|st| st.tex.clone()).and_then(|t| self.map(&t, r)).map(|m| (m, n)),
                _ => None,
            };
            self.prev_mapped.push(prev);
            let slot = &mut self.slots[i];
            slot.cur = wb;
            slot.held[wb] = Some((rect4(c), number));
        }
        self.timing.read = t_read.elapsed();
        Some(GrabStatus::Fresh)
    }
}

impl Drop for WgcGrabber {
    fn drop(&mut self) {
        self.unmap();
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl Grabber for WgcGrabber {
    fn method(&self) -> Method {
        Method::Wgc
    }

    fn skip(&mut self) {
        self.unmap();
        // The pool holds 2 frames and drops newer ones while full: release them.
        if let Some(f) = self.newest_frame() {
            let _ = f.Close();
        }
    }

    fn image(&self, i: usize) -> Option<Bgra<'_>> {
        self.mapped.get(i)?.as_ref().map(WgcGrabber::view)
    }

    fn previous(&self, i: usize) -> Option<(Bgra<'_>, u64)> {
        self.prev_mapped.get(i)?.as_ref().map(|(m, n)| (WgcGrabber::view(m), *n))
    }

    fn frame_number(&self) -> u64 {
        self.frames
    }

    fn release(&mut self) {
        self.unmap();
    }

    fn into_wgc_device(mut self: Box<Self>) -> Option<WgcDevice> {
        self.unmap();
        self.slots.clear();
        let _ = self.session.Close();
        let _ = self.pool.Close();
        self.dev.take()
    }

    fn timing(&self) -> GrabTiming {
        self.timing
    }

    fn set_min_interval(&mut self, ms: u32) {
        if ms != self.interval_ms && self.session.SetMinUpdateInterval(timespan_ms(ms)).is_ok() {
            self.interval_ms = ms;
        }
    }

    fn grab(&mut self, wow: &WowInfo, regions: &[Rect]) -> GrabStatus {
        self.unmap();
        let t0 = Instant::now();
        let frame = self.newest_frame();
        self.timing = GrabTiming { acquire: t0.elapsed(), ..GrabTiming::default() };
        let Some(frame) = frame else { return GrabStatus::NoNewFrame };
        let result = self.read_frame(&frame, wow, regions);
        let _ = frame.Close();
        match result {
            Some(GrabStatus::Fresh) => GrabStatus::Fresh,
            other => {
                self.unmap();
                other.unwrap_or(GrabStatus::Failed)
            }
        }
    }
}
