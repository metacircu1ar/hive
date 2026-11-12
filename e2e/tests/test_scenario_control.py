"""Control measurements must reject misleading responses and retain failures."""

import copy
import json
import re
import socket
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from e2e.tests.status_fixture import status as status_record
from e2e.scripts.scenario_control import ping, validate_status


def missing_field_diagnostic(path, missing):
    if (not path and missing == "writer_observation") or (
        path == ("writer_observation",) and missing == "source"
    ):
        return "Invalid control writer observation"
    location = "control status" + "".join(
        f"[{component}]" if isinstance(component, int) else f".{component}" for component in path
    )
    return re.escape(f"Invalid fields at {location}") + "$"


class ControlProbeTests(unittest.TestCase):
    def exchange(self, handler, **options):
        with tempfile.TemporaryDirectory(prefix="hl-probe-", dir="/tmp") as directory:
            path = Path(directory) / "control.sock"
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
                server.bind(str(path))
                server.listen()
                server.settimeout(1)

                def serve():
                    with server.accept()[0] as client:
                        request = json.loads(client.recv(4096))
                        try:
                            handler(client, request)
                        except BrokenPipeError:
                            pass  # A deadline/byte limit deliberately closes the peer.

                thread = threading.Thread(target=serve)
                thread.start()
                try:
                    return ping(path, 123, **options)
                finally:
                    thread.join(timeout=2)
                    self.assertFalse(thread.is_alive())

    def test_fragmented_reply_and_ordered_clock_observations(self):
        def reply(client, request):
            self.assertEqual(request["args"], dict(op="ping"))
            data = json.dumps(status_record()) + "\n"
            client.sendall(data[:10].encode())
            client.sendall(data[10:].encode())

        value, observation = self.exchange(reply)
        self.assertEqual(value["pid"], 123)
        stamps = [
            observation[name + "_monotonic_ns"]
            for name in ("start", "connected", "sent", "first_byte", "received", "end")
        ]
        self.assertEqual(stamps, sorted(stamps))
        self.assertNotIn("error", observation)

    def test_failures_are_observations(self):
        cases = [
            b"{}\n",
            b"not-json\n",
            b"{}",
            b"{}\n{}\n",
            json.dumps(status_record(pid=999)).encode() + b"\n",
        ]
        for data in cases:
            with self.subTest(data=data):
                value, observation = self.exchange(lambda c, _: c.sendall(data))
                self.assertIsNone(value)
                self.assertIn("error", observation)
                self.assertGreater(observation["seconds"], 0)

    def test_one_deadline_survives_trickling_reply(self):
        def trickle(client, _):
            for _ in range(20):
                client.sendall(b" ")
                time.sleep(0.015)

        value, observation = self.exchange(trickle, timeout=0.05)
        self.assertIsNone(value)
        self.assertIn("Timeout", observation["error"])

    def test_response_byte_limit(self):
        with patch("e2e.scripts.harness.daemon.MAX_RESPONSE_BYTES", 64):
            value, observation = self.exchange(lambda c, _: c.sendall(b" " * 128))
        self.assertIsNone(value)
        self.assertIn("byte limit", observation["error"])

    def test_build_capability_shape_is_current_and_exact(self):
        core = status_record()
        core["compiled_features"] = []
        core["comb_backlog"] = dict(collectors=1, rows=3, delivery_supported=False)
        validate_status(core, 123)
        for bad in (
            {**core, "product_version": 1},
            {k: v for k, v in core.items() if k != "product_version"},
            {**core, "compiled_features": ["comb"]},
            {**core, "comb_backlog": dict(collectors=1, rows=3, delivery_supported=True)},
            {**status_record(), "compiled_features": ["comb", "comb"]},
            {k: v for k, v in core.items() if k != "comb_backlog"},
            {k: v for k, v in status_record().items() if k != "compiled_features"},
        ):
            with self.subTest(bad=bad), self.assertRaises(RuntimeError):
                validate_status(bad, 123)

    def test_current_status_rejects_missing_unknown_and_invalid_fields(self):
        good = status_record()
        paths = [
            (),
            ("storage",),
            ("capabilities",),
            ("capabilities", "providers", 0),
            ("writer_observation",),
            ("reader_observation",),
            ("reader_observation", "counts"),
            ("runtime_resources",),
            ("runtime_resources", "limits"),
        ]
        for path in paths:
            original = good
            for component in path:
                original = original[component]
            for missing in [*original, None]:
                changed = copy.deepcopy(good)
                record = changed
                for component in path:
                    record = record[component]
                if missing is None:
                    record["unexpected"] = True
                else:
                    del record[missing]
                with (
                    self.subTest(path=path, missing=missing),
                    self.assertRaisesRegex(RuntimeError, missing_field_diagnostic(path, missing)),
                ):
                    validate_status(changed, 123)
        for path, bad, expected in [
            (("status",), "unsupported", "Invalid enum at control status.status"),
            (
                ("runtime_resources", "limits", "allow_spawn_widening"),
                1,
                "Invalid type at control status.runtime_resources.limits.allow_spawn_widening",
            ),
            (("pid",), True, "Invalid integer at control status.pid"),
            (("pid",), 2**32, "Invalid integer at control status.pid"),
            (
                ("capabilities", "providers", 0, "provider"),
                "unsupported",
                "Invalid enum at control status.capabilities.providers",
            ),
            (
                ("capabilities", "providers", 0, "steering_policy"),
                "unsupported",
                "Invalid enum at control status.capabilities.providers",
            ),
            (("writer_observation", "source"), "unsupported", "Invalid control writer observation"),
            (
                ("reader_observation", "source"),
                "unsupported",
                "Invalid enum at control status.reader_observation.source",
            ),
        ]:
            changed = copy.deepcopy(good)
            record = changed
            for component in path[:-1]:
                record = record[component]
            record[path[-1]] = bad
            with self.subTest(path=path, bad=bad), self.assertRaisesRegex(RuntimeError, expected):
                validate_status(changed, 123)

    def test_setup_identity_is_structural_and_service_label_is_optional(self):
        identity = dict(path="/tmp/hive.yaml", sha256="a" * 64)
        good = status_record()
        good.update(configuration=identity, managed_service="hive")
        validate_status(good, 123)
        for change in (
            dict(configuration={"path": "/tmp/hive.yaml"}),
            dict(configuration={**identity, "extra": True}),
            dict(configuration={**identity, "sha256": 1}),
            dict(managed_service=None),
            dict(runtime_resources={**good["runtime_resources"], "active_turns": -1}),
        ):
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                validate_status({**good, **change}, 123)


if __name__ == "__main__":
    unittest.main()
