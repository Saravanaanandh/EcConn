# M9: Display Orientation & Rotation

## 1. Research Findings

### A. Virtual Display Driver (IddCx) and `ChangeDisplaySettingsEx`

**Question**: Does the Virtual Display Driver (IddCx) monitor support Windows rotation via `ChangeDisplaySettingsEx`?

**Investigation & Findings**:
1. **Mode Enumeration**:
   When queried using `EnumDisplaySettingsExW(lpszDeviceName, ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm, EDS_ROTATEDMODE)`, the Virtual Display Driver (`\\.\DISPLAY11` / `Root\MttVDD`) successfully reports 168 display modes covering all 4 orientations:
   - `DMDO_DEFAULT` (0°): e.g., 1280x720, 1920x1080 at 30, 60, 90, 120, 144, 165, 244 Hz.
   - `DMDO_90` (90°): e.g., 720x1280, 1080x1920 at 30, 60, 90, 120, 144, 165, 244 Hz.
   - `DMDO_180` (180°): e.g., 1280x720, 1920x1080 at 30, 60, 90, 120, 144, 165, 244 Hz.
   - `DMDO_270` (270°): e.g., 720x1280, 1080x1920 at 30, 60, 90, 120, 144, 165, 244 Hz.

2. **Application via `ChangeDisplaySettingsEx`**:
   - Calling `ChangeDisplaySettingsExW(deviceName, &dm, NULL, CDS_UPDATEREGISTRY | CDS_NORESET, NULL)` followed by `ChangeDisplaySettingsExW(NULL, NULL, NULL, 0, NULL)` with swapped `dmPelsWidth`/`dmPelsHeight` and `dmDisplayOrientation` returns `DISP_CHANGE_FAILED (-1)`.
   - On modern Windows 11 (builds 22621+ / 24H2), the Windows graphics architecture routes indirect display (IddCx) topology and rotation changes through the CCD (Connecting and Configuring Displays) API (`SetDisplayConfig`), rather than legacy GDI `ChangeDisplaySettingsEx`.
   - When calling `SetDisplayConfig`, user-space / non-elevated background calls return `ERROR_ACCESS_DENIED (5)` unless attached to the interactive console desktop with appropriate session rights.
   - **Conclusion**: The driver's mode table fully defines all 4 rotated modes, but Windows GDI `ChangeDisplaySettingsEx` fails with `DISP_CHANGE_FAILED (-1)` when attempting dynamic rotation on indirect display driver outputs in Windows 11.

---

### B. DXGI Desktop Duplication Rotation (`DXGI_OUTDUPL_DESC.Rotation`)

**Question**: What does DXGI Desktop Duplication return for a rotated output (`DXGI_OUTDUPL_DESC.Rotation`): is the frame already rotated or do we have to rotate it?

**Documentation & Findings**:
1. **Microsoft MSDN Specification**:
   According to Microsoft's official documentation for [`IDXGIOutputDuplication::AcquireNextFrame`](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe):
   > *"In a rotated mode, the surface that you receive from `IDXGIOutputDuplication::AcquireNextFrame` is always in the un-rotated orientation, and the desktop image is rotated within the surface."*

2. **Output Description (`DXGI_OUTDUPL_DESC`)**:
   - `ModeDesc.Width` and `ModeDesc.Height` describe the un-rotated raster format (e.g., 1280x720).
   - `Rotation` describes how the desktop image is rotated within that surface:
     - `DXGI_MODE_ROTATION_IDENTITY` (1): Desktop image is upright (0°).
     - `DXGI_MODE_ROTATION_ROTATE90` (2): Desktop image is rotated 90° clockwise.
     - `DXGI_MODE_ROTATION_ROTATE180` (3): Desktop image is rotated 180°.
     - `DXGI_MODE_ROTATION_ROTATE270` (4): Desktop image is rotated 270°.

3. **Frame Orientation & Handling**:
   - Because the surface received from `AcquireNextFrame` remains in the un-rotated buffer dimensions, but the desktop image inside it is rendered in the rotated orientation by the DWM, transmitting the raw surface directly would present a sideways or upside-down picture unless the client or pipeline applies the corresponding rotation.
   - When the monitor mode itself is reconfigured (or when rotating at the encoder/client), the width and height swap (e.g. 1280x720 becomes 720x1280), requiring texture reallocation and coordinate remapping.

---

## 2. Server Architecture

1. **CLI Flag**:
   `--orientation <0|90|180|270>` sets the initial target orientation of the captured display.
2. **Runtime Orientation Control**:
   The server accepts client control messages requesting an orientation switch (`0`, `90`, `180`, `270`).
3. **Display Settings Application**:
   When orientation changes, the server:
   - Invokes `ChangeDisplaySettingsExW` with `dmDisplayOrientation` (`DMDO_DEFAULT`, `DMDO_90`, `DMDO_180`, `DMDO_270`) and swaps `dmPelsWidth`/`dmPelsHeight` for 90°/270°.
   - Re-creates capture duplication upon `DXGI_ERROR_ACCESS_LOST` or resolution change.
   - Restarts the encoder with the new dimensions.
   - Sends a `DisplayConfig` notification to the client with the updated `(width, height, orientation)`.

---

## 3. Client Architecture

1. **Dynamic Resizing**:
   On receiving a size change or `DisplayConfig` notification, the client tears down and recreates:
   - The SDL video texture with the new `width` and `height`.
   - The SDL window size (or updates fullscreen canvas dimensions).
2. **Client-Side Request Keys & Flags**:
   - CLI flag `--orientation <0|90|180|270>` to request an initial orientation from the server.
   - Key `R` cycles rotation (`0° -> 90° -> 180° -> 270° -> 0°`) by dispatching an orientation control packet to the server.
3. **Independent Client Flip (`--flip`)**:
   - CLI flag `--flip` flips the displayed screen 180° client-side (for physically inverted displays), independent of Windows OS rotation.
   - Input coordinates under `--flip` apply the inverse transform (`x' = w - 1 - x`, `y' = h - 1 - y`) before normalizing.
