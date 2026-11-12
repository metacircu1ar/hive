"""Live preparation only discovers prerequisites unless installation is explicit."""

import argparse
import contextlib
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from e2e.live.prepare import main
from release.files import atomic_write_json, atomic_write_text, create_directory


class LivePreparation(unittest.TestCase):
    def test_report_only_never_installs_authenticates_or_runs_a_model(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = root / "embedding.json"
            atomic_write_json(
                config, dict(backend="http", url="http://127.0.0.1:8081/v1/embeddings")
            )
            args = SimpleNamespace(cache=root, embedding_config=config, install=False, port=8081)
            with (
                patch("e2e.live.prepare.product_root", return_value=root),
                patch("e2e.live.prepare.shutil.which", return_value=None),
                patch("e2e.live.prepare.run") as native,
                patch("e2e.live.prepare.post", return_value={"status": "ok"}) as http,
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                with self.assertRaises(SystemExit) as status:
                    main(args, argparse.ArgumentParser())
            self.assertEqual(status.exception.code, 1)
            native.assert_not_called()
            http.assert_called_once_with("http://127.0.0.1:8081/health", None, timeout=5)
            report = json.loads(output.getvalue())
            self.assertEqual(
                set(report["missing"]),
                {"hive release binaries", "comb release binaries", "codex", "claude"},
            )
            self.assertIn(["npm", "install", "--global", "@openai/codex"], report["commands"])
            self.assertIn(
                ["npm", "install", "--global", "@anthropic-ai/claude-code"], report["commands"]
            )
            self.assertFalse(report["ready"])

    def test_complete_prerequisites_still_do_not_check_login_or_start_services(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            create_directory(root / "target/release", mode=0o700)
            for name in ("hivectl", "hive-daemon", "comb"):
                atomic_write_text(root / "target/release" / name, "fixture")
            config = root / "embedding.json"
            atomic_write_json(
                config, dict(backend="http", url="http://localhost:8081/v1/embeddings")
            )
            args = SimpleNamespace(cache=root, embedding_config=config, install=False, port=8081)
            with (
                patch("e2e.live.prepare.product_root", return_value=root),
                patch("e2e.live.prepare.shutil.which", return_value="fixture/native"),
                patch("e2e.live.prepare.require_paths") as admit,
                patch("e2e.live.prepare.run") as native,
                patch("e2e.live.prepare.post", return_value={"status": "ok"}),
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                main(args, argparse.ArgumentParser())
            native.assert_not_called()
            self.assertEqual(admit.call_count, 2)
            self.assertTrue(json.loads(output.getvalue())["ready"])
