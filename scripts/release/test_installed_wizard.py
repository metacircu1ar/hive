import hashlib
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from .files import atomic_write_bytes, create_directory
from .installed_wizard import InstalledWizard


class InstalledRemovalTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        home = Path(temporary.name)
        self.wizard = InstalledWizard(SimpleNamespace(env={}), home, "unused")
        w = self.wizard
        w.prefix = home / "prefix"
        for name in ("bin", "share", "lib/hive/generations/fixture/bin"):
            create_directory(w.prefix / name)
        w.installer = w.prefix / "bin/hive-install"
        w.retained_installer = w.prefix / "lib/hive/generations/fixture/bin/hive-install"
        helper = b"fixture installed helper"
        atomic_write_bytes(w.installer, helper)
        atomic_write_bytes(w.retained_installer, helper)
        w.owner.identities = {
            "hive-install": {"bytes": len(helper), "sha256": hashlib.sha256(helper).hexdigest()}
        }
        w.config, w.bridge, w.root, w.state = (
            home / "config.yaml",
            home / "bridge.json",
            home / "knowledge",
            home / "state",
        )
        page, profile = w.root / "concepts/wizard.md", w.state / "profiles/mock"
        create_directory(page.parent)
        create_directory(profile.parent)
        w.retained = {
            w.config: b"fixture config",
            w.bridge: b"fixture bridge",
            page: b"retained knowledge",
            profile: b"retained mock auth",
        }
        for path, data in w.retained.items():
            atomic_write_bytes(path, data)
        w.remove = ["installed-uninstall"]
        w.purge = ["installed-purge"]
        w.retry_purge = ["bootstrap"]
        w.log = home / "removal.log"
        self.calls = []

    def clear_payload(self):
        w = self.wizard
        w.config.unlink()
        w.bridge.unlink()
        shutil.rmtree(w.prefix / "lib/hive")

    def execute(self, first_purge, bootstrap):
        def invoke(command, **_options):
            self.calls.append(command[0])
            if command == self.wizard.remove:
                self.wizard.installer.unlink()
            elif command == self.wizard.purge:
                first_purge()
            elif command == self.wizard.retry_purge:
                bootstrap()
            else:
                self.fail("Unexpected removal command")
            return subprocess.CompletedProcess(command, 0)

        with patch("release.installed_wizard.run_process", side_effect=invoke):
            self.wizard.remove_selected()

    def test_successful_noop_installed_purge_cannot_be_repaired_by_bootstrap(self):
        with self.assertRaisesRegex(RuntimeError, "Full purge left selected payload"):
            self.execute(lambda: None, self.clear_payload)
        self.assertEqual(self.calls, ["installed-uninstall", "installed-purge"])

    def test_installed_purge_proves_removal_and_retention_before_idempotent_retry(self):
        self.execute(self.clear_payload, lambda: None)
        self.assertEqual(self.calls, ["installed-uninstall", "installed-purge", "bootstrap"])

    def test_corrupt_retained_data_cannot_be_repaired_by_bootstrap(self):
        profile = self.wizard.state / "profiles/mock"

        def corrupt():
            self.clear_payload()
            atomic_write_bytes(profile, b"corrupt")

        def repair():
            atomic_write_bytes(profile, self.wizard.retained[profile])

        with self.assertRaisesRegex(RuntimeError, "Purge changed retained knowledge/auth"):
            self.execute(corrupt, repair)
        self.assertEqual(self.calls, ["installed-uninstall", "installed-purge"])
