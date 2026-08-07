#!/usr/bin/env bash
# Runs the lan_mesh GUI. Root/CAP_NET_ADMIN is needed to create the
# virtual network adapter -- if not already root, re-execs itself under
# sudo automatically so this is a true double-click-and-go script.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

if [ "$(id -u)" -ne 0 ]; then
    echo "lan_mesh needs root privileges to create its virtual network adapter."
    echo "Re-running with sudo..."
    exec sudo -E "$0" "$@"
fi

exec ./lan_mesh_gui "$@"
