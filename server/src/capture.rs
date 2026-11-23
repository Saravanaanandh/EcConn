//! DXGI Desktop Duplication capture.
//!
//! The duplication API hands us a GPU texture per changed frame. We copy it
//! into a CPU-readable staging texture, then into a tight (no row padding)
//! `Vec<u8>` that ffmpeg can consume as raw `bgra`/`rgba`.
//!
//! M6: `--monitor <index>` selects any DXGI output (enumerated across all
//! adapters) and the capture size/format come from the duplication itself.
//!
//! Cursor overlay: DXGI Desktop Duplication does NOT mix the hardware cursor
//! into the captured frame; it delivers cursor shape and position separately
//! via `DXGI_OUTDUPL_FRAME_INFO`. We fetch that data each frame and software-
//! blit it onto the CPU buffer so the remote viewer sees the cursor move.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::{IUnknown, Interface};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_MODE_ROTATION_IDENTITY,
    DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};

/// Outcome of one `acquire()` call.
pub struct Acquired {
    /// `true` if a new desktop frame was copied into the buffer.
    pub new_frame: bool,
    /// Time blocked inside `AcquireNextFrame`. This is mostly idle time when
    /// the desktop is static, so it is reported separately from `copy`.
    pub wait: Duration,
    /// Time spent moving pixels into the CPU buffer (the real capture cost).
    pub copy: Duration,
    /// Average pixel brightness of the captured frame (0 = black, 255 = white).
    pub brightness: f32,
    /// What happened this tick: "new", "static", "repeat" or "lost".
    pub frame_status: &'static str,
}

/// Describes a DXGI output (monitor) with its resolution and desktop position.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// Zero-based index across all adapters, in enumeration order.
    pub index: u32,
    /// GDI device name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Description of the adapter that drives this output.
    pub adapter: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Left edge in desktop coordinates.
    pub left: i32,
    /// Top edge in desktop coordinates.
    pub top: i32,
}

/// Cursor overlay state: last known shape (BGRA pixels) and position on the
/// captured output. Updated each frame from `DXGI_OUTDUPL_FRAME_INFO`.
struct CursorState {
    /// BGRA pixel data of the cursor, pre-converted from whatever DXGI gives.
    /// Empty when the cursor shape has never been fetched.
    shape: Vec<u8>,
    /// Width of the cursor image in pixels.
    shape_width: i32,
    /// Height of the cursor image in pixels.
    shape_height: i32,
    /// Hot-spot offset within the cursor image (pixels).
    hot_x: i32,
    hot_y: i32,
    /// Cursor position in desktop coordinates (top-left of hot-spot).
    pos_x: i32,
    pos_y: i32,
    /// Whether the cursor is currently visible.
    visible: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            shape: Vec::new(),
            shape_width: 0,
            shape_height: 0,
            hot_x: 0,
            hot_y: 0,
            pos_x: 0,
            pos_y: 0,
            visible: false,
        }
    }
}

/// Captures raw video frames from a DXGI output using Desktop Duplication.
pub struct Capture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// The DXGI output (monitor) being captured.
    output: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    /// Staging texture, as the parent interface the D3D11 calls expect.
    staging: ID3D11Resource,
    width: u32,
    height: u32,
    /// Desktop position of the captured output (for input mapping).
    x: i32,
    y: i32,
    format: DXGI_FORMAT,
    pixel_format: &'static str,
    buffer: Vec<u8>,
    /// Average brightness of the last copied frame, reported again on
    /// repeat ticks so the stats line never falls back to 0.
    brightness: f32,
    /// Set when a desktop mode change resized the capture, so the caller can
    /// rebuild the encoder for the new input size.
    size_changed: bool,
    /// Cursor state: last known position (relative to this output) and shape.
    cursor: CursorState,
}

impl Capture {
    /// Enumerate every DXGI output (monitor) across all adapters.
    ///
    /// `Capture::new` walks the same list, so a `--monitor <index>` always
    /// refers to the output that `--list-monitors` printed at that index.
    pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1 failed")?;
        let outputs = enumerate_outputs(&factory)?;

        let monitors = outputs
            .iter()
            .enumerate()
            .map(|(index, out)| {
                let rect = out.desc.DesktopCoordinates;
                MonitorInfo {
                    index: index as u32,
                    name: device_name(&out.desc.DeviceName),
                    adapter: out.adapter_name.clone(),
                    width: (rect.right - rect.left).max(0) as u32,
                    height: (rect.bottom - rect.top).max(0) as u32,
                    left: rect.left,
                    top: rect.top,
                }
            })
            .collect::<Vec<_>>();

