"""Resume prerequisites, final host acceptance and fallback admission contracts."""

import contextlib
import copy
import io
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from e2e.campaign import Admission, read_receipt
from e2e.scripts import vm_validation as vm
from e2e.vm import assets, checks, state
from e2e.vm.folder import Folder, delete as delete_vm
from e2e.vm.lima import Lima
from e2e.tests.test_vm_contracts import Transport, actor_fixture
from release.cancellation import Cancelled
from release.files import atomic_write_bytes, atomic_write_json, create_directory
from release.progress import entry
from release.process import run
from release.test_package_fixture import PackageFixture


class Prerequisites(unittest.TestCase):
    def test_scoped_remedy_does_not_replace_stock_distribution_profiles(self):
        with actor_fixture() as (actor, _):
            saved = state.fresh(actor.state)
            saved["baseline_profiles"] = {"bwrap": "enforce", "unpriv_bwrap": "enforce"}
            saved["baseline_policy_files"] = []
            saved["units"]["host-truth"] = dict(
                preflight=dict(
                    sysctls={"kernel.apparmor_restrict_unprivileged_userns": "1"},
                    profiles=saved["baseline_profiles"],
                ),
                lima_additions=dict(unowned_apparmor_files=[]),
            )
            events = []

            def guest_run(name, script, **options):
                events.append((name, options.get("args")))
                return {}, subprocess.CompletedProcess([], 0, "", "")

            with (
                patch.object(checks, "current", return_value=actor),
                patch.object(checks, "Guest", return_value=SimpleNamespace(run=guest_run)),
                patch.object(checks, "guest_script", return_value="fixture policy"),
                patch.object(checks.Campaign, "service_state", return_value={}),
            ):
                campaign = checks.Campaign(SimpleNamespace(), actor.output, saved)
                campaign.user = "tester"
                campaign.prerequisites("user")
            self.assertIn(("user-restore-scoped", ("user-stock", "tester")), events)
            self.assertFalse(any(name == "user-restore-distribution" for name, _ in events))
            self.assertFalse(saved["policy_owned"]["distribution"])
            self.assertTrue(saved["policy_owned"]["user"])

    def test_source_copy_and_clone_payload_has_valid_native_shell_syntax(self):
        with actor_fixture() as (actor, _):
            actor.create()
            scripts = {}

            def guest_run(name, script, **options):
                scripts[name] = script

            guest = SimpleNamespace(run=guest_run, value=lambda *a, **kw: "/home/tester")
            bundle = actor.output / "lima.yaml"
            with (
                patch.object(checks, "current", return_value=actor),
                patch.object(checks, "Guest", return_value=guest),
                patch.object(checks, "host_run") as copy_bundle,
                patch("e2e.scripts.local_ci.bundle_head", return_value=bundle),
            ):
                campaign = checks.Campaign(
                    SimpleNamespace(instance=actor.name), actor.output, state.fresh(actor.state)
                )
                result = campaign.sources()
            self.assertEqual(set(scripts), {"sources-dir", "sources-clone"})
            self.assertEqual(result["commit"], actor.state["source_commit"])
            self.assertIn("git clone -q", scripts["sources-clone"])
            self.assertIn(actor.state["source_commit"], scripts["sources-clone"])
            copy_bundle.assert_called_once_with(
                ["limactl", "copy", str(bundle), f"{actor.name}:/home/tester/src/head.bundle"],
                timeout=900,
            )
            for name, script in scripts.items():
                with self.subTest(step=name):
                    run(["bash", "-n"], input=script, check=True, timeout=5)

    def test_probe_preserves_post_provision_profile_comparison(self):
        campaign = SimpleNamespace(guest=object(), user=None)
        with (
            patch.object(checks, "provision", return_value=("tester", {"stock": "enforce"})),
            patch.object(vm, "host_truth", return_value=dict(profiles={})),
            patch.object(checks, "probe") as probe,
        ):
            with self.assertRaisesRegex(ValueError, "profile set changed"):
                vm.probe_work(campaign)
            self.assertFalse(probe.called)

    def test_user_revert_failure_still_reverts_distribution_and_stops_vm(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            actor.start()
            actor.admission.complete()
            saved = state.fresh(actor.state)
            saved["policy_owned"] = dict(user=True, distribution=True)
            events = []

            def guest_run(name, script, **options):
                action = options.get("args", ("service",))[0]
                events.append(action)
                if action == "user-revert":
                    raise RuntimeError("fixture user policy failure")
                return {}, subprocess.CompletedProcess([], 0, "", "")

            with (
                patch.object(checks, "current", return_value=actor),
                patch.object(checks, "Guest", return_value=SimpleNamespace(run=guest_run)),
                patch.object(checks, "guest_script", return_value="fixture"),
            ):
                campaign = checks.Campaign(SimpleNamespace(), actor.output, saved)
                campaign.user = "tester"
                with self.assertRaises(RuntimeError), actor.held("fixture-policy-cleanup"):
                    actor.admission.advance("executing")
                    vm.cleanup_guest(campaign)
                self.assertEqual(events[-2:], ["user-revert", "distribution-revert"])
                self.assertEqual(saved["policy_owned"], dict(user=True, distribution=False))
                self.assertEqual(transport.row["status"], "Stopped")
                self.assertIsNone(actor.admission.active)

    def test_stopped_partial_distribution_then_user_resume_restores_operational_state(self):
        with actor_fixture() as (actor, _):
            saved = state.fresh(actor.state)
            saved["baseline_profiles"], saved["baseline_policy_files"] = {}, []
            saved["configs"]["distribution"] = {"immutable": "prior evidence"}
            prior = copy.deepcopy(saved["configs"])
            events = []
            fail_user = False

            def guest_run(name, script, **options):
                events.append((name, options.get("args")))
                if fail_user and options.get("args", (None,))[0] == "user":
                    raise RuntimeError("fixture interrupted policy application")
                return {}, subprocess.CompletedProcess([], 0, "", "")

            guest = SimpleNamespace(run=guest_run)
            with (
                patch.object(checks, "current", return_value=actor),
                patch.object(checks, "Guest", return_value=guest),
                patch.object(checks, "guest_script", return_value="fixture policy"),
                patch.object(checks.Campaign, "service_state", return_value={"returncode": 0}),
            ):
                campaign = checks.Campaign(SimpleNamespace(), actor.output, saved)
                campaign.user = "tester"
                campaign.prerequisites("distribution")
                vm.cleanup_guest(campaign)
                self.assertEqual(saved["policy_owned"], dict(distribution=False, user=False))
                self.assertTrue(any(name == "cleanup-service" for name, _ in events))
                events.clear()
                resumed = checks.Campaign(SimpleNamespace(), actor.output, saved)
                resumed.user = "tester"
                resumed.prerequisites("user")
                self.assertEqual([args[0] for _, args in events if args], ["distribution", "user"])
                self.assertIn("user-restore-service", [name for name, _ in events])
                self.assertEqual(saved["configs"], prior)
                fail_user = True
                with self.assertRaises(RuntimeError):
                    resumed.prerequisites("user")
                self.assertTrue(saved["policy_owned"]["user"])
                fail_user = False
                vm.cleanup_guest(resumed)
                self.assertEqual(saved["policy_owned"], dict(distribution=False, user=False))
                resumed.prerequisites("user")
                self.assertEqual(saved["configs"], prior)

    def test_extra_suite_scripts_are_valid_python_and_only_explicit_model_download(self):
        with actor_fixture() as (actor, _):
            actor.state["memory_gib"] = 20
            scripts = []

            def guest_run(name, script, **options):
                scripts.append((name, script, options))
                payload = dict(passed=True)
                return {"log": "fixture.log"}, subprocess.CompletedProcess(
                    [], 0, "HIVE_VM_JSON " + json.dumps(payload), ""
                )

            guest = SimpleNamespace(run=guest_run, value=lambda *a, **kw: "/home/tester")
            saved = state.fresh(actor.state)

            def account_run(campaign, name, script, **options):
                options.pop("rust", None)
                record, done = guest_run(name, script, **options)
                return record, done, {"origin_home": "/home/tester"}

            with (
                patch.object(checks, "current", return_value=actor),
                patch.object(checks, "Guest", return_value=guest),
                patch.object(checks, "host_run"),
                patch("e2e.vm.checks_account.run", side_effect=account_run),
                patch("e2e.vm.checks_account.collect", return_value=True),
                patch("e2e.vm.context.free_bytes", return_value=100 * 1024**3),
            ):
                campaign = checks.Campaign(
                    SimpleNamespace(instance=actor.name), actor.output, saved
                )
                self.assertTrue(campaign.suite_comparison()["passed"])
                self.assertTrue(campaign.embedding_smoke()["passed"])
            for name, script, options in scripts:
                body = script.split("<<'EOF'\n", 1)[1].split("\nEOF", 1)[0]
                compile(body, name, "exec")
                self.assertNotIn("target/debug", script)
                self.assertGreater(options["timeout"], 0)
            model_script = scripts[-1][1]
            self.assertIn("prepare --tier embeddings --install", model_script)
            self.assertIn("python3 -m e2e.vm.embedding", model_script)
            self.assertIn(checks.SUITE_TMPDIR, scripts[0][1])
            self.assertIn("python3 e2e/run.py unit", scripts[0][1])


class FinalHostAcceptance(unittest.TestCase):
    def test_create_resume_after_delete_refuses_without_overwriting_accepted_report(self):
        with actor_fixture() as (actor, _):
            actor.create()
            actor.state["mode"] = "create"
            actor.save()
            atomic_write_json(actor.output / "state.json", state.fresh(actor.state))
            report = actor.output / "results.json"
            atomic_write_json(
                report,
                dict(host_completed=True, validation="vm-create", retained=dict(name=actor.name)),
            )
            original = report.read_bytes()
            delete_vm(actor.admission, actor.name, home=actor.home)

            @contextlib.contextmanager
            def held(**options):
                yield actor.admission

            args = vm.parse_args(["create", "--resume", "--output-dir", str(actor.output)])
            with (
                patch.object(vm, "admit_host"),
                patch.object(vm, "Admission", return_value=actor.admission),
                patch.object(actor.admission, "held", side_effect=held),
                patch.object(vm, "selected_head", return_value=actor.state["source_commit"]),
                patch.object(vm, "default_home", return_value=actor.home),
                patch.object(vm, "guard"),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(entry(lambda: vm.run_campaign(args), name="verify"), 1)
            self.assertEqual(report.read_bytes(), original)
            self.assertIsNone(read_receipt(actor.admission.root))

    def test_only_complete_passing_non_keep_run_deletes_before_acceptance(self):
        for outcome in (
            "pass",
            "fail",
            "keep",
            "partial",
            "cancel",
            "uncertain",
            "step-fail",
            "guard-fail",
        ):
            with (
                self.subTest(outcome=outcome),
                tempfile.TemporaryDirectory(prefix="hv-", dir="/tmp") as temporary,
            ):
                root = Path(temporary).resolve()
                actors, transports = [], []

                def factory(admission, output, value):
                    actor = Lima(admission, output, value)
                    transport = Transport(actor)
                    if outcome == "uncertain":
                        actor.retire = lambda: (_ for _ in ()).throw(
                            RuntimeError("fixture cleanup unknown")
                        )
                    actors.append(actor)
                    transports.append(transport)
                    return actor

                def execute(actor, args, saved):
                    saved["units"]["native-stock"] = dict(passed=outcome != "fail")
                    if outcome == "cancel":
                        import signal

                        raise Cancelled(signal.SIGTERM)
                    if outcome == "step-fail":
                        raise RuntimeError("fixture guest build failed")

                args = vm.parse_args(
                    [
                        "run",
                        "--output-dir",
                        str(root / "out"),
                        *(["--keep"] if outcome == "keep" else []),
                    ]
                )

                def evaluate(args, saved):
                    return (
                        outcome == "partial",
                        None if outcome == "partial" else outcome != "fail",
                        [],
                    )

                def prepared_workflow(output, saved, result):
                    self.assertIsNone(actors[0].admission.active)
                    self.assertIsNone(actors[0].admission.lock_fd)
                    self.assertTrue(result["cleanup"]["confirmed"])
                    if outcome == "pass":
                        return output / "fixture-ci.json", b"validated fixture workflow\n"
                    return None

                with (
                    patch.object(vm, "admit_host"),
                    patch.object(vm, "default_home", return_value=root / "lima"),
                    patch.object(
                        vm, "Admission", side_effect=lambda: Admission(root / "admission")
                    ),
                    patch.object(vm, "selected_head", return_value="b" * 40),
                    patch.object(vm, "image_path", return_value=root / "image.img"),
                    patch.object(
                        vm,
                        "guard",
                        side_effect=RuntimeError("fixture host is busy")
                        if outcome == "guard-fail"
                        else None,
                    ),
                    patch.object(vm, "execute", side_effect=execute),
                    patch.object(vm, "evaluate", side_effect=evaluate),
                    patch.object(vm.assets, "prepare_workflow", side_effect=prepared_workflow),
                    patch.object(vm, "Lima", side_effect=factory),
                    patch(
                        "e2e.vm.native.client.run",
                        side_effect=lambda argv, **options: transports[0].native(argv, **options),
                    ),
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    status = entry(lambda: vm.run_campaign(args), name="scenario")
                accepted = root / "out/results.json"
                self.assertEqual((root / "out/fixture-ci.json").exists(), outcome == "pass")
                self.assertEqual(
                    status,
                    143
                    if outcome == "cancel"
                    else 1
                    if outcome in ("fail", "uncertain", "step-fail", "guard-fail")
                    else 0,
                )
                self.assertEqual(
                    any(call[0] == "delete" for call in transports[0].calls), outcome == "pass"
                )
                self.assertEqual(
                    accepted.exists(),
                    outcome not in ("cancel", "uncertain", "step-fail", "guard-fail"),
                )
                resource = json.loads((root / "out/retained.json").read_text())
                self.assertFalse(resource["accepted"])
                with patch("e2e.vm.native.client.run", side_effect=transports[0].native):
                    discovered = Folder(home=root / "lima").listing()
                self.assertEqual(len(discovered), 0 if outcome in ("pass", "guard-fail") else 1)
                if discovered:
                    self.assertIn(actors[0].name, discovered[0]["delete_command"])
                if outcome in ("guard-fail", "keep"):
                    (root / "out").rename(root / "original-output")
                    create_directory(root / "out", mode=0o700)
                    atomic_write_json(root / "out/owner.json", {"foreign": True})
                    with patch("e2e.vm.native.client.run", side_effect=transports[0].native):
                        rows = Folder(home=root / "lima").listing()
                    if outcome == "guard-fail":
                        self.assertEqual(rows, [])
                        self.assertIsNone(resource["retained"])
                    else:
                        self.assertTrue(rows[0]["owned"])
                        self.assertEqual(rows[0]["name"], actors[0].name)
                if accepted.exists():
                    report = json.loads(accepted.read_text())
                    self.assertTrue(report["host_completed"])
                    self.assertEqual(report["retained"] is None, outcome == "pass")
                    self.assertTrue(report["cleanup"]["confirmed"])


class FallbackAssets(PackageFixture, unittest.TestCase):
    def linux_parts(self):
        self.record["target"] = "aarch64-unknown-linux-gnu"
        deps = dict(libraries=["libc.so.6"], required_glibc="2.35")
        for binary in self.record["binaries"].values():
            binary["dependencies"] = deps
        self.save()
        with (
            contextlib.redirect_stdout(io.StringIO()),
            patch("package_release.native_target", return_value=self.record["target"]),
            patch("package_release.inspect_binary", return_value=deps),
        ):
            self.all_parts()

    def test_only_accepted_native_exact_source_file_set_and_release_parts_are_consumed(self):
        self.linux_parts()
        report = dict(
            host_completed=True,
            passed=True,
            profile="release",
            validation="distribution",
            platform="linux-arm64",
            commit=self.record["source"]["commit"],
            failed_steps=[],
            assets=sorted(path.name for path in self.output.iterdir()),
        )
        report_path = self.output.parent / "results.json"
        atomic_write_json(report_path, report)
        self.assertEqual(assets.accepted(self.output, "arm64")["commit"], report["commit"])
        for key, value in (
            ("host_completed", False),
            ("passed", False),
            ("profile", "debug"),
            ("validation", "essential"),
            ("platform", "linux-amd64"),
            ("commit", "c" * 40),
            ("assets", report["assets"][:-1]),
        ):
            with self.subTest(key=key):
                atomic_write_json(report_path, dict(report, **{key: value}))
                with self.assertRaises(ValueError):
                    assets.accepted(self.output, "arm64")
        report_path.unlink()
        with self.assertRaises(FileNotFoundError):
            assets.accepted(self.output, "arm64")

    def test_ci_export_preserves_modes_required_by_vm_fallback(self):
        from e2e.ci.runner import copy_tree

        self.linux_parts()
        destination = self.root / "exported/assets"
        create_directory(destination, mode=0o700)
        copy_tree(self.output, destination)
        report = dict(
            host_completed=True,
            passed=True,
            profile="release",
            validation="distribution",
            platform="linux-arm64",
            commit=self.record["source"]["commit"],
            failed_steps=[],
            assets=sorted(path.name for path in destination.iterdir()),
        )
        atomic_write_json(destination.parent / "results.json", report)
        self.assertEqual(assets.accepted(destination, "arm64")["commit"], report["commit"])
        for source in self.output.iterdir():
            copied = destination / source.name
            self.assertEqual(copied.read_bytes(), source.read_bytes())
            self.assertEqual(copied.stat().st_mode & 0o777, source.stat().st_mode & 0o777)

    def test_vm_finalization_preserves_real_workflow_and_validates_copied_parts(self):
        output = self.root / "vm"
        self.output = output / "guest-suite-1/assets"
        create_directory(self.output.parent, mode=0o700)
        self.linux_parts()
        workflow = dict(
            passed=True, profile="release", validation="ci", platform="linux-arm64",
            commit=self.record["source"]["commit"], failed_steps=[],
            assets=sorted(path.name for path in self.output.iterdir()),
            steps=[dict(variant="hive", step="tests", returncode=0)],
        )
        result = dict(
            passed=True, partial=False, profile="release", validation="vm-full",
            platform="linux-arm64", commit=workflow["commit"], host_completed=True,
            cleanup=dict(confirmed=True),
        )
        unit = dict(
            passed=True, returncode=0, attempt=1, retained_logs="guest-suite-1", workflow=workflow
        )
        saved = dict(
            passed=True, finish={}, commit=workflow["commit"], units={"suite-comparison": unit}
        )
        report = self.output.parent / "results.json"
        ancestor_state, ancestor_result = copy.deepcopy(saved), copy.deepcopy(result)
        ancestor_state["commit"] = ancestor_result["commit"] = "c" * 40
        ancestor_workflow = ancestor_state["units"]["suite-comparison"]["workflow"]
        ancestor_workflow["commit"] = "c" * 40
        ancestor_workflow["asset_origin"] = "prebuilt VM assets"
        self.assertIsNone(assets.prepare_workflow(output, ancestor_state, ancestor_result))
        self.assertFalse(report.exists())
        for key, value in (("passed", False), ("partial", True)):
            self.assertIsNone(assets.prepare_workflow(output, saved, dict(result, **{key: value})))
        for label in ("cleanup", "source", "workflow", "path"):
            bad_result, bad_state = copy.deepcopy(result), copy.deepcopy(saved)
            if label == "cleanup":
                bad_result["cleanup"]["confirmed"] = False
            elif label == "source":
                bad_state["commit"] = "c" * 40
            elif label == "workflow":
                bad_state["units"]["suite-comparison"]["workflow"]["failed_steps"] = ["hive: tests"]
            else:
                bad_state["units"]["suite-comparison"]["retained_logs"] = "../guest-suite-1"
            with self.subTest(label=label), self.assertRaises(ValueError):
                assets.prepare_workflow(output, bad_state, bad_result)
            self.assertFalse(report.exists())
        bootstrap = self.output / "install.sh"
        original_bootstrap = bootstrap.read_bytes()
        atomic_write_bytes(bootstrap, original_bootstrap + b"\n# corrupted", mode=0o755)
        with self.assertRaises(ValueError):
            assets.prepare_workflow(output, saved, result)
        self.assertFalse(report.exists())
        atomic_write_bytes(bootstrap, original_bootstrap, mode=0o755)
        path, raw = assets.prepare_workflow(output, saved, result)
        self.assertEqual(path, report)
        self.assertEqual(json.loads(raw), dict(workflow, host_completed=True))
        atomic_write_bytes(path, raw, mode=0o600)
        self.assertEqual(assets.accepted(self.output, "arm64")["commit"], workflow["commit"])
        self.assertIsNone(assets.prepare_workflow(output, saved, result))
        original = path.read_bytes()
        broken = copy.deepcopy(saved)
        broken["units"]["suite-comparison"]["workflow"]["steps"] = ["changed workflow"]
        with self.assertRaises(ValueError):
            assets.prepare_workflow(output, broken, result)
        self.assertEqual(path.read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
