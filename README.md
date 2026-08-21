# lan_mesh

A small, self-hosted, pure peer-to-peer **virtual LAN** — in the spirit of Radmin VPN /
Hamachi, but with **no third-party relay, rendezvous server, or VPN service**. Every peer
runs the same binary, exchanges a one-line "peer card" once, and gets a virtual network
adapter (`10.66.0.x`) that behaves like a real LAN NIC: LAN games discover each other via
simulated broadcast, TCP services are reachable by virtual IP or mesh domain name, and all
traffic is encrypted end to end with ChaCha20-Poly1305.

Linux + Windows. Two front ends, one engine: a **native GUI** (egui/eframe) and a **CLI**
(`lan_mesh`), both built on the same `lan_mesh_core` library.

## Features

- **Pure P2P** — direct UDP hole-punching between peers. No accounts, no relay, no central server.
- **Real virtual adapter (TUN)** — Wintun on Windows, `/dev/net/tun` on Linux; the mesh looks
  like a normal network interface, so any LAN-capable app works unchanged.
- **Encrypted everywhere** — every packet is sealed with ChaCha20-Poly1305 AEAD under a pre-shared key.
- **Automatic discovery** — peers gossip their peer tables (~20 s), so one manual bootstrap
  connection is enough; everyone else is discovered transitively.
- **Self-healing addresses** — the first authenticated packet from a peer updates its address
  (WireGuard-style roaming); NAT port changes propagate via gossip + self-STUN.
- **Same-router fallback** — peers also exchange LAN-facing addresses and automatically fall
  back to the direct LAN path when two peers share a router without NAT hairpin support.
- **Mesh domain names** — every peer is reachable as `<name>.mesh`, and advertised services as
  `<service>.<name>.mesh` (hosts-file sync or a built-in DNS resolver).
- **Desktop integration** — system tray, start-on-login, and one-click self-update from
  GitHub Releases.
- **In-app browser** — a standalone WebKitGTK/WebView2 window for mesh-hosted web UIs.

## How it works

Each peer creates a TUN adapter and assigns itself a unique virtual IP on a shared subnet.
IP packets addressed to that subnet are wrapped in a small header, sealed with
ChaCha20-Poly1305, and sent directly over UDP to the right peer; broadcast/multicast packets
(the mechanism most LAN games use for discovery) are flooded to every peer. Incoming packets
are authenticated, decrypted, and injected into the local adapter as if they arrived over a
real cable. Keepalives are sent every 15 s (doubling as NAT-mapping refreshes and RTT probes),
peer tables are gossiped every ~20 s, and each peer re-checks its own public `ip:port` via
self-STUN so the mesh recovers automatically after NAT reassignments.

## Getting started

### 1. Install