        if monitors.is_empty() {
            bail!("no DXGI outputs found - is a monitor attached?");
        }
        Ok(monitors)
    }

    /// How many monitors Windows itself reports on the desktop
    /// (`SM_CMONITORS`), which is the reference `--list-monitors` checks
    /// itself against: if Windows sees more monitors than DXGI exposes, the
    /// extra display is not capturable and the caller should say so.
    pub fn windows_monitor_count() -> u32 {
        (unsafe { GetSystemMetrics(SM_CMONITORS) }).max(0) as u32
    }

    /// Attach to the DXGI output at `monitor_index` (0 = first output found)
    /// and prepare the CPU buffer.
    pub fn new(monitor_index: u32) -> Result<Self> {
        // Duplication is COM-based. MTA is what a capture thread wants; a
        // failure here (e.g. RPC_E_CHANGED_MODE) is not fatal for our use.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        // One walk of the factory for both listing and selection, so the
        // index printed by --list-monitors is the index captured here.
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1 failed")?;
        let outputs = enumerate_outputs(&factory)?;

        let available = outputs
            .iter()
            .enumerate()
            .map(|(index, out)| {
                let rect = out.desc.DesktopCoordinates;
                format!(
                    "{} ({}x{})",
                    index,
                    (rect.right - rect.left).max(0),
                    (rect.bottom - rect.top).max(0)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");

        let Some(selected) = outputs.into_iter().nth(monitor_index as usize) else {
            bail!("monitor index {monitor_index} not found; available: {available}");
        };

        // DuplicateOutput only works when the D3D11 device was created on the
        // adapter that owns this output, so both come from the same record.
        let DxgiOutput {
            adapter,
            output,
            desc: out_desc,
            ..
        } = selected;

        // Desktop position of this output, used to map client input coords.
        let x = out_desc.DesktopCoordinates.left;
        let y = out_desc.DesktopCoordinates.top;

        let (device, context) = create_device(Some(&adapter))?;

        // DuplicateOutput takes the D3D11 device as IUnknown; windows 0.62 has
        // no implicit interface upcast, so make the cast explicit.
        let device_unknown: IUnknown = device.cast()?;
        let duplication = unsafe { output.DuplicateOutput(&device_unknown) }.context(
            "DuplicateOutput failed - is another capture app (OBS, Sunshine, ...) \
             already duplicating this monitor?",
        )?;

        let desc = unsafe { duplication.GetDesc() };
        let width = desc.ModeDesc.Width;
        let height = desc.ModeDesc.Height;
        let format = desc.ModeDesc.Format;

        if width == 0 || height == 0 {
            bail!("duplication reported an empty mode ({}x{})", width, height);
        }

        let pixel_format = pixel_format_name(format)?;

        if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
            && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED
        {
            eprintln!(
                "warning: monitor {monitor_index} is rotated ({:?}); \
                 the stream is sent in the panel's native orientation",
                desc.Rotation
            );
        }

        let staging = create_staging_texture(&device, width, height, format)?;

        let buffer = vec![0u8; width as usize * height as usize * 4];

        Ok(Self {
            device,
            context,
            output,
            duplication,
            staging,
            width,
            height,
            x,
            y,
            format,
            pixel_format,
            buffer,
            brightness: 0.0,
            size_changed: false,
            cursor: CursorState::default(),
        })
    }

    /// Width of the captured output in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the captured output in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// X position of the upper-left corner in pixels (relative to desktop).
    pub fn x(&self) -> i32 {
        self.x
    }

    /// Y position of the upper-left corner in pixels (relative to desktop).
    pub fn y(&self) -> i32 {
        self.y
    }

    /// ffmpeg input pixel format name for the captured desktop.
    pub fn pixel_format(&self) -> &'static str {
        self.pixel_format
    }

    /// Returns `true` once after the capture was resized by a mode change.
    pub fn take_size_change(&mut self) -> bool {
        std::mem::take(&mut self.size_changed)
    }

    /// Latest frame as tightly packed 4-bytes-per-pixel data.
    pub fn frame(&self) -> &[u8] {
        &self.buffer
    }

    /// Wait up to `timeout_ms` for a new frame and copy it into the buffer.
    ///
    /// Returns `Ok(new_frame == false)` on timeout, and re-creates the
    /// duplication on `DXGI_ERROR_ACCESS_LOST` (desktop switch, UAC prompt,
    /// display mode change).
    pub fn acquire(&mut self, timeout_ms: u32) -> Result<Acquired> {
        let started = Instant::now();
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        match unsafe { self.duplication.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                return Ok(Acquired {
                    new_frame: false,
                    wait: started.elapsed(),
                    copy: Duration::ZERO,
                    brightness: self.brightness,
                    frame_status: "repeat",
                });
            }
            Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
                // Desktop changed underneath us (mode switch, lock screen,
                // UAC). Rebuild the duplication; keep serving the last frame
                // until it succeeds.
                let recreated = self.recreate_duplication().unwrap_or(false);
                return Ok(Acquired {
                    new_frame: false,
                    wait: started.elapsed(),
                    copy: Duration::ZERO,
                    brightness: self.brightness,
                    frame_status: if recreated { "lost" } else { "repeat" },
                });
            }
            Err(err) => {
                return Err(anyhow::Error::new(err)).context("AcquireNextFrame failed");
            }
        }

        let wait = started.elapsed();

        // Pointer-only updates return a frame with no accumulated desktop
        // image. Releasing it without a copy keeps the previous buffer.
        // However we still need to update the cursor position/shape first.
        let has_desktop = !(info.AccumulatedFrames == 0 && info.LastPresentTime == 0);

        // --- Cursor update ---
        // Position update: happens even on pointer-only frames.
        if info.LastMouseUpdateTime != 0 {
            self.cursor.visible = info.PointerPosition.Visible.as_bool();
            self.cursor.pos_x = info.PointerPosition.Position.x;
            self.cursor.pos_y = info.PointerPosition.Position.y;
        }
        // Shape update: only when DXGI says the shape changed.
        if info.PointerShapeBufferSize > 0 {
            self.fetch_cursor_shape(info.PointerShapeBufferSize);
        }

        if !has_desktop {
            let _ = unsafe { self.duplication.ReleaseFrame() };
            // If the cursor moved but no desktop change, re-composite the
            // cursor onto the existing buffer and emit a new frame so the
            // viewer sees the cursor glide smoothly.
            if info.LastMouseUpdateTime != 0 {
                self.overlay_cursor();
                self.brightness = average_brightness(&self.buffer);
                return Ok(Acquired {
                    new_frame: true,
                    wait,
                    copy: Duration::ZERO,
                    brightness: self.brightness,
                    frame_status: "cursor",
                });
            }
            return Ok(Acquired {
                new_frame: false,
                wait,
                copy: Duration::ZERO,
                brightness: self.brightness,
                frame_status: "static",
            });
        }

        let copy_started = Instant::now();

        let resource = resource.context("AcquireNextFrame returned no desktop resource")?;
        let texture: ID3D11Texture2D = resource
            .cast()
            .context("IDXGIResource -> ID3D11Texture2D cast failed")?;
        let texture: ID3D11Resource = texture.cast()?;

        unsafe {
            self.context.CopyResource(&self.staging, &texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(err) =
                self.context
                    .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped as *mut _))
            {
                // The frame stays acquired until ReleaseFrame, so release it
                // before bailing out or every later AcquireNextFrame fails.
                let _ = self.duplication.ReleaseFrame();
                return Err(anyhow::Error::new(err)).context("mapping the staging texture failed");
            }

            // The staging texture is padded to RowPitch; the buffer is not.
            let row_bytes = self.width as usize * 4;
            let src = mapped.pData as *const u8;
            let dst = self.buffer.as_mut_ptr();
            for y in 0..self.height as usize {
                std::ptr::copy_nonoverlapping(
                    src.add(y * mapped.RowPitch as usize),
                    dst.add(y * row_bytes),
                    row_bytes,
                );
            }

            self.context.Unmap(&self.staging, 0);
            self.duplication
                .ReleaseFrame()
                .context("ReleaseFrame failed")?;
        }

        // Composite the cursor on top of the freshly-copied desktop pixels.
        self.overlay_cursor();

        self.brightness = average_brightness(&self.buffer);

        Ok(Acquired {
            new_frame: true,
            wait,
            copy: copy_started.elapsed(),
            brightness: self.brightness,
            frame_status: "new",
        })
    }

    /// Re-create the duplication after ACCESS_LOST. Returns `false` (without
    /// failing) when the desktop is still unavailable, so the caller keeps
    /// retrying on later ticks.
    fn recreate_duplication(&mut self) -> Result<bool> {
        let device_unknown: IUnknown = self.device.cast()?;
        let duplication = match unsafe { self.output.DuplicateOutput(&device_unknown) } {
            Ok(duplication) => duplication,
            Err(_) => return Ok(false),
        };
        self.duplication = duplication;

        let desc = unsafe { self.duplication.GetDesc() };
        let (width, height, format) = (desc.ModeDesc.Width, desc.ModeDesc.Height, desc.ModeDesc.Format);

        if width == 0 || height == 0 {
            return Ok(false);
        }

        if width != self.width || height != self.height || format != self.format {
            self.pixel_format = pixel_format_name(format)?;
            self.staging = create_staging_texture(&self.device, width, height, format)?;
            self.width = width;
            self.height = height;
            self.buffer = vec![0u8; width as usize * height as usize * 4];
            self.brightness = 0.0;
            self.size_changed = true;
            eprintln!("capture resized to {width}x{height}, rebuilding the encoder");
        }

        // After ACCESS_LOST the new duplication will re-send cursor shape.
        self.cursor.shape.clear();

        Ok(true)
    }

    /// Fetch the cursor shape from DXGI into `self.cursor.shape` (BGRA).
    ///
    /// `buf_size` is the value DXGI reported in `PointerShapeBufferSize`; it
    /// is used to allocate the right-sized buffer before calling the API.
    fn fetch_cursor_shape(&mut self, buf_size: u32) {
        let mut raw: Vec<u8> = vec![0u8; buf_size as usize];
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        let mut required = 0u32;
        let result = unsafe {
            self.duplication.GetFramePointerShape(
                buf_size,
                raw.as_mut_ptr() as *mut _,
                &mut required,
                &mut info,
            )
        };
        if result.is_err() {
            return;
        }

        let w = info.Width as usize;
        let h = info.Height as usize;
        let pitch = info.Pitch as usize;

        self.cursor.hot_x = info.HotSpot.x;
        self.cursor.hot_y = info.HotSpot.y;

        // Convert to BGRA regardless of source format.
        // info.Type is u32; compare against .0 (i32) value of each constant.
        let shape_type = info.Type as i32;
        if shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 {
            self.cursor.shape_width = w as i32;
            self.cursor.shape_height = h as i32;
            let mut bgra = vec![0u8; w * h * 4];
            for row in 0..h {
                let src_off = row * pitch;
                let dst_off = row * w * 4;
                bgra[dst_off..dst_off + w * 4]
                    .copy_from_slice(&raw[src_off..src_off + w * 4]);
            }
            self.cursor.shape = bgra;
        } else if shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 {
            self.cursor.shape_width = w as i32;
            self.cursor.shape_height = h as i32;
            let mut bgra = vec![0u8; w * h * 4];
            for row in 0..h {
                let src_off = row * pitch;
                let dst_off = row * w * 4;
                for col in 0..w {
                    let b = raw[src_off + col * 4];
                    let g = raw[src_off + col * 4 + 1];
                    let r = raw[src_off + col * 4 + 2];
                    let a = raw[src_off + col * 4 + 3];
                    bgra[dst_off + col * 4]     = b;
                    bgra[dst_off + col * 4 + 1] = g;
                    bgra[dst_off + col * 4 + 2] = r;
                    bgra[dst_off + col * 4 + 3] = if a == 0 { 255 } else { a };
                }
            }
            self.cursor.shape = bgra;
        } else if shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 {
            let and_height = h;
            let row_bytes = pitch;
            self.cursor.shape_width = w as i32;
            self.cursor.shape_height = and_height as i32;
            let mut bgra = vec![0u8; w * and_height * 4];
            for row in 0..and_height {
                let and_off = row * row_bytes;
                let xor_off = (row + and_height) * row_bytes;
                for col in 0..w {
                    let byte_idx = col / 8;
                    let bit = 7 - (col % 8);
                    let and_bit = (raw[and_off + byte_idx] >> bit) & 1;
                    let xor_bit = (raw[xor_off + byte_idx] >> bit) & 1;
                    let dst = (row * w + col) * 4;
                    let (color, alpha) = match (and_bit, xor_bit) {
                        (1, 0) => (0u8, 0u8),
                        (0, 0) => (0u8, 255u8),
                        (0, 1) => (255u8, 255u8),
                        _      => (128u8, 128u8),
                    };
                    bgra[dst]     = color;
                    bgra[dst + 1] = color;
                    bgra[dst + 2] = color;
                    bgra[dst + 3] = alpha;
                }
            }
            self.cursor.shape = bgra;
        }
    }

    /// Blit the cursor stored in `self.cursor` onto `self.buffer` (BGRA/RGBA).
    ///
    /// Alpha-blends the cursor shape at the current cursor position, clipping
    /// to the output rectangle. The pixel format follows `self.pixel_format`.
    fn overlay_cursor(&mut self) {
        if !self.cursor.visible || self.cursor.shape.is_empty() {
            return;
        }

        // Cursor position is in desktop coordinates; convert to output-local.
        let cur_x = self.cursor.pos_x - self.x - self.cursor.hot_x;
        let cur_y = self.cursor.pos_y - self.y - self.cursor.hot_y;
        let cw = self.cursor.shape_width;
        let ch = self.cursor.shape_height;
        let fw = self.width as i32;
        let fh = self.height as i32;

        // Determine the overlapping rectangle.
        let src_x0 = (-cur_x).max(0);
        let src_y0 = (-cur_y).max(0);
        let dst_x0 = cur_x.max(0);
        let dst_y0 = cur_y.max(0);
        let blit_w = (cw - src_x0).min(fw - dst_x0);
        let blit_h = (ch - src_y0).min(fh - dst_y0);

        if blit_w <= 0 || blit_h <= 0 {
            return;
        }

        let is_rgba = self.pixel_format == "rgba";

        for row in 0..blit_h as usize {
            let sy = (src_y0 as usize + row) * cw as usize;
            let dy = (dst_y0 as usize + row) * fw as usize;
            for col in 0..blit_w as usize {
                let sp = (sy + src_x0 as usize + col) * 4;
                let dp = (dy + dst_x0 as usize + col) * 4;

                // Cursor shape is always BGRA.
                let (cb, cg, cr, ca) = (
                    self.cursor.shape[sp]     as u32,
                    self.cursor.shape[sp + 1] as u32,
                    self.cursor.shape[sp + 2] as u32,
                    self.cursor.shape[sp + 3] as u32,
                );
                if ca == 0 {
                    continue;
                }

                // Frame buffer may be BGRA or RGBA.
                let (fb, fg, fr) = if is_rgba {
                    (self.buffer[dp + 2] as u32,
                     self.buffer[dp + 1] as u32,
                     self.buffer[dp]     as u32)
                } else {
                    (self.buffer[dp]     as u32,
                     self.buffer[dp + 1] as u32,
                     self.buffer[dp + 2] as u32)
                };

                let inv = 255 - ca;
                let rb = (cb * ca + fb * inv) / 255;
                let rg = (cg * ca + fg * inv) / 255;
                let rr = (cr * ca + fr * inv) / 255;

                if is_rgba {
                    self.buffer[dp]     = rr as u8;
                    self.buffer[dp + 1] = rg as u8;
                    self.buffer[dp + 2] = rb as u8;
                } else {
                    self.buffer[dp]     = rb as u8;
                    self.buffer[dp + 1] = rg as u8;
                    self.buffer[dp + 2] = rr as u8;
                }
                self.buffer[dp + 3] = 255;
            }
        }
    }
}

