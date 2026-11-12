"""The host's final decision owns the consumed result, after namespace cleanup."""

import contextlib
import io
import json
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from e2e.campaign import Admission
from e2e.scripts import local_ci
from e2e.tests.test_local_ci import repository
from release.cancellation import Cancelled
from release.files import atomic_write_json
from release.progress import entry


class DockerResult:
    def __init__(self, output, report):
        self.output, self.report = output, report

    def owner(self):
        return dict(endpoint="unix:///fixture.sock", daemon_id="fixture-daemon", resource="none")

    def call(self, command, **options):
        atomic_write_json(self.output / "container-result.json", self.report)
        return subprocess.CompletedProcess(command, 0, "", "")


class ReportOwner:
    def __init__(self, admission, output, report, outcome, events):
        self.admission, self.output = admission, output
        self.docker = DockerResult(output, report)
        self.outcome, self.events = outcome, events
        self.cleanup = None

    def ensure_image(self, root, commit):
        return "fixture-image"

    def create(self, *args, **options):
        active = self.admission.active
        self.admission.advance(
            "created",
            resource="container",
            name="hive-e2e-" + active["nonce"],
            container_id="a" * 64,
            create_completed=True,
        )
        return "a" * 64

    def retire(self):
        self.events.append("retire")
        if self.outcome == "cancelled":
            raise Cancelled(signal.SIGTERM)
        if self.outcome == "uncertain":
            raise RuntimeError("Fixture namespace cleanup is uncertain")
        self.cleanup = dict(confirmed=True)
        self.admission.complete()


class ReportAcceptance(unittest.TestCase):
    def test_cancelled_and_uncertain_hosts_never_publish_the_provisional_pass(self):
        for outcome, status in (("cancelled", 143), ("uncertain", 1), ("passed", 0)):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = root / "source"
                commit = repository(source)
                platform = "linux-" + local_ci.native_architecture()
                report = dict(
                    commit=commit,
                    platform=platform,
                    profile="release",
                    validation="ci",
                    passed=True,
                    failed_steps=[],
                )
                events = []
                original_publish = local_ci.publish

                def publish(results):
                    self.assertEqual(events, ["retire"])
                    events.append("publish")
                    original_publish(results)

                def owner(admission, output, architecture):
                    return ReportOwner(admission, output, report, outcome, events)

                with (
                    patch.object(local_ci, "product_root", return_value=source),
                    patch.object(
                        local_ci, "Admission", side_effect=lambda: Admission(root / "admission")
                    ),
                    patch.object(local_ci.Container, "resolve", side_effect=owner),
                    patch.object(local_ci, "publish", side_effect=publish),
                    patch(
                        "sys.argv",
                        [
                            "local_ci",
                            "--full",
                            "--platform",
                            platform,
                            "--output-dir",
                            str(root / "runs"),
                            "--run-id",
                            "fixture",
                        ],
                    ),
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    self.assertEqual(entry(local_ci.main, name="scenario"), status)
                output = root / "runs" / (platform + "-fixture")
                self.assertTrue(
                    json.loads((output / "container-result.json").read_text())["passed"]
                )
                if outcome == "passed":
                    accepted = json.loads((output / "results.json").read_text())
                    self.assertTrue(accepted["host_completed"])
                    self.assertTrue(accepted["container_cleanup"]["confirmed"])
                    self.assertEqual(events, ["retire", "publish"])
                else:
                    self.assertFalse((output / "results.json").exists())
                    self.assertEqual(events, ["retire"])
