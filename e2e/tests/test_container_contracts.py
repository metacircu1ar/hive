"""Container invocation and cache ownership without contacting Docker."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from e2e.campaign import Admission
from e2e.ci.container import CACHE_LABEL, Container
from e2e.docker import Docker
from e2e.scripts import local_ci
from release.files import create_directory


class ContainerCommands:
    def __init__(self):
        self.calls = []
        self.foreign = False

    def call(self, args, **options):
        self.calls.append(args)
        if args[:2] == ["volume", "ls"]:
            body = json.dumps(dict(Name="fixture"))
        elif args[:2] == ["volume", "inspect"]:
            name = args[2]
            expected = "arm64:" + str(os.getuid()) + ":target"
            body = json.dumps(
                [dict(Name=name, Labels={CACHE_LABEL: "foreign" if self.foreign else expected})]
            )
        elif args[:2] == ["container", "create"]:
            body = "a" * 64 + "\n"
        else:
            body = "{}\n"
        return subprocess.CompletedProcess(args, 0, body, "")

    def owner(self):
        return dict(endpoint="unix:///fixture.sock", daemon_id="fixture", resource="none")

    def owned_container(self, active):
        return dict(Id=active["owner"]["container_id"])


class ContainerContracts(unittest.TestCase):
    def test_execution_uses_verified_image_id_even_if_tag_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            docker = ContainerCommands()
            docker.native = None
            admission = Admission(root / "admission")
            inputs = dict(image="mutable-tag", digest="fixture-digest", context=root)
            immutable = "sha256:" + "b" * 64
            owner = Container(admission, docker, root, "arm64")
            with admission.held():
                admission.begin("docker", "fixture", docker.owner())
                with (
                    patch("e2e.ci.container.image_inputs", return_value=inputs),
                    patch.object(owner, "lookup_image", return_value=immutable),
                    patch.object(owner, "verify_image", return_value=immutable),
                ):
                    verified = owner.ensure_image(root, "c" * 40)
                inputs["image"] = "different-tag"
                with patch.object(owner, "volume", side_effect=["target", "registry"]):
                    owner.create(verified, fresh=False, commit="c" * 40, platform="linux-arm64")
                self.assertEqual(docker.calls[0][-3], immutable)
                self.assertEqual(
                    json.loads((root / "image-inputs.json").read_text())["image_id"], immutable
                )
                admission.complete()

    def test_create_uses_init_only_run_output_and_owned_cache_mounts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "output"
            create_directory(output)
            admission = Admission(root / "admission")
            docker = ContainerCommands()
            with admission.held():
                active = admission.begin("docker", "fixture", docker.owner())
                owner = Container(admission, docker, output, "arm64")
                with patch.object(owner, "volume", side_effect=["owned-target", "owned-registry"]):
                    owner.create(
                        "owned-image", fresh=False, commit="b" * 40, platform="linux-arm64"
                    )
                create = docker.calls[0]
                self.assertIn("--init", create)
                self.assertIn("--restart=no", create)
                mounts = [
                    create[index + 1] for index, value in enumerate(create) if value == "--volume"
                ]
                self.assertEqual(
                    mounts,
                    [
                        str(output) + ":/out",
                        "owned-target:/work/target",
                        "owned-registry:/home/runner/.cargo/registry",
                    ],
                )
                self.assertEqual(create[-3:], ["owned-image", "sleep", "infinity"])
                self.assertEqual(active["owner"]["container_id"], "a" * 64)
                self.assertTrue(active["owner"]["create_completed"])
                self.assertEqual(
                    docker.calls[-1],
                    [
                        "container",
                        "exec",
                        "a" * 64,
                        "git",
                        "clone",
                        "-q",
                        "/out/head.bundle",
                        "/work/src",
                    ],
                )
                admission.complete()

    def test_foreign_cache_refuses_and_fresh_never_forces_in_use_volume(self):
        with tempfile.TemporaryDirectory() as temporary:
            admission = Admission(Path(temporary) / "admission")
            docker = ContainerCommands()
            with admission.held():
                admission.begin("docker", "fixture", docker.owner())
                owner = Container(admission, docker, Path(temporary), "arm64")
                docker.foreign = True
                with self.assertRaises(ValueError):
                    owner.volume("target", fresh=True)
                self.assertFalse(any(call[:2] == ["volume", "rm"] for call in docker.calls))
                docker.foreign = False
                name = owner.volume("target", fresh=True)
                self.assertIn(["volume", "rm", name], docker.calls)
                self.assertFalse(any("--force" in call or "-f" in call for call in docker.calls))
                admission.complete()

    def test_endpoint_is_frozen_and_provider_secrets_are_not_forwarded(self):
        calls = []

        def native(argv, **options):
            calls.append((argv, options["env"]))
            if argv[1:3] == ["context", "inspect"]:
                reply = [dict(Endpoints=dict(docker=dict(Host="unix:///selected.sock")))]
            else:
                reply = dict(ID="fixture-daemon", SecurityOptions=[])
            return subprocess.CompletedProcess(argv, 0, json.dumps(reply), "")

        owner = Docker.resolve(
            environment=dict(
                PATH="/bin",
                DOCKER_CONTEXT="selected",
                DOCKER_HOST="unix:///ignored.sock",
                OPENAI_API_KEY="private-fixture",
            ),
            native=native,
        )
        with patch.dict(os.environ, DOCKER_HOST="unix:///changed.sock", DOCKER_CONTEXT="changed"):
            owner.call(["container", "ls"])
        self.assertEqual(calls[0][0], ["docker", "context", "inspect", "selected"])
        for argv, environment in calls[1:]:
            self.assertEqual(argv[1:3], ["--host", "unix:///selected.sock"])
            self.assertNotIn("DOCKER_CONTEXT", environment)
            self.assertNotIn("DOCKER_HOST", environment)
        self.assertTrue(all("OPENAI_API_KEY" not in environment for _, environment in calls))
        with self.assertRaises(ValueError):
            Docker("ssh://remote", "fixture")

    def test_native_work_path_is_short_and_container_inside_cannot_bypass_owner(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            create_directory(root / "checkout")
            local_ci.link_target(root / "checkout", root / "cache/target")
            self.assertEqual(
                (root / "checkout/target").resolve(), (root / "cache/target").resolve()
            )
        args = local_ci.parse_args(
            ["--inside", "--platform", "linux-arm64", "--output-dir", "/out"]
        )
        with self.assertRaises(ValueError):
            local_ci.validate_inside(args)
