"""Opt-in selection never starts a live model or installs a prerequisite implicitly."""

import contextlib
import io
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

from e2e import prepare, run


class EntryTests(unittest.TestCase):
    def test_selected_driver_failure_status_is_preserved(self):
        argv = ["run.py", "compare"]
        fake = SimpleNamespace(main=lambda: 7)
        with (
            patch.object(run.sys, "argv", argv),
            patch.object(run.importlib, "import_module", return_value=fake),
        ):
            with self.assertRaises(SystemExit) as raised:
                run.main()
            self.assertEqual(raised.exception.code, 7)
            self.assertIs(run.sys.argv, argv)

    def test_cpu_prepare_refuses_small_memory_before_creating_or_downloading_cache(self):
        from e2e.models import llama, qwen3
        from e2e import cache

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            release = root / "target/release"
            release.mkdir(parents=True)
            (release / "comb").touch()
            selected = root / "absent-cache"
            with (
                patch.object(prepare, "product_root", return_value=root),
                patch.object(prepare, "require_paths"),
                patch.object(prepare.sys, "platform", "linux"),
                patch.object(prepare.shutil, "which", return_value="/usr/bin/tool"),
                patch.object(llama, "installed", return_value=None),
                patch.object(qwen3, "available_memory", return_value=8 * 1024**3),
                patch.object(cache, "fetch") as fetch,
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                with self.assertRaises(SystemExit):
                    prepare.main(["--tier", "embeddings", "--cache", str(selected)])
                fetch.assert_not_called()
                self.assertIn("16 GiB", output.getvalue())
                self.assertFalse(selected.exists())

    def test_default_is_small_mock_and_preserves_original_argv(self):
        seen = []
        argv = ["run.py"]
        fake = SimpleNamespace(main=lambda: seen.append(list(run.sys.argv)))
        with (
            patch.object(run.sys, "argv", argv),
            patch.object(run.importlib, "import_module", return_value=fake) as load,
        ):
            run.main()
            self.assertIs(run.sys.argv, argv)
        load.assert_called_once_with("e2e.scripts.run_mock_ping_pong_pair")
        self.assertEqual(seen, [["run_mock_ping_pong_pair", "--agents", "100"]])

    def test_prepare_reports_missing_build_without_running_cargo(self):
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(prepare, "product_root", return_value=Path(temp)),
        ):
            with (
                patch.object(prepare, "run") as child,
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                with self.assertRaises(SystemExit):
                    prepare.main([])
                child.assert_not_called()
                self.assertIn("cargo build --release --locked -p hive", output.getvalue())

    def test_help_never_runs_a_scenario(self):
        with (
            patch.object(run.sys, "argv", ["run.py", "--help"]),
            patch.object(run.importlib, "import_module") as load,
        ):
            with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as result:
                run.main()
            self.assertEqual(result.exception.code, 0)
            load.assert_not_called()

    def test_prepare_admits_release_control_and_requires_its_daemon_sibling(self):
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(prepare, "product_root", return_value=Path(temp)),
            patch.object(prepare, "require_paths") as admit,
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            release = Path(temp) / "target/release"
            release.mkdir(parents=True)
            for name in ("hivectl", "hive-daemon"):
                (release / name).touch()
            prepare.main([])
            self.assertIn('"ready": true', output.getvalue())
            admit.assert_called_once()
            self.assertEqual(admit.call_args.args[0].binary, release / "hivectl")
