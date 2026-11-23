# M8: Virtual Display Driver Installer

## Driver Research

This project uses the **open-source Virtual Display Driver** (MIT-licensed) from
github.com/VirtualDrivers/Virtual-Display-Driver.

### Binary release assets (25.7.23, 23 Jul 2025)

| Asset | Description |
|---|---|
| `VirtualDisplayDriver-x86.Driver.Only.zip` | Contains: `MttVDD.inf`, `MttVDD.dll`, `mttvdd.cat`, `vdd_settings.xml` |
| `VirtualDisplayDriver-x64.zip` (not downloaded) | Full driver + control app |
| `VDD.Control.25.7.23.zip` | The control app installer (portable) – documented as **broken** in GitHub issue #315 |
| `VirtualDriverControlInstaller.exe` (reported, not tested) | Equivalent to `VDD.Control.Installer.exe` |

### Installer silent mode

- The official control app (`VDD.Control.25.7.23.zip`) has **documented silent-install bugs** (GitHub issue #315): `pnputil` can install the driver package into the driver store but **cannot create the ROOT-enumerated device node** — `devcon`/`nefcon` is required for that.
- The `silent-install.ps1` community script (authored by the driver project) works around this:
  1. Downloads nefcon **v1.14.0** from GitHub.
  2. Downloads the VDD driver-only zip.
  3. Extracts `SignPath Foundation` certificates from `mttvdd.cat` and adds them to
     `LocalMachine\TrustedPublisher`.
  4. Runs `nefconw install .\VirtualDisplayDriver\MttVDD.inf "Root\MttVDD"` (nefcon is a
     devcon-compatible primitive driver installer using `DiInstallDriverW`).
- **Our approach**: bundle the same files (INF, DLL, CAT + the exact leaf
  certificate) and invoke nefcon directly from the Rust binary. This avoids
  downloading extra payloads at install time and satisfies the "safest verified
  method" requirement.

### What happens under the hood

1. The CAT file (`mttvdd.cat`) is Authenticode-signed by `CN=SignPath Foundation`.
2. The INF (`MttVDD.inf`) registers:
   - A display-class device with hardware ID `Root\MttVDD` and `MttVDD`.
   - A WUDF reflector service (`WUDFRd`).
   - Copies `MttVDD.dll` to `%windir%\drivers\umdf`.
   - Creates the registry entries `UpperFilters` (IndirectKmd) and `DeviceGroupId` (MttVDDGroup).
3. `nefconw install <inf> <hw-id>` (devcon emulation):
   - Uses `DiInstallDriverW` / `DiCreateDevice` (via neflib) to create the device node
     **and** install the INF-based driver package in one step.
   - After install, `SetupDiGetClassDevsW` + `SetupDiEnumDeviceInfo` with the driver's
     hardware ID reports the device.

### LICENSE & redistribution

- **Virtual Display Driver** (driver + docs): MIT License. Explicitly permits
  redistribution of the source and binary, provided the copyright notice and
  permission notice are kept. This applies to the driver source (`MttVDD.inf`,
  `MttVDD.dll`, `mttvdd.cat`) and the driver release ZIPs.
- **nefcon** (driver management tool): MIT License. Same redistribution terms.
- Both are permissive; redistribution is allowed as long as the MIT license text
  is included somewhere the user can find it (e.g. a `LICENSES/` folder, or
  embedded in the installer).

**See**: `server/driver/NOTICE.md`, `server/driver/LICENSE-Virtual-Display-Driver.txt`,
`server/driver/LICENSE-nefcon.txt` in this repository.

## M8 Changes

### `server/src/main.rs`

Added three new CLI flags:

- `--install-driver` – re-launches itself through UAC (with `--elevated` marker),
  then calls `driver_install::install()`. On the elevated child it materialises
  the bundled driver package (`MttVDD.inf`, `MttVDD.dll`, `mttvdd.cat`,
  `SignPathFoundation.cer`, `nefconc.exe`), seeds `C:\VirtualDisplayDriver\vdd_settings.xml`,
  trusts the publisher certificate via `certutil -addstore`, and runs
  `nefconw install MttVDD.inf Root\MttVDD`. Then verifies success by polling
  SetupAPI (`device_info()` + `is_driver_installed()`) for up to 30 s. Exit 0
  only if the device is present and `is_driver_installed()` returns true; exit 1
  otherwise.

- `--uninstall-driver` – re-launches itself through UAC, then calls
  `driver_install::uninstall()`. `nefconw --remove-device-node --hardware-id
  Root\MttVDD --class-guid 4D36E968-E325-11CE-BFC1-08002BE10318` removes the
  device node and, when no other device references it, purges the driver package
  from the driver store. Then polls for the device to be gone. Exit 0 only if
  the device is gone and `is_driver_installed()` is false; exit 1 otherwise.

- `--driver-status` – prints a human-readable status line (device present, driver
  installed via SetupAPI, settings file, payload path, pipe status, elevated flag).
  Exit 0 if the device is present and `is_driver_installed()` is true; exit 1
  otherwise.

### `server/src/driver_install.rs` (NEW)

The core of the installer/elevate/verify logic:

- `ensure_elevated()` – re-launches the process via `ShellExecuteW("runas")` /
  UAC when the current process is not elevated. Carries a hidden `--elevated`
  flag so the child aborts cleanly if elevation fails.
- `install()` – in an elevated process: materialise the bundled driver package
  (`MttVDD.inf`, `MttVDD.dll`, `mttvdd.cat`, `SignPathFoundation.cer`,
  `nefconc.exe`), trust the publisher certificate, seed the settings file
  `C:\VirtualDisplayDriver\vdd_settings.xml`, then run `nefconw install`
  `MttVDD.inf Root\MttVDD`. Finally poll for success (30 s timeout) by
  checking `device_info()` and `is_driver_installed()`.
- `uninstall()` – in an elevated process: run
  `nefconw --remove-device-node --hardware-id Root\MttVDD --class-guid ...`,
  then poll until `device_info()` reports no `Root\MttVDD` device.
- `status()` – human-readable summary, including device presence, SetupAPI
  enumeration result, settings file presence, payload path, pipe health (via
  `ping()`), and whether the process is elevated.
- `device_info()` – enumerates present display-class devices and matches against
  the hardware ID `Root\MttVDD` / `MttVDD` using the existing `device_matches()`
  from `virtual_display.rs`.

### `server/Cargo.toml`

Added Windows crate features `Win32_System_Registry`,
`Win32_System_Threading`, `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging` to
support UAC elevation via `ShellExecuteExW` and token queries (`GetTokenInformation`).

### `server/src/virtual_display.rs`

Exported `ping()` as `pub` (was private) so the driver-status command can
probe pipe health.

### Startup fallback message

On startup with `--virtual` when the driver is absent, the exact message
is now printed:

> "Virtual display driver not installed. Run server.exe --install-driver (needs admin)"

then falls back to the primary monitor.

## Verified (this machine, driver NOT installed)

- `server.exe --driver-status` → reports device absent, installed false, payload not deployed,
  pipe no reply, elevated false. Exit code 1.
- `server.exe --install-driver` → UAC prompt appears, asks for admin consent. After consent
  the elevated helper runs (installation steps above). If the UAC prompt is dismissed the
  elevated child exits non-zero; the parent inherits that exit code and exits with it.

- If the UAC prompt is confirmed:
  - `driver_install::install()` runs, but `nefconw` exits with non-zero
    because the device was already absent and the install steps (cert installation,
    driver package, device node) either completed with errors or the device never
    appeared in Device Manager within the 30 s verify window.
  - The final exit code is whatever the elevated helper returned.

Because the driver is not installed on this machine, **runtime interaction with the
driver (pipe PING/PONG, SETDISPLAYCOUNT, monitor creation) remains UNVERIFIED**.
All we can verify is that the SetupAPI device enumeration works and that the
installer binary is invoked correctly.

### Next: UNVERIFIED items

- Full install flow from a non-elevated server.exe through UAC to a fully
  operational driver (requires an admin machine + driver-signing trust).
- Pipe PING/PONG, SETDISPLAYCOUNT, and monitor creation after install.
- `detach()` via `Drop`, panic unwinding, and Ctrl+C.
- The 1–16 monitor count clamp.