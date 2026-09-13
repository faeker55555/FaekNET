# Shared FaekNET network

This repository can be a **public bootstrap directory**, with automatic endpoint
updates from enrolled members. It is not a relay: symmetric NAT and firewalls can
still prevent direct UDP connections. GitHub availability, workflow execution and
raw-file caching affect discovery speed; this is not real-time rendezvous.

## Trust and privacy

- `members.toml` assigns GitHub accounts to unique virtual IPs in `10.66.0.0/24`.
- `peers.toml` contains only public names, virtual IPs, public IPv4/UDP endpoints,
  owner logins and timestamps. It may contain public bootstrap entries; endpoint rows must be enrolled and validated before automated updates.
- The shared network encryption key stays **outside GitHub**, distributed privately.
  Neither publisher nor workflow reads it. The publisher never reads `mesh.toml`.
- Publishing creates a public GitHub issue and a public Git commit. Old endpoints
  remain in Git/issue history. Removing the current row does not erase that history.
- GitHub ownership authorizes directory edits; the existing PSK authorizes mesh traffic.
  Anyone holding that PSK can still impersonate a mesh peer under the current protocol.
  Removing directory enrollment is **not** cryptographic revocation: rotate the PSK
  and distribute it to remaining members when revoking network access.
- The client imports bootstrap hints, not firewall policy. Already learned/configured
  peers and gossip continue working when a row expires or disappears. Expiry stops
  *new imports* of that row; it does not kick connected peers out.

## Maintainer: one-time setup

1. Review/test and merge this implementation into the repository's default branch
   (`main`). The native client reads the fixed `main/network/peers.toml` URL. The
   workflow must exist on the default branch before issue events can run it.
2. Enable Issues and GitHub Actions. The `peer-registry.yml` workflow needs permission
   to write repository contents and close issues. If branch protection blocks its
   commits, configure an appropriate narrowly scoped automation policy; do not
   distribute a maintainer token to clients or broadly disable branch protection.
3. Enroll GitHub accounts in `network/members.toml`, giving each device a distinct
   host IP. For example (replace the placeholder login before using):

   ```toml
   schema_version = 1
   [members]
   your-github-login = ["10.66.0.2", "10.66.0.3"]
   ```

4. Give enrolled members the same network key **privately**. Never put it in issues,
   workflow secrets, the directory, screenshots, or command-line arguments.

This initial identity/IP assignment is manual. Subsequent endpoint changes and daily
heartbeats are automatic. Network-key possession alone does not grant GitHub write
access. Every enrolled user can update only their assigned IPs, without repository
write permission. Accounts need permission to open an issue in this public repository.

## Member: run the network and automatic publisher

1. Install the native app plus `curl` (used for read-only discovery), Python **3.11+**,
   and GitHub CLI (`gh`). Authenticate your own enrolled GitHub account with
   `gh auth login`; check it using `gh auth status`.
2. Configure the native app with your assigned `10.66.0.x/24` address and the privately
   shared key. Use a name of 1–32 letters/digits/underscores/hyphens, starting with a
   letter or digit. Stop the mesh and enable **Settings → Peer directory → Discover
   the shared network from GitHub**, then start it. Alternatively add this under
   the existing `[me]` table in `mesh.toml` while stopped:

   ```toml
   repository_discovery = true
   ```

   New installations enable this automatically. Existing configurations that explicitly
   set it to false remain local-only until you enable it.
3. The mesh fetches the directory on startup in a background thread, then every
   five minutes. A missing file/download failure does not stop existing networking.
   For existing peers, a changed directory endpoint is kept as a probe candidate;
   the confirmed send address changes only after authenticated traffic arrives.
   First-time bootstrap peers still start from their configured directory hint.
4. Fresh STUN observations or explicit manual public-address overrides are written
   to `.faeknet-endpoint.toml` in the app's working directory. Cached-only endpoints
   are not refreshed into this handoff. No key is included.
5. In a **separate non-elevated terminal**, run the publisher from that directory:

   ```sh
   python /path/to/FaekNET/scripts/registry/publish.py --dry-run
   python /path/to/FaekNET/scripts/registry/publish.py --watch
   ```

   Use `python3` or `py` if that is the Python command on your OS. If needed, pass
   `--endpoint /path/to/.faeknet-endpoint.toml` and a writable `--state /path/to/state.json`.
   The mesh may run elevated for TUN, but the publisher should run as the ordinary
   user who authenticated `gh`. Authentication inside an elevated account is separate.

The watch-mode publisher checks every 30 seconds and waits for the same endpoint
across two polls to coalesce NAT flapping. It submits changes no more often than
every two minutes and renews unchanged endpoints daily. A one-shot invocation is
an explicit publication attempt and skips this watch-mode debounce. It ignores handoffs
older than 120 seconds. It submits a new issue with an exact allowlisted TOML body;
GitHub Actions validates the author's enrollment, applies a SHA-guarded update to
`peers.toml`, then closes accepted issues. Rows expire after seven days without an
accepted update. Unchanged pending requests are not resubmitted for 24 hours.

The publisher is a separate helper, **not bundled into the desktop executable or
started automatically**. Keep it running (or configure a user-level startup task).
Closing it stops endpoint publication, not the mesh itself. No issues or endpoints
are created by merely viewing the HTML design preview.

## Failure modes

- **Not enrolled:** maintainer must assign your GitHub login the same virtual IP as
  your local config. Multiple devices under one login need distinct assignments.
- **No fresh endpoint:** check STUN/firewall settings or configure a real public IP
  and UDP port forwarding manually. Registry storage cannot fix NAT traversal.
- **Open request, no update:** inspect the issue's Actions run. Disabled workflows,
  branch protection, invalid fields or enrollment conflicts require attention. Once
  fixed, submit a fresh request; editing the old issue does not retrigger publication.
- **Rapid address changes:** debounce, the publication rate limit, Actions scheduling,
  and GitHub caching introduce
  delays. Existing gossip/roaming still handles live peers independently.
- **Private IP rejected:** only globally routable IPv4 addresses are accepted. Do not
  submit LAN, loopback, CGNAT, multicast or documentation addresses.

## Verification

```sh
python3 -m unittest discover -s scripts/registry -v
cargo test --workspace --locked
cargo build --workspace --locked
```

Python tests use mocked GitHub calls: they publish nothing. Rust tests cover directory
parsing/expiry and reject invalid endpoints. Real GitHub Actions publication and a
multi-machine native mesh still require a deployed workflow, enrollment and runtime
testing; local unit tests do not prove NAT connectivity.
