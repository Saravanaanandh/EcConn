//! DXGI Desktop Duplication capture.
//!
//! The duplication API hands us a GPU texture per changed frame. We copy it
//! into a CPU-readable staging texture, then into a tight (no row padding)
//! `Vec<u8>` that ffmpeg can consume as raw `bgra`/`rgba`.
//!
//! M6: `--monitor <index>` selects any DXGI output (enumerated across all
//! adapters) and the capture size/format come from the duplication itself.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
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
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

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
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Left edge in desktop coordinates.
    pub left: i32,
    /// Top edge in desktop coordinates.
    pub top: i32,
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
    format: DXGI_FORMAT,
    pixel_format: &'static str,
    buffer: Vec<u8>,
    /// Average brightness of the last copied frame, reported again on
    /// repeat ticks so the stats line never falls back to 0.
    brightness: f32,
    /// Set when a desktop mode change resized the capture, so the caller can
    /// rebuild the encoder for the new input size.
    size_changed: bool,
}

impl Capture {
    /// Enumerate every DXGI output (monitor) across all adapters.
    pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1 failed")?;

        let mut monitors = Vec::new();
        let mut index = 0u32;
        let mut adapter_index = 0u32;

        loop {
            let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(adapter_index) } {
                Ok(adapter) => adapter,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => return Err(err).context("EnumAdapters1 failed"),
            };
            adapter_index += 1;

            let mut output_index = 0u32;
            loop {
                let output = match unsafe { adapter.EnumOutputs(output_index) } {
                    Ok(output) => output,
                    Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                    // An adapter can report an error mid-enumeration (e.g. a
                    // detached output); skip the rest of that adapter.
                    Err(_) => break,
                };
                output_index += 1;

                let desc = match unsafe { output.GetDesc() } {
                    Ok(desc) => desc,
                    Err(_) => continue,
                };
                let rect = desc.DesktopCoordinates;
                let width = (rect.right - rect.left).max(0) as u32;
                let height = (rect.bottom - rect.top).max(0) as u32;

                monitors.push(MonitorInfo {
                    index,
                    name: device_name(&desc.DeviceName),
                    width,
                    height,
                    left: rect.left,
                    top: rect.top,
                });
                index += 1;
            }
        }

        if monitors.is_empty() {
            bail!("no DXGI outputs found - is a monitor attached?");
        }
        Ok(monitors)
    }

    /// Attach to the DXGI output at `monitor_index` (0 = first output found)
    /// and prepare the CPU buffer.
    pub fn new(monitor_index: u32) -> Result<Self> {
        // Duplication is COM-based. MTA is what a capture thread wants; a
        // failure here (e.g. RPC_E_CHANGED_MODE) is not fatal for our use.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let monitors = Self::list_monitors()?;
        if monitor_index as usize >= monitors.len() {
            let available = monitors
                .iter()
                .map(|m| format!("{} ({}x{})", m.index, m.width, m.height))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "monitor index {} not found; available: {}",
                monitor_index,
                available
            );
        }

        // Find the adapter that owns this output: DuplicateOutput only works
        // when the D3D11 device was created on the same adapter.
        let factory: IDXGIFactory1 =
            unsafe { CreateDXGIFactory1() }.context("CreateDXGIFactory1 failed")?;
        let (adapter, output) = find_output(&factory, monitor_index)?;

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

        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            device.CreateTexture2D(&desc, None, Some(&mut staging as *mut _))
                .context("CreateTexture2D (staging) failed")?;
        }
        let staging = staging.context("CreateTexture2D returned no texture")?;
        let staging: ID3D11Resource = staging.cast().context("Cast to ID3D11Resource failed")?;

        let buffer = vec![0u8; width as usize * height as usize * 4];

        Ok(Self {
            device,
            context,
            staging,
            width,
            height,
            format,
            pixel_format,
            buffer,
            brightness: 0.0,
            size_changed: false,
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
        if info.AccumulatedFrames == 0 && info.LastPresentTime == 0 {
            let _ = unsafe { self.duplication.ReleaseFrame() };
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

        Ok(true)
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

/// Walk the factory in the same order as `list_monitors` and return the
/// adapter plus output for `monitor_index`.
fn find_output(factory: &IDXGIFactory1, monitor_index: u32) -> Result<(IDXGIAdapter1, IDXGIOutput1)> {
    let mut index = 0u32;
    let mut adapter_index = 0u32;

    loop {
        let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(err) => return Err(err).context("EnumAdapters1 failed"),
        };
        adapter_index += 1;

        let mut output_index = 0u32;
        loop {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => break,
            };
            output_index += 1;

            if index == monitor_index {
                let output: IDXGIOutput1 = output
                    .cast()
                    .context("IDXGIOutput -> IDXGIOutput1 cast failed (Windows 8+ required)")?;
                return Ok((adapter, output));
            }
            index += 1;
        }
    }

    bail!("monitor index {monitor_index} not found")
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
