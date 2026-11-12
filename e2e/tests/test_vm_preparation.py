"""Preparation reports or prepares exact inputs without booting a VM."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from e2e.campaign import Admission, read_receipt
from e2e.tests.test_vm_contracts import actor_fixture
from e2e.vm import images, prepare, state
from release.files import atomic_write_bytes, atomic_write_json
from release.progress import entry


class Preparation(unittest.TestCase):
    def test_report_only_never_installs_downloads_or_creates_and_install_only_fetches(self):
        for install in (False, True):
            with self.subTest(install=install), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                args = SimpleNamespace(install=install, cache=root / "images", ubuntu="24.04")
                admission = Admission(root / "admission")
                calls = []

                def native(argv, **options):
                    calls.append(argv)
                    return subprocess.CompletedProcess(argv, 0, "limactl fixture", "")

                with (
                    patch.object(prepare, "admit_host"),
                    patch.object(prepare, "Admission", return_value=admission),
                    patch.object(prepare.shutil, "which", return_value="/fixture/limactl"),
                    patch.object(prepare.client, "run", side_effect=native),
                    patch.object(prepare, "free_bytes", return_value=100 * 1024**3),
                    patch.object(prepare, "image_path") as image,
                    patch.object(prepare, "run") as installation,
                    contextlib.redirect_stdout(io.StringIO()) as output,
                ):
                    status = entry(lambda: prepare.main(args), name="verify")
                self.assertEqual(status, 0 if install else 1)
                self.assertEqual(calls, [["limactl", "--version"]])
                self.assertFalse(installation.called)
                self.assertEqual(image.called, install)
                if install:
                    self.assertEqual(image.call_args.kwargs, dict(install=True))
                else:
                    self.assertFalse(args.cache.exists())
                result = json.loads(output.getvalue().splitlines()[-1])
                self.assertEqual(result["ready"], install)
                self.assertIsNone(read_receipt(admission.root))

    def test_missing_lima_requires_install_and_reports_exact_command(self):
        args = SimpleNamespace(install=False, cache=None, ubuntu="24.04")
        with (
            patch.object(prepare, "admit_host"),
            patch.object(prepare.shutil, "which", return_value=None),
            patch.object(prepare, "run") as native,
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            with self.assertRaises(SystemExit):
                prepare.main(args)
        self.assertFalse(native.called)
        self.assertEqual(json.loads(output.getvalue())["commands"], ["brew install lima"])

    def test_cached_image_is_verified_and_corruption_is_not_silently_downloaded(self):
        body = b"pinned fixture image"
        item = dict(
            name="fixture.img",
            url="https://invalid.example/fixture.img",
            pin=dict(bytes=len(body), sha256=hashlib.sha256(body).hexdigest()),
        )
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            atomic_write_bytes(root / item["name"], body)
            with (
                patch.object(images, "image_input", return_value=item),
                patch.object(images, "fetch") as fetch,
            ):
                self.assertEqual(images.image_path(root, "24.04"), root / item["name"])
                atomic_write_bytes(root / item["name"], b"wrong fixture image")
                with self.assertRaises(ValueError):
                    images.image_path(root, "24.04")
                self.assertFalse(fetch.called)

    def test_explicit_lima_home_is_frozen_and_create_chooses_execution_mode_once(self):
        with actor_fixture() as (actor, _):
            # Exercise the actual call method rather than the fixture transport.
            from e2e.vm.lima import Lima

            with (
                patch("e2e.vm.native.client.run") as native,
                patch.dict("os.environ", LIMA_HOME="/foreign"),
            ):
                Lima.call(actor, ["list"])
                self.assertEqual(native.call_args.kwargs["env"]["LIMA_HOME"], str(actor.home))
                self.assertNotIn("pass_fds", native.call_args.kwargs)
            actor.state["mode"] = "create"
            path = actor.output / "state.json"
            atomic_write_json(path, state.fresh(actor.state))
            selected = state.resume(path, actor.state, "full", [])
            atomic_write_json(path, selected)
            self.assertEqual(state.resume(path, actor.state, "full", []), selected)
            with self.assertRaises(ValueError):
                state.resume(path, actor.state, "essential", [])


if __name__ == "__main__":
    unittest.main()
