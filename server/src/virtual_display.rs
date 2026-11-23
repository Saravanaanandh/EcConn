//! Virtual display control for the Virtual Display Driver (M7).
//!
//! Windows has no user-mode API to add a monitor, so this module drives the
//! open-source Virtual Display Driver (MIT, github.com/VirtualDrivers/
//! Virtual-Display-Driver). The facts below come from its source, not guesses:
//!
//! * device: display-class device with hardware ID `Root\MttVDD` (or `MttVDD`)
//!   - `Virtual Display Driver (HDR)/MttVDD/MttVDD.inf`
//! * config: `C:\VirtualDisplayDriver\vdd_settings.xml`
//!   - `confpath = L"C:\\VirtualDisplayDriver"` in `Driver.cpp`
//!   - `<monitors><count>N</count></monitors>` (driver treats 0 as 1)
//!   - `<resolutions><resolution>{width,height,refresh_rate}</resolution>`
//! * control pipe: `\\.\pipe\MTTVirtualDisplayPipe`
//!   - `#define PIPE_NAME L"\\\\.\\pipe\\MTTVirtualDisplayPipe"`
//! * commands are raw UTF-16LE text (no length prefix, no terminator needed):
//!   - `PING` -> driver writes narrow `PONG`
//!   - `SETDISPLAYCOUNT <n>` -> driver rewrites the XML count and reinitialises
//!     the adapter (`ReloadDriver` -> `InitAdapter` -> `FinishInit`)
//!
//! Everything here is Windows-only.

#![cfg(windows)]

use std::fs;
use std::mem::size_of;
use std::path::Path;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::{HRESULT, BOOL, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceRegistryPropertyW, DIGCF_PRESENT, GUID_DEVCLASS_DISPLAY, SPDRP_HARDWAREID,
    SP_DEVINFO_DATA,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_PIPE_BUSY, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ,
    GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Pipes::{PeekNamedPipe, WaitNamedPipeW};

use crate::capture::{Capture, MonitorInfo};

/// Directory the driver reads its settings from (`confpath` in Driver.cpp).
const SETTINGS_FILE: &str = r"C:\VirtualDisplayDriver\vdd_settings.xml";
/// Named pipe exposed by the driver (`PIPE_NAME` in Driver.cpp).
const PIPE_NAME: &str = r"\\.\pipe\MTTVirtualDisplayPipe";
/// The driver advertises 1-16 monitors; the XML reader itself only rejects 0.
const MAX_MONITORS: u32 = 16;
/// How long to wait for Windows to surface the new DXGI output.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to keep retrying a busy / not-yet-recreated pipe instance.
const PIPE_OPEN_TIMEOUT: Duration = Duration::from_secs(2);

/// What we have to undo on exit.
struct Attached {
    /// `<monitors><count>` value before we added our monitor.
    previous_count: u32,
}

static STATE: Mutex<Option<Attached>> = Mutex::new(None);
static CTRL_HANDLER: AtomicBool = AtomicBool::new(false);

/// `true` when a display-class device with the driver's hardware ID is present.
///
/// The INF registers `Root\MttVDD` and `MttVDD`; match either, case-insensitively.
pub fn is_driver_installed() -> bool {
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_DISPLAY),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT,
        ) else {
            return false;
        };

        let mut found = false;
        let mut index = 0u32;
        loop {
            let mut data = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(set, index, &mut data).is_err() {
                break;
            }
            index += 1;
            if device_matches(set, &data) {
                found = true;
                break;
            }
        }

        let _ = SetupDiDestroyDeviceInfoList(set);
        found
    }
}

