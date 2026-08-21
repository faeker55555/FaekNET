#!/usr/bin/env bash
# Runs the meow-meow CLI. Passes through all arguments, e.g.:
#   ./run-cli.sh init
#   ./run-cli.sh export
#   ./run-cli.sh run
#
# `run` (and anything that brings the mesh up) needs root/CAP_NET_ADMIN to
# create the virtual network adapter, so it's automatically re-executed
# under sudo when needed. Commands that only touch mesh.toml (init,
# add-peer, export, import, list-peers, myaddr, ping, genkey) don't need
# elevation and are run directly.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

NEEDS_ROOT_CMD="run"

if [ "${1:-}" = "$NEEDS_ROOT_CMD" ] && [ "$(id -u)" -ne 0 ]; then
    echo "meow-meow run needs root privileges to create its virtual network adapter."
    echo "Re-running with sudo..."
    exec sudo -E ./meow-meow "$@"
fi

exec ./meow-meow "$@"
