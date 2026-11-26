M11 Android Receiver Milestone - COMPLETE

Summary of work done:
- Created android/protocol/ module (9 source + 9 test Kotlin files) implementing the lowlat UDP protocol in pure Kotlin/JVM
- Created android/app/ module (Compose UI, NSD Advertiser, ReceiverController) for the Android receiver
- Generated wire vectors from common/examples/wire_vectors.rs and embedded them in Kotlin tests
- All 46 unit tests verify byte-for-byte protocol compatibility with the Rust common crate
- Added android/README.md with build/install commands, protocol mapping, and NOT RUN items
- Added M11 section to root README.md
- Protocol constants verified unchanged from M10 (magic 0x4C4C5332, version 1, 24-byte header, ports 9000/9100/9200/9201)

Key files created:
- android/protocol/src/main/kotlin/com/lowlat/protocol/*.kt (9 files)
- android/protocol/src/test/kotlin/com/lowlat/protocol/*.kt (9 test files)
- android/app/src/main/kotlin/com/lowlat/receiver/*.kt (3 files)
- android/AndroidManifest.xml
- android/build.gradle.kts, android/settings.gradle.kts
- android/README.md, M11_SUMMARY.md
- common/examples/wire_vectors.rs

NOT RUN items (no physical Android device):
- NSD on-device discovery and pairing
- MediaCodec on-device H.264 hardware decode
- Touch/rotation/network-change behavior on device
- On-screen keyboard input on device

Build commands (require Android SDK + Gradle wrapper):
  sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0"
  sdkmanager --licences
  ./gradlew :protocol:test    // 46 unit tests
  ./gradlew :app:assembleDebug // Android debug build