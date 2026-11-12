"""Native-context identity and changed-HOME asset/export contracts."""
import copy
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from e2e.vm import checks, checks_account as account
from release.process import ProcessFailure, run
from release.files import atomic_write_bytes, create_directory


def context():
    return dict(uid=1001, gid=1002, resuid=[1001] * 3, resgid=[1002] * 3,
                groups=[1002], user=account.NAME, home=account.HOME,
                runtime="/run/user/1001", inherited_session=False,
                capabilities=dict.fromkeys(("CapInh", "CapPrm", "CapEff", "CapBnd", "CapAmb"), 0),
                commit="a" * 40, assets_sha256="b" * 64)


class Accounts(unittest.TestCase):
    def test_suite_temporary_root_is_private_and_in_account_home(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            result = run(
                ["bash", "-euc", checks.SUITE_TMPDIR + '\nprintf "%s" "$TMPDIR"'],
                env={"HOME": str(home), "PATH": os.environ["PATH"], "TMPDIR": "/tmp"},
                text=True, check=True, timeout=5,
            )
            chosen = Path(result.stdout)
            self.assertEqual(chosen.parent, home)
            self.assertTrue(chosen.is_dir())
            self.assertEqual(chosen.stat().st_mode & 0o777, 0o700)
            self.assertEqual(chosen.stat().st_dev, home.stat().st_dev)

    def test_privilege_and_session_mismatches_are_refused(self):
        original = context()
        self.assertEqual(account.validate_context(original), original)
        for key, value in (
            ("uid", 0), ("resuid", [0, 1001, 0]), ("resgid", [1002, 0, 1002]),
            ("groups", [1002, 0]), ("home", "/home/lima"),
            ("runtime", "/run/user/501"), ("inherited_session", True),
            ("capabilities", {"CapEff": 0}),
            ("capabilities", dict(original["capabilities"], CapBnd=1)),
        ):
            with self.subTest(field=key, value=value):
                changed = copy.deepcopy(original)
                changed[key] = value
                with self.assertRaisesRegex(ValueError, "unprivileged account"):
                    account.validate_context(changed)

    def test_preparation_stages_the_exact_existing_assets_before_changing_home(self):
        calls = []
        expected = context()

        def guest_run(name, script, **options):
            calls.append((name, script, options))
            return {}, subprocess.CompletedProcess([], 0, "HIVE_VM_JSON " + json.dumps(expected), "")

        campaign = SimpleNamespace(
            state=dict(commit="a" * 40, units=dict(package=dict(assets="$HOME/assets-container"))),
            guest=SimpleNamespace(value=lambda *a: "/home/lima", run=guest_run),
        )
        result = account.prepare(campaign)
        self.assertEqual(result["origin_home"], "/home/lima")
        self.assertEqual(calls[0][2]["args"], ("/home/lima", "/home/lima/assets-container", "a" * 40))
        self.assertTrue(calls[0][2]["root"])
        self.assertIn('cp -a "$assets" "$home/assets"', calls[0][1])
        self.assertNotIn("build_release", calls[0][1])
        campaign.state["units"]["package"]["assets"] = "$HOME/unknown"
        with self.assertRaisesRegex(ValueError, "Unknown packaged"):
            account.prepare(campaign)
        self.assertEqual(len(calls), 1)

    def test_installed_checks_use_the_staged_account_assets_and_record_context(self):
        calls = []
        expected = context()

        def isolated(campaign, label, script, **options):
            calls.append((label, script, options))
            return {"log": "fixture.log"}, subprocess.CompletedProcess(
                [], 0, 'HIVE_VM_JSON {"wizard": {"seconds": 1}}', ""
            ), expected

        campaign = SimpleNamespace(attempt=1)
        with patch.object(account, "run", side_effect=isolated):
            result = checks.Campaign.release_install(campaign, "stock")
        self.assertTrue(result["passed"])
        self.assertEqual(result["execution_context"], expected)
        self.assertIn('--assets "$HOME/assets"', calls[0][1])
        self.assertNotIn("assets-container", calls[0][1])
        self.assertEqual(calls[0][2]["timeout"], 3600)

    def test_suite_export_uses_checked_root_staging_from_the_private_account(self):
        calls = []
        def guest_run(*args, **options):
            calls.append((args, options))
            return {}, subprocess.CompletedProcess([], 0, "HIVE_VM_SUITE_OUTPUT_EXPORTED", "")

        campaign = SimpleNamespace(guest=SimpleNamespace(run=guest_run))
        account.collect(campaign, dict(origin_home="/home/lima"), "dist/vm-suite-1", verify_assets=True)
        self.assertTrue(calls[0][1]["root"])
        self.assertEqual(calls[0][1]["args"], ("/home/lima", "dist/vm-suite-1", account.HOME))
        self.assertIn('source="$home/src/hive/$relative"', calls[0][0][1])
        self.assertIn('find "$source" -type l', calls[0][0][1])
        self.assertIn('cp -a "$source" "$destination"', calls[0][0][1])
        self.assertNotIn("root", calls[1][1])
        self.assertIn("asset_set.load", calls[1][0][1])
        for relative in ("../elsewhere", "dist/vm-suite-0", "dist/vm-suite-1/../../etc"):
            with self.assertRaisesRegex(ValueError, "exact suite"):
                account.collect(campaign, dict(origin_home="/home/lima"), relative, verify_assets=True)
        self.assertEqual(len(calls), 2)

    def test_actual_export_preserves_files_and_modes_and_rejects_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            origin, home = root / "origin", root / "account"
            relative = "dist/vm-suite-1"
            create_directory(home, mode=0o700)
            create_directory(origin / "src/hive/dist")
            source = home / "src/hive" / relative
            create_directory(source / "logs", mode=0o700)
            atomic_write_bytes(source / "logs/native.stderr", b"native failure", mode=0o600)
            atomic_write_bytes(source / "executable", b"program bytes", mode=0o755)
            done = run(["bash", "-s", "--", str(origin), relative, str(home)],
                       input=account.COLLECT, text=True, check=True, timeout=5)
            self.assertEqual(done.stdout.strip(), "HIVE_VM_SUITE_OUTPUT_EXPORTED")
            destination = origin / "src/hive" / relative
            for name, mode, data in (("logs/native.stderr", 0o600, b"native failure"),
                                     ("executable", 0o755, b"program bytes")):
                path = destination / name
                self.assertEqual(path.read_bytes(), data)
                self.assertEqual(path.stat().st_mode & 0o777, mode)
                self.assertEqual(path.stat().st_uid, os.getuid())
            absent = run(["bash", "-s", "--", str(origin), "dist/vm-suite-2", str(home)],
                         input=account.COLLECT, text=True, check=True, timeout=5)
            self.assertEqual(absent.stdout.strip(), "HIVE_VM_NO_SUITE_OUTPUT")
            linked = home / "src/hive/dist/vm-suite-3"
            create_directory(linked)
            (linked / "escape").symlink_to(source / "executable")
            refused = run(["bash", "-s", "--", str(origin), "dist/vm-suite-3", str(home)],
                          input=account.COLLECT, timeout=5)
            self.assertNotEqual(refused.returncode, 0)
            self.assertFalse((origin / "src/hive/dist/vm-suite-3").exists())

    def test_failed_unmarked_workflow_still_exports_its_real_diagnostic(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            origin, home = root / "origin", root / "account"
            source = home / "src/hive/dist/vm-suite-1"
            create_directory(source / "logs")
            create_directory(origin / "src/hive/dist")
            atomic_write_bytes(source / "logs/failed.stderr", b"original native error", mode=0o600)
            expected = dict(context(), origin_home=str(origin))

            def guest_run(name, script, **options):
                if name != "checks-export":
                    self.fail("Failed partial output must not be treated as accepted assets")
                args = (str(origin), options["args"][1], str(home))
                done = run(["bash", "-s", "--", *args], input=script,
                           text=True, check=True, timeout=5)
                return {}, done

            campaign = SimpleNamespace(
                attempt=1, args=SimpleNamespace(instance="fixture"), out=root,
                state={"units": {"package": {"source": "accepted native CI assets"}}},
                guest=SimpleNamespace(run=guest_run),
            )
            failed = subprocess.CompletedProcess([], 1, "", "original native error")
            with (patch.object(checks, "current", return_value=SimpleNamespace(state={"config": {"arch": "aarch64"}})),
                  patch.object(account, "run", return_value=({"log": "native.log"}, failed, expected)) as execute,
                  patch.object(checks, "host_run") as copy_to_host):
                result = checks.Campaign.suite_comparison(campaign)
            self.assertFalse(result["passed"])
            self.assertEqual(result["returncode"], 1)
            self.assertIn("installed_assets=Path.home() / 'assets'", execute.call_args.args[2])
            self.assertIsNone(result["workflow"])
            self.assertEqual(result["retained_logs"], "guest-suite-1")
            self.assertEqual((origin / "src/hive/dist/vm-suite-1/logs/failed.stderr").read_bytes(),
                             b"original native error")
            self.assertEqual(copy_to_host.call_count, 1)

    def test_export_failure_never_replaces_the_native_workflow_failure(self):
        expected = dict(context(), origin_home="/home/lima")
        failed = subprocess.CompletedProcess([], 9, "", "original native failure")
        secondary = RuntimeError("fixture export failure")
        campaign = SimpleNamespace(attempt=1, state={})
        with (patch.object(checks, "current", return_value=SimpleNamespace(state={"config": {"arch": "aarch64"}})),
              patch.object(account, "run", return_value=({"log": "native.log"}, failed, expected)) as execute,
              patch.object(account, "collect", side_effect=secondary),
              self.assertRaises(ProcessFailure) as raised):
            checks.Campaign.suite_comparison(campaign)
        self.assertIs(raised.exception.result, failed)
        self.assertEqual(raised.exception.cleanup_errors, (secondary,))
        self.assertNotIn("installed_assets=", execute.call_args.args[2])

    def test_root_payloads_have_valid_native_shell_syntax(self):
        for body in (account.PREPARE, account.COLLECT):
            run(["bash", "-n"], input=body, check=True, timeout=5)

    def test_execution_drops_all_capability_sets_and_inherited_environment(self):
        expected = dict(context(), origin_home="/home/lima")
        guest = SimpleNamespace(run=lambda *a, **kw: (dict(log="fixture.log"), SimpleNamespace(returncode=0)))
        with patch.object(account, "prepare", return_value=expected), patch.object(guest, "run") as invoke:
            invoke.return_value = ({"log": "fixture.log"}, SimpleNamespace(returncode=0))
            account.run(SimpleNamespace(guest=guest), "fixture", "printf ready", timeout=5)
        body = invoke.call_args.args[1]
        run(["bash", "-n"], input=body, check=True, timeout=5)
        for required in ("--bounding-set=-all", "--inh-caps=-all", "--ambient-caps=-all", "--no-new-privs", "env -i", "RUST_TEST_THREADS=4"):
            self.assertIn(required, body)
        self.assertTrue(invoke.call_args.kwargs["root"])
        self.assertFalse(invoke.call_args.kwargs["require"])


if __name__ == "__main__":
    unittest.main()
