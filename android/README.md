# Android Receiver

This folder contains the Android receiver implementation for the low-latency screen sharing system.

## Overview

The Android receiver app:
- Discovers hosts via mDNS/NSD (`_lowlat._udp.`)
- Pairs with a host using a 6-digit code
- Receives H.264 video UDP datagrams on port 9100
- Reassembles frames and decodes with Android MediaCodec
- Renders video to a SurfaceView
- Handles touch input (tap, drag, scroll) and sends it to the host
- Supports orientation changes and keeps the screen on

## Project Structure

```
android/
├── protocol/         # Pure-Kotlin/JVM module with wire formats, assembler, pairing, store, input codec
│   ├── src/main/...
│   └── src/test/...  # Unit tests using Rust test vectors
├── app/              # Android Compose UI + NSD + MediaCodec + SurfaceView
│   ├── src/main/...
│   └── src/test/...
├── build.gradle.kts  # Root project build config
├── settings.gradle.kts
└── gradle/
```

## Protocol Module (`:protocol`)

A pure-Kotlin/JVM library (`build.gradle.kts` with `org.jetbrains.kotlin.jvm` plugin) that implements the lowlat UDP protocol:

- **Wire**: Constants and big-endian encoding/decoding (magic `0x4C4C5332`, version 1, 24-byte header)
- **FrameHeader**: 24-byte video datagram header (flags, frame_id, packet_index, packet_count, timestamp_us, payload_len)
- **FrameAssembler**: Reassembles H.264 access units from packets, keyframe gating, stale frame eviction
- **Beacon**: mDNS beacon parsing/encoding (`LOWLAT1|id|name|platform|w|h|orient|video_port|pair_port`)
- **Pairing**: PAIR1 protocol (REQ/RESUME/CHALLENGE/ACCEPT/REJECT) with state machine using SecureRandom
- **PairedHostStore**: Text-based storage (`# lowlat paired hosts v1`, `host|name|%016x|hex`)
- **InputMessages**: 24-byte-header + 17-byte-payload input datagrams (MouseMove, MouseBtn, MouseWheel, Key, SetOrientation, ReleaseAll, DisplayConfig)
- **Avc**: H.264 Annex-B NAL unit splitting, SPS/PPS extraction from keyframes
- **SdlKeys**: Character-to-SDL scancode mapping for on-screen keyboard
- **TouchMath**: Normalized coordinate mapping with letterbox/fit modes

Unit tests (`RustVectorsTest`, `FrameAssemblerTest`, `PairingTest`, `BeaconTest`, `PairedHostStoreTest`, `InputMessagesTest`, `TouchMathTest`, `SdlKeysTest`, `AvcTest`) assert byte-for-byte compatibility with `cargo run -p common --example wire_vectors`.

To run protocol tests:
```bash
./gradlew :protocol:test
```

## App Module (`:app`)

An Android App module (minSdk 26, compileSdk 35, targetSdk 35) with Jetpack Compose UI:

- **MainActivity**: Entry point, NSD advertising, pairing UI, video screen
- **Advertiser**: NSD service advertising `_lowlat._udp.` with beacon TXT record
- **ReceiverController**: UDP video reception, FrameAssembler, MediaCodec decode to Surface
- **ReceiverUI**: Compose UI with pairing dialog and connected surface
- **DisplayInfo**, **InputEvent**: Data models for video dimensions and input events

### Build & Run

Ensure the Android SDK is installed with:
- `platform-tools`
- `platforms;android-35`
- `build-tools;35.0.0`

Accept SDK licenses:
```bash
sdkmanager --licules
```

Build and run:
```bash
./gradlew :app:runDebug
```

### Permissions

The app requires these permissions in `AndroidManifest.xml`:
- `INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE`, `CHANGE_WIFI_MULTICAST_STATE`
- For MediaCodec and network operations

### Protocol Mapping

| Component | Port | Direction |
|-----------|------|-----------|
| Beacon/mDNS | 9200 | broadcast |
| Pairing | 9201 | host -> receiver (REQ/CHALLENGE), receiver -> host (ACCEPT/REJECT) |
| Video | 9100 | host -> receiver (H.264 datagrams) |
| Control/Input | 9000 | receiver -> host (input/keyframe request) |

### NOT RUN (real-device untestable)

- NSD mDiscovery and pairing on physical Android device
- MediaCodec H.264 hardware decoding
- Touch input (tap/drag/scroll) on device
- Rotation handling and surface recreation
- Network change reconnection
- On-screen keyboard input
- Multi-display configurations

These items require a physical Android device and are marked NOT RUN for this milestone.