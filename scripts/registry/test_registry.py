import copy
import datetime as dt
import json
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import publish
import update

NOW = 1789000000


def endpoint():
    return {"schema_version": 1, "name": "alice", "virtual_ip": "10.66.0.2", "public_ip": "8.8.8.8", "public_port": 54321}


def event():
    stamp = dt.datetime.fromtimestamp(NOW, dt.timezone.utc).isoformat().replace("+00:00", "Z")
    return {"action": "opened", "issue": {"title": update.TITLE, "body": publish.build_body(endpoint()),
        "created_at": stamp, "updated_at": stamp, "user": {"login": "Alice", "type": "User"}}}


def enrollment():
    return {"schema_version": 1, "members": {"alice": ["10.66.0.2"], "bob": ["10.66.0.3"]}}


class RegistryTests(unittest.TestCase):
    def apply(self, request=None, members=None, directory=None, now=NOW):
        return update.update(request or event(), members or enrollment(), directory or {"schema_version": 1, "peers": []}, now)

    def test_enrolled_request_round_trips(self):
        result = self.apply()
        self.assertEqual(tomllib.loads(update.encode(result)), result)
        self.assertEqual(result["peers"][0]["owner"], "alice")
        self.assertEqual(result["peers"][0]["expires_at"], NOW + update.TTL)

    def test_cannot_overwrite_another_members_ip(self):
        request = event()
        request["issue"]["user"]["login"] = "bob"
        with self.assertRaises(ValueError): self.apply(request)

    def test_unenrolled_user_rejected(self):
        request = event()
        request["issue"]["user"]["login"] = "outsider"
        with self.assertRaises(ValueError): self.apply(request)

    def test_multiple_devices_and_duplicate_assignments(self):
        members = enrollment()
        members["members"]["alice"].append("10.66.0.4")
        self.apply(members=members)
        members["members"]["bob"].append("10.66.0.2")
        with self.assertRaises(ValueError): self.apply(members=members)

    def test_unknown_fields_and_secrets_rejected(self):
        request = event()
        request["issue"]["body"] += "psk = 'secret'\n"
        with self.assertRaises(ValueError): self.apply(request)

    def test_ip_port_and_name_validation(self):
        for field, value in [("public_ip", "127.0.0.1"), ("public_ip", "10.0.0.1"),
            ("public_ip", "100.64.0.1"), ("public_ip", "203.0.113.1"),
            ("public_ip", "224.0.0.1"), ("public_ip", "::1"), ("public_ip", "example.com"), ("public_ip", 134744072),
            ("public_port", 0), ("public_port", 65536), ("public_port", True),
            ("virtual_ip", "10.66.0.0"), ("virtual_ip", "10.66.0.255"),
            ("virtual_ip", "10.67.0.2"), ("name", "$(curl bad)"), ("name", ""), ("name", "x" * 33)]:
            with self.subTest(field=field, value=value):
                data = endpoint()
                data[field] = value
                with self.assertRaises(ValueError): update.validate_endpoint(data)

    def test_replay_does_not_replace_newer_endpoint(self):
        directory = self.apply()
        directory["peers"][0]["updated_at"] = NOW + 1
        directory["peers"][0]["public_port"] = 12345
        self.assertEqual(self.apply(directory=directory), directory)

    def test_different_owners_entries_preserved(self):
        directory = self.apply()
        request = event()
        request["issue"]["user"]["login"] = "bob"
        data = endpoint()
        data["name"], data["virtual_ip"] = "bob", "10.66.0.3"
        request["issue"]["body"] = publish.build_body(data)
        result = self.apply(request, directory=directory)
        self.assertEqual(len(result["peers"]), 2)

    def test_expired_and_unenrolled_entries_pruned(self):
        directory = self.apply()
        old = copy.deepcopy(directory["peers"][0])
        old.update(virtual_ip="10.66.0.3", owner="bob", expires_at=NOW - 1)
        directory["peers"] = [old]
        self.assertEqual(len(self.apply(directory=directory)["peers"]), 1)
        old["expires_at"] = NOW + 100
        old["owner"] = "outsider"
        self.assertEqual(len(self.apply(directory=directory)["peers"]), 1)

    def test_edited_future_and_stale_requests_rejected(self):
        request = event()
        request["issue"]["updated_at"] = "2026-09-11T00:00:00Z"
        with self.assertRaises(ValueError): self.apply(request)
        with self.assertRaises(ValueError): self.apply(now=NOW + 86401)
        with self.assertRaises(ValueError): self.apply(now=NOW - 61)

    def test_non_request_bot_and_oversized_body_rejected(self):
        for key, value in [("title", "hello"), ("body", "x" * 2049), ("user", {"login": "alice", "type": "Bot"})]:
            request = event()
            request["issue"][key] = value
            with self.assertRaises(ValueError): self.apply(request)

    @patch.dict("os.environ", {"GITHUB_REPOSITORY": "faeker55555/FaekNET", "REGISTRY_BRANCH": "main"})
    @patch("time.sleep")
    @patch("subprocess.run")
    def test_concurrent_commit_is_reloaded_and_merged(self, run, sleep):
        import base64
        from types import SimpleNamespace
        def fetched(value, sha):
            return SimpleNamespace(stdout=json.dumps({"sha": sha, "content": base64.b64encode(value.encode()).decode()}))
        members = "schema_version=1\n[members]\nalice=['10.66.0.2']\nbob=['10.66.0.3']\n"
        empty = "schema_version=1\npeers=[]\n"
        bob_event = event()
        bob_event["issue"]["user"]["login"] = "bob"
        data = endpoint()
        data["name"], data["virtual_ip"] = "bob", "10.66.0.3"
        bob_event["issue"]["body"] = publish.build_body(data)
        bob_directory = update.encode(self.apply(bob_event))
        run.side_effect = [fetched(members, "members-sha"), fetched(empty, "old-sha"),
            SimpleNamespace(returncode=1, stderr="HTTP 409 Conflict"),
            fetched(members, "members-sha"), fetched(bob_directory, "new-sha"),
            SimpleNamespace(returncode=0, stderr="")]
        update.publish(event(), NOW)
        request = json.loads(run.call_args_list[-1].kwargs["input"])
        self.assertEqual(request["sha"], "new-sha")
        written = tomllib.loads(base64.b64decode(request["content"]).decode())
        self.assertEqual([p["virtual_ip"] for p in written["peers"]], ["10.66.0.2", "10.66.0.3"])


