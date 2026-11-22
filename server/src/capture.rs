//! DXGI Desktop Duplication capture of the primary monitor.
//!
//! The duplication API hands us a GPU texture per changed frame. We copy it
//! into a CPU-readable staging texture, then into a tight (no row padding)
//! `Vec<u8>` that ffmpeg can consume as raw `bgra`/`rgba`.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::{Interface, IUnknown};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Outcome of one `acquire()` call.
pub struct Acquired {
    /// `true` if a new desktop frame was copied into the buffer.
    /// `false` means the desktop did not change (or the duplication was
    /// re-created) and the buffer still holds the previous frame.
    pub new_frame: bool,
    /// Time blocked inside `AcquireNextFrame`. This is mostly idle time when
    /// the desktop is static, so it is reported separately from `copy`.
    pub wait: Duration,
    /// Time spent moving pixels into the CPU buffer (the real capture cost).
    pub copy: Duration,
}

pub struct Capture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    /// Staging texture, as the parent interface the D3D11 calls expect.
    staging: ID3D11Resource,
    width: u32,
    height: u32,
    pixel_format: &'static str,
    buffer: Vec<u8>,
}

impl Capture {
    /// Attach to the first output of the default adapter (the primary monitor
    /// on a normal single-GPU laptop) and prepare the CPU buffer.
    pub fn new() -> Result<Self> {
        // Duplication is COM-based. MTA is what a capture thread wants; a
        // failure here (e.g. RPC_E_CHANGED_MODE) is not fatal for our use.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let (device, context) = create_device()?;

        let dxgi_device: IDXGIDevice = device
            .cast()
            .context("ID3D11Device -> IDXGIDevice cast failed")?;
        let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter() }.context("GetAdapter failed")?;
        let output: IDXGIOutput1 = unsafe { adapter.EnumOutputs(0) }
            .context("EnumOutputs(0) failed - no monitor attached?")?
            .cast()
            .context("IDXGIOutput -> IDXGIOutput1 cast failed (Windows 8+ required)")?;

        // DuplicateOutput takes the D3D11 device as IUnknown; windows 0.62 has
        // no implicit interface upcast, so make the cast explicit.
        let device_unknown: IUnknown = device.cast()?;
        let duplication = unsafe { output.DuplicateOutput(&device_unknown) }.context(
            "DuplicateOutput failed - is another capture app (OBS, Sunshine, ...) already duplicating this monitor?",
        )?;

        let desc = unsafe { duplication.GetDesc() };
        let width = desc.ModeDesc.Width;
        let height = desc.ModeDesc.Height;
        let format = desc.ModeDesc.Format;

        let pixel_format = if format == DXGI_FORMAT_B8G8R8A8_UNORM
            || format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        {
            "bgra"
        } else if format == DXGI_FORMAT_R8G8B8A8_UNORM
            || format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        {
            "rgba"
        } else {
            bail!(
                "unsupported desktop format {format:?} (M1 handles 8-bit BGRA/RGBA only; \
                 HDR / 10-bit desktops are not supported yet)"
            );
        };

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
            pixel_format,
            buffer,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// ffmpeg input pixel format name for the captured desktop.
    pub fn pixel_format(&self) -> &'static str {
        self.pixel_format
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
                });
            }
            Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
                // Desktop changed underneath us: rebuild the duplication and
                // let the caller keep the last good frame for this tick.
                let device_unknown: IUnknown = self.device.cast()?;
                self.duplication = unsafe { self.output.DuplicateOutput(&device_unknown) }
                    .context("re-creating the desktop duplication failed")?;
                return Ok(Acquired {
                    new_frame: false,
                    wait: started.elapsed(),
                    copy: Duration::ZERO,
                });
            }
            Err(err) => {
                return Err(anyhow::Error::new(err)).context("AcquireNextFrame failed");
            }
        }

        let wait = started.elapsed();
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

        Ok(Acquired {
            new_frame: true,
            wait,
            copy: copy_started.elapsed(),
        })
    }
}

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut device: Option<ID3D11Device> = None;
    let mut level = D3D_FEATURE_LEVEL_11_0;

    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            // Required to get BGRA desktop textures out of Desktop Duplication.
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
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
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
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
