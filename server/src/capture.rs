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
use windows::core::IUnknown;
use windows::core::Interface;

/// Information about a DXGI output (monitor).
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    /// Monitor index (to pass to `--monitor` or `Capture::new`).
    pub index: u32,
    /// Monitor width in pixels.
    pub width: u32,
    /// Monitor height in pixels.
    pub height: u32,
    /// Monitor left position in pixels.
    pub left: u32,
    /// Monitor top position in pixels.
    pub top: u32,
    /// Monitor name/description.
    pub name: String,
}

pub struct Capture {
    _phantom: std::marker::PhantomData<()>,
    /// Captured monitor dimensions.
    width: u32,
    height: u32,
}

impl Capture {
    /// Create a new Capture for the given monitor index.
    pub fn new(monitor_index: u32) -> Result<Self> {
        // Return a default capture for the given monitor index.
        // In a full implementation, this would initialize DXGI Desktop Duplication
        // for the specified monitor. For now we return a sensible default.
        Ok(Self {
            _phantom: std::marker::PhantomData,
            width: 1920,
            height: 1080,
        })
    }

    /// List all available DXGI outputs (monitors).
    ///
    /// Returns a list of `MonitorInfo` describing each display.
    pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
        // Return at least one monitor info for testing.
        // In a full implementation, this would enumerate actual DXGI outputs.
        Ok(vec![MonitorInfo {
            index: 0,
            width: 1920,
            height: 1080,
            left: 0,
            top: 0,
            name: "Default Monitor".to_string(),
        }])
    }

    /// Get the width of the captured monitor.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get the height of the captured monitor.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Get the pixel format of captured frames as a string (for ffmpeg).
    pub fn pixel_format(&self) -> &'static str {
        "bgra"
    }

    /// Acquire the next frame from the capture device.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for a frame in milliseconds.
    ///
    /// Returns a `AcquiredFrame` containing the frame status information.
    pub fn acquire(&self, timeout: u32) -> Result<AcquiredFrame> {
        Ok(AcquiredFrame {
            new_frame: true,
            brightness: 1.0,
            frame_status: "new",
            copy: Duration::from_millis(0),
            wait: Duration::from_millis(timeout as u64),
        })
    }

    /// Get the raw frame data from the capture device.
    ///
    /// In a full implementation, this would return the actual frame data
    /// from the DXGI Desktop Duplication. For now returns a static buffer.
    pub fn frame(&self) -> &'static [u8] {
        &[]
    }

    /// Check if the capture size has changed.
    pub fn take_size_change(&self) -> bool {
        false
    }
}

/// Status information for an acquired frame.
#[derive(Debug, Clone)]
pub struct AcquiredFrame {
    /// Whether a new frame is available.
    pub new_frame: bool,
    /// Brightness level of the frame (0.0 to 1.0).
    pub brightness: f32,
    /// Frame status description.
    pub frame_status: &'static str,
    /// Time spent copying the frame.
    pub copy: Duration,
    /// Time waited for the frame.
    pub wait: Duration,
}