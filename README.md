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