class PublisherTests(unittest.TestCase):
    def test_nat_churn_is_coalesced_but_latest_endpoint_is_published(self):
        debounce = publish.EndpointDebouncer()
        first = endpoint()
        second = dict(first, public_port=60000)
        self.assertFalse(debounce.ready(first, NOW))
        self.assertFalse(debounce.ready(second, NOW + 30))
        self.assertFalse(debounce.ready(second, NOW + 59))
        self.assertTrue(debounce.ready(second, NOW + 60))
        self.assertTrue(debounce.ready(second, NOW + 90))


    def test_only_public_fields_are_serialized(self):
        data = dict(endpoint(), psk="secret", token="credential")
        text = publish.build_body(data)
        self.assertNotIn("secret", text)
        self.assertNotIn("credential", text)
        self.assertEqual(tomllib.loads(text), endpoint())

    def test_fresh_handoff_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "endpoint.toml"
            path.write_text(publish.build_body(endpoint()) + f"observed_at = {NOW}\n")
            self.assertEqual(publish.request_from_endpoint(path, NOW), endpoint())
            for now in [NOW + 121, NOW - 1]:
                with self.assertRaises(ValueError): publish.request_from_endpoint(path, now)
            path.write_text(path.read_text() + "psk = 'secret'\n")
            with self.assertRaises(ValueError): publish.request_from_endpoint(path, NOW)

    @patch("publish.time.time", return_value=NOW)
    @patch("publish.read_public_toml")
    @patch("publish.gh")
    def test_publication_uses_stdin_and_throttles(self, gh, read, clock):
        gh.side_effect = [json.dumps({"login": "alice"}), "https://github.com/faeker55555/FaekNET/issues/1", json.dumps({"login": "alice"})]
        read.side_effect = [enrollment(), {"schema_version": 1, "peers": []}, enrollment()]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "endpoint.toml"
            path.write_text(publish.build_body(endpoint()) + f"observed_at = {NOW}\n")
            state = Path(tmp) / "state.json"
            publish.tick(path, state)
            call = gh.call_args_list[1]
            self.assertIn("--body-file", call.args)
            self.assertEqual(tomllib.loads(call.kwargs["body"]), endpoint())
            publish.tick(path, state)
            self.assertEqual(gh.call_count, 3)

    @patch("publish.time.time", return_value=NOW)
    @patch("publish.read_public_toml", return_value=enrollment())
    @patch("publish.gh", return_value='{"login":"outsider"}')
    def test_unenrolled_publisher_never_creates_issue(self, gh, read, clock):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "endpoint.toml"
            path.write_text(publish.build_body(endpoint()) + f"observed_at = {NOW}\n")
            with self.assertRaises(ValueError): publish.tick(path, Path(tmp) / "state.json")
            self.assertEqual(gh.call_count, 1)


if __name__ == "__main__":
    unittest.main()