pub(crate) unsafe fn device_matches(
    set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &SP_DEVINFO_DATA,
) -> bool {
    // SPDRP_HARDWAREID is a REG_MULTI_SZ, i.e. UTF-16 strings separated by NULs.
    let mut buffer = vec![0u8; 4096];
    if SetupDiGetDeviceRegistryPropertyW(set, data, SPDRP_HARDWAREID, None, Some(&mut buffer), None)
        .is_err()
    {
        return false;
    }

    let wide: &[u16] = slice::from_raw_parts(buffer.as_ptr().cast::<u16>(), buffer.len() / 2);
    let mut start = 0usize;
    for (index, &unit) in wide.iter().enumerate() {
        if unit == 0 {
            if index == start {
                break; // empty string terminates the MULTI_SZ
            }
            let id = String::from_utf16_lossy(&wide[start..index]);
            if id.to_ascii_lowercase().contains("mttvdd") {
                return true;
            }
            start = index + 1;
        }
    }
    false
}

/// Add one virtual monitor at `width` x `height` @ `fps` and return its DXGI
/// output index (the value to pass to `--monitor` / `Capture::new`).
///
/// The XML is edited only when the requested mode is missing: the driver gives
/// the first `<resolution>` in the file to the new monitor as its preferred
/// mode, so a missing mode is inserted at the top of `<resolutions>`.
pub fn attach(width: u32, height: u32, fps: u32) -> Result<u32> {
    if width == 0 || height == 0 || fps == 0 {
        bail!("--virtual needs a non-zero width, height and fps");
    }
    if !is_driver_installed() {
        bail!(
            "Virtual Display Driver not found (no display device with hardware ID Root\\MttVDD)"
        );
    }

    let settings = Path::new(SETTINGS_FILE);
    if !settings.is_file() {
        bail!(
            "{SETTINGS_FILE} not found - is the Virtual Display Driver installed and started?"
        );
    }

    if !ping() {
        eprintln!("virtual display: warning: the driver pipe did not answer PING; continuing anyway");
    }

    let xml = fs::read_to_string(settings).with_context(|| format!("reading {SETTINGS_FILE}"))?;
    let previous_count = read_count(&xml)?;
    if previous_count >= MAX_MONITORS {
        bail!("the driver already has the maximum of {MAX_MONITORS} virtual monitors");
    }

    let (updated, changed) = ensure_resolution(&xml, width, height, fps)?;
    if changed {
        write_xml(settings, &updated)?;
        eprintln!("virtual display: added {width}x{height}@{fps} to {SETTINGS_FILE}");
    }

    // Snapshot before touching the driver, so the new output can be identified
    // by name (falling back to "one more output than before").
    let before = Capture::list_monitors().context("listing monitors before attaching")?;

    let target = previous_count + 1;
    set_display_count(target)
        .with_context(|| format!("asking the driver for {target} display(s)"))?;
    eprintln!(
        "virtual display: requested {target} display(s) (was {previous_count}), \
         waiting up to 10 s for the new output"
    );

    match wait_for_new_monitor(&before) {
        Ok(monitor) => {
            if let Ok(mut state) = STATE.lock() {
                *state = Some(Attached { previous_count });
            }
            install_ctrl_handler();
            eprintln!(
                "virtual display: attached as monitor {} ({}x{} at ({},{}), name='{}')",
                monitor.index,
                monitor.width,
                monitor.height,
                monitor.left,
                monitor.top,
                monitor.name
            );
            Ok(monitor.index)
        }
        Err(err) => {
            // Do not leave an extra monitor behind when the wait times out.
            if let Err(restore) = set_display_count(previous_count) {
                eprintln!(
                    "virtual display: could not restore display count to {previous_count}: {restore:#}"
                );
            }
            Err(err)
        }
    }
}

/// Remove the monitor we added. Safe to call more than once.
pub fn detach() {
    let state = match STATE.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    let Some(state) = state else {
        return;
    };

    match set_display_count(state.previous_count) {
        Ok(()) => eprintln!(
            "virtual display: detached, display count restored to {}",
            state.previous_count
        ),
        Err(err) => eprintln!("virtual display: detach failed: {err:#}"),
    }
}