/// Average luminance of a tightly packed BGRA/RGBA buffer, sampled every 16
/// pixels so the per-frame cost stays negligible.
fn average_brightness(buffer: &[u8]) -> f32 {
    let mut total = 0f64;
    let mut count = 0u64;
    let mut offset = 0usize;
    while offset + 3 < buffer.len() {
        // Both supported formats carry G in byte 1, so green alone is a good
        // and cheap proxy for luminance.
        total += buffer[offset + 1] as f64;
        count += 1;
        offset += 16 * 4;
    }
    if count == 0 {
        0.0
    } else {
        (total / count as f64) as f32
    }
}

fn pixel_format_name(format: DXGI_FORMAT) -> Result<&'static str> {
    if format == DXGI_FORMAT_B8G8R8A8_UNORM || format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB {
        Ok("bgra")
    } else if format == DXGI_FORMAT_R8G8B8A8_UNORM || format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB {
        Ok("rgba")
    } else {
        bail!(
            "unsupported desktop format {format:?} (8-bit BGRA/RGBA only; \
             HDR / 10-bit desktops are not supported yet)"
        );
    }
}

/// One DXGI output together with the adapter that drives it.
struct DxgiOutput {
    adapter: IDXGIAdapter1,
    /// Adapter description, for diagnostics.
    adapter_name: String,
    /// `IDXGIOutput1`: what Desktop Duplication needs.
    output: IDXGIOutput1,
    desc: DXGI_OUTPUT_DESC,
}

