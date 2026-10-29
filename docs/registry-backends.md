# Peer directory backends

Status: **interface implemented** (`core/src/backends.rs`). No hosted-service
adapter is implemented yet. Each adapter will live in its own crate and
implement the `Backend` trait.

## Goal

Every member publishes a current endpoint (`ip:port`). Clients read the
directory from one or more free stores, so a member who changes IP is reached
again without anyone running a server. Storage must be free, read-write, and
usable from any OS without an account the user has to manage.

## Design

- `Backend` trait: `fetch() -> Option<bytes>`, `publish(bytes)`, `name()`.
  One opaque blob per backend. The trait knows nothing about any service.
- `Document`: `{version, seq, issued_at, peers[]}`. Each peer is
  `{name, virtual_ip, public_ip, public_port, updated_at}`.
- **Authentication:** HMAC-SHA256 under a key derived from the network PSK.
  Only members can produce a directory that verifies. A writer on an open
  store can delete or withhold a directory, but cannot forge one.
- **Ordering:** highest `seq` wins. A `floor_seq` (highest value the client has
  already accepted) refuses rollback to an older directory.
- **Fan-out:** `fetch_latest` reads every backend and reports problems per
  backend. `publish_all` succeeds if at least one backend accepts the write.
- **Size:** 64 KB per directory, which fits the smallest common free-tier item
  limits (MantleDB unclaimed: 64 KB; jsonstorage.net free: 64 KB).

## Research: free JSON stores (checked 2026-10-29)

| Service | Anonymous read/write | Limits (free) | Fit | Notes |
|---|---|---|---|---|
| **MantleDB** (`mantledb.sh`) | Yes, unclaimed namespaces. `GET /v2/claim/:ns` gives a write key (`X-Mantle-Key`) | 100 entries, 64 KB each, 10,000 req/24h; unclaimed purged after 30 days without writes, claimed after 90 | **Best candidate** | Live (verified: returns JSON 404 for unknown paths). Labelled V2.0.0-BETA. Claimed namespace gives write protection on top of our signature. |
| **jsonstorage.net** | Read by item ID; writes need an API key | 25 items, 1,000 req/month, 64 KB per item | Possible secondary | Quota is very low for polling clients. |
| **kvdb.io** | Buckets via `POST`; access keys optional | 14-day trial; free-tier keys expire after 3 months; Pro is $19/mo. Not permanent. | Poor fit | Not free long-term. |
| **key-value.co** | Token is the capability; reads need the token | 100 KB; 100 reads/hour per token on free tier; 5 token creations/hour per IP | Poor fit | Read limit is too tight for many clients. |
| **npoint.io** | Read-only over the API for most users; API editing is in private beta | 100 req/min per IP | **Rejected** | Not read-write. |
| **jsonbox.io** | — | The domain now serves unrelated spam content. The GitHub repo returns 404. | **Rejected** | Treat as dead; do not send data there. |

Sources: MantleDB (`https://mantledb.sh/`), jsonstorage.net (`https://www.jsonstorage.net/`), kvdb.io (`https://kvdb.io/`), key-value.co docs (`https://key-value.co/docs`), npoint.io (`https://www.npoint.io/`).

### Recommendation

1. **MantleDB** as the first hosted adapter. Claim one namespace per network
   and keep the write key in the network's config.
2. **GitHub** (existing `registry.rs`) as a second, independent backend. It is
   free and already relied on.
3. Any third, such as jsonstorage.net, only if it is needed for redundancy.

Google Drive and similar services fit the same trait. Each needs its own
adapter crate and OAuth setup, which is out of scope for now.

## Not yet done

- No adapter is implemented (MantleDB, GitHub, Drive). The trait is ready for them.
- The mesh runtime does not yet use the directory. `registry.rs` still reads the
  GitHub file directly.
- A publisher that is not yet a member cannot publish. Publishing requires the
  network key, so it stays with members.
- Race between two publishers: `seq` makes it detectable, but the loser's
  change is dropped and must be re-applied on the next fetch.
