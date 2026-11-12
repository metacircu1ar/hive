"""Root histories and distribution validation do not require a full campaign."""

import tempfile
import unittest
from pathlib import Path
import contextlib
import io
from types import SimpleNamespace
from unittest.mock import patch

from e2e.campaign import Admission, read_receipt
from e2e.ci.container import image_inputs
from e2e.ci import prepare as container_prepare
from e2e.ci import runner
from e2e.ci.steps import PARTS, compatibility_baseline, distribution_steps
from e2e.scripts import local_ci
from release.files import PublicationError, atomic_write_text, create_directory
from release.process import ProcessFailure, run
from release.progress import entry


def repository(root):
    create_directory(root)
    for args in (
        ["init", "-q"],
        ["config", "user.name", "Test"],
        ["config", "user.email", "test@example.invalid"],
        ["config", "commit.gpgsign", "false"],
    ):
        run(["git", *args], cwd=root, check=True)
    create_directory(root / "e2e/ci")
    atomic_write_text(root / "e2e/ci/Dockerfile", "FROM ubuntu:22.04\n")
    atomic_write_text(root / "rust-toolchain.toml", '[toolchain]\nchannel = "1.98.1"\n')
    run(["git", "add", "."], cwd=root, check=True)
    run(["git", "commit", "-qm", "Init"], cwd=root, check=True)
    return local_ci.selected_head(root, run)


