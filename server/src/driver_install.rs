//! M8: install, uninstall and query the Virtual Display Driver.
//!
//! `--install-driver` has to run elevated, so it is a two-stage operation:
//!
//! 1. the normal (non-elevated) `server.exe` re-launches itself through UAC
//!    with `ShellExecuteW("runas")` and the hidden `--elevated` marker;
//! 2. the elevated child writes the bundled driver package to
//!    `%ProgramData%\lowlat-screen\driver`, trusts the package's signing
//!    certificate, seeds `C:\VirtualDisplayDriver\vdd_settings.xml`, and runs
//!    nefcon's devcon-compatible `install <inf> Root\MttVDD` (nefcon creates
//!    the ROOT-enumerated device node *and* installs the package - `pnputil`
//!    can only stage the package, it cannot create the device node).
//!
//! Success is only reported when the device is enumerated by SetupAPI **and**
//! `virtual_display::is_driver_installed()` is true. Everything here is
//! Windows-only.

#![cfg(windows)]

use std::ffi::{c_void, OsString};
use std::fs;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_Status, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
    SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, CM_DEVNODE_STATUS_FLAGS, CM_PROB,
    DIGCF_PRESENT, GUID_DEVCLASS_DISPLAY, SP_DEVINFO_DATA, CR_SUCCESS,
};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_CANCELLED, HANDLE,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::virtual_display::{device_matches, is_driver_installed, ping};

/// Driver release that the bundled files come from (see `driver/NOTICE.md`).
pub const BUNDLED_VERSION: &str = "25.7.23";

/// Hardware ID the INF registers (`MttVDD.inf`, `[Standard.NTamd64]`).
const HARDWARE_ID: &str = r"Root\MttVDD";
/// Device setup class of the INF (`ClassGUID` in `MttVDD.inf`).
const DISPLAY_CLASS_GUID: &str = "4d36e968-e325-11ce-bfc1-08002be10318";
/// Thumbprint of the `CN=SignPath Foundation` certificate inside `mttvdd.cat`.
const PUBLISHER_THUMBPRINT: &str = "3cf8cf26d8ba266c3a483ab7d26d4a818e317d76";
/// Where the driver reads its settings from (`confpath` in `Driver.cpp`).
const SETTINGS_DIR: &str = r"C:\VirtualDisplayDriver";
const SETTINGS_FILE: &str = r"C:\VirtualDisplayDriver\vdd_settings.xml";
/// DN_STATUS_STARTED from cfg.h (not exported by the `windows` crate).
const DN_STATUS_STARTED: u32 = 0x0000_0008;
/// How long to wait for Windows to surface the device after an install.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// Files embedded into `server.exe` at build time.
struct Embedded {
    name: &'static str,
    bytes: &'static [u8],
}

const PAYLOAD: &[Embedded] = &[
    Embedded {
        name: "MttVDD.inf",
        bytes: include_bytes!("../driver/MttVDD.inf"),
    },
    Embedded {
        name: "MttVDD.dll",
        bytes: include_bytes!("../driver/MttVDD.dll"),
    },
    Embedded {
        name: "mttvdd.cat",
        bytes: include_bytes!("../driver/mttvdd.cat"),
    },
    Embedded {
        name: "SignPathFoundation.cer",
        bytes: include_bytes!("../driver/SignPathFoundation.cer"),
    },
    Embedded {
        name: "nefconc.exe",
        bytes: include_bytes!("../driver/nefconc.exe"),
    },
];
/// Seeded separately: only written to `C:\VirtualDisplayDriver` when missing.
const DEFAULT_SETTINGS: &[u8] = include_bytes!("../driver/vdd_settings.default.xml");

/// A `Root\MttVDD` display device as Windows currently reports it.
pub struct DeviceInfo {
    pub instance_id: String,
    /// `CM_DEVNODE_STATUS_FLAGS`; bit 3 (`DN_STATUS_STARTED`) means started.
    pub status: u32,
    /// `CM_PROB_*`; 0 means no problem.
    pub problem: u32,
}

