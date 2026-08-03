# lan_mesh

A tiny, self-hosted, pure peer-to-peer virtual LAN — similar in spirit to
Radmin VPN / Hamachi, but with **no third-party relay/VPN service**
involved. Everyone runs this binary, exchanges connection info once
(manually, e.g. over Discord/chat), and gets a virtual network adapter that
behaves like a real LAN NIC for game discovery/joining.

## How it works, in one paragraph

Each peer creates a **virtual network adapter** (TUN, Layer 3) using the
same technology WireGuard uses (the [`tun-rs`](https://github.com/tun-rs/tun-rs)
crate — `wintun.dll` on Windows, `/dev/net/tun` on Linux). Every IP packet
your game/OS sends to that adapter is picked up, encrypted
(ChaCha20-Poly1305, RFC 8439) with a pre-shared key only your group knows,
and sent directly over UDP to the right peer (or **flooded to every peer**
if it's a broadcast/multicast packet — the mechanism most LAN games use
for server discovery). Incoming packets are decrypted, authenticated, and
injected back into the local virtual adapter as if they'd arrived over a
real Ethernet cable. No data ever passes through a third party.

## Why TUN (not TAP) — and what that means for you

LAN games often rely on Ethernet broadcast/ARP. A full Layer-2 (TAP)
adapter would replicate a real LAN switch perfectly, but there is no clean,
installer-free way to get TAP on Windows (it needs a separate signed
kernel driver package like OpenVPN's `tap-windows6`). TUN, on the other
hand, uses **Wintun** on Windows — the same lightweight driver WireGuard
uses, distributed as a single `wintun.dll` next to your executable, no
separate installer or reboot.

This mesh therefore works at Layer 3 (IP) and **simulates LAN broadcast in
software**: any packet destined for your subnet's broadcast address
(e.g. `10.66.0.255`), `255.255.255.255`, or any multicast address
(`224.0.0.0/4`, which covers things like SSDP/mDNS-style discovery) is
flooded to every configured peer. This covers the overwhelming majority of
LAN games, which discover each other via UDP broadcast. It will **not**
help a game that requires raw non-IP Ethernet frames or IPX — those are
rare today, but if you hit one, let me know and a true TAP-based (Linux
+ a heavier Windows driver) variant would be the next step.

## Setup

Every peer needs:
1. A **unique virtual IP** in the same subnet (e.g. `10.66.0.1`, `10.66.0.2`, `10.66.0.3`, ...).
2. The **same pre-shared key** (treat it like a Wi-Fi password — anyone
   with it can join your mesh; share it over a trusted channel, never
   publicly).
3. Each other's **public IP:port** (see the CGNAT note below).

### Build

```
cargo build --release
```
The binary is at `target/release/lan_mesh`.

**Linux**: needs `CAP_NET_ADMIN` to create the virtual adapter — run with
`sudo`.
**Windows**: run as Administrator, and place `wintun.dll` (matching your
CPU architecture) next to `lan_mesh.exe`. Get it from https://www.wintun.net/.

### First-time setup (do this on every machine)

```
./lan_mesh init
```
This interactively asks for your virtual IP, subnet prefix, listen port,
and the pre-shared key. **The first person in the group** should leave the
PSK prompt empty to generate a new one, then share the printed key with
everyone else, who paste it in when they run `init`.

### Find your public address to share with peers

```
./lan_mesh myaddr
```
This uses STUN (the same technique we used earlier to diagnose your CGNAT
issue) to discover your external `ip:port` for the port this mesh will
listen on. If different STUN servers report *different* ports, your NAT
is doing endpoint-dependent (symmetric-like) mapping and direct P2P may
not work reliably for you — see "Known limitations" below.

### Add each peer

```
./lan_mesh add-peer
```
Asks for the peer's name, their virtual IP (what they chose in their own
`init`), and their public `ip:port` (from their `myaddr`). Repeat for each
peer. Run `./lan_mesh list-peers` any time to review.

### Run the mesh

```
sudo ./lan_mesh run
```
Brings up the virtual adapter, starts listening, and begins sending
keepalive/hole-punch packets to every configured peer every 15 seconds
(this is what keeps NAT/CGNAT mappings alive, exactly like the fix we
applied to the chat program earlier). Leave it running in a terminal while
you play. Once at least one packet has been exchanged, peers show as
"reachable" in the periodic status log.

At this point, your virtual IP (e.g. `10.66.0.2`) is just another address
on your machine — LAN game software that scans/broadcasts on your local
subnets should find peers running the mesh automatically. If a game asks
you to type in a host IP directly, use the peer's virtual IP.

## Notable design points

- **Roaming / self-healing addresses**: every encrypted packet embeds the
  sender's virtual IP. The first time a valid, decryptable packet arrives
  from a peer, that peer's *actual* observed `ip:port` is remembered and
  preferred over whatever was typed into the config — so if your `myaddr`
  guess is slightly stale, or NAT remaps the port between sessions, the
  mesh self-corrects the moment any packet gets through, the same way
  WireGuard's "roaming" works.
- **Encryption**: ChaCha20-Poly1305 AEAD with a 12-byte random nonce per
  packet. Wrong-key or tampered packets are silently dropped (verified in
  automated tests) rather than crashing anything.
- **Keepalives**: sent every 15s per peer to prevent idle NAT/CGNAT UDP
  mappings from expiring — directly addressing the same class of bug we
  found and fixed in the earlier chat program (the receive loop here
  likewise treats socket read-timeouts as "nothing yet", not a fatal
  error).

## Known limitations

- **Symmetric/endpoint-dependent NAT on both sides is unfixable by this
  tool alone.** If `myaddr` shows different ports per STUN server for you
  *and* the same is true for a peer you're trying to reach, classic UDP
  hole punching cannot establish a direct path (this is a fundamental NAT
  traversal limitation, not a bug — see the earlier discussion about your
  peer1's CGNAT). The practical fix in that case is a small relay server
  with a real public IP; ask if you want one built.
- **No relay/rendezvous fallback yet.** This is intentionally "pure P2P,
  no VPN/relay services" per your request — if direct P2P fails for a
  given pair, the mesh will not silently route around it.
- **IPv4 only.** IPv6 game traffic isn't handled.
- **No automatic peer discovery.** Adding a peer is a manual, one-time
  step per relationship (like adding a Wi-Fi password), by design.
- Tested so far on Linux; the Windows path relies on `tun-rs`'s
  documented Wintun support but hasn't been verified on an actual Windows
  machine in this session — please report back if you hit anything odd
  there.
