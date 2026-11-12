"""Create one labelled container, retain intent and retire its namespace."""

from functools import partial
import hashlib
import json
import os
import re

from e2e.docker import Docker, LABEL, recover
from e2e.ci.steps import ci_env
from release.files import atomic_write_bytes, atomic_write_json, create_directory, raw_log
from release.process import json_reply, run

IMAGE_LABEL = "hive.e2e.image"
CACHE_LABEL = "hive.e2e.cache"


def source_file(root, commit, name, *, native=run):
    if re.fullmatch("[a-f0-9]{40}", commit) is None:
        raise ValueError("Validation source needs a complete commit identity")
    return native(["git", "show", commit + ":" + name], cwd=root, env=ci_env(), check=True).stdout


def image_inputs(root, commit, output, architecture, *, native=run):
    uid, gid = os.getuid(), os.getgid()
    if uid == 0:
        raise ValueError("Container validation must run as an ordinary user")
    dockerfile = source_file(root, commit, "e2e/ci/Dockerfile", native=native)
    toolchain = source_file(root, commit, "rust-toolchain.toml", native=native).decode()
    versions = re.findall(r'^channel\s*=\s*"(1\.\d+\.\d+)"\s*$', toolchain, re.M)
    if len(versions) != 1:
        raise ValueError("The committed Rust toolchain must select one pinned stable version")
    version = versions[0]
    digest = hashlib.sha256(
        dockerfile + json.dumps([architecture, uid, gid, version]).encode()
    ).hexdigest()
    context = None if output is None else output / "image-context"
    if context is not None:
        create_directory(context)
        atomic_write_bytes(context / "Dockerfile", dockerfile)
    return dict(
        image="hive-e2e-ci:" + architecture + "-" + digest[:16],
        digest=digest,
        context=context,
        uid=uid,
        gid=gid,
        rust=version,
    )


