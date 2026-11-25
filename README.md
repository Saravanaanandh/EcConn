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

## M10: GUI apps (`app-host`, `app-receiver`)

Two egui/eframe apps on top of the existing code. No capture, encoder, decoder
or orientation logic was rewritten: the host starts the existing `server.exe`
and calls `server::virtual_display` in-process; the receiver builds the existing
`client::net::Receiver` + `client::decoder::Decoder` and only replaces the SDL2
window with an egui texture.

```
cargo build --release            # builds app-host, app-receiver, server, client
target\release\app-receiver.exe
target\release\app-host.exe
```

The host finds `server.exe` next to itself (or `LOWLAT_SERVER_EXE` if set), so
build the whole workspace rather than only the two apps.

Headless modes used for verification (identical discovery/pairing/stream code,
no window):

```
target\release\app-receiver.exe --demo --auto-allow --seconds 25
target\release\app-host.exe --demo --duration 10
```

### Discovery

Receivers advertise with the mDNS service `_lowlat._udp.local.` (crate
`mdns-sd`) and, as a fallback for networks that block multicast, with a UDP
broadcast beacon every 2 s on port 9200. Both carry: id, name, platform, screen
width/height, natural orientation, video port (9100) and pairing port (9201).
The beacon is a single text line (`LOWLAT1|id|name|platform|w|h|orientation|
video|pair`); parsing/sanitising is unit-tested in `common/src/discovery.rs`.

The host binds UDP 9200 and browses mDNS; the receiver sends the beacon to
`255.255.255.255` **and** `127.0.0.1`, so two apps on one machine find each
other (that is how this milestone is tested). `LOWLAT_NO_MDNS=1` disables the
mDNS half on either side, which was used to exercise the beacon fallback.

### Pairing

```
PAIR1|REQ|host-id|host-name          host -> receiver (first contact)
PAIR1|RESUME|host-id|host-name       host -> receiver (already known)
PAIR1|CHALLENGE|123456               receiver -> host (code to compare)
PAIR1|ACCEPT|123456|token_hex|key    receiver -> host (after Allow)
PAIR1|REJECT|reason                  receiver -> host
```

The receiver shows `Allow "<host>" to extend your screen?` with the 6-digit
code; the host shows the same code. Allowing creates a random 32-byte session
key and a 64-bit input token. The receiver stores `(host id, name, token, key)`
in `%APPDATA%\lowlat-screen\paired_hosts.txt` (`~/.config/lowlat-screen/` on
Linux), and a returning host gets a one-click `Confirm` instead; the host
remembers receivers in `paired_receivers.txt` next to it. Denying sends
`REJECT`. Requests retry every 3 s for 45 s; a duplicate request repeats the
same code instead of opening a second dialog, and a lost `ACCEPT` is re-sent
when the host retries within 60 s.

The session key is stored for the next milestone; **the video stream is still
unencrypted**. Randomness is a clock/PID-seeded PRNG (documented in
`common/src/pairing.rs`), not a CSPRNG.

Input injection is tied to the pairing token: `server --token <n>` switches
`InputInjector` to `with_token`, which rejects every datagram whose token does
not match. `--token 0` keeps the legacy "first non-zero token wins" behaviour
for plain CLI use. Tests: `server/tests/token_reject.rs`.

### Host UI

