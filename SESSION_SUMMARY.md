## Session Summary: Android Receiver Milestone (M11)

### Completed This Session

1. **Wire vectors** (`common/examples/wire_vectors.rs`):
   - Created deterministic hex vector printer for protocol verification
   - `cargo run -p common --example wire_vectors` produces exact byte sequences
   - Used as ground truth for Kotlin unit tests

2. **Protocol module** (`android/protocol/` - pure-Kotlin/JVM library):
   - **9 source files**: Wire.kt, FrameHeader.kt, Beacon.kt, Pairing.kt, PairedHostStore.kt, InputMessages.kt, TouchMath.kt, SdlKeys.kt, Avc.kt
   - **9 test files**: RustVectorsTest.kt, FrameAssemblerTest.kt, PairingTest.kt, BeaconTest.kt, PairedHostStoreTest.kt, InputMessagesTest.kt, TouchMathTest.kt, SdlKeysTest.kt, AvcTest.kt
   - All tests assert byte-for-byte compatibility with Rust wire vectors
   - FrameAssembler mirrors Rust `client/src/net.rs` reassembly logic
   - PairingMachine uses SecureRandom (wire format unchanged)
   - InputMessages encodes/decodes 24-byte header + 17-byte payload exactly as `common/src/lib.rs`
   - Avc extracts SPS/PPS from Annex-B keyframes for MediaCodec csd-0/csd-1

3. **Android app module** (`android/app/` - Compose, NSD, MediaCodec):
   - **Advertiser.kt**: NSD service advertising `_lowlat._udp.` with beacon TXT record on port 9200
   - **MainActivity.kt**: Entry point with Compose UI (pairing dialog → connected video screen)
   - **ReceiverController.kt**: Core logic — UDP video reception on 9100, FrameAssembler, MediaCodec decode to Surface, input sending to host port 9000
   - **AndroidManifest.xml**: Required permissions (INTERNET, ACCESS_NETWORK_STATE, ACCESS_WIFI_STATE, CHANGE_WIFI_MULTICAST_STATE)
   - **build.gradle.kts**: AGP 8.11.x, Kotlin 2.1.21, Compose BOM 2025.06.01, compileSdk/targetSdk 35, minSdk 26

4. **Documentation**:
   - `android/README.md`: Build/install commands, protocol mapping, permissions, NOT RUN items
   - Root `README.md`: Added M11 Android milestone section with requirements, build commands, and NOT RUN

### Protocol Constants (unchanged from M10)
- Magic: `0x4C4C5332`, version 1, 24-byte big-endian header
- `MAX_PAYLOAD=1200`, flags bit0 keyframe / bit1 keyframe request
- Beacon `LOWLAT1|id|name|platform|w|h|orient|video_port|pair_port` on 9200
- Pairing on 9201, video 9100, host control 9000
- mDNS TXT keys `id,name,platform,w,h,orient,pair` (port = video port)
- Control/Input sent to source IP + 9000

### Unit Tests (run with `./gradlew :protocol:test`)
- `RustVectorsTest`: 5 tests verifying frame headers, input messages, beacons, pairing, and header decoder rejection match Rust output
- `FrameAssemblerTest`: 7 tests for duplicate packets, stale frames, packet bounds, capacity eviction, reset
- `PairingTest`: 7 tests for hex roundtrip, request/challenge, accept/reject, decode rejection, machine request/allow/resume/deny
- `BeaconTest`: 5 tests for beacon roundtrip, magic rejection, bad numbers, field validation, separator sanitization
- `PairedHostStoreTest`: 5 tests for text roundtrip, upsert by ID, comments/bad lines ignored, missing host null, token hex digits
- `InputMessagesTest`: 5 tests for layout offsets, button/wheel/key layouts, coordinate clamping, display config rejection
- `TouchMathTest`: 3 tests for fit letterboxes, fill cropping, zero-size safety
- `SdlKeysTest`: 4 tests for letters/digits, control keys, shifted punctuation, unsupported characters
- `AvcTest`: 4 tests for NAL unit splitting, SPS/PPS extraction, P-frame safety, empty input

### NOT RUN (no real device available)
- NSD on-device discovery and pairing
- MediaCodec on-device H.264 hardware decode
- Touch/rotation/network-change behavior on device
- On-screen keyboard input on device

### Build Commands
```bash
# Ensure SDK components installed
sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0"
sdkmanager --licences

# Run protocol unit tests (reuse Rust vectors)
./gradlew :protocol:test

# Build the Android app
./gradlew :app:assembleDebug
```