"""Native build failures retain private bounded stderr without diagnostic disclosure."""

from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from .common import run
from .native_evidence import evidence
from .progress import entry, Session
from . import progress
from . import files


class NativeEvidenceTests(unittest.TestCase):
    def test_native_stderr_does_not_replace_publication_retention_metadata(self):
        import sys

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target, retained = root / "output", root / "saved"

            def replace_preparation(stream):
                stream.write(b"original preparation")
                stream.flush()
                preparation = next(root.glob(".hive-tmp-*"))
                preparation.rename(retained)
                preparation.write_bytes(b"foreign replacement")

            with self.assertRaises(files.PublicationError) as caught:
                with evidence(root / "diagnostics"):
                    run([sys.executable, "-c", "import sys;sys.stderr.write('native detail')"])
                    files.atomic_stream(target, replace_preparation)
            failure = caught.exception
            self.assertFalse(failure.published)
            self.assertEqual(Path(failure.retained).read_bytes(), b"foreign replacement")
            self.assertEqual(retained.read_bytes(), b"original preparation")
            self.assertEqual(Path(failure.native_stderr).read_bytes(), b"native detail")
            self.assertNotEqual(failure.retained, failure.native_stderr)

    def test_actual_locked_release_compiler_error_is_retained_and_redacted(self):
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryFile() as diagnostics:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname="stderr-fixture"\nversion="0.0.0"\nedition="2024"\n'
            )
            (root / "Cargo.lock").write_text(
                'version = 4\n[[package]]\nname = "stderr-fixture"\nversion = "0.0.0"\n'
            )
            (root / "src/main.rs").write_text(
                'fn main() { compile_error!("PRIVATE_NATIVE_STDERR"); }'
            )

            def work():
                with evidence(root / "diagnostics"):
                    run(
                        [
                            "cargo",
                            "build",
                            "--release",
                            "--locked",
                            "--offline",
                            "--manifest-path",
                            str(root / "Cargo.toml"),
                            "--target-dir",
                            str(root / "target"),
                        ],
                        timeout=15,
                    )

            with patch.object(
                progress, "Session", side_effect=lambda: Session(fd=diagnostics.fileno())
            ):
                self.assertEqual(entry(work, name="build"), 1)
            paths = list((root / "diagnostics").glob("*.stderr"))
            self.assertEqual(len(paths), 1)
            self.assertIn(b"PRIVATE_NATIVE_STDERR", paths[0].read_bytes())
            self.assertEqual(paths[0].stat().st_mode & 0o777, 0o600)
            diagnostics.seek(0)
            text = diagnostics.read()
            self.assertNotIn(b"PRIVATE_NATIVE_STDERR", text)
            self.assertNotIn(str(root).encode(), text)

    def test_evidence_budget_is_checked_and_does_not_hide_a_native_failure(self):
        import sys
        from .process import ProcessFailure

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(ProcessFailure) as caught:
                with evidence(root, limit=4):
                    run(
                        [
                            sys.executable,
                            "-c",
                            "import sys;sys.stderr.write('long failure');sys.exit(17)",
                        ]
                    )
            self.assertEqual(caught.exception.result.returncode, 17)
            self.assertTrue(caught.exception.cleanup_errors)