/// Walk every adapter and every output once, in DXGI order.
///
/// Used by both `Capture::list_monitors` and `Capture::new` so a monitor
/// index means the same thing to the user and to the capture. Enumeration is
/// deliberately permissive: one output that fails `GetDesc` (or cannot be cast
/// to `IDXGIOutput1`) is skipped with a warning, and a non-`DXGI_ERROR_NOT_FOUND`
/// error from `EnumOutputs` no longer abandons the rest of the adapter â€” the
/// old behaviour could silently hide a monitor. Warnings are printed for every
/// output we could not read, so a truncated list is never silent.
fn enumerate_outputs(factory: &IDXGIFactory1) -> Result<Vec<DxgiOutput>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut outputs: Vec<DxgiOutput> = Vec::new();
    let mut adapter_index = 0u32;

    loop {
        let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            // DXGI does not define any ordering after a failure, so stop here
            // instead of guessing - but do not hide the problem.
            Err(err) => {
                eprintln!("warning: EnumAdapters1({adapter_index}) failed: {err}");
                break;
            }
        };
        adapter_index += 1;

        let adapter_name = match unsafe { adapter.GetDesc1() } {
            Ok(desc) => device_name(&desc.Description),
            Err(err) => {
                eprintln!("warning: reading adapter {adapter_index} description failed: {err}");
                format!("adapter {adapter_index}")
            }
        };

        let mut output_index = 0u32;
        let mut failures = 0u32;
        loop {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => {
                    failures = 0;
                    output
                }
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                // Some drivers report a real error instead of NOT_FOUND for an
                // output they cannot expose (a detached display, or one being
                // reconfigured). Keep trying later outputs rather than giving
                // up on the whole adapter, but do not spin forever.
                Err(err) => {
                    if failures == 0 {
                        eprintln!(
                            "warning: EnumOutputs({adapter_index}, {output_index}) on \
                             '{adapter_name}' failed: {err}; continuing"
                        );
                    }
                    failures += 1;
                    output_index += 1;
                    if failures >= 8 {
                        break;
                    }
                    continue;
                }
            };
            output_index += 1;

            let desc = match unsafe { output.GetDesc() } {
                Ok(desc) => desc,
                Err(err) => {
                    eprintln!(
                        "warning: skipping output {output_index} on '{adapter_name}': \
                         reading its description failed: {err}"
                    );
                    continue;
                }
            };

            // Desktop Duplication needs IDXGIOutput1 (Windows 8+); an output
            // that cannot be cast is not capturable, so leave it out of both
            // the listing and the selection list.
            let output: IDXGIOutput1 = match output.cast() {
                Ok(output) => output,
                Err(err) => {
                    eprintln!(
                        "warning: skipping output {output_index} on '{adapter_name}': \
                         IDXGIOutput1 cast failed (Windows 8+ required): {err}"
                    );
                    continue;
                }
            };

            outputs.push(DxgiOutput {
                adapter: adapter.clone(),
                adapter_name: adapter_name.clone(),
                output,
                desc,
            });
        }
    }

    Ok(outputs)
}

fn create_device(adapter: Option<&IDXGIAdapter1>) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut device: Option<ID3D11Device> = None;
    let mut level = D3D_FEATURE_LEVEL_11_0;

    // When an explicit adapter is supplied, D3D11 requires DRIVER_TYPE_UNKNOWN.
    let driver_type = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };

    unsafe {
        D3D11CreateDevice(
            adapter.map(|a| &**a),
            driver_type,
            HMODULE::default(),
            windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_FLAG(0),
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device as *mut _),
            Some(&mut level as *mut _),
            None,
        )
        .context("D3D11CreateDevice failed")?;
    }

    let device = device.context("D3D11CreateDevice returned no device")?;
    let context = unsafe { device.GetImmediateContext() }.context("GetImmediateContext failed")?;
    Ok((device, context))
}

fn create_staging_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
) -> Result<ID3D11Resource> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };

    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture as *mut _)) }
        .context("CreateTexture2D (staging) failed")?;
    let texture = texture.context("CreateTexture2D returned no texture")?;
    let texture: ID3D11Resource = texture.cast()?;
    Ok(texture)
}

/// Convert the fixed-size wide device name to a printable string.
fn device_name(name: &[u16]) -> String {
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..len])
}
