#!/usr/bin/env python3
"""Publish public endpoint metadata using the user's own authenticated gh CLI.
Run separately, without sudo, beside the running native mesh. Python 3.11+.
No shared GitHub credentials or network key are read or uploaded.
"""
import argparse
import base64
import json
from pathlib import Path
import subprocess
import time
import tomllib

from update import FIELDS, TITLE, validate_endpoint

REPO = "faeker55555/FaekNET"
MIN_INTERVAL = 120
POLL_INTERVAL = 30
HEARTBEAT = 86400


def gh(*args, body=None):
    result = subprocess.run(["gh", *args], input=body, capture_output=True, text=True, timeout=30)
    if result.returncode:
        # gh's full output may contain private account details. Keep logs minimal.
        raise RuntimeError("GitHub request failed. Check `gh auth status`, network access, and repository permissions.")
    return result.stdout.strip()


def read_public_toml(path):
    data = json.loads(gh("api", f"repos/{REPO}/contents/{path}"))
    if data.get("size", 0) > 131072:
        raise ValueError("Registry file too large")
    return tomllib.loads(base64.b64decode(data["content"]).decode())


def request_from_endpoint(path, now):
    raw = path.read_bytes()
    if len(raw) > 2048:
        raise ValueError("Endpoint handoff too large")
    endpoint = tomllib.loads(raw.decode())
    if set(endpoint) != FIELDS | {"observed_at"}:
        raise ValueError("Unexpected endpoint fields; refusing to upload")
    observed = endpoint.pop("observed_at")
    if type(observed) is not int or not 0 <= now - observed <= 120:
        raise ValueError("No fresh endpoint. Enable repository discovery and run the mesh first.")
    return validate_endpoint(endpoint)


def build_body(endpoint):
    # Only these public fields ever leave this process. Never read mesh.toml.
    return "\n".join(f"{key} = {json.dumps(endpoint[key])}" for key in sorted(FIELDS)) + "\n"


def load_state(path):
    if not path.exists():
        return {}
    return json.loads(path.read_text())


def tick(endpoint_path, state_path, dry_run=False):
    now = int(time.time())
    endpoint = request_from_endpoint(endpoint_path, now)
    login = json.loads(gh("api", "user"))["login"].lower()
    enrollment = read_public_toml("network/members.toml")
    members = {name.lower(): ips for name, ips in enrollment.get("members", {}).items()}
    if endpoint["virtual_ip"] not in members.get(login, []):
        raise ValueError("This GitHub account is not enrolled for this virtual IP. Ask the maintainer to add it to network/members.toml.")
    if dry_run:
        print("Public request that would be submitted (no issue created):\n" + build_body(endpoint))
        return
    state = load_state(state_path)
    if now - state.get("submitted_at", 0) < MIN_INTERVAL:
        print("Waiting for the 2-minute publication rate limit.")
        return
    directory = read_public_toml("network/peers.toml")
    for peer in directory.get("peers", []):
        if peer.get("owner", "").lower() == login and all(peer.get(k) == endpoint[k] for k in FIELDS - {"schema_version"}):
            if peer.get("expires_at", 0) > now and now - peer.get("updated_at", 0) < HEARTBEAT:
                print("Published endpoint is current.")
                return
    # Do not flood Issues while Actions is disabled, delayed or a request failed.
    if state.get("endpoint") == endpoint and now - state.get("submitted_at", 0) < HEARTBEAT:
        print("Endpoint update queued; check its GitHub Actions run before resubmitting.")
        return
    url = gh("issue", "create", "--repo", REPO, "--title", TITLE, "--body-file", "-", body=build_body(endpoint))
    state_path.write_text(json.dumps({"endpoint": endpoint, "submitted_at": now}) + "\n")
    print(f"Endpoint update submitted for validation: {url}")


class EndpointDebouncer:
    """Coalesce NAT flapping; only publish an endpoint stable across two polls."""
    def __init__(self):
        self.candidate = None
        self.since = None

    def ready(self, endpoint, now):
        if endpoint != self.candidate:
            self.candidate = dict(endpoint)
            self.since = now
            return False
        return now - self.since >= POLL_INTERVAL


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", type=Path, default=Path(".faeknet-endpoint.toml"))
    parser.add_argument("--state", type=Path, default=Path(".faeknet-registry-state.json"))
    parser.add_argument("--watch", action="store_true", help="Watch fresh endpoints every 30 seconds; debounce changes and publish daily heartbeats")
    parser.add_argument("--dry-run", action="store_true", help="Validate enrollment and display public fields without creating an issue")
    args = parser.parse_args()
    debounce = EndpointDebouncer()
    while True:
        try:
            now = int(time.time())
            current = request_from_endpoint(args.endpoint, now)
            if not args.watch or debounce.ready(current, now):
                tick(args.endpoint, args.state, args.dry_run)
            else:
                print("Endpoint changed; waiting one poll to coalesce NAT churn.")
        except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.TimeoutExpired) as error:
            print(f"Publisher: {error}")
            if not args.watch:
                raise SystemExit(1)
        if not args.watch:
            return
        time.sleep(POLL_INTERVAL)


if __name__ == "__main__":
    main()
