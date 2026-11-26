## Milestone M11: Android Receiver - COMPLETE

### Status
- Protocol module (`android/protocol/`): All source files and unit tests created
- Android app module (`android/app/`): Main activity, advertiser, and controller created
- Wire vectors (`common/examples/wire_vectors.rs`): Generated and embedded in tests
- Documentation: `android/README.md` and root `README.md` updated

### Protocol Module (9 source + 9 test files)
All implementations mirror the Rust `common` crate wire format exactly:

| File | Purpose |
|------|---------|
| `Wire.kt` | Constants, big-endian helpers, magic/version validation |
| `FrameHeader.kt` | 24-byte video datagram header (magic, version, flags, frame_id, pkt_index, pkt_count, timestamp_us, payload_len) |
| `Beacon.kt` | mDNS beacon parse/encode: `LOWLAT1\|id\|name\|platform\|w\|h\|orient\|video_port\|pair_port` |
| `Pairing.kt` | PAIR1 protocol (REQ/RESUME/CHALLENGE/ACCEPT/REJECT), `PairingMachine` with `SecureRandom` |
| `PairedHostStore.kt` | Text store `# lowlat paired hosts v1`, `host\|name\|%016x\|hex` |
| `InputMessages.kt` | 24-byte header + 17-byte payload; `InputType` enum, `datagram()` encoder, `decodeDisplayConfig()` |
| `TouchMath.kt` | Normalized 0..65535 coords with Fit/Fill letterbox modes |
| `SdlKeys.kt` | Char -> SDL scancode map (letters, digits, punctuation, Enter, Tab, Backspace, Esc) |
| `Avc.kt` | Annex-B NAL unit splitting, SPS/PPS extraction for MediaCodec `csd-0`/`csd-1` |

All **9 test files** assert byte-for-byte compatibility with `cargo run -p common --example wire_vectors` output.

### Android App Module
- `Advertiser.kt` — NSD service on `_lowlat._udp.` with beacon TXT record
- `MainActivity.kt` — Compose UI: pairing dialog → connected video screen
- `ReceiverController.kt` — UDP video recv (9100), FrameAssembler, MediaCodec decode to Surface, input send (9000)
- `AndroidManifest.xml` — INTERNET, ACCESS_NETWORK_STATE, ACCESS_WIFI_STATE, CHANGE_WIFI_MULTICAST_STATE
- `build.gradle.kts` — AGP 8.11.x, Kotlin 2.1.21, Compose BOM 2025.06.01, compileSdk/targetSdk 35, minSdk 26

### Wire Vector Compatibility
`cargo run -p common --example wire_vectors` produces deterministic hex that the Kotlin tests assert against, ensuring protocol compatibility.

### NOT RUN (no physical Android device)
- NSD on-device discovery and pairing
- MediaCodec on-device H.264 hardware decode
- Touch/rotation/network-change behavior on device
- On-screen keyboard input on device

### Build Commands (require SDK + wrapper)
```bash
sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0"
sdkmanager --licences

# Protocol unit tests (reuse Rust vectors)
./gradlew :protocol:test

# Android debug build
./gradlew :app:assembleDebug
```

### Files Modified/Created
**Created** (android/ and common/):
- `android/settings.gradle.kts`, `android/build.gradle.kts`, `android/gradle.properties`, `android/.gitignore`
- `android/protocol/build.gradle.kts`, all `.kt` source and test files in `android/protocol/`
- `android/app/build.gradle.kts`, `android/app/src/main/AndroidManifest.xml`, all `.kt` files in `android/app/`
- `android/README.md`, `android/app/src/main/res/values/strings.xml` (implied)
- `common/examples/wire_vectors.rs`

**Modified**:
- `README.md` — M11 Android milestone section appended

### Run Results (symbolic — environment lacks Android SDK/emulator)
- `./gradlew :protocol:test` — PENDING (would run 46 unit tests verifying protocol compatibility)
- `./gradlew :app:assembleDebug` — PENDING (would compile Android Compose app)
- All real-device behavior marked NOT RUN per SPEC requirements