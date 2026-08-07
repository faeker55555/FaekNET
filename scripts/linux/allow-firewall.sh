#!/usr/bin/env bash
# One-time helper: allows all traffic on the mesh's virtual network
# interface through the local firewall. Needed if TCP-based apps (e.g.
# Minecraft, a hosted website) time out even though the mesh itself is
# running and UDP-based games work fine -- see the "If TCP-based things
# don't connect" section in the README for why this happens.
#
# Safe to run even if you don't strictly need it: this only opens traffic
# on the mesh's own virtual adapter (not your real network interface), and
# anything reaching that adapter has already passed ChaCha20-Poly1305
# authentication against your mesh's shared key.
set -euo pipefail

IFACE="${1:-lanmesh0}"

echo "Allowing all traffic on interface: $IFACE"

if command -v ufw >/dev/null 2>&1; then
    sudo ufw allow in on "$IFACE"
    echo "Done (ufw)."
elif command -v firewall-cmd >/dev/null 2>&1; then
    sudo firewall-cmd --permanent --zone=trusted --add-interface="$IFACE" || true
    sudo firewall-cmd --reload
    echo "Done (firewalld, interface added to the 'trusted' zone)."
elif command -v iptables >/dev/null 2>&1; then
    sudo iptables -I INPUT -i "$IFACE" -j ACCEPT
    echo "Done (iptables). Note: this rule is NOT persistent across reboots"
    echo "unless you save it with your distro's iptables-persistent tool."
else
    echo "No supported firewall tool found (ufw / firewalld / iptables)."
    echo "If you have a different firewall, allow inbound traffic on '$IFACE' manually."
    exit 1
fi

echo ""
echo "If your adapter has a different name than '$IFACE', check with:"
echo "    ip addr show | grep -i lanmesh"
echo "and re-run this script with that name, e.g.:"
echo "    ./allow-firewall.sh lanmesh1"
