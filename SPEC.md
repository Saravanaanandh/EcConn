# SPEC.md: Low-Latency Screen Extend (working title: `lowlat-screen`)

## 1. Goal
Use a slow laptop as an extra display for a main laptop over **wired LAN**.
- Server (main laptop): captures a screen, encodes, streams.
- Client (slow laptop): receives, decodes, shows fullscreen.
- Target: **glass-to-glass latency under 40 ms** on wired LAN, smooth 60 fps.
- Long term: platform independent (Windows, Linux, macOS). Prototype first targets **Windows server to Linux Mint client**.

## 2. Tech stack
- Language: **Rust** (stable). Fallback to C++ only if a needed API has no usable Rust binding.
- Capture (Windows): DXGI Desktop Duplication via the `windows` crate.
- Encode/Decode: FFmpeg (`ffmpeg-next` crate, or FFmpeg child process for milestone 1 if bindings are painful on Windows).
- Render: SDL2 (`sdl2` crate).
- Transport: **UDP** (std `UdpSocket` or `tokio`). No TCP for video.
- Layout: Cargo workspace with crates `server`, `client`, `common`.

## 3. Hard rules for the agent
1. Work on **one milestone at a time**. Stop after each milestone and wait for me.
2. Every milestone must end with something I can **run and verify**, plus the exact commands to build and run it.
3. Do not invent API names. If unsure about an FFmpeg flag or Windows API, say so and show how to check the docs.
4. Keep code simple and readable. No premature abstraction.
5. Log timings (capture, encode, send, receive, decode, render) in milliseconds from the first milestone onward.
6. Commit-friendly: small logical changes, a short README section per milestone.

## 4. Latency rules (apply from milestone 3 onward)
- H.264, low-latency preset, **no B-frames**, CBR or tight VBV, short GOP (about 1 second), periodic intra refresh or on-demand keyframes.
- Hardware encode when available (NVENC, QuickSync, AMF). Hardware decode on client (VAAPI on Linux).
- No frame queues deeper than 1 on the client. Always render the **latest** frame and drop older ones.
- Do not wait for vsync longer than necessary; avoid extra buffering anywhere.

## 5. Milestones
Each is a separate task. Do not start the next without my go-ahead.

### M1: Capture to H.264 file (Windows)
- Capture the primary monitor with DXGI Desktop Duplication at up to 60 fps.
- Encode to H.264 and write `out.mp4` (or raw `.h264`) for 10 seconds, then exit.
- Print average capture and encode time per frame.
- Done when: `out.mp4` plays in VLC and shows the screen content.

### M2: UDP send/receive, software decode, SDL window
- Server sends encoded packets over UDP to a given IP:port. Client receives, reassembles frames, decodes, shows in an SDL window.
- Define a simple packet header in `common`: frame id, packet index, packet count, timestamp.
- Handle lost packets by dropping the frame and requesting a keyframe.
- Done when: screen appears live on the second machine over LAN.

### M3: Hardware encode and decode, low-latency settings
- Use NVENC/QuickSync/AMF on server and VAAPI on client with fallback to software.
- Apply all rules from section 4.
- Done when: stream runs smoothly at 1080p60 with measured timings printed.

### M4: Latency measurement
- Overlay a millisecond timestamp on captured frames (or use a timestamp in the packet header plus synchronized clocks / same-machine loopback test).
- Report average, p95, and max end-to-end latency.
- Done when: I can read real numbers and compare with Sunshine/Moonlight.

### M5: Input forwarding
- Client sends mouse and keyboard events over a separate small UDP channel.
- Server injects them on Windows (SendInput).

### M6: Virtual display
- Use the existing Virtual Display Driver (do **not** write a driver). Capture only that monitor.
- Add a CLI flag to choose which monitor to capture.

## 6. Out of scope for now
Audio, encryption, authentication, WiFi optimization, macOS and Linux server support, GUI.

## 7. Environment
- Server: Windows 10/11, main laptop.
- Client: Linux Mint, slow laptop with limited CPU/RAM (keep client lightweight).
- Network: direct LAN cable between the two machines, static IPs (192.168.50.1 / 192.168.50.2).