Device list (name, IP, resolution, platform, mDNS/beacon), resolution, FPS,
orientation (0/90/180/270), encoder path, "Create virtual monitor", **Pair &
Connect**, **Stop**, pairing code, live status (`sent N frames`, kbit/s from the
server's own log; latency is not measured yet) and an event log.

On Connect it calls `server::virtual_display::attach()` when the driver is
installed; if it is missing or fails, the server mirrors the primary monitor
and the UI says so. `Stop` kills the server process and calls `detach()`.

### Receiver UI

"Waiting for a device" while advertising, the Allow/Deny dialog with the big
code, then borderless fullscreen video with Fit/Fill, 0/90/180/270 rotation,
decoded/packet/dropped counters and Disconnect in a small overlay (Esc toggles
it). "Control host (paired token)" is **off by default** — on a loopback test
it would move the real cursor of the same machine.

### Windows firewall helper

The host does **not** change firewall rules on startup. The UI shows the exact
command and runs it only after the user clicks the button:

```
netsh advfirewall firewall add rule name="lowlat-screen" dir=in action=allow protocol=UDP localport=9000,9200,9201
```

Run it from an elevated prompt if the button reports `netsh failed`. UDP 9100
belongs to the receiver, which Windows normally treats as an outbound
connection, so no rule is needed there.

### What was RUN on this machine (Windows 11, loopback)

Build and tests (raw output in the task log):

- `cargo build --release` — **RUN, passed** for `common`, `server`, `client`,
  `app-host`, `app-receiver`, no warnings.
- `cargo test --release -p common -p app-receiver` — **RUN, 31 passed**
  (24 discovery/pairing/store/protocol, 7 yuv/rotation/screen/identity).
- `cargo test --release -p server --test token_reject` — **RUN, 2 passed**
  (unpaired token is rejected without calling `SendInput`; paired token passes
  the gate using `ReleaseAll`, which performs no OS input).

Loopback, mDNS path (`app-receiver --demo --auto-allow --seconds 30`, then
`app-host --demo --duration 8`):

```
[host] discovered DESKTOP-A4O80PO at 192.168.56.1 (1920x1080, windows) via mdns
[host] sent REQUEST to 192.168.56.1:9201
[host] receiver asks to confirm code 029712
[host] PAIR_ACCEPT code=029712 token=bf68a05c96075dac
[host] PAIRED token=bf68a05c96075dac key=32 bytes (stored for the next task)
[host] starting ...\server.exe (monitor 0, 1920x1080@30) -> 192.168.56.1:9100
[server] sent   18 frames,  1322 packets, 12782.4 kbit/s, ... input received 0/s injected 0/s rejected 0
[receiver] pair request from DESKTOP-A4O80PO (host-DESKTOP-A4O80PO) at 192.168.56.1:60791
[receiver] dialog: Allow "DESKTOP-A4O80PO" to extend your screen? code 029712
[receiver] allowed DESKTOP-A4O80PO: token bf68a05c96075dac, 32-byte session key stored
[receiver] first decoded frame: 3110400 bytes, 1920x1080 (orientation=0 deg, rotation=1)
[receiver] status: decoded=107 packets=7916 dropped=20 running=true
```

Loopback, beacon-only + resume (`LOWLAT_NO_MDNS=1`, second run, same stores):

```
[host] note: mDNS disabled by LOWLAT_NO_MDNS; beacon only
[host] discovered DESKTOP-A4O80PO at 127.0.0.1 (1920x1080, windows) via beacon
[host] pairing (resume), host='DESKTOP-A4O80PO'
[host] sent RESUME to 127.0.0.1:9201
[host] PAIR_ACCEPT code=127575 token=bf68a05c96075dac
[receiver] pair request from ... at 127.0.0.1:61210 - previously paired
[receiver] PAIR REQUEST from 'DESKTOP-A4O80PO' (...) code=127575 resume=true
[receiver] status: decoded=130 packets=7133 dropped=0 running=true
```

The stored token from the first run was reused, and the second run dropped no
frames (first run dropped 20 while joining mid-GOP).

Virtual display path (`app-host --demo --virtual --duration 6`): **RUN once**.
`attach()` created a monitor, the host captured it and the receiver decoded
frames:

```
virtual display: requested 2 display(s) (was 1), waiting up to 10 s for the new output
virtual display: attached as monitor 2 (720x1280 at (1920,0), name='\\.\DISPLAY5')
[host] virtual display attached as monitor 2 (1920x1080@30)
[server] warning: monitor 2 is rotated (DXGI_MODE_ROTATION(2)); ...
[server] encoder size: 1920x1080
[server] scaling from capture size 1280x720 to encode size 1920x1080 via ffmpeg -vf scale=1920:1080
virtual display: detached, display count restored to 1
[host] stopped; virtual_monitor=true
```

Two things are visible there: the driver gave us a 720x1280 panel although the
receiver asked 1920x1080, and the pre-fix host still asked for 1920x1080, so
the image was transposed and stretched. The host now re-reads the real monitor
size after `attach()` and encodes that instead (`virtual monitor is actually
WxH ...; encoding WxH`). Subsequent attach attempts failed with
`MTTVirtualDisplayPipe stayed busy for 2 s` / `driver pipe: no reply` while the
device still reported `started: true, problem code: 0`, and the host fell back
to mirror mode with that message. `--list-monitors` shows two stale virtual
outputs (DISPLAY5 720x1280, DISPLAY6 1280x720) left from earlier milestones; no
admin/driver reset was attempted. So **the size-corrected virtual path is NOT
VERIFIED**, while the mirror fallback was verified twice.

GUI smoke test: `app-host.exe` and `app-receiver.exe` were started, kept
running for 4 s and reported window titles `lowlat-screen host` and
`lowlat-screen receiver`; then they were terminated.

### NOT RUN / UNVERIFIED

- **A real second device / real LAN**: no second machine was available. Only
  loopback (both apps on this PC) was exercised, as required for this check.
- **Clicking the GUI**: Pair & Connect, Allow/Deny, Fit/Fill, rotation,
  Disconnect and the firewall button were not clicked. The headless demo runs
  the same discovery/pairing/stream/stop code; rendering and button wiring are
  **NOT RUN**.
- **Video visible in the egui window**: frames were decoded and counted, and
  `yuv_to_rgba` rotation is unit-tested, but nobody watched the texture (no Fit
  vs Fill or 90/180/270 visual check). **NOT RUN**.
- **Firewall helper**: not executed (it edits Windows Firewall and needs
  elevation). The UI shows the exact `netsh` command and only runs it on click.
- **Keyboard input forwarding**: not implemented; only mouse move/buttons/wheel
  are wired to the existing `InputSender`, and they are off by default.
- **Linux build/run**: `app-receiver` contains no Windows-only code outside the
  `#[cfg(windows)]` screen-size lookup, but it was **not** compiled or run on
  Linux here. Linux input injection remains the existing count-only stub.
- **Latency**: not measured. The GUIs show the server's frame/bitrate counters
  and say "latency: not measured".
- **Video encryption**: out of scope for this milestone; the 32-byte key is
  stored for the next task.