impl DeviceInfo {
    pub fn started(&self) -> bool {
        self.status & DN_STATUS_STARTED != 0
    }
}

/// Enumerate present display devices and return the `Root\MttVDD` one.
pub fn device_info() -> Option<DeviceInfo> {
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_DISPLAY),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT,
        )
        .ok()?;

        let mut found = None;
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
            if !device_matches(set, &data) {
                continue;
            }

            let mut buffer = [0u16; 512];
            let instance_id = match SetupDiGetDeviceInstanceIdW(set, &data, Some(&mut buffer), None)
            {
                Ok(()) => {
                    let len = buffer.iter().position(|&ch| ch == 0).unwrap_or(buffer.len());
                    String::from_utf16_lossy(&buffer[..len])
                }
                Err(_) => String::new(),
            };

            let mut status = CM_DEVNODE_STATUS_FLAGS(0);
            let mut problem = CM_PROB(0);
            let queried = CM_Get_DevNode_Status(&mut status, &mut problem, data.DevInst, 0)
                == CR_SUCCESS;
            found = Some(DeviceInfo {
                instance_id,
                status: if queried { status.0 } else { 0 },
                problem: if queried { problem.0 } else { 0 },
            });
            break;
        }

        let _ = SetupDiDestroyDeviceInfoList(set);
        found
    }
}

/// `true` when this process runs with an elevated token.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Re-launch `server.exe` elevated through UAC.
///
/// Returns `Ok(Some(exit_code))` in the parent process (the caller must exit
/// with that code) and `Ok(None)` when the process is already elevated.
///
/// The child carries the hidden `--elevated` flag. If that flag is present but
/// the token is still not elevated, UAC was bypassed or elevation is disabled,
/// so bail out instead of looping.
pub fn ensure_elevated(operation_flag: &str) -> Result<Option<i32>> {
    if is_elevated() {
        return Ok(None);
    }
    if std::env::args().any(|arg| arg == "--elevated") {
        bail!(
            "administrator rights are required and this process was launched with \
             --elevated but still has no elevated token (is UAC disabled?)"
        );
    }

    let exe = std::env::current_exe().context("locating server.exe")?;
    let parameters = format!("{operation_flag} --elevated");
    eprintln!(
        "{operation_flag} needs administrator rights; asking Windows for elevation (UAC) ..."
    );

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = parameters
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR::from_raw(verb.as_ptr()),
        lpFile: PCWSTR::from_raw(file.as_ptr()),
        lpParameters: PCWSTR::from_raw(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };

    let launched = unsafe { ShellExecuteExW(&mut info) };
    if let Err(err) = launched {
        let cancelled = err.code() == HRESULT::from_win32(ERROR_CANCELLED.0)
            || unsafe { GetLastError() } == ERROR_CANCELLED;
        if cancelled {
            bail!("elevation was cancelled at the UAC prompt; nothing was changed");
        }
        return Err(anyhow::Error::new(err)).context("starting the elevated helper failed");
    }
    if info.hProcess.0.is_null() {
        bail!("Windows did not return a process handle for the elevated helper");
    }

    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let result = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        result.context("reading the elevated helper's exit code")?;
        eprintln!("elevated helper finished with exit code {code}");
        Ok(Some(code as i32))
    }
}