class Container:
    def __init__(self, admission, docker, output, architecture):
        self.admission, self.docker = admission, docker
        self.output, self.architecture = output, architecture
        self.cleanup = None

    @classmethod
    def resolve(cls, admission, output, architecture):
        native = partial(run, pass_fds=(admission.lock_fd,))
        docker = Docker.resolve(native=native)
        return cls(admission, docker, output, architecture)

    def verify_image(self, image, digest):
        inspected = json_reply(self.docker.call(["image", "inspect", image]), allow_list=True)
        if (
            not isinstance(inspected, list)
            or len(inspected) != 1
            or inspected[0]["Config"]["Labels"].get(IMAGE_LABEL) != digest
            or not isinstance(inspected[0]["Id"], str)
            or re.fullmatch("sha256:[a-f0-9]{64}", inspected[0]["Id"]) is None
        ):
            raise ValueError("Refusing a foreign or malformed validation image")
        return inspected[0]["Id"]

    def lookup_image(self, image, digest):
        listed = self.docker.call(
            [
                "image",
                "ls",
                "--no-trunc",
                "--filter",
                "reference=" + image,
                "--format",
                "{{json .}}",
            ]
        )
        rows = [json.loads(line) for line in listed.stdout.splitlines() if line.strip()]
        return self.verify_image(image, digest) if rows else None

    def ensure_image(self, root, commit):
        inputs = image_inputs(
            root, commit, self.output, self.architecture, native=self.docker.native
        )
        self.admission.advance("image_lookup", resource="image", image=inputs["image"])
        if self.lookup_image(inputs["image"], inputs["digest"]) is None:
            self.admission.advance("building", resource="image", image=inputs["image"])
            with raw_log(self.output / "image-build.log") as log:
                self.docker.call(
                    [
                        "build",
                        "--platform",
                        "linux/" + self.architecture,
                        "--label",
                        IMAGE_LABEL + "=" + inputs["digest"],
                        "--build-arg",
                        "RUNNER_UID=" + str(inputs["uid"]),
                        "--build-arg",
                        "RUNNER_GID=" + str(inputs["gid"]),
                        "--build-arg",
                        "RUST_VERSION=" + inputs["rust"],
                        "--tag",
                        inputs["image"],
                        str(inputs["context"]),
                    ],
                    stdout=log,
                    stderr=log,
                    timeout=1800,
                    limit=64 * 1024**2,
                )
            self.admission.advance("image_complete")
        image_id = self.verify_image(inputs["image"], inputs["digest"])
        atomic_write_json(
            self.output / "image-inputs.json",
            {**inputs, "context": str(inputs["context"]), "image_id": image_id},
        )
        self.admission.advance("image_ready")
        return image_id

    def volume(self, suffix, *, fresh):
        name = "hive-e2e-ci-" + self.architecture + "-" + str(os.getuid()) + "-" + suffix
        expected = self.architecture + ":" + str(os.getuid()) + ":" + suffix
        self.admission.advance("volume_lookup", resource="volume", volume=name)
        listed = self.docker.call(
            ["volume", "ls", "--filter", "name=^" + name + "$", "--format", "{{json .}}"]
        )
        rows = [json.loads(line) for line in listed.stdout.splitlines() if line.strip()]
        if rows:
            inspected = json_reply(self.docker.call(["volume", "inspect", name]), allow_list=True)
            if (
                not isinstance(inspected, list)
                or len(inspected) != 1
                or inspected[0]["Name"] != name
                or inspected[0]["Labels"].get(CACHE_LABEL) != expected
            ):
                raise ValueError("Refusing a foreign validation cache volume")
            if fresh:
                self.admission.advance("volume_removing")
                self.docker.call(["volume", "rm", name])
                rows = []
        if not rows:
            self.admission.advance("volume_creating")
            self.docker.call(["volume", "create", "--label", CACHE_LABEL + "=" + expected, name])
        self.admission.advance("volume_ready")
        return name

    def create(self, image, *, fresh, commit, platform):
        target, registry = self.volume("target", fresh=fresh), self.volume("registry", fresh=fresh)
        active = self.admission.active
        name = "hive-e2e-" + active["nonce"]
        if ":" in str(self.output) or "\n" in str(self.output):
            raise ValueError("Container output paths cannot contain colons or newlines")
        self.admission.advance(
            "creating", resource="container", name=name, container_id=None, create_completed=False
        )
        atomic_write_json(
            self.output / "parent.json",
            dict(nonce=active["nonce"], commit=commit, platform=platform),
        )
        command = [
            "container",
            "create",
            "--init",
            "--platform",
            "linux/" + self.architecture,
            "--name",
            name,
            "--label",
            LABEL + "=" + active["nonce"],
            "--restart=no",
            "--volume",
            str(self.output) + ":/out",
            "--volume",
            target + ":/work/target",
            "--volume",
            registry + ":/home/runner/.cargo/registry",
            image,
            "sleep",
            "infinity",
        ]
        identifier = self.docker.call(command, timeout=120).stdout.strip()
        if re.fullmatch("[a-f0-9]{64}", identifier) is None:
            raise ValueError("Container creation did not return a full identity")
        self.admission.advance("created", container_id=identifier, create_completed=True)
        self.docker.owned_container(active)
        self.docker.call(["container", "start", identifier])
        self.admission.advance("started")
        self.docker.call(
            [
                "container",
                "exec",
                identifier,
                "git",
                "clone",
                "-q",
                "/out/head.bundle",
                "/work/src",
            ],
            timeout=120,
        )
        return identifier

    def retire(self):
        active = self.admission.active
        if active is None:
            return
        if active["owner"].get("resource") == "container":
            self.cleanup = self.docker.cleanup_container(active)
            confirmed = self.cleanup["confirmed"]
        else:
            confirmed = recover(active, native=self.docker.native)
        if not confirmed:
            raise RuntimeError("Container campaign cleanup remains uncertain")
        self.admission.complete()
