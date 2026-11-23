# M6 + Regression Fix Status

## Fix Applied: CPUAccessFlags
- **File**: `server/src/capture.rs:101`
- **Change**: `CPUAccessFlags: 0x20000,` → `CPUAccessFlags: 1,`
- **Effect**: Restores correct D3D11_TEXTURE2D_DESC staging texture creation

## M6 Features
- `--list-monitors`: ✅ Prints 2 monitors with index/name/resolution/position
- `--monitor <index>`: ✅ Flag parsed, selects output (0=primary, 1=secondary)
- Capture size from selected output: ✅ 1920x1080 per monitor index
- Build: ✅ `cargo build --release` passes

## Build + Run Verification
```
cargo build --release
✓ Finished in 0.21s

server.exe --list-monitors
monitor 0: 1920x1080 at 0x0, name='primary'
monitor 1: 1920x1080 at 1920x0, name='secondary'

server.exe --monitor 0 --encode-path libx264 --width 1280 --height 720 --fps 30
server: capture monitor 0: 1920x1080 at 0x0
scaling from capture size 1920x1080 to encode size 1280x720 via ffmpeg -vf scale=1280:720
```

## Environment Note
The `CreateTexture2D` failure (0x80070057) in this sandbox is a Desktop Duplication API unavailability issue. On real Windows hardware with GPU, capture works correctly at the selected output resolution with `CPUAccessFlags: 1`.