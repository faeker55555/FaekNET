# Android port and routing plan

## Feasibility

Yes, FaekNET can be ported to Android as an APK, but Android needs a platform
adapter. An Android app cannot create `/dev/net/tun` itself or install a normal
network interface. It must use [`android.net.VpnService`](https://developer.android.com/reference/android/net/VpnService)
with user consent. The service receives a TUN file descriptor; the Rust mesh reads
and writes that descriptor instead of opening the Linux `/dev/net/tun` device.

The existing core is a useful starting point because transport, encryption, peer
state and the virtual-IP model are UI-independent. It currently assumes a desktop
TUN lifecycle and has no Android target, JNI/UniFFI boundary, service, notification,
or Android permission/build project. This is therefore a port, not an APK packaging
change.

## Proposed APK architecture

```text
Android UI (Kotlin/Compose or existing egui shell)
        |
Foreground VpnService + notification + lifecycle
        |
JNI/UniFFI bridge, cancellation and event callbacks
        |
Rust core: mesh UDP socket, crypto, discovery, route policy
        |
VpnService ParcelFileDescriptor <-> TUN packet pump
```

The service must keep a visible foreground notification while the VPN is active,
request `BIND_VPN_SERVICE` through the service declaration, call `Builder.establish()`
on the user's explicit action, and close the descriptor on stop/revoke. The Rust
runtime must not outlive the service's descriptor. Android battery/power-management
and process-death behavior need explicit handling.

## Split routing modes

### Mesh-only (safe default)

Install only the FaekNET virtual route:

```text
10.66.0.0/24 -> FaekNET TUN
0.0.0.0/0   -> Android's normal network
```

Known virtual peer addresses are delivered to FaekNET. Everything else bypasses the
VPN and continues through Android normally. This is the correct first implementation
and preserves ordinary apps, DNS and non-mesh traffic.

Android route inclusion/exclusion behavior must be verified on the minimum supported
API. Do not claim that a route is excluded merely because it is not in the app's
peer list: `VpnService.Builder` routing is CIDR-based and the resulting route table
must be tested on real devices.

### Proxy mode (optional, explicit)

To send non-mesh traffic through a proxy, the VPN must capture that traffic and the
app must implement a user-space forwarder. A VPN route alone does not magically
apply an HTTP or SOCKS proxy.

Recommended supported modes:

- **HTTP CONNECT:** TCP only; HTTPS works through CONNECT, but UDP/QUIC and arbitrary
  non-TCP protocols do not.
- **SOCKS5:** TCP CONNECT and optionally UDP ASSOCIATE if the selected proxy supports
  it. UDP association, DNS behavior and timeout semantics must be tested separately.
- **Direct:** leave non-mesh routes outside the VPN and let Android use its normal
  network.

A full default route (`0.0.0.0/0`, and IPv6 if supported) is required to capture
non-mesh traffic for proxying. The forwarding layer then needs a packet-to-flow
implementation (TCP SYN/ACK/state handling, MTU/fragmentation, DNS policy, UDP
association, IPv6, close/error propagation and backpressure). A simple HTTP proxy
cannot carry arbitrary LAN games or UDP. Until this exists, proxy mode must be
labeled TCP-only and must not silently claim to support all Android traffic.

Never forward the FaekNET CIDR to the external proxy. Mesh packets must be consumed
by the mesh path first. Avoid VPN loops by protecting the proxy socket with
`VpnService.protect()` so it uses the physical Android network rather than the VPN.

## Manual route mode

Add a route controller separate from the automatic RTT policy:

```text
Automatic: choose a validated direct/relay route using measured health.
Manual:    choose the user's selected path and hold it until disabled/invalid.
```

Manual selection should be a route of stable peer identities, for example:

```text
faeker -> gaming-pc -> alice
```

It must not store only IP addresses or public endpoints. The controller resolves the
identities to current authenticated endpoints, so NAT changes do not silently turn a
manual route into a different peer.

Safety rules:

1. User can select only a direct path or one explicitly consenting relay hop in the
   first version. Do not expose arbitrary multi-hop paths.
2. Each edge must have a recent authenticated probe. A manual choice is a preference,
   not permission to send to an unverified address.
3. If the selected edge fails, stop or use a clearly configured fallback; do not
   silently switch to an unexpected route while the UI says Manual.
4. Reject loops, removed peers, disabled relays, stale probes and routes whose hop
   budget is exceeded.
5. Apply new route selection make-before-break where possible, preserve the virtual
   source/destination IPs, and show `Manual · degraded` when packets are failing.
6. Persist manual policy only by stable identities. Clear it when the user changes
   network membership or rotates the shared key.

The Android UI should show the active route, each hop's RTT/loss, why a manual route
is unavailable, an explicit `Return to automatic` action, and a confirmation before
using a peer as a relay.

## NAT and repository updates on Android

The existing repository workflow can be reused for Android endpoint changes, but it
is eventual discovery rather than a live signaling channel:

```text
observe public mapping -> debounce -> publish public IP:port
-> repository revision -> fetch -> authenticated probe -> promote
```

The Android service can write a small public endpoint handoff for the separate
publisher, or the Android app can use a carefully scoped authenticated publication
component if that policy is later approved. It must never upload the mesh key. Keep a
working endpoint until the candidate answers authenticated probes. If all paths are
lost, GitHub cannot itself punch through a NAT or replace a relay.

## Delivery phases

1. Create an Android workspace/module, Rust Android targets and a minimal
   `VpnService`; pass a TUN descriptor to Rust and echo test packets locally.
2. Run the existing mesh over the descriptor with mesh-only `10.66.0.0/24` routing.
   Add lifecycle, foreground notification, MTU and IPv4/IPv6 tests.
3. Add Android peer list, repository discovery, endpoint candidate probing and the
   manual/automatic route controller. Test direct traffic across real mobile/Wi-Fi
   networks and NAT changes.
4. Add TCP HTTP CONNECT and/or SOCKS5 proxy mode behind an explicit setting. Add
   route-loop tests, `protect()` tests, DNS/IPv6/UDP behavior documentation and
   battery measurements.
5. Sign an APK, test VPN consent/revocation, process death, sleep/wake, rotation,
   captive portals and Android versions. Only then advertise proxy coverage.

## Important limits

- Android VPN consent is per app/device and the VPN notification is mandatory.
- A VPN does not make NAT traversal possible; it only supplies the local packet
  interception point.
- A public Git repository is not a low-latency rendezvous server.
- HTTP proxying is not generic UDP forwarding. Games and LAN discovery need the
  FaekNET mesh path or a compatible UDP-capable proxy/relay.
- The current Rust workspace has no Android build or device verification yet.
