# FaekNET Android alpha

This is the first Android implementation scaffold. It creates a Gradle Android app
with a `VpnService` boundary and the intended mesh-only route:

```text
10.66.0.0/24 -> FaekNET VPN
other traffic -> Android normal routing
```

The Rust mesh core is not connected to the TUN file descriptor yet. The service is
therefore an **alpha shell**, not a usable VPN: it establishes the Android consent
and lifecycle path, but does not forward mesh packets. Do not use this build as a
production VPN.

## Build

Requirements:

- Android Studio or Gradle 8.7+
- Android SDK 35
- JDK 17
- Android device/API 26+

From this directory:

```sh
gradle :app:assembleDebug
adb install app/build/outputs/apk/debug/app-debug.apk
```

There is intentionally no checked-in Gradle wrapper yet; generate it from a trusted
Android/Gradle installation before publishing a release APK.

## Next bridge steps

1. Add Rust Android targets (`aarch64-linux-android`, `armv7-linux-androideabi`,
   `x86_64-linux-android`) and build `libfaeknet.so`.
2. Pass `ParcelFileDescriptor.detachFd()` through JNI/UniFFI to a Rust TUN adapter.
3. Reuse the core encrypted UDP transport and route `10.66.0.0/24` packets through
   that descriptor.
4. Add stable member ID, cached manifest, endpoint probing and repository gossip.
5. Add manual route policy and authenticated one-hop relay forwarding.
6. Add HTTP CONNECT/SOCKS5 proxy forwarding only after packet-loop, DNS, IPv6,
   UDP-associate and `protect()` tests exist.
7. Package signed APK/AAB artifacts only after real-device VPN lifecycle tests.
