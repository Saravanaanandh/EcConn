# lowlatency screen extend over wired LAN. Server (Windows) captures, encodes
and streams; client (Linux Mint) receives, decodes and shows fullscreen.
See `SPEC.md` for the full plan.

```
server/   capture + encode (M1); UDP send from M2
client/   M2: UDP receive + SDL2 window, reassembly, ffmpeg decode
common/   shared constants; packet header lands in M2
```

## M3: Hardware encode and decode, low-latency settings (work in progress)

- **Server**: Hardware encode paths available: NVENC (NVIDIA), QuickSync (Intel),
  AMD AMF. All confirmed via `ffmpeg -encoders` on this Windows machine.
  **Default** (`encode-path "auto"`): auto-selects first available path
  (NVENC > QuickSync > AMF > libx264 software). Prints detected path on first run.
  **Software fallback**: ffmpeg `libx264` (ultrafast preset, no B-frames, short GOP,
  CBR, CRF 20) — the M2 path, fully measured and verified.
- **Client**: VAAPI hardware decode (`-hwaccel vaapi`) not yet tested on this
  Windows build. Software ffmpeg decode confirmed working (M2 path).
  **Fallback**: ffmpeg software decode always available.
- **Latency rules** (from SPEC §4): H.264 low-latency preset, no B-frames, CBR or
  tight VBV, short GOP (~1 s), periodic intra refresh / on-demand keyframes.
- **Done when**: stream runs smoothly at 1080p60 with measured timings printed.
  Measured with `--seconds N` and per-second stats report.

### Measured numbers (loopback, M2 software path only)

```
recv 20/s 353 pkts 3.8 Mbit/s | shown 20/s | assembly 0.1/2.0 ms | decode 110.6/151.7 ms | render 17.0/39.9 ms | dropped 0 replaced 0 kf-req 0
```

Server encoder restart on keyframe request #1, #2, #3 as expected (restart cost ~100–300 ms).

### M3 changes (code implemented, hardware untested)

- **Server encoder** (`server/src/encoder.rs`): Auto-detect path on first run
  (NVENC > QuickSync > AMF > libx264). Prints detected path. All four paths
  build successfully; only libx264 software path has been measured in end-to-end
  testing.
- **Client decoder** (`client/src/decoder.rs`): VAAPI hardware decode code paths
  added but untested on Windows. Software fallback confirmed working (M2).
- **Both paths**: Per-frame timing instrumentation added (capture → encode →
  submit → decode → render).

### UNMEASURED

- VAAPI hardware decode on Linux Mint client: not tested (no Windows test
  possible for VAAPI). Marked UNMEASURED until tested with real hardware.
- NVENC/QSV/AMF actual encode performance/latency: not measured end-to-end.
  Only software path measured.
- Auto-detection printout: verified on this Windows machine only.
- **Client decoder**: yuv420p + SDL IYUV texture path (replaces BGRA).
  Software decode + IYUV render measured as part of M2 loopback pipeline.
- **Server**: `--width`, `--height`, `--fps` flags added with defaults
  `1280x720 @ 30`. Configures libx264 encode params (`-preset ultrafast`,
  `-tune zerolatency`, `-bf 0`, short GOP `fps/2`). CPU % of server process
  not yet measured (marked UNMEASURED).

## M7: Virtual display (`--virtual`, Windows only)

`server --virtual` adds a temporary monitor with the
[Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver)
(MIT), captures it, and removes it when the server exits. If the driver is not
installed it prints a message and captures the primary monitor instead.

```
server.exe --virtual --width 1920 --height 1080 --fps 60
server.exe --virtual --seconds 30          # auto-detaches after 30 s
```

Implementation: `server/src/virtual_display.rs`.

### Facts taken from the driver source (not guessed)

| Fact | Source |
| --- | --- |
| Hardware ID `Root\MttVDD` (also `MttVDD`), class `Display` | `Virtual Display Driver (HDR)/MttVDD/MttVDD.inf` |
| Config path `C:\VirtualDisplayDriver\vdd_settings.xml` | `Driver.cpp`, `confpath` |
| `<monitors><count>N</count>`, 0 is coerced to 1 | `Driver.cpp`, XML reader |
| `<resolutions><resolution>{width,height,refresh_rate}` | `Driver.cpp`, XML reader |
| First `<resolution>` becomes the preferred mode (`PreferredMonitorModeIdx = 0`) | `Driver.cpp`, monitor description |
| Pipe `\\.\pipe\MTTVirtualDisplayPipe`, UTF-16LE commands | `Driver.cpp`, `PIPE_NAME` |
| `PING` → `PONG` | `Driver.cpp`, `HandleClient` |
| `SETDISPLAYCOUNT <n>` rewrites the XML count and reinitialises the adapter | `Driver.cpp`, `HandleClient` |

`attach()` inserts the requested mode at the top of `<resolutions>` when it is
missing (so the new monitor prefers it), snapshots DXGI outputs, sends
`SETDISPLAYCOUNT <current+1>`, then polls for up to 10 s for an output name that
was not present before. `detach()` restores the previous count. Both
`Drop`/panic unwinding and a `SetConsoleCtrlHandler` handler call `detach()`.

### What was actually RUN on this machine

- `cargo build --release`: **RUN, passed** (see build output in the task log).
- `cargo test --release`: **RUN, passed** — 3 common protocol tests and 4 new
  `virtual_display` XML tests (`read_count`, resolution detection/insertion).
- Fallback path (`--virtual` with no driver): **RUN**. The driver is **not
  installed** here, and the server printed:

  ```
  --virtual: Virtual Display Driver not installed (no Root\MttVDD display device).
  Falling back to the primary monitor (index 0). ...
  capture monitor 0: 1920x1080
  ```

  and then streamed normally (real brightness, 0.42 ms copy).

### NOT RUN / UNVERIFIED

The Virtual Display Driver is **NOT installed on this machine**, so every
runtime interaction with it is **NOT RUN** and **UNVERIFIED**:

- `is_driver_installed()` returning `true` (the device-enumeration match against
  `Root\MttVDD`): **NOT RUN** — only the `false` path was exercised.
- Editing `C:\VirtualDisplayDriver\vdd_settings.xml` on a real installation
  (the XML logic is unit-tested, but not against the driver's file):
  **UNVERIFIED**.
- Opening the pipe, `PING`/`PONG`, `SETDISPLAYCOUNT`, and the
  `ERROR_PIPE_BUSY` (231) / `ERROR_BROKEN_PIPE` (109) /
  `ERROR_PIPE_NOT_CONNECTED` (233) retry handling: **NOT RUN**.
- Waiting for the new DXGI output and returning its index: **NOT RUN**.
- Whether the driver actually applies the requested resolution to the new
  monitor (Windows may keep another mode): **UNVERIFIED**. The server always
  captures the real size reported by DXGI and prints it.
- `detach()` via `Drop`, panic unwinding, and Ctrl+C: **NOT RUN**.
- The 1–16 clamp: the driver source only rejects `0`; 16 is the documented UI
  limit, so the clamp is **UNVERIFIED** at runtime.

Known limitations:

- The driver coerces a count of `0` to `1`, so this module cannot remove the
  last virtual monitor; `detach()` restores the count that existed before
  `attach()`.
- `<resolutions>` is global to all virtual monitors. Adding a mode can change
  the preferred mode of existing virtual monitors.
- If the process is killed without running destructors (Task Manager,
  `taskkill /F`), the driver's XML count stays incremented. Run the server
  again with `--virtual` (it adds another monitor) or reset `<count>` manually.
- The config file is edited in place; a `.xml.lowlat-tmp` file is used while
  writing.
