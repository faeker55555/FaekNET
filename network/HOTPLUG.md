# Hot-plug membership and local directory cache

This is the proposed smoothing flow for existing desktop versions. It keeps GitHub as
an eventually consistent bootstrap source, while ordinary peer-to-peer gossip carries
updates after one member has fetched them.

## Desired startup flow

```text
launch
  ├─ load local authenticated mesh snapshot immediately
  ├─ start using cached peer identities/endpoints where still valid
  ├─ if a bootstrap path exists, fetch the current repository manifest
  ├─ publish this member's fresh public endpoint/alive revision
  └─ ask one active mesh member for the newest manifest

active member
  ├─ periodically checks GitHub (not every peer)
  ├─ notices no repository revision change → tells peers "unchanged"
  └─ notices a new revision → fetches once and gossips the manifest delta

receiver
  ├─ validates manifest revision/signature/expiry
  ├─ merges by stable member ID, not by public IP
  ├─ probes a changed endpoint
  └─ promotes it only after authenticated traffic succeeds
```

`no new -> ok; new -> tell` is a good wire behavior. The response should carry a
repository revision and a compact delta or a snapshot request, not repeatedly send
the same full table.

## Important limitation: who can publish

A newly launched client cannot silently "push alive" to GitHub unless it has a
separate authenticated publisher path. The current design uses the user's own
authenticated GitHub CLI/helper and does not upload the private mesh key. A future
native helper may create a narrowly scoped issue, but it must not embed a shared
repository write token in the APK/binary.

A new user still needs one of:

- a private network invitation containing the key, assigned virtual IP and one
  bootstrap endpoint/card;
- an already connected member reachable through a configured or cached endpoint; or
- a fresh repository publication made using that user's own enrolled GitHub identity.

GitHub cannot discover a new NAT mapping by itself.

## Active fetcher / gossip fan-out

Do not use a permanent "first peer" or a GitHub lock. The active fetcher should be
an ephemeral mesh role:

1. Members advertise a `directory_revision` and last successful fetch time in
   authenticated gossip.
2. The member with the freshest manifest and lowest deterministic `(member_id)`
   becomes preferred fetcher for a short lease.
3. A lease is only an optimization, not an authorization. If the preferred member
   is silent past the lease, another eligible member fetches.
4. A fetcher sends `unchanged(revision)` when the raw repository revision is the
   same, or `delta(base_revision, new_revision, entries)` when it changes.
5. Recipients ACK the revision once validated and cache it. Duplicate deltas are
   idempotent and harmless.
6. Limit fetches with a minimum interval and jitter. On fetch failure, retain the
   last good snapshot and let another member take over after the lease expires.

This reduces routine GitHub reads from one per peer to approximately one per active
mesh, while avoiding a single permanent coordinator. GitHub issue publication by
members remains separate from manifest fetching; endpoint updates still need an
owner-authorized path.

## Stable identity and virtual IP

Add a persistent opaque `member_id`, generated once on first setup and stored in the
local config with restrictive permissions. It must survive public IP and UDP port
changes. A repository/cache entry should conceptually contain:

```toml
member_id = "m_..."
name = "alice"
virtual_ip = "10.66.0.2"
public_ip = "..."
public_port = 54321
revision = "..."
```

The merge key is `member_id`. The virtual IP is the stable address applications use;
`public_ip:public_port` is only the current transport candidate. A new endpoint for
the same `member_id` must update the candidate rather than create a second peer or
assign a new virtual IP.

An opaque random ID is not proof of identity under the current shared-PSK protocol:
any holder of the PSK could impersonate one. Repository enrollment must therefore
bind `member_id` to the owner's authorized account and assigned virtual IP. For a
stronger future identity, add a per-member signing key/public key and authenticate
manifest deltas and gossip independently of the shared network key. Never use a
public IP, display name, or random ID alone as authorization.

## On-device latest-known snapshot

Keep two local records:

- `mesh-cache.toml`: last validated manifest snapshot plus repository revision,
  fetched time and schema version.
- runtime peer state: authenticated endpoint observations and RTT/health, never
  written into the public repository.

Write the cache atomically (`.new` then rename where supported), cap its size and
reject unknown fields, duplicate IDs, duplicate virtual IPs, invalid public
addresses, expired entries and revisions older than the current cache. The cache is
bootstrap data, not proof that a peer is currently alive.

At launch, use cached entries only as unconfirmed candidates. Keep a working
authenticated endpoint until a replacement answers a fresh encrypted probe. If the
cache is stale or corrupt, discard only the bad snapshot and start with configured
bootstrap cards; do not erase the private key or identity.

## Wire additions

The existing encrypted gossip channel can carry a versioned control payload separate
from application data:

```text
DIRECTORY_HELLO(member_id, revision, fetched_at, lease_hint)
DIRECTORY_UNCHANGED(revision)
DIRECTORY_DELTA(base_revision, revision, entries)
DIRECTORY_SNAPSHOT_REQUEST(revision)
DIRECTORY_ACK(revision)
```

These messages must be bounded, replay-resistant and authenticated by the existing
mesh protocol. A delta is accepted only when its base is known or it is replaced by
a full snapshot request. Applying a directory delta must never directly replace a
confirmed transport endpoint; it creates a probe candidate.

## Hot-plug state machine

```text
CACHED → PROBING → ACTIVE
   │       │          │
   │       └─ failed ─┘ keep old endpoint if any
   └─ stale/invalid → BOOTSTRAP_REQUIRED

ACTIVE + endpoint update → CANDIDATE → PROBING → ACTIVE
```

A member is announced to peers only after its identity, virtual IP, endpoint field
and revision pass validation. A peer that disappears from the latest manifest is not
immediately kicked from the running mesh; expiry and cryptographic revocation remain
separate operations.

## Acceptance tests

- First launch with a valid cache starts without waiting for GitHub.
- Same member ID with a changed IP and port updates one candidate, not two peers and
  not a new virtual IP.
- Two members observing the same new repository revision cause only one fetch; all
  connected peers receive one idempotent delta.
- Fetcher death causes lease takeover and does not stop mesh traffic.
- GitHub outage leaves the last validated cache and live gossip usable.
- Stale, malformed, replayed, duplicate-ID, duplicate-VIP and wrong-owner data is
  rejected without replacing live endpoints.
- A forged ID cannot claim another member's virtual IP under enrollment rules.
- A candidate is never promoted without authenticated encrypted traffic.
- Cache writes survive interruption and never contain the private PSK in the public
  handoff or Git metadata.