/// Keeps the virtual monitor alive for the lifetime of the value.
///
/// `Drop` covers normal returns and panics (the default `panic = "unwind"`).
/// Ctrl+C / console close is handled separately by `SetConsoleCtrlHandler`,
/// because Windows terminates the process without running destructors.
pub struct Guard;

impl Guard {
    pub fn new() -> Self {
        install_ctrl_handler();
        Self
    }
}

impl Default for Guard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        detach();
    }
}

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> BOOL {
    // Best effort: never block the console handler for long, and never panic.
    if let Ok(mut guard) = STATE.try_lock() {
        if let Some(state) = guard.take() {
            let _ = set_display_count(state.previous_count);
        }
    }
    // Return FALSE so the default handler still terminates the process.
    BOOL(0)
}

fn install_ctrl_handler() {
    if CTRL_HANDLER.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        if let Err(err) = SetConsoleCtrlHandler(Some(ctrl_handler), true) {
            eprintln!("virtual display: could not install the Ctrl+C handler: {err}");
        }
    }
}

/// Poll DXGI until an output that was not in `before` appears.
fn wait_for_new_monitor(before: &[MonitorInfo]) -> Result<MonitorInfo> {
    let deadline = Instant::now() + ATTACH_TIMEOUT;
    let known: Vec<&str> = before.iter().map(|m| m.name.as_str()).collect();

    loop {
        if let Ok(current) = Capture::list_monitors() {
            if let Some(monitor) = current
                .iter()
                .find(|m| !known.contains(&m.name.as_str()))
            {
                return Ok(monitor.clone());
            }
            // Some driver/OS combinations reuse a device name; a longer list
            // still means the new output is the last one.
            if current.len() > before.len() {
                if let Some(monitor) = current.last() {
                    return Ok(monitor.clone());
                }
            }
        }

        if Instant::now() >= deadline {
            bail!(
                "the Virtual Display Driver did not create a new monitor within 10 s \
                 (check C:\\VirtualDisplayDriver\\Logs and Device Manager)"
            );
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Send `PING` and look for the driver's `PONG`.
pub fn ping() -> bool {
    match send_command("PING", true) {
        Ok(Some(reply)) => reply.starts_with(b"PONG"),
        _ => false,
    }
}

/// Ask the driver for `count` monitors. The driver rewrites `<count>` itself.
fn set_display_count(count: u32) -> Result<()> {
    if !(1..=MAX_MONITORS).contains(&count) {
        bail!("display count {count} is outside the driver's 1-{MAX_MONITORS} range");
    }
    send_command(&format!("SETDISPLAYCOUNT {count}"), false).map(|_| ())
}

/// Write one UTF-16LE command to the driver pipe.
fn send_command(command: &str, want_reply: bool) -> Result<Option<Vec<u8>>> {
    let handle = open_pipe()?;

    let result = (|| -> Result<Option<Vec<u8>>> {
        let wide: Vec<u16> = command.encode_utf16().collect();
        let bytes =
            unsafe { slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
        let mut written = 0u32;
        unsafe { WriteFile(handle, Some(bytes), Some(&mut written), None) }
            .context("writing to MTTVirtualDisplayPipe failed")?;

        if want_reply {
            read_reply(handle)
        } else {
            Ok(None)
        }
    })();

    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

/// Open a pipe instance, retrying `ERROR_PIPE_BUSY` (231) and treating
/// `ERROR_BROKEN_PIPE` (109) / `ERROR_PIPE_NOT_CONNECTED` (233) as transient:
/// the driver disconnects and recreates the instance after every command.
fn open_pipe() -> Result<HANDLE> {
    let wide_name: Vec<u16> = PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let name = PCWSTR::from_raw(wide_name.as_ptr());
    let deadline = Instant::now() + PIPE_OPEN_TIMEOUT;

    loop {
        match unsafe {
            CreateFileW(
                name,
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        } {
            Ok(handle) => return Ok(handle),
            Err(err) => {
                let code = err.code();
                let busy = code == HRESULT::from_win32(ERROR_PIPE_BUSY.0);
                let gone = code == HRESULT::from_win32(ERROR_BROKEN_PIPE.0)
                    || code == HRESULT::from_win32(ERROR_PIPE_NOT_CONNECTED.0);

                if !busy && !gone {
                    return Err(anyhow::Error::new(err)).context(
                        "cannot open MTTVirtualDisplayPipe - is the Virtual Display Driver running?",
                    );
                }
                if Instant::now() >= deadline {
                    bail!(
                        "MTTVirtualDisplayPipe stayed busy for 2 s - the driver may still be \
                         applying a previous command; retry in a moment"
                    );
                }

                if busy {
                    unsafe {
                        let _ = WaitNamedPipeW(name, 200);
                    }
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

/// Wait briefly for the driver's reply, without blocking forever on a driver
/// version that does not answer.
fn read_reply(handle: HANDLE) -> Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let mut available = 0u32;
        if unsafe { PeekNamedPipe(handle, None, 0, None, Some(&mut available), None) }.is_err() {
            return Ok(None);
        }
        if available > 0 {
            let mut buffer = vec![0u8; available.min(256) as usize];
            let mut read = 0u32;
            if unsafe { ReadFile(handle, Some(&mut buffer), Some(&mut read), None) }.is_ok() {
                buffer.truncate(read as usize);
                return Ok(Some(buffer));
            }
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Read `<monitors><count>N</count>` from the driver config.
fn read_count(xml: &str) -> Result<u32> {
    let monitors = xml
        .find("<monitors>")
        .context("vdd_settings.xml has no <monitors> section")?;
    let rest = &xml[monitors..];
    let open = "<count>";
    let close = "</count>";
    let start = rest
        .find(open)
        .context("vdd_settings.xml has no <monitors><count>")?
        + open.len();
    let end = rest[start..]
        .find(close)
        .context("vdd_settings.xml has an unclosed <count>")?
        + start;
    rest[start..end]
        .trim()
        .parse()
        .context("vdd_settings.xml <count> is not a number")
}

/// Make the requested mode the first `<resolution>` (the driver uses
/// `PreferredMonitorModeIdx = 0` for every virtual monitor).
///
/// Returns the (possibly updated) XML and whether anything changed.
fn ensure_resolution(xml: &str, width: u32, height: u32, fps: u32) -> Result<(String, bool)> {
    let open = "<resolutions>";
    let section_start = xml
        .find(open)
        .context("vdd_settings.xml has no <resolutions> section")?
        + open.len();

    let first_block = xml[section_start..]
        .find("<resolution>")
        .map(|offset| section_start + offset);

    if let Some((start, end)) = find_resolution_block(xml, width, height, fps) {
        if Some(start) == first_block {
            return Ok((xml.to_string(), false));
        }
        // Move the existing block to the top, keeping the document intact.
        let mut updated = String::with_capacity(xml.len() + 10);
        updated.push_str(&xml[..section_start]);
        updated.push_str("\n        ");
        updated.push_str(&xml[start..end]);
        updated.push_str(&xml[section_start..start]);
        updated.push_str(&xml[end..]);
        return Ok((updated, true));
    }

    let block = format!(
        "\n        <resolution>\n            <width>{width}</width>\n            \
         <height>{height}</height>\n            <refresh_rate>{fps}</refresh_rate>\n        \
         </resolution>"
    );

    let mut updated = String::with_capacity(xml.len() + block.len());
    updated.push_str(&xml[..section_start]);
    updated.push_str(&block);
    updated.push_str(&xml[section_start..]);
    Ok((updated, true))
}

/// Byte range of the `<resolution>` block that matches the requested mode.
fn find_resolution_block(xml: &str, width: u32, height: u32, fps: u32) -> Option<(usize, usize)> {
    let open = "<resolutions>";
    let section_start = xml.find(open)? + open.len();
    let section_len = xml[section_start..].find("</resolutions>")?;
    let section = &xml[section_start..section_start + section_len];

    let mut offset = 0usize;
    while let Some(relative) = section[offset..].find("<resolution>") {
        let start = offset + relative;
        let end = section[start..].find("</resolution>")? + start + "</resolution>".len();
        let block = &section[start..end];
        if parse_tag(block, "width") == Some(width)
            && parse_tag(block, "height") == Some(height)
            && parse_tag(block, "refresh_rate") == Some(fps)
        {
            return Some((section_start + start, section_start + end));
        }
        offset = end;
    }
    None
}

#[cfg(test)]
fn has_resolution(xml: &str, width: u32, height: u32, fps: u32) -> bool {
    let Some(open) = xml.find("<resolutions>") else {
        return false;
    };
    let Some(len) = xml[open..].find("</resolutions>") else {
        return false;
    };
    let section = &xml[open..open + len];

    section
        .split("<resolution>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</resolution>").next())
        .any(|block| {
            parse_tag(block, "width") == Some(width)
                && parse_tag(block, "height") == Some(height)
                && parse_tag(block, "refresh_rate") == Some(fps)
        })
}

fn parse_tag(block: &str, tag: &str) -> Option<u32> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    block[start..end].trim().parse().ok()
}

/// Replace the config without leaving a half-written file behind.
fn write_xml(path: &Path, contents: &str) -> Result<()> {
    let temp = path.with_extension("xml.lowlat-tmp");
    fs::write(&temp, contents).with_context(|| format!("writing {}", temp.display()))?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows refuses to rename over an existing file.
            fs::copy(&temp, path).with_context(|| format!("replacing {}", path.display()))?;
            let _ = fs::remove_file(&temp);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<vdd_settings>
    <monitors>
        <count>1</count>
    </monitors>
    <resolutions>
        <resolution>
            <width>1920</width>
            <height>1080</height>
            <refresh_rate>60</refresh_rate>
        </resolution>
    </resolutions>
</vdd_settings>
"#;

    #[test]
    fn reads_monitor_count() {
        assert_eq!(read_count(SAMPLE).unwrap(), 1);
    }

    #[test]
    fn existing_resolution_is_detected() {
        assert!(has_resolution(SAMPLE, 1920, 1080, 60));
        assert!(!has_resolution(SAMPLE, 1280, 720, 60));
        assert!(!has_resolution(SAMPLE, 1920, 1080, 30));
    }

    #[test]
    fn missing_resolution_is_inserted_first() {
        let (updated, changed) = ensure_resolution(SAMPLE, 1280, 720, 60).unwrap();
        assert!(changed);
        assert!(has_resolution(&updated, 1280, 720, 60));
        let new_pos = updated.find("<width>1280</width>").unwrap();
        let old_pos = updated.find("<width>1920</width>").unwrap();
        assert!(new_pos < old_pos, "new mode must be the preferred one");
        // The rest of the document is preserved.
        assert!(updated.contains("<count>1</count>"));
    }

    #[test]
    fn existing_resolution_is_not_duplicated() {
        let (updated, changed) = ensure_resolution(SAMPLE, 1920, 1080, 60).unwrap();
        assert!(!changed);
        assert_eq!(updated, SAMPLE);
    }

    #[test]
    fn later_resolution_is_moved_to_the_front() {
        let xml = SAMPLE.replace(
            "    <resolutions>\n",
            "    <resolutions>\n        <resolution>\n            <width>2560</width>\n            \
             <height>1440</height>\n            <refresh_rate>60</refresh_rate>\n        \
             </resolution>\n",
        );
        let (updated, changed) = ensure_resolution(&xml, 1920, 1080, 60).unwrap();
        assert!(changed);
        let ours = updated.find("<width>1920</width>").unwrap();
        let other = updated.find("<width>2560</width>").unwrap();
        assert!(ours < other, "requested mode must become the preferred one");
        assert_eq!(updated.matches("<width>1920</width>").count(), 1);
        assert_eq!(updated.matches("<width>2560</width>").count(), 1);
    }

    #[test]
    fn test_change_display_settings_live() {
        use windows::Win32::Devices::Display::*;
        use windows::Win32::Graphics::Gdi::*;
        use windows::core::HSTRING;
        unsafe {
            let mut num_paths = 0u32;
            let mut num_modes = 0u32;
            let res = GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut num_paths, &mut num_modes);
            println!("GetDisplayConfigBufferSizes res={:?}, paths={}, modes={}", res, num_paths, num_modes);

            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); num_paths as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); num_modes as usize];

            let query_res = QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut num_paths,
                paths.as_mut_ptr(),
                &mut num_modes,
                modes.as_mut_ptr(),
                None,
            );
            println!("QueryDisplayConfig query_res={:?}", query_res);

            for (idx, p) in paths.iter().enumerate() {
                let mut target_name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
                target_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
                target_name.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
                target_name.header.adapterId = p.targetInfo.adapterId;
                target_name.header.id = p.targetInfo.id;
                let _ = DisplayConfigGetDeviceInfo(&mut target_name.header);
                let monitor_friendly = String::from_utf16_lossy(&target_name.monitorFriendlyDeviceName).trim_matches('\0').to_string();

                let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                source_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                source_name.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                source_name.header.adapterId = p.sourceInfo.adapterId;
                source_name.header.id = p.sourceInfo.id;
                let _ = DisplayConfigGetDeviceInfo(&mut source_name.header);
                let gdi_name = String::from_utf16_lossy(&source_name.viewGdiDeviceName).trim_matches('\0').to_string();

                println!("Path {}: gdi='{}', monitor='{}', rotation={:?}", idx, gdi_name, monitor_friendly, p.targetInfo.rotation);
            }

            // Find VDD path (Path 2 or gdi '\\.\DISPLAY11')
            let vdd_idx = paths.iter().position(|p| {
                let mut source_name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                source_name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                source_name.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                source_name.header.adapterId = p.sourceInfo.adapterId;
                source_name.header.id = p.sourceInfo.id;
                let _ = DisplayConfigGetDeviceInfo(&mut source_name.header);
                let gdi_name = String::from_utf16_lossy(&source_name.viewGdiDeviceName).trim_matches('\0').to_string();
                gdi_name == r"\\.\DISPLAY11"
            }).expect("found VDD path");

            println!("Found VDD at index {}", vdd_idx);

            let name_wide: Vec<u16> = format!("{}\0", r"\\.\DISPLAY11").encode_utf16().collect();
            let mut dm = DEVMODEW::default();
            dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let mut i = 0;
            println!("Enumerating modes of DISPLAY11 with EDS_ROTATEDMODE:");
            while EnumDisplaySettingsExW(windows::core::PCWSTR(name_wide.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm, EDS_ROTATEDMODE).as_bool() {
                println!("  mode {}: {}x{} @ {}Hz, orient={}", i, dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency, dm.Anonymous1.Anonymous2.dmDisplayOrientation.0);
                i += 1;
            }
            let mut dm_curr = DEVMODEW::default();
            dm_curr.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let _ = EnumDisplaySettingsW(windows::core::PCWSTR(name_wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm_curr);
            println!("Current VDD: pos=({}, {}), {}x{}, orient={}",
                dm_curr.Anonymous1.Anonymous2.dmPosition.x, dm_curr.Anonymous1.Anonymous2.dmPosition.y,
                dm_curr.dmPelsWidth, dm_curr.dmPelsHeight, dm_curr.Anonymous1.Anonymous2.dmDisplayOrientation.0);

            let mut dm_target = DEVMODEW::default();
            dm_target.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let _ = EnumDisplaySettingsExW(windows::core::PCWSTR(name_wide.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(1), &mut dm_target, EDS_ROTATEDMODE);
            dm_target.Anonymous1.Anonymous2.dmPosition = dm_curr.Anonymous1.Anonymous2.dmPosition;
            dm_target.dmFields |= DM_POSITION;

            println!("dm_target: pos=({}, {}), {}x{}, orient={}, fields={:x}",
                dm_target.Anonymous1.Anonymous2.dmPosition.x, dm_target.Anonymous1.Anonymous2.dmPosition.y,
                dm_target.dmPelsWidth, dm_target.dmPelsHeight, dm_target.Anonymous1.Anonymous2.dmDisplayOrientation.0, dm_target.dmFields.0);

            for sz in [188u16, 220u16] {
                let mut dm_test = dm_curr;
                dm_test.dmSize = sz;
                dm_test.dmDriverExtra = 0;
                println!("Testing dm_curr with dmSize={}:", sz);
                let r1 = ChangeDisplaySettingsExW(windows::core::PCWSTR(name_wide.as_ptr()), Some(&dm_test), None, CDS_UPDATEREGISTRY | CDS_NORESET, None);
                let r2 = ChangeDisplaySettingsExW(windows::core::PCWSTR::null(), None, None, CDS_TYPE(0), None);
                println!("  CDS_UPDATEREGISTRY|CDS_NORESET: {:?}, commit: {:?}", r1, r2);

                // Now test rotating 90
                let mut dm90 = dm_curr;
                dm90.dmSize = sz;
                dm90.dmDriverExtra = 0;
                dm90.Anonymous1.Anonymous2.dmDisplayOrientation = DMDO_90;
                dm90.dmPelsWidth = dm_curr.dmPelsHeight;
                dm90.dmPelsHeight = dm_curr.dmPelsWidth;
                dm90.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION | DM_POSITION;
                let r90_1 = ChangeDisplaySettingsExW(windows::core::PCWSTR(name_wide.as_ptr()), Some(&dm90), None, CDS_UPDATEREGISTRY | CDS_NORESET, None);
                let r90_2 = ChangeDisplaySettingsExW(windows::core::PCWSTR::null(), None, None, CDS_TYPE(0), None);
                println!("  dm90: {:?}, commit: {:?}", r90_1, r90_2);
            }

            // Check current settings after commit
            let mut dm_now = DEVMODEW::default();
            dm_now.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let _ = EnumDisplaySettingsW(windows::core::PCWSTR(name_wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm_now);
            println!("Current settings after commit: pos=({}, {}), {}x{}, orient={}",
                dm_now.Anonymous1.Anonymous2.dmPosition.x, dm_now.Anonymous1.Anonymous2.dmPosition.y,
                dm_now.dmPelsWidth, dm_now.dmPelsHeight, dm_now.Anonymous1.Anonymous2.dmDisplayOrientation.0);

            // Revert back to 0
            let mut dm_0 = dm_now;
            dm_0.Anonymous1.Anonymous2.dmDisplayOrientation = DMDO_DEFAULT;
            dm_0.dmPelsWidth = dm_now.dmPelsHeight;
            dm_0.dmPelsHeight = dm_now.dmPelsWidth;
            dm_0.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION | DM_POSITION;
            let _ = ChangeDisplaySettingsExW(windows::core::PCWSTR(name_wide.as_ptr()), Some(&dm_0), None, CDS_UPDATEREGISTRY | CDS_NORESET, None);
            let _ = ChangeDisplaySettingsExW(windows::core::PCWSTR::null(), None, None, CDS_TYPE(0), None);
            let _ = EnumDisplaySettingsW(windows::core::PCWSTR(name_wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm_now);
            println!("Current settings after revert: pos=({}, {}), {}x{}, orient={}",
                dm_now.Anonymous1.Anonymous2.dmPosition.x, dm_now.Anonymous1.Anonymous2.dmPosition.y,
                dm_now.dmPelsWidth, dm_now.dmPelsHeight, dm_now.Anonymous1.Anonymous2.dmDisplayOrientation.0);
        }
    }
}