class LocalCi(unittest.TestCase):
    def test_vm_workflow_installs_and_exports_provided_assets(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            assets = root / "accepted assets"
            create_directory(assets)
            atomic_write_text(assets / "receipt", "accepted")
            workflow = runner.Runner("linux-arm64", root, root / "out", full=True,
                                     installed_assets=assets)
            expected = [("installed release parts", "build the native archives", False)]
            with (patch.object(runner, "run", return_value=SimpleNamespace(stdout="a" * 40)),
                  patch.object(runner, "variants", return_value=[{"name": "hive"}]),
                  patch.object(runner, "variant_steps", return_value=expected),
                  patch.object(runner, "compatibility_baseline", return_value="HEAD"),
                  patch.object(workflow, "step", return_value=True) as step):
                result = workflow.run()
            command = step.call_args_list[-1].args[2]
            self.assertIn("scripts/check_release_install.py", command)
            self.assertIn("'" + str(assets) + "'", command)
            self.assertNotIn("build_release.py", command)
            self.assertEqual(result["asset_origin"], "prebuilt VM assets")
            self.assertEqual((root / "out/assets/receipt").read_text(), "accepted")
            with self.assertRaises(ValueError):
                runner.Runner("macos", root, root / "bad", full=True, installed_assets=assets)
            link = root / "linked-assets"
            link.symlink_to(assets)
            for invalid in (link, root / "missing", assets / "receipt"):
                with self.subTest(path=invalid), self.assertRaises(ValueError):
                    runner.Runner("linux-arm64", root, root / "bad", full=True,
                                  installed_assets=invalid)

            ordinary = runner.Runner("linux-arm64", root, root / "ordinary", full=True)
            with (patch.object(runner, "run", return_value=SimpleNamespace(stdout="a" * 40)),
                  patch.object(runner, "variants", return_value=[{"name": "hive"}]),
                  patch.object(runner, "variant_steps", return_value=expected),
                  patch.object(runner, "compatibility_baseline", return_value="HEAD"),
                  patch.object(ordinary, "step", return_value=True) as step):
                result = ordinary.run()
            self.assertEqual(step.call_args_list[-1].args[2], "build the native archives")
            self.assertEqual(result["asset_origin"], "workflow build")

    def test_bundle_publication_wrapper_preserves_uncertain_staging_intent(self):
        for uncertain in (False, True):
            with self.subTest(uncertain=uncertain), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                admission = Admission(root / "admission")
                cause = TimeoutError("Fixture bundle writer failed")
                cause.owned_cleanup_uncertain = uncertain

                def failed_bundle(*args, **options):
                    raise cause

                with self.assertRaises(PublicationError) as raised:
                    with admission.held(), local_ci.staging(admission, "fixture"):
                        local_ci.bundle_head(root, root, "a" * 40, failed_bundle)
                self.assertIs(raised.exception.__cause__, cause)
                self.assertEqual(read_receipt(root / "admission") is not None, uncertain)

    def test_native_macos_checkout_and_cache_use_short_home_root_not_output(self):
        with tempfile.TemporaryDirectory(prefix="hive-ci-", dir="/tmp") as temporary:
            root = Path(temporary).resolve()
            output = root / "nested/output/results"
            create_directory(output)
            admission = Admission(root / "admission")
            commit = "b" * 40
            seen = []

            def native(argv, **options):
                self.assertEqual(argv[:3], ["git", "clone", "-q"])
                self.assertEqual(options["pass_fds"], (admission.lock_fd,))
                create_directory(Path(argv[-1]))

            def runner(platform, checkout, destination, **options):
                seen.append(checkout)
                self.assertEqual(platform, "macos")
                self.assertEqual(destination, output)
                self.assertNotIn(output, checkout.parents)
                self.assertEqual(checkout.parent.parent, root / ".hive-e2e.noindex")
                self.assertTrue(checkout.parent.name.startswith("ci-"))
                self.assertEqual(checkout.name, "src")
                self.assertLess(len(str(checkout).encode()), 120)
                self.assertFalse(any(character.isspace() for character in str(checkout)))
                self.assertEqual(
                    (checkout / "target").resolve(), root / ".hive-e2e.noindex/cache-macos/target"
                )
                return SimpleNamespace(run=lambda: dict(commit=commit, passed=True))

            args = SimpleNamespace(fresh=False, variant=None, full=True, distribution_only=False)
            with (
                patch.object(
                    local_ci.pwd, "getpwuid", return_value=SimpleNamespace(pw_dir=str(root))
                ),
                patch.object(local_ci, "selected_head", return_value=commit),
                patch.object(local_ci, "bundle_head", return_value=output / "head.bundle"),
                patch.object(local_ci, "run", side_effect=native),
                patch.object(local_ci, "Runner", side_effect=runner),
                admission.held(),
            ):
                self.assertTrue(local_ci.native_macos(root, args, output, admission)["passed"])
            self.assertEqual(len(seen), 1)
            self.assertFalse(seen[0].exists())
            self.assertTrue((root / ".hive-e2e.noindex/cache-macos/target").is_dir())
            self.assertIsNone(read_receipt(root / "admission"))

    def test_default_uses_native_linux_essential_and_full_is_explicit(self):
        for machine, expected in (("arm64", "linux-arm64"), ("x86_64", "linux-amd64")):
            with patch.object(local_ci.platform, "machine", return_value=machine):
                args = local_ci.parse_args([])
            self.assertEqual(args.platform, [expected])
            self.assertFalse(args.full)
            self.assertFalse(args.distribution_only)
            self.assertIsNone(args.variant)
        self.assertTrue(local_ci.parse_args(["--full", "--platform", "linux-arm64"]).full)

    def test_bundle_is_atomic_readable_and_image_uses_the_recorded_commit(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, output = root / "source", root / "output"
            commit = repository(source)
            create_directory(output)
            bundle = local_ci.bundle_head(source, output, commit, run)
            clone = root / "clone"
            run(["git", "clone", "-q", str(bundle), str(clone)], check=True)
            self.assertEqual(local_ci.selected_head(clone, run), commit)
            atomic_write_text(source / "e2e/ci/Dockerfile", "UNCOMMITTED CONTENT\n")
            inputs = image_inputs(source, commit, output, "arm64")
            self.assertEqual((inputs["context"] / "Dockerfile").read_text(), "FROM ubuntu:22.04\n")
            self.assertEqual(inputs["rust"], "1.98.1")
            run(["git", "commit", "-qam", "Next"], cwd=source, check=True)
            create_directory(root / "second")
            with self.assertRaises(ValueError):
                local_ci.bundle_head(source, root / "second", commit, run)

    def test_prepare_missing_docker_prints_command_without_install_or_admission(self):
        with (
            patch.object(container_prepare.shutil, "which", return_value=None),
            patch.object(container_prepare, "run") as native,
            patch.object(container_prepare, "Admission") as admission,
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            with self.assertRaises(SystemExit) as status:
                container_prepare.main(SimpleNamespace(install=False))
            self.assertEqual(status.exception.code, 1)
            self.assertIn("docker", output.getvalue())
            self.assertIn("commands", output.getvalue())
            native.assert_not_called()
            admission.assert_not_called()

    def test_prepare_installs_only_when_explicit_and_reuses_verified_image(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            repository(source)
            owner = SimpleNamespace(architecture="arm64", lookup_image=lambda *_: "verified")
            with (
                patch.object(
                    container_prepare.shutil, "which", side_effect=[None, "/fixture/docker"]
                ),
                patch.object(container_prepare, "run") as native,
                patch.object(
                    container_prepare,
                    "Admission",
                    side_effect=lambda: Admission(root / "admission"),
                ),
                patch.object(container_prepare, "product_root", return_value=source),
                patch.object(container_prepare.Container, "resolve", return_value=owner),
                patch.object(
                    container_prepare,
                    "image_inputs",
                    return_value=dict(image="fixture", digest="digest"),
                ),
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                self.assertEqual(
                    entry(
                        lambda: container_prepare.main(SimpleNamespace(install=True)), name="verify"
                    ),
                    0,
                )
                self.assertEqual(
                    [call.args[0] for call in native.call_args_list],
                    container_prepare.install_commands(),
                )
                self.assertIn('"ready": true', output.getvalue())

    def test_root_and_later_commit_baselines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return run(
                    ["git", *args], cwd=root, check=True, capture_output=True, text=True
                ).stdout.strip()

            git("init", "-q")
            git("config", "user.name", "Test")
            git("config", "user.email", "test@example.invalid")
            git("config", "commit.gpgsign", "false")
            git("commit", "-q", "--allow-empty", "-m", "Init")
            first = git("rev-parse", "HEAD")
            self.assertEqual(compatibility_baseline(root), first)
            git("commit", "-q", "--allow-empty", "-m", "Next")
            self.assertEqual(compatibility_baseline(root), first)
            second = git("rev-parse", "HEAD")
            git("commit", "-q", "--allow-empty", "-m", "Third")
            self.assertEqual(compatibility_baseline(root), second)

    def test_invalid_checkout_does_not_silently_select_head(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ProcessFailure):
                compatibility_baseline(Path(directory))

    def test_distribution_builds_all_parts_before_installed_acceptance(self):
        steps = distribution_steps()
        self.assertEqual(
            [name for name, _, _ in steps[: len(PARTS)]], [f"package {part}" for part in PARTS]
        )
        for part, (_, command, required) in zip(PARTS, steps):
            self.assertIn(f"build_release.py --part {part}", command)
            self.assertIn("package_release.py", command)
            self.assertTrue(required)
        self.assertEqual(steps[-1][0], "installed release parts")
        self.assertTrue(all(required for _, _, required in steps))
        self.assertFalse(any("cargo test" in command for _, command, _ in steps))
