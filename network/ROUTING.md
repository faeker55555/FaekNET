# Peer-relay routing: design contract (not yet implemented in the native engine)

The HTML reference demonstrates a ping-weighted graph and route selection. It does
not demonstrate that the native mesh can relay packets. The native transport remains
direct UDP; implementing forwarding safely requires a separate protocol change.

## Separate identity, endpoint and route

- **Identity:** a stable peer identity and assigned virtual IP. An IP address alone
  is not cryptographic identity; the current shared-PSK protocol has that limitation.
- **Endpoint:** the currently observed public IP:port. A Git revision supplies a
  candidate, not authenticated reachability or a reason to discard a working path.
- **Route:** direct or via a consenting peer. A relay is another mesh member, not an
  open proxy to arbitrary internet hosts.

## Native protocol requirements before enabling relay UI

1. A versioned, authenticated relay envelope with original source, final destination,
   route identifier and a strict hop budget. Start with one intermediate relay.
2. Validate both endpoints and relay permission. Reject loops, nested relay frames,
   unknown destinations and excessive packet sizes. Do not allow arbitrary UDP
   destinations or turn a peer into a public forwarding service.
3. Do not mistake the relay's UDP source address for the origin peer's public endpoint.
   Relayed packets must not trigger ordinary direct-address roaming for the origin.
4. Bound per-peer queue memory, bandwidth and packet rate. Stop relaying on permission
   withdrawal. Handle congestion/loss before claiming better throughput.
5. Probe prospective routes end to end. Link RTT sums are estimates, not sufficient
   evidence for switching a live flow. Measure loss/jitter and, if desired, throughput
   separately; use timestamps and freshness limits on all reports.
6. Prefer a validated working route until an alternative consistently improves it.
   The reference uses three samples, max 10-second age, 20% plus 3 ms improvement,
   and a 10-second hold-down. Route failure may bypass hold-down only if a validated
   alternative already exists. Never switch to an untested relay.
7. Switch routes without changing the application's virtual source/destination IPs.
   Keep a previous route available during validation (make-before-break); bound
   duplicate/reordered traffic during transition. Do not promise zero packet loss.
8. The shared PSK means any member can authenticate as another and can decrypt mesh
   traffic. A relay must not be described as unable to read payloads under this model.
   For untrusted relays, implement per-peer keys, authenticated identity binding and
   end-to-end encryption independent of relay/link authentication first.

## Repository semantics for NAT changes

The flow is analogous to a package manifest update, not uploading packets to Git:

`observe → debounce → submit → validate owner → commit → fetch → probe → promote`

The enrolled virtual IP stays fixed; public IP/port and revision change. Never commit
the PSK or access tokens. Existing native peers now retain directory addresses as
separate probe candidates; authenticated receive still performs address promotion.
First-contact bootstrap uses the published endpoint as an initial unconfirmed hint.

A client must be able to discover a fresh endpoint and submit the update; storing
old endpoints cannot discover new ones by itself. GitHub Actions queues, API limits,
raw-file caches and polling introduce delays. Local roaming and authenticated gossip
remain the fast path while at least one connection survives. If all direct paths
are impossible, a Git directory alone cannot replace a relay.

## Acceptance tests for a real forwarding implementation

- Three real peers across separate networks: direct success, direct blocked, valid
  relay success and relay loss. Include TCP sessions and simulated broadcast.
- Reject forged/replayed envelopes, wrong keys, wrong destination, loops and invalid
  hop budgets. Confirm that no private payload is exposed in logs or Git metadata.
- NAT IP **and** port changes: working old path preserved until a fresh candidate
  authenticates; no promotion on failed probes or stale/out-of-order revisions.
- Withdraw relay permission mid-flow and verify bounded queues and failover behavior.
- RTT jitter does not flap routes; compare actual throughput as well as latency.
- Native Linux and Windows builds, plus privileged TUN smoke tests on both platforms.

Until these are implemented and tested, routing remains a clearly labeled simulation
in the reference and no native relay capability should be advertised.