Download the package for your OS from [GitHub Releases](https://github.com/faeker55555/FaekNET/releases)
(Linux `.tar.gz`, Windows `.zip` — the Windows package includes `wintun.dll`), extract it, and
use the bundled `run-gui.*` / `run-cli.*` scripts, which handle root/Administrator elevation.
Or build from source:

```sh
cargo build --workspace --release
```

Linux needs `CAP_NET_ADMIN` (run with `sudo`); Windows needs Administrator and `wintun.dll`
next to the executables.

### 2. First-time setup (every machine)

```sh
./lan_mesh init
```

Interactive: display name, a **unique virtual IP** on the shared subnet (e.g. `10.66.0.2`),
subnet prefix, listen port, and the pre-shared key. The first member leaves the key empty to
generate one; everyone else pastes it in.

### 3. Add peers

```sh
./lan_mesh export                # prints a one-line peer card
./lan_mesh import <their card>   # run on the other machine, and vice versa
```

The card encodes only name + virtual IP + public address — **not** the key — so it is safe to
paste in a group chat. You only need one bootstrap connection per mesh: after that, peers
gossip their tables and everyone else is discovered automatically.

### 4. Run

```sh
sudo ./lan_mesh run              # or launch the GUI
```

Games that scan/broadcast on local subnets will now find mesh peers automatically; to connect
directly, use the peer's virtual IP (or mesh name, see below).

## CLI

| Command | What it does |
|---|---|
| `lan_mesh init` | Interactive first-time setup (creates `mesh.toml`) |
| `lan_mesh run` | Start the mesh (creates the virtual adapter) |
| `lan_mesh export` / `lan_mesh import <card>` | Print / add a one-line peer card |
| `lan_mesh add-peer` / `lan_mesh list-peers` | Manually add / review peers |
| `lan_mesh ping [N]` | Measure RTT to every peer over the mesh transport |
| `lan_mesh myaddr` | Discover your own external `ip:port` via STUN |
| `lan_mesh genkey` | Generate a fresh pre-shared key |
| `lan_mesh domains` | Show mesh domain names |
| `lan_mesh add-service <name> <port>` | Advertise a service as `<service>.<yourname>.mesh` |
| `lan_mesh remove-service <name>` / `lan_mesh list-services` | Manage advertised services |
| `lan_mesh set-public-addr <ip> <port>` / `clear-public-addr` | Manual public-address override |
| `lan_mesh cache-public-addr <on\|off>` | Persist the discovered address to `mesh.toml` |
| `lan_mesh reset-public-addr` | Clear cached address, force a fresh self-STUN probe |
| `lan_mesh warp-compat <on\|off>` | Toggle VPN/WARP-compatible interface pinning |

## The GUI

`lan_mesh_gui` is a native desktop app (egui/eframe — not a web view): a first-run setup
wizard, network overview with live topology, peer list with latency/status, domains screen
with the in-app browser, activity log, and settings (identity, key, peer cards, public-address
discovery, and the sections below).

Desktop integration:

- **System tray** — closing or minimizing hides the window to the tray (toggleable in
  Settings); the tray's "Quit" item exits for real.
- **Start on login** — Settings checkbox: XDG autostart entry on Linux, `HKCU\...\Run` on
  Windows; starts minimized in the tray.
- **Self-update** — Settings → Updates checks GitHub Releases, shows the release notes, and
  installs the platform package after SHA-256 verification, restarting the app automatically.
  Never offers a downgrade, and a failed checksum aborts without touching anything.

## Mesh domain names

With the default suffix `mesh`, the peer named `alice` is reachable as `alice.mesh` from any
application — `ping`, `curl`, a browser, a game's "connect by hostname" box. Two mechanisms,
both configurable in `mesh.toml`:

- **Hosts-file sync** (on by default) — keeps a managed block of `<name> → virtual IP`
  entries in the OS hosts file, updated automatically as peers join/roam.
- **Built-in DNS resolver** (optional) — answers `*.mesh` queries on `127.0.0.1:53` from the
  live peer table and forwards everything else upstream. With it enabled, *any* subdomain of a
  peer's name resolves to that peer, and advertised services get their own subdomains
  (`game.alice.mesh`).

## Networking notes

- **TCP apps not connecting (Minecraft, web servers, ...)** — usually the host firewall
  dropping new inbound connections *on the virtual adapter*, not a mesh bug. Fix once:
  `sudo ufw allow in on <mesh-device>` (check the name with `ip addr show`).
- **Same-router peers** — two peers behind one router may never reach each other over their
  shared public address (many consumer routers lack NAT hairpin). lan_mesh handles this
  automatically by gossiping LAN-facing addresses and probing them in parallel; whichever path
  answers wins, no configuration needed.
- **Always-on VPNs (Cloudflare WARP, ...)** — the mesh pins its socket to a real, non-VPN
  interface (`warp_compat`, on by default). If self-STUN still never resolves on Windows with
  no VPN active, try `lan_mesh warp-compat off` or set your public address manually.
- **NAT limitations** — symmetric NAT on *both* sides cannot be hole-punched (fundamental, not
  a bug); a relay would be required and is out of scope by design. IPv4 only. LAN broadcast is
  simulated at Layer 3, so games needing raw Ethernet frames are not supported.

## Build & Release

GitHub Actions (`.github/workflows/release.yml`) tests and builds the whole workspace on every
push/PR, and on a version tag (`v*`) additionally cross-compiles Windows binaries from the
Linux runner, bundles `wintun.dll`, and publishes ready-to-run packages as a GitHub Release.

To cut a release: `git tag v0.31.0 && git push --tags`. Keep the tag in sync with the crate
versions in the `Cargo.toml` files — the GUI's self-updater compares them numerically.
