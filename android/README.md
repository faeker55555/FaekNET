# FaekNET Android alpha

This is the first Android implementation scaffold. It creates a Gradle Android app
with a `VpnService` boundary and the intended mesh-only route:

```text
10.66.0.0/24 -> FaekNET VPN
other traffic -> Android normal routing
```

The first direct-peer transport is implemented in Kotlin: it routes IPv4 packets for
`10.66.0.0/24` through the TUN, encrypts them with the desktop-compatible
ChaCha20-Poly1305 envelope, and sends them to configured peer `IP:port` endpoints.
It also fetches the public `network/peers.toml` manifest over HTTPS every five minutes
when GitHub discovery is enabled, merging discovered endpoints by virtual IP and
skipping the local address. Manual peers remain available as bootstrap entries.

It is still an alpha: there is no endpoint publisher/roaming, relay forwarding, IPv6,
broadcast flooding, or proxy mode yet. Use a private test network and do not expose
the UDP port without a firewall.

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
