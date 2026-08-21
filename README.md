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

### Get it

**Easiest: download a release.** Every pushed git tag (`v*`) automatically
builds and publishes ready-to-run packages for Linux and Windows via
GitHub Actions — see [Build & Release](#build--release-pipeline) below.
Grab the `.tar.gz` (Linux) or `.zip` (Windows) from the repo's Releases
page, extract it, and use the `run-gui.sh`/`run-gui.bat` (or
`run-cli.sh`/`run-cli.bat`) scripts inside — they handle
Administrator/root elevation automatically.

**Or build from source:**
```
cargo build --workspace --release
```
This produces two binaries:
- `target/release/lan_mesh` — the terminal/CLI tool (all the commands below)
- `target/release/lan_mesh_gui` — the native GUI

**Linux**: creating the virtual adapter needs `CAP_NET_ADMIN` — run with
`sudo`, or use `scripts/linux/run-gui.sh` / `run-cli.sh`, which detect this
and re-launch themselves elevated automatically.
**Windows**: needs Administrator, and `wintun.dll` (get it from
https://www.wintun.net/, matching your CPU architecture) next to the
`.exe`. Downloaded release packages already include this; `scripts/build/build-windows-cross.sh`
fetches it automatically if you're building from source. The
`scripts/windows/run-gui.bat` / `run-cli.bat` wrappers handle the UAC
elevation prompt for you.

### First-time setup (do this on every machine)

```
./lan_mesh init
```
This interactively asks for your display name, virtual IP, subnet prefix,
listen port, and the pre-shared key. **The first person in the group**
should leave the PSK prompt empty to generate a new one, then share the
printed key with everyone else, who paste it in when they run `init`.

### The fast way to connect two people: `export` / `import`

```
./lan_mesh export
```
This discovers your external `ip:port` via STUN and prints **one single
line** encoding your name, virtual IP, and public address. Send that exact
line to your friend (Discord, chat, whatever). They run:

```
./lan_mesh import <the line you sent them>
```

...and you're added as a peer on their end — no manual typing of IPs/ports.
Do the same in reverse (they `export`, you `import`) so you're peers with
each other. Repeat pairwise for every relationship in the group (see "Does
everyone need everyone" below).

This card only encodes your name/virtual-IP/public-address — it does
**not** include the pre-shared key, so it's safe to paste in a group chat;
the key still needs to be shared separately once per group, exactly as in
`init` above.

### Alternative: fully manual `add-peer`

If you'd rather type things in by hand (e.g. no internet access to reach a
STUN server, or you already know each other's IP:port from another
source):

```
./lan_mesh myaddr
```
Discovers your external `ip:port` via STUN and prints it for you to relay
manually. If different STUN servers report *different* ports, your NAT is
doing endpoint-dependent (symmetric-like) mapping and direct P2P may not
work reliably for you — see "Known limitations" below.

```
./lan_mesh add-peer
```
Asks for the peer's name, their virtual IP (what they chose in their own
`init`), and their public `ip:port` (from their `myaddr`). Repeat for each
peer. Run `./lan_mesh list-peers` any time to review.

### Check connectivity/latency to your peers

```
./lan_mesh ping
```
Sends a handful of encrypted probes to every configured peer over the real
mesh transport and reports round-trip time and packet loss per peer — like
a normal `ping`, but specifically measuring the mesh link itself. Doesn't
need root/Administrator, and works even without `lan_mesh run` active
(though a peer obviously can't answer if their mesh isn't running). Pass a
number to change the probe count, e.g. `./lan_mesh ping 20`.

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

### If TCP-based things (Minecraft, a hosted website, etc.) don't connect

UDP-heavy games (Mindustry, Factorio) often work immediately, while a
TCP-based server (Minecraft Java, a web server, etc.) times out even
though the mesh itself is fine. This is almost always a **host-side
firewall dropping new inbound connections on the virtual adapter**, not a
mesh bug — Linux firewalls (`ufw`/`iptables`/`nftables`) apply their
default-deny policy to *every* interface, including the mesh's virtual
one, unless told otherwise. Confirm with `sudo tcpdump -ni lanmesh0 -n
tcp` while a peer tries to connect: if you see their `SYN` arrive
repeatedly with no reply, that's the firewall silently eating it. Fix:

```
sudo ufw allow in on lanmesh0
```
(substitute your actual adapter name, check with `ip addr show`; if you
set `LAN_MESH_DEV_NAME` it'll be that instead of `lanmesh0`). This is a
one-time, permanent fix that covers every port and protocol on the mesh
interface, since anything reaching it already passed ChaCha20-Poly1305
authentication against your shared key.

## Bonus: a self-hosted Discord-styled text + voice channel over the mesh

Since the mesh carries arbitrary TCP (proven above), `extras/messenger.py`
(plus `extras/frontend.py` and `extras/emoji_data.json`, which it needs
alongside it) is a small group chat with a single text channel and a
single voice channel, laid out like Discord (channel sidebar, chat, member
list, bottom-left user bar), built on nothing but the Python 3 standard
library (no pip installs, ever). This project has no affiliation with
Discord; it doesn't use Discord's logo, brand colors, wordmark, or
proprietary Twemoji-style emoji artwork -- only a similar structural
layout and the standard Unicode emoji characters (rendered by your
OS/browser's own emoji font).

```
python3 extras/messenger.py --host 10.66.0.1 --port 8765
```
(use your own virtual IP; omit `--host` to bind `0.0.0.0`). On first run
it generates a self-signed TLS certificate and serves over **HTTPS**
(`--no-tls` forces plain HTTP if you don't need voice). Peers open
`https://10.66.0.1:8765/` — the browser will show a one-time "not
trusted" warning for the self-signed cert; click Advanced → Proceed, this
is expected. HTTPS is required here purely because **browsers only allow
microphone access on a secure origin** (HTTPS, or literally `localhost`);
your actual traffic confidentiality still comes from the mesh's own
ChaCha20-Poly1305 encryption underneath, not from this certificate.

Features:
- **Lightweight accounts**: register a username + password (or "continue
  as a guest, no account" for a one-off session). Since everyone who can
  reach the server already has your mesh's shared key, an account here is
  *not* an access-control boundary against strangers — it's a persistent
  identity (your name, avatar color, and settings follow you across
  sessions/devices) so people can't casually post as you. Passwords are
  salted + hashed (PBKDF2-HMAC-SHA256) before being stored, never in
  plaintext, in `extras/messenger_accounts.json`.
- **Settings** (gear icon, bottom-left): choose Voice Activity vs.
  Push-to-Talk, set your PTT key (click the key field, then press any key
  to bind it), and adjust input/output volume. Settings are saved per
  account and re-applied automatically next time you log in; guests keep
  settings only for the current session.
- **Push-to-talk**: when enabled, hold your bound key to transmit — your
  mic is silent otherwise. Works alongside or instead of the default
  voice-activity mode.
- **Text channel** with message history (persisted to
  `extras/messenger_history.jsonl`, survives restarts), a live typing
  indicator, and reactions using the full emoji picker below (click "+ 😀"
  under any message).
- **Full standard Unicode emoji set** (1,900+ emoji) in a Discord-style
  categorized picker with search and category tabs (😀🖐️🐻🍔✈️⚽💡❤️🏳️),
  available both in the message composer and for reactions.
- **GIF/image support**: paste or type a link ending in
  `.gif/.png/.jpg/.webp` and it auto-embeds inline; or drag-and-drop /
  paste-from-clipboard / use the ➕ button to upload an image or GIF file
  directly (saved under `extras/messenger_uploads/`). There's no built-in
  GIF search box — that would require calling a third-party API
  (Tenor/Giphy), which breaks the "no external services" design of this
  whole project — so find a GIF elsewhere and paste/upload it here.
- **Member list sidebar** showing who's online, who's in voice, and a live
  "speaking" highlight, plus an inline mini-list under the voice channel
  entry itself (just like Discord).
- **Voice channel**: click the voice channel entry or the mic icon in your
  user bar to join; a separate mute button toggles muting (disabled/hidden
  behavior when push-to-talk is active, since PTT controls transmission
  instead). Audio is relayed through the server as raw 16kHz mono PCM over
  a hand-rolled WebSocket (no external STUN/TURN/WebRTC-signaling service
  involved, consistent with the rest of lan_mesh) — quality is modest by
  design, to leave bandwidth for whatever game you're also playing over
  the same mesh link.

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
- **Keepalives double as latency probes**: every 15s per peer, a PING is
  sent (not a bare keepalive) — it refreshes NAT/CGNAT mappings exactly
  like a keepalive would, but also gets echoed back as a PONG so `run`'s
  periodic status log shows a live `~Nms` round-trip reading per peer, and
  `lan_mesh ping` can measure it on demand. The receive loop treats socket
  read-timeouts as "nothing yet", not a fatal error — directly addressing
  the same class of bug we found and fixed in the earlier chat program.
- **Address roaming is persisted to disk**: the first time a peer's
  observed address changes (including the very first packet from them),
  it's written back into `mesh.toml` immediately, not just kept in memory.
  So after a successful connection, restarting `lan_mesh run` starts
  "warm" with the last-known-good address instead of the possibly-stale
  one from `export`/`add-peer`.
- **`export`/`import` peer cards**: a single base64 line encoding name +
  virtual IP + public ip:port (not the PSK). Turns adding a peer into
  "run one command, paste the result, they run one command" instead of
  five separate manual prompts.
- **One-sided adds just work**: if your friend adds your address but you
  never add theirs (or vice versa), the mesh used to be a dead end in
  that direction — their PING packets would physically arrive at your
  machine but get silently dropped, since your peer table didn't
  recognize the sender. Now, an unsolicited-but-authenticated PING from a
  virtual IP you've never configured is treated as a valid introduction
  (the packet already passed ChaCha20-Poly1305 authentication against
  your shared key, the same trust bar gossip-discovered peers are held
  to) — you add them under a placeholder name, start replying, and their
  real name arrives via gossip shortly after. Still add both directions
  when you can; this exists so a mistake or a one-off "just give them my
  card" doesn't permanently break connectivity.

## Same-router peers (two of your own machines behind one home router)

Multiple lan_mesh peers behind the same public IP works fine in general —
each machine gets its own NAT port mapping and is addressed independently,
same as if they were on opposite sides of the planet. But two peers that
are *both* on your home LAN talking to each other over their shared
**public** address specifically needs your router to support **NAT
hairpin/loopback** (sending a packet to your own WAN IP and having the
router route it back inward to the right internal host) — a lot of
consumer routers simply don't do this, which shows up as one peer
permanently stuck at "not yet reachable" even though both sides are
online and self-STUN succeeded correctly.

lan_mesh works around this automatically: every peer also discovers its
own LAN-facing IP (e.g. `192.168.1.74`) at startup and gossips it
alongside its public address as a same-network fallback candidate.
Whenever both a public and a LAN candidate are known for a peer, the
keepalive/hole-punch ticker pings *both* every 15s — whichever one
actually gets a reply becomes the address used for real traffic (via the
same `observe()`-always-wins logic that already handles normal NAT
roaming), with zero manual configuration and no relay/third party
involved. If the two peers really are on the same LAN, this converges
onto the direct LAN path automatically, typically within one keepalive
interval; if they're on genuinely different networks, the LAN probe just
never gets a reply and is silently ignored, same as pinging any other
unreachable address.

This is on by default and needs nothing from you — but if you'd rather
skip the wait, you can still manually point a peer's `public_ip`/
`public_port` at the other machine's LAN IP directly in `mesh.toml`
(works today regardless of this feature, and is a fine permanent choice
for two machines you know will always be on the same LAN).

## Does everyone need everyone? (mesh topology)

No, not anymore. **Peers gossip their known peer table to each other**
(name, virtual IP, best-known address, freshness) every ~20 seconds, and
immediately whenever a peer's own address changes. If A knows C, and C
knows B, then within one gossip cycle A automatically learns about B,
starts hole-punching to it directly, and vice versa — no manual
`export`/`import` needed for that pair. You only need **one** initial
bootstrap connection into an existing mesh; everyone else is discovered
automatically after that (this is what makes it behave like a real
self-propagating subnet rather than a fixed peer list). This also means
if your NAT/CGNAT reassigns you a new external port on reconnect (a full
cone NAT that swaps ports every time, for example), the mesh self-heals:
your own background self-STUN check notices the change and re-gossips
your new address immediately, so peers pick it up without you doing
anything.

## The native GUI

`lan_mesh_gui` is a standalone, cross-platform desktop app (Linux +
Windows) built with [`egui`/`eframe`](https://github.com/emilk/egui) —
not a web view, not Electron, no browser involved. It has its own visual
identity (dark, monospace, sharp-edged "network operations console"
look), deliberately distinct from chat-app UI conventions.

Screens: a first-run setup wizard (identity + optional bootstrap-peer
import), a network overview with a live topology diagram (solid lines for
manually-added peers, dashed for gossip-auto-discovered ones), a
peers list with per-peer latency/status/discovery-method detail, a live
activity log streaming the mesh engine's own log lines, and settings
(pre-shared key, "my peer card" generator, identity info).

It uses the exact same `lan_mesh_core` engine as the CLI — same gossip,
same encryption, same config file (`mesh.toml`) — so you can mix and
match: set up with the CLI, monitor with the GUI, or vice versa.

A sixth screen, **Domains**, lists every peer's local domain name and
launches the in-app browser — see the next two sections.

## Local domain names

Every peer is reachable by name, not just by its raw virtual IP: with the
default suffix `mesh`, a peer named `alice` is reachable as `alice.mesh`
from any application on the machine — a browser, a game's "connect by
hostname" box, `ping`, `curl`, anything. This is purely local to your
mesh; it has nothing to do with the real internet's DNS.

Two independent mechanisms provide this, both configurable per-machine in
`mesh.toml`:

- **Hosts-file sync** (`sync_hosts_file`, on by default) — the mesh keeps
  a clearly-marked, auto-managed block inside the OS hosts file
  (`/etc/hosts` on Linux, `System32\drivers\etc\hosts` on Windows) up to
  date with every known peer's name → virtual IP mapping, rewriting only
  its own block on every change (new peer discovered, address roamed,
  etc.) and leaving the rest of the file untouched. This needs no extra
  privilege beyond what creating the virtual adapter already requires,
  and works with literally every application unconditionally. The block
  is removed automatically when the mesh stops.
- **Built-in DNS resolver** (`dns_server`, off by default) — a tiny DNS
  server bound to `127.0.0.1:<dns_port>` (default port 53) that answers
  `*.<suffix>` queries directly from the live peer table and forwards
  everything else upstream to a real resolver (so normal internet
  browsing isn't affected). This is the heavier-weight option: it needs
  the OS/network stack to actually be pointed at it (an optional
  best-effort `dns_auto_configure` setting attempts this — on Linux it
  edits `/etc/resolv.conf`; on Windows it runs `netsh interface ip set
  dns` against the real interface, skipping VPN/WARP-style adapters the
  same way the WARP workaround below does) but supports resolving mesh
  names from *other* devices on your LAN too, not just this machine.

Use `lan_mesh domains` (CLI) or the **Domains** screen (GUI) to see the
current name list and which mechanisms are active.

### Subdomains: named services and wildcards

Beyond a peer's own name, you can advertise **named services** — a game
server, a web dashboard, whatever you host — as their own subdomain of
your mesh name:

```
lan_mesh add-service game 25565
```

This is stored in mesh.toml as a top-level `[[services]]` table (same
shape as `[[peers]]`):

```toml
[[services]]
name = "game"
port = 25565
```

Once the mesh is running, `game` becomes reachable as
`game.alice.mesh` (`<service>.<your mesh name>.<suffix>`) — and this is
**gossiped to the whole mesh automatically**, the same way peer
addresses are, so every member eventually learns about it without you
needing to tell them individually. The GUI's Domains screen has an
inline form for this instead of the CLI command, plus a live list of
every service you and your peers have advertised, each remove-able with
one click; `lan_mesh list-services`/`lan_mesh remove-service <name>`
are the CLI equivalents.

If the built-in DNS resolver (`dns_server = true`) is enabled, it goes a
step further: **any** subdomain of a peer's own name resolves to that
peer automatically, even ones that were never explicitly registered as a
service — `whatever.alice.mesh`, `dev.alice.mesh`, anything. This is
useful for application-level virtual hosting (e.g. an nginx
`server_name` block, or a game server that inspects the `Host` header)
that wants to mint its own subdomains without touching mesh
configuration at all. A registered service name still takes precedence
if both would otherwise match. Hosts-file sync can't do this (a hosts
file only ever maps exact names), so wildcard subdomains are the DNS
resolver's one genuine capability edge over the always-on-by-default
hosts-file mechanism.

## In-app browser

A standalone browser window (`lan_mesh_browser`, its own executable) for
viewing anything a peer hosts on the mesh — a game server's web admin
panel, Plex/Jellyfin, a self-hosted dashboard, whatever. It's a real
embedded webview (WebView2 on Windows, WebKitGTK on Linux), not a
hand-rolled renderer, so actual modern sites work correctly — with its
own address bar, back/forward/reload/home controls, and a visual identity
that matches the main GUI's dark "ops console" look (while still being
visually distinct from any particular browser or the bonus messenger's
Discord-styled UI).

It runs as a **separate process** from the main GUI rather than a panel
embedded inside its window — both `eframe` (winit) and `wry`'s
windowing each want to own an OS event loop on the calling thread, and
running two full GUI toolkits' event loops in one process (especially
mixing GTK's main loop with winit on Linux) is fragile, platform-specific
territory not worth the risk for a convenience feature. "In-app" here
means: part of the same application suite, opened with one click from
the GUI's Domains screen, sharing the mesh's `mesh.toml` to build a
"mesh home page" of clickable shortcuts to every peer.

Launch it:
- From the GUI: **Domains** screen → **OPEN IN BROWSER** next to any
  peer, or **OPEN BROWSER (MESH HOME)** for the shortcut page.
- Standalone: `./lan_mesh_browser [address]` (or the bundled
  `run-browser.sh` / `run-browser.bat`, which — unlike the GUI/CLI
  scripts — do **not** need root/Administrator, since the browser never
  touches the virtual adapter).

### Linux: X11 vs Wayland

wry's webviews are WebKitGTK widgets under the hood. Embedding one via a
raw window handle only actually works under X11 -- under Wayland (the
default session on many modern distros: CachyOS, Fedora Workstation,
recent Ubuntu/GNOME, ...) that path fails immediately with
`Error: UnsupportedWindowHandle`. To support both, the browser builds
each webview as a native GTK widget (`WebViewBuilderExtUnix::build_gtk`)
inside a `gtk::Fixed` container obtained from the window
(`WindowExtUnix::gtk_window`) instead of going through a raw window
handle at all -- this works identically under X11 and Wayland. Verified
by running the browser against both a real X11 server (Xvfb) and a real
headless Wayland compositor (Weston) in this repo's own testing; only
the X11 path is screenshot-verified since headless Wayland compositors
don't expose a working screenshot tool, but process liveness (no crash,
webview loads, click-through to links works) was confirmed on both.
Windows/macOS are unaffected by any of this -- they only ever had one
windowing backend to begin with, and keep using the ordinary
`HasWindowHandle`-based embedding path.

## Build & Release Pipeline

This repo includes a GitHub Actions workflow
(`.github/workflows/release.yml`) that:
- Runs on every push/PR to `main`: builds the whole workspace and runs
  the test suite, so breakage is caught immediately.
- Runs on every pushed tag matching `v*` (e.g. `v0.2.0`): additionally
  builds release binaries for **both Linux and Windows** (Windows is
  cross-compiled from the Linux runner using `mingw-w64` — no Windows
  runner needed), automatically downloads and bundles the correct
  `wintun.dll` into the Windows package, assembles both into ready-to-run
  `.tar.gz`/`.zip` archives (binaries + the `run-*.sh`/`run-*.bat` helper
  scripts + `extras/`), and publishes them as a GitHub Release with
  checksums.

To cut a release: `git tag v0.2.0 && git push --tags`. That's it — the
workflow does the rest and the Release page gets populated automatically.

### Building locally (without CI)

```
./scripts/build/build-linux.sh          # native Linux build
./scripts/build/build-windows-cross.sh  # cross-compile for Windows from Linux
```
Both scripts produce the same package layout CI does, under `dist/`.
`build-windows-cross.sh` needs the Windows Rust target and mingw-w64:
```
rustup target add x86_64-pc-windows-gnu
sudo apt-get install -y mingw-w64
```
(On an actual Windows machine, just `cargo build --workspace --release`
directly works too — you'd only need to grab `wintun.dll` yourself from
wintun.net in that case, since the automated fetch step lives in the
Linux-hosted build scripts.)

### Helper scripts (included in every release package)

| Script | What it does |
|---|---|
| `run-gui.sh` / `run-gui.bat` | Launches the GUI, auto-elevating (sudo/UAC) since creating the virtual adapter needs it |
| `run-cli.sh` / `run-cli.bat` | Runs the CLI with any arguments, auto-elevating only for `run` |
| `run-browser.sh` / `run-browser.bat` | Launches the in-app browser standalone, with an optional address argument. Does **not** need root/Administrator. |
| `allow-firewall.sh` / `allow-firewall.bat` | One-time fix for the "TCP apps like Minecraft don't connect" issue described above — opens the local firewall for the mesh's virtual interface (Linux) or the lan_mesh executables (Windows) |

## Working around always-on VPNs (Cloudflare WARP, etc.)

If you run Cloudflare WARP (or a similar always-on VPN/proxy client)
alongside the mesh, it rewrites your OS's routing table so that, by
default, *all* outbound traffic — including the mesh's own UDP socket
doing self-STUN address discovery — gets routed through the VPN's virtual
adapter. Left unhandled, this means the mesh reports the VPN's address as
"yours" instead of your real internet-facing address, breaking hole
punching. Telling WARP itself to exclude the mesh's traffic isn't a
reliable general fix: WARP's Linux/macOS CLI (`warp-cli`) supports
scriptable per-route exclusions, but its **Windows GUI client does not
expose an equivalent option**, so a fix that only works via `warp-cli`
would silently fail for Windows users.

Instead, the mesh routes around this at the socket level on both
platforms, the same way it already avoids picking up an unexpected
interface from its own virtual TUN adapter:

- **Linux**: pins the mesh's UDP socket to the real default-route
  interface via `SO_BINDTODEVICE`, explicitly skipping any interface
  named `CloudflareWARP`.
- **Windows**: enumerates network adapters, skips any whose driver
  description matches a known VPN/tunnel pattern (Cloudflare WARP's is
  literally `"Cloudflare WARP Interface Tunnel"`, matched
  case-insensitively, alongside a few other common VPN clients), and
  pins the socket to the first remaining physical adapter with an IPv4
  address via the `IP_UNICAST_IF` socket option — the direct Winsock
  equivalent of Linux's `SO_BINDTODEVICE`.

This is implemented and cross-compiles cleanly for Windows, and the
underlying Linux mechanism (same technique, same "skip CloudflareWARP by
name" logic) has been running correctly in this project since earlier
in its life. The Windows-specific code path, however, has **not been
runtime-tested on an actual Windows machine with WARP installed** — there
wasn't one available while building this. If it needs a fix once you
try it for real (e.g. a different adapter description string on your
WARP version, or multiple physical NICs needing better ranking than
"first match wins"), tell me what you see and it's a quick follow-up.

### Fixed: self-STUN never resolving on Windows ("infinite resolving of public address")

A real bug was found and fixed in the adapter-selection logic above: it
originally excluded the mesh's *own* virtual TUN adapter from
consideration by checking whether the adapter's **name** started with
`"lanmesh"`. On Windows this is fragile — the OS doesn't reliably
preserve the requested adapter name (it can surface as `"Ethernet 3"`,
`"Local Area Connection 2"`, etc., depending on driver/Windows version),
and the mesh's own adapter's *driver description* (the other signal this
code checks) defaults to its dev name too, so it never contained
`"wintun"` either. When both checks missed, the mesh would end up
picking **its own virtual adapter** as the "real" internet-facing
interface, pin its UDP socket to it via `IP_UNICAST_IF`, and then every
self-STUN probe would silently fail forever (no route to the internet
from a virtual adapter) — which is exactly the "public address never
resolves, warning repeats every ~25s" symptom.

The fix: the mesh's own configured virtual IP is now passed into the
adapter-selection logic directly, and any candidate adapter carrying
that exact address is excluded outright — this can never miss (it's the
address we ourselves asked the OS to assign) or misfire against a real
NIC, unlike string-matching on a name/description Windows doesn't
guarantee. The old name/description checks are kept as secondary,
best-effort fallbacks. This selection logic has 7 dedicated unit tests
(`core/src/mesh.rs`, `mesh::tests::*picks*`/`*excludes*`) that run on
every platform's CI, including one that reproduces the exact failure
mode (an adapter named `"Ethernet 3"` with description `"lanmesh0"` —
i.e. neither legacy heuristic would have caught it) to make sure it
can't regress silently.

### Still stuck on Windows self-STUN? New diagnostic/workaround tools

The interface-selection fix above turned out **not** to fix every
Windows self-STUN failure — one user tried it and it made no difference
for them. Since their exact Windows network environment can't be
reproduced or inspected from here, rather than guess again blindly at
another adapter-selection heuristic, the mesh now gives you direct
tools to work around or diagnose the problem yourself:

- **Manually enter your own public IP/port**, bypassing self-STUN
  entirely. Useful if STUN itself is blocked on your network, or you
  already know your address (e.g. a box with a static IP and a
  manually port-forwarded router).
  - GUI: Settings → "PUBLIC ADDRESS DISCOVERY" → "MANUAL OVERRIDE".
  - CLI: `lan_mesh set-public-addr <ip> <port>` / `lan_mesh clear-public-addr`.
- **Disable WARP-compatibility (interface pinning)** — the interface
  enumeration/pinning logic described above, entirely. If you are
  *not* using a VPN and self-STUN still won't resolve, this rules out
  that logic as the culprit: with it off, the mesh's socket binds to
  `0.0.0.0` and lets the OS route normally, exactly like any other
  application on the machine. Takes effect on the next (re)start.
  - GUI: Settings → "PUBLIC ADDRESS DISCOVERY" → "WARP COMPATIBILITY" checkbox.
  - CLI: `lan_mesh warp-compat <on|off>`.
- **Cache the discovered/manual public address to `mesh.toml`**, so the
  mesh doesn't have to re-run self-STUN (and doesn't sit unusable if it
  fails) on every launch — once it succeeds once, or you set a manual
  address, that value is immediately usable from the very next start
  while self-STUN keeps retrying quietly in the background.
  - GUI: Settings → "PUBLIC ADDRESS DISCOVERY" → "CACHE PUBLIC ADDRESS" checkbox.
  - CLI: `lan_mesh cache-public-addr <on|off>`.
- **Reset public address** button/command: clears the cached value and
  forces a fresh self-STUN probe immediately (useful after a NAT
  reassignment, or just to start over).
  - GUI: Settings → "PUBLIC ADDRESS DISCOVERY" → "RESET PUBLIC ADDRESS" button.
  - CLI: `lan_mesh reset-public-addr`.
- **Expanded the built-in STUN server list** from 3 to 11 servers, so a
  single blocked/rate-limited/down provider is far less likely to be
  the whole problem: all 5 of Google's STUN servers, Cloudflare,
  **Yandex** (`stun.rtc.yandex.net:3478`), Nextcloud, stunprotocol.org,
  sipnet.ru, and Xiaomi's miwifi.com. Only one needs to answer for
  self-STUN to succeed.

All of the above are implemented, unit-tested (config, engine, and CLI
layers), and verified against a real live-running mesh in this sandbox
(manual override, cache-on-success, cache-seeding on startup, and
WARP-compat-off all confirmed via `mesh.toml` and log output during an
actual `lan_mesh run`). **Honest caveat: none of this is confirmed to
be a fix for the specific Windows self-STUN failure that was reported
this session** — the root cause on that machine is still unknown, since
it can't be reproduced here. These are meant as tools to get you
working again (manual override / cache) and to help narrow down the
cause (WARP-compat toggle) rather than a guaranteed resolution.

While verifying the caching feature live, a real (and separate) bug
was found and fixed: the self-STUN background thread's `if let Some(x)
= mutex.lock().unwrap().some_method() { .. } else { .. }` pattern kept
the `MutexGuard` alive for the *entire* `if`/`else`, including the
`else` branch — so the very first time self-STUN succeeded with
"cache public address" enabled, the code inside the `else` branch tried
to lock the same mutex again to persist the cached value, and
deadlocked that thread silently forever (no crash, no log — it just
stopped logging its periodic self-STUN status). Fixed by capturing the
lock's result into an owned value in its own statement first, dropping
the guard before entering the `if`/`else`.

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
- **Windows VPN-avoidance logic is compile-verified, not yet
  runtime-verified** on real Windows/WARP hardware — see "Working around
  always-on VPNs" above.
- Tested so far primarily on Linux (including real multi-node mesh runs);
  the Windows binaries cross-compile cleanly and have the same feature
  set, but haven't been run on an actual Windows machine in this
  session — please report back if you hit anything odd there.
