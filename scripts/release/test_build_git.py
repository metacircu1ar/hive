"""Real Git filename and ignore handling must agree with the compiled source tree."""

from functools import partial
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from . import common
from .source import Snapshot


class BuildGitTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.git = partial(subprocess.check_output, cwd=self.root, stderr=subprocess.PIPE)
        self.git(["git", "init", "--quiet"])
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("mod new;\n")
        (self.root / ".gitignore").write_text("src/new.rs\n")
        (self.root / "src/carriage\rreturn.rs").write_text("original input")
        self.git(["git", "add", "."])
        self.git(
            [
                "git",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ]
        )

    def test_ignored_source_is_hashed_and_later_mutation_refuses_attribution(self):
        ignored = self.root / "src/new.rs"
        ignored.write_text("pub fn value() -> u8 { 1 }\n")
        with patch.object(common, "ROOT", self.root):
            snapshot = Snapshot(self.root, common.run)
            self.assertEqual(snapshot.records["src/new.rs"]["kind"], "file")
            ignored.write_text("pub fn value() -> u8 { 2 }\n")
            with self.assertRaisesRegex(ValueError, "Source changed"):
                snapshot.check()

    def test_carriage_return_filename_is_not_translated_to_a_different_path(self):
        with patch.object(common, "ROOT", self.root):
            snapshot = Snapshot(self.root, common.run)
            self.assertEqual(snapshot.records["src/carriage\rreturn.rs"]["kind"], "file")
            (self.root / "src/carriage\rreturn.rs").write_text("changed input")
            with self.assertRaisesRegex(ValueError, "Source changed"):
                snapshot.check()