/// Install the driver. Must be called in an elevated process.
pub fn install() -> Result<()> {
    if !is_elevated() {
        bail!("--install-driver must run elevated (start server.exe and let it ask for UAC)");
    }

    println!("virtual display driver {BUNDLED_VERSION} (MIT, VirtualDrivers/Virtual-Display-Driver)");
    let dir = materialise()?;
    trust_publisher(&dir)?;
    seed_settings()?;

    if let Some(info) = device_info() {
        if info.problem != 0 {
            println!(
                "device {} is present but has problem code {}; removing and reinstalling...",
                info.instance_id, info.problem
            );
            let _ = remove_package(&dir);
            std::thread::sleep(Duration::from_millis(1000));
            println!("creating the Root\\MttVDD device and installing the driver package ...");
            install_package(&dir)?;
        } else {
            println!(
                "device already present: {} (not creating another one)",
                info.instance_id
            );
        }
    } else {
        println!("creating the Root\\MttVDD device and installing the driver package ...");
        install_package(&dir)?;
    }

    verify(VERIFY_TIMEOUT)
}

/// Remove the device node and the driver package. Must run elevated.
pub fn uninstall() -> Result<()> {
    if !is_elevated() {
        bail!("--uninstall-driver must run elevated (start server.exe and let it ask for UAC)");
    }

    let dir = materialise()?;
    println!("removing Root\\MttVDD devices and the driver package ...");
    let removed = remove_package(&dir)?;

    let deadline = Instant::now() + VERIFY_TIMEOUT;
    loop {
        let present = device_info();
        let installed = is_driver_installed();
        if present.is_none() && !installed {
            println!("verified: no Root\\MttVDD display device and no matching driver package");
            if Path::new(SETTINGS_FILE).is_file() {
                println!("kept {SETTINGS_FILE} (delete it by hand if you want a clean slate)");
            }
            println!(
                "note: the publisher certificate stays in LocalMachine\\TrustedPublisher; \
                 remove it with: certutil -delstore TrustedPublisher {PUBLISHER_THUMBPRINT}"
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            if removed != 0 {
                bail!(
                    "nefcon exited with {removed} and the device is still present; \
                     check Device Manager, then retry (a reboot may be required)"
                );
            }
            bail!("the device is still present after 30 s; a reboot may be required");
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Print the driver state and return whether it counts as installed.
pub fn status() -> bool {
    println!("Virtual Display Driver {BUNDLED_VERSION} (MIT, bundled)");
    match device_info() {
        Some(info) => {
            println!("device:      present, instance ID {}", if info.instance_id.is_empty() { "?" } else { &info.instance_id });
            println!(
                "status:      0x{:08x} (started: {}, problem code: {})",
                info.status,
                info.started(),
                info.problem
            );
            if info.problem != 0 {
                println!(
                    "             problem code {} means Device Manager shows an error",
                    info.problem
                );
            }
        }
        None => println!("device:      NOT present (no display device with hardware ID Root\\MttVDD)"),
    }

    let installed = is_driver_installed();
    println!("installed:   {installed} (SetupAPI, DIGCF_PRESENT + hardware ID match)");
    println!(
        "settings:    {} ({})",
        SETTINGS_FILE,
        if Path::new(SETTINGS_FILE).is_file() {
            "present"
        } else {
            "missing"
        }
    );
    let dir = payload_dir();
    println!(
        "payload:     {} ({})",
        dir.display(),
        if dir.is_dir() { "present" } else { "not deployed" }
    );
    println!(
        "driver pipe: {}",
        if installed && ping() {
            "PING -> PONG"
        } else {
            "no reply (driver not installed or not running)"
        }
    );
    println!("elevated:    {}", is_elevated());
    installed
}

/// Write the embedded package and nefcon to `%ProgramData%\lowlat-screen\driver`.
fn materialise() -> Result<PathBuf> {
    let dir = payload_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    for file in PAYLOAD {
        let path = dir.join(file.name);
        fs::write(&path, file.bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(dir)
}

fn payload_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("lowlat-screen").join("driver")
}

fn seed_settings() -> Result<()> {
    let dir = Path::new(SETTINGS_DIR);
    if !dir.is_dir() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let settings = Path::new(SETTINGS_FILE);
    if settings.is_file() {
        println!("kept existing {SETTINGS_FILE}");
        return Ok(());
    }
    fs::write(settings, DEFAULT_SETTINGS)
        .with_context(|| format!("writing {}", settings.display()))?;
    println!("created {SETTINGS_FILE} from the bundled defaults");
    Ok(())
}

/// Trust the catalog's signing certificate so the install needs no prompt.
fn trust_publisher(dir: &Path) -> Result<()> {
    let trusted = certutil(&["-store", "TrustedPublisher", PUBLISHER_THUMBPRINT])?;
    if trusted {
        println!("publisher certificate is already in LocalMachine\\TrustedPublisher");
        return Ok(());
    }

    let cer = dir.join("SignPathFoundation.cer");
    println!(
        "trusting the driver publisher certificate (CN=SignPath Foundation) for LocalMachine ..."
    );
    let output = Command::new("certutil")
        .arg("-addstore")
        .arg("-f")
        .arg("TrustedPublisher")
        .arg(&cer)
        .output()
        .context("running certutil (part of Windows)")?;
    if !output.status.success() {
        bail!(
            "certutil -addstore TrustedPublisher failed with {}:\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("publisher certificate trusted");
    Ok(())
}

/// `certutil -store <store> <thumbprint>` returns 0 when the certificate exists.
fn certutil(args: &[&str]) -> Result<bool> {
    let output = Command::new("certutil")
        .args(args)
        .output()
        .context("running certutil (part of Windows)")?;
    Ok(output.status.success())
}

/// Run nefcon's devcon-compatible install and fail on a non-zero exit code.
fn install_package(dir: &Path) -> Result<()> {
    let inf = dir.join("MttVDD.inf");
    let code = nefcon(
        dir,
        vec![
            OsString::from("install"),
            inf.into_os_string(),
            OsString::from(HARDWARE_ID),
        ],
    )?;
    // 3010 = ERROR_SUCCESS_REBOOT_REQUIRED: installed, but a reboot is pending.
    if code != 0 && code != 3010 {
        bail!("nefcon install exited with {code}");
    }
    Ok(())
}

/// Remove every `Root\MttVDD` device and the matching driver package.
fn remove_package(dir: &Path) -> Result<i32> {
    nefcon(
        dir,
        [
            "--remove-device-node",
            "--hardware-id",
            HARDWARE_ID,
            "--class-guid",
            DISPLAY_CLASS_GUID,
        ]
        .iter()
        .map(OsString::from)
        .collect(),
    )
}

fn nefcon(dir: &Path, args: Vec<OsString>) -> Result<i32> {
    let exe = dir.join("nefconc.exe");
    let shown: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    println!("running: {} {}", exe.display(), shown.join(" "));

    let output = Command::new(&exe)
        .current_dir(dir)
        .args(&args)
        .output()
        .context("running nefconc.exe")?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    print!("{}", String::from_utf8_lossy(&output.stderr));
    Ok(output.status.code().unwrap_or(-1))
}

/// Poll SetupAPI until the device exists and matches `is_driver_installed()`.
fn verify(timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    println!("verifying (up to {} s) ...", timeout.as_secs());
    loop {
        let info = device_info();
        let installed = is_driver_installed();
        if let Some(info) = info {
            println!(
                "device enumerated: {} (status 0x{:08x}, started: {}, problem: {})",
                if info.instance_id.is_empty() { "?" } else { &info.instance_id },
                info.status,
                info.started(),
                info.problem
            );
            if installed && info.problem == 0 {
                println!("verified: driver installed and the device has no problem code");
                return Ok(());
            }
            if installed && info.problem != 0 {
                bail!(
                    "the device is present but Device Manager reports problem code {} \
                     (check C:\\VirtualDisplayDriver\\Logs and Device Manager)",
                    info.problem
                );
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "the driver did not appear within {} s; check Device Manager for \
                 \"Virtual Display Driver\" and C:\\VirtualDisplayDriver\\Logs",
                timeout.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
}

