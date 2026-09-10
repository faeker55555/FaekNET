# Graph and repository UI reference

Open `redesign.html`, or use the existing static preview server. This reference has
no build step and makes no network/GitHub requests. Inter is bundled locally under
the SIL Open Font License (`assets/INTER-LICENSE.txt`).

`redesign-preview.png` is a Chromium screenshot at 1440 × 1250 (full page).

## Obsidian-style graph

- Quiet circular nodes and thin edges, with a highlighted selected route.
- Drag nodes, pan the background, scroll to zoom, or use the zoom/reset controls.
- Force-directed layout: lower RTT gives a shorter spring target. Edge labels are
  simulated RTT in milliseconds. Distances are approximate: arbitrary RTT graphs
  cannot generally be embedded as exact Euclidean distances.
- Samples drift gently; the direct-link RTT slider lets you test a larger change. Position targets are rendered with time-based tweening, so RTT updates do not teleport nodes and remain smooth across different update/frame rates.
- Peer search and selection synchronize the graph and detail inspector.
- Light/dark appearance and reduced-motion support.

## Routing simulation — NOT native relay transport

`mesh-model.js` is a pure prototype policy model, not a packet forwarding engine.
The native application still uses direct UDP and does not yet proxy through peers.
No real cryptographic checks, packets or throughput measurements occur in this HTML.

The sample model requires:
- Explicit relay permission for the intermediate node.
- At most one relay (two edges); no loops or unknown nodes.
- Three fresh, simulated authenticated end-to-end observations for a route.
- At least a 20% **and** 3 ms improvement, plus a 10-second hold-down before replacing
  a working route. Small improvements do not cause route flapping.
- A previously validated fallback when the active relay fails. Otherwise no route.

The demo manufactures probe observations from link RTTs; real integration must
measure the candidate end-to-end path. Smaller RTT does **not** establish higher
bandwidth. Use **Fail active relay**, the relay permission checkbox, and the RTT
slider to exercise the model. The native design requirements are recorded in
`../network/ROUTING.md`.

## Repository / manifest reference

The directory panel now resembles a repository/package manifest: stable virtual IDs,
published endpoints, revision IDs, install/probe state, and a short event log.

**Simulate NAT change** demonstrates:
1. Detect a different public IP:port without changing the virtual identity.
2. Queue a metadata request and commit a new sample revision.
3. Pull the revision and probe the candidate endpoint.
4. Promote only after successful sample validation; failed probes keep the old
   active endpoint. `meshDemo.simulateNat('alice', true)` exercises rejection.

All hashes, addresses, timings, commits and events shown here are fabricated sample
data. No GitHub write happens. The actual registry automation is separate; see
`../network/README.md`. Native discovery now keeps updates to existing peers as
probe candidates instead of blindly overwriting the confirmed endpoint. That Rust
change is not compiled/runtime-verified in this environment.

## Verification

Pure policy tests (no dependencies):

```sh
node --test gui_mockup/mesh-model.test.cjs
```

Browser tests, from this directory:

```sh
npm install --no-save --package-lock=false playwright
npx playwright install chromium
node verify.cjs
```

Browser checks regenerate the screenshot and cover graph rendering, route display,
relay failure, permission withdrawal, search/empty/reset, dragging, zoom, RTT-weighted
motion, successful and rejected NAT updates, pause/resume, themes, dialogs and
horizontal overflow at 1440, 980, 760, 600 and 390px. Tests perform no real GitHub or
mesh operations. The older `mockup.html` remains a historical reference.
