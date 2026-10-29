# Shared directory

The shared directory lets a network work without anyone re-sharing addresses by hand.
Each member publishes its current public IP and port to a storage service, and every
member reads the list back. If your ISP changes your IP, or your NAT changes your port,
everyone else picks up the new endpoint within about a minute. A new member needs only
the network key plus one invite line.

## What propagates

| Change                                  | Reaches others within              |
|-----------------------------------------|------------------------------------|
| Your public IP or port changes          | ~1–2 min (publish, then their sync) |
| A new member joins                      | ~1–2 min                           |
| A member's display name changes         | ~1–2 min                           |
| A member stops running for >24 h        | dropped from the list after 24 h   |

Services (`add-service`) still travel through gossip between connected peers, as before.

## Storage services

Two free services are supported. Configure one or both. Each holds the same signed
document, so one going down does not stop the network.

- **GitHub**: one JSON file in a repository you own. Create a repository, then create a
  fine-grained token that can read and write **contents of that one repository only**.
  Writes are compare-and-swap, so two members updating at once cannot silently overwrite
  each other.
- **MantleDB** (https://mantledb.sh): a JSON entry, no account needed. A namespace
  without a key can be written by anyone who knows its name. Claim it to require a key:
  `meow-meow directory mantle-claim <namespace>`.

Trust does not depend on either service. The directory is signed with a key derived from
the network key (PSK). Anyone who can write to a storage service can delete or withhold
the directory, but cannot forge one. A running member rejects any directory older than
one it has already accepted, so an attacker cannot roll it back to an address it has left.

Known limit: that memory is not kept across restarts. A member that has just started can
be shown an old, validly signed directory until it sees a newer one, which takes about a
minute. The effect is a short delay in reaching peers, not a takeover.

## Setting it up

On the first machine:

```
meow-meow directory github <owner/repo> <token>       # or:
meow-meow directory mantle <namespace>                # unclaimed, or add a write key
meow-meow directory invite                            # prints one FNDIR1:... line
```

On every other machine (after it has the network key and `mesh.toml`):

```
meow-meow directory join FNDIR1:...
meow-meow run
```

`directory status` shows the configured services, the members they list, and any problems,
without publishing anything.

Settings are stored in `directory.toml`, next to `mesh.toml`. On Unix the file is mode
0600. **The invite contains the GitHub token and MantleDB write key.** Share it the same
way as the network key: privately, never publicly.

Changes take effect the next time the mesh starts.

## Privacy

The directory lists each member's name, mesh address, and public IP and port. Anyone who
can read the storage service can see them. It never contains the network key. Do not use the directory if your IP must stay hidden
from other members.

## Limits

- At most 254 members per directory (the mesh subnet is 10.66.0.0/24).
- Each directory document is at most 64 KB.
- GitHub: each publish is a commit. Heartbeats run every 30 minutes, so expect about 48
  commits a day per network. Use a dedicated repository.
- Clocks on members should be roughly in sync. Freshness is decided by publish time.

## Relation to the older public directory

`network/peers.toml` and the `repository_discovery` option predate this feature. They
use GitHub logins and issues, and they only publish endpoints. The shared directory is
separate and works with any network key. You can use both.
