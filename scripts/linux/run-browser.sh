#!/usr/bin/env bash
# Runs the meow-meow in-app browser standalone (e.g. from a desktop
# shortcut, or to open a specific address without going through the main
# GUI). Unlike run-gui.sh/run-cli.sh, this does NOT need root -- the
# browser never touches the virtual network adapter, it just connects to
# whatever mesh IPs/names are already reachable.
#
# Usage: ./run-browser.sh [address]
#   ./run-browser.sh                -> opens the mesh home page
#   ./run-browser.sh alice.mesh     -> opens a specific mesh peer
#   ./run-browser.sh https://...    -> opens any normal URL too
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

exec ./meow-meow_browser "$@"
