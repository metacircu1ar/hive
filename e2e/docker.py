"""Local Docker operations bound to one explicit endpoint and daemon identity."""

import json
import os
import re

from release.process import checked, json_reply, run
from release.progress import finalizer

LABEL = "hive.e2e.nonce"
CLIENT_ENV = (
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "TMPDIR",
    "XDG_RUNTIME_DIR",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    "DOCKER_HOST",
    "DOCKER_API_VERSION",
)


def client_environment(source=None):
    source = os.environ if source is None else source
    return {key: source[key] for key in CLIENT_ENV if key in source}


def endpoint_path(endpoint):
    if (
        not isinstance(endpoint, str)
        or not endpoint.startswith("unix:///")
        or "\x00" in endpoint
        or len(endpoint) > 4096
    ):
        raise ValueError("Local campaigns require a Unix-socket Docker endpoint")
    return endpoint


class Docker:
    def __init__(self, endpoint, daemon_id, *, environment=None, native=run):
        self.endpoint = endpoint_path(endpoint)
        if not isinstance(daemon_id, str) or not 1 <= len(daemon_id) <= 256:
            raise ValueError("Docker daemon identity is missing")
        self.daemon_id = daemon_id
        self.env = client_environment(environment)
        self.env.pop("DOCKER_HOST", None)
        self.env.pop("DOCKER_CONTEXT", None)
        self.native = native

    @classmethod
    def resolve(cls, *, environment=None, native=run):
        env = client_environment(environment)
        selected = env.get("DOCKER_CONTEXT")
        endpoint = env.get("DOCKER_HOST") if not selected else None
        if not endpoint:
            if not selected:
                selected = native(
                    ["docker", "context", "show"], env=env, check=True, text=True
                ).stdout.strip()
            contexts = json_reply(
                native(["docker", "context", "inspect", selected], env=env, check=True, text=True),
                allow_list=True,
            )
            if not isinstance(contexts, list) or len(contexts) != 1:
                raise ValueError("Docker context inspection is ambiguous")
            endpoint = contexts[0]["Endpoints"]["docker"]["Host"]
        endpoint_path(endpoint)
        pinned = dict(env)
        pinned.pop("DOCKER_CONTEXT", None)
        pinned.pop("DOCKER_HOST", None)
        info = json_reply(
            native(
                ["docker", "--host", endpoint, "info", "--format", "{{json .}}"],
                env=pinned,
                check=True,
                text=True,
            )
        )
        if any("rootless" in option for option in info.get("SecurityOptions", [])):
            raise ValueError("Private validation mounts require a rootful local Docker daemon")
        return cls(endpoint, info["ID"], environment=pinned, native=native)

    def identity(self):
        info = json_reply(
            self.native(
                ["docker", "--host", self.endpoint, "info", "--format", "{{json .}}"],
                env=self.env,
                check=True,
                text=True,
            )
        )
        if info.get("ID") != self.daemon_id:
            raise RuntimeError("The recorded Docker daemon has changed")

    def call(self, args, **options):
        self.identity()
        with finalizer(self.identity):
            return self.native(
                ["docker", "--host", self.endpoint, *args],
                env=self.env,
                check=options.pop("check", True),
                text=options.pop("text", True),
                **options,
            )

    def owner(self):
        return dict(endpoint=self.endpoint, daemon_id=self.daemon_id, resource="none")

    def owned_container(self, active):
        owner = active["owner"]
        nonce = active["nonce"]
        if owner.get("name") != "hive-e2e-" + nonce:
            raise ValueError("The recorded container name is not owned by this run")
        identifier = owner.get("container_id")
        if identifier is not None and (
            not isinstance(identifier, str) or re.fullmatch("[a-f0-9]{64}", identifier) is None
        ):
            raise ValueError("Invalid recorded Docker container identity")
        selector = "id=" + identifier if identifier is not None else "name=^/" + owner["name"] + "$"
        listed = self.call(
            [
                "container",
                "ls",
                "--all",
                "--no-trunc",
                "--filter",
                selector,
                "--format",
                "{{json .}}",
            ]
        )
        rows = [json.loads(line) for line in listed.stdout.splitlines() if line.strip()]
        if not rows:
            return None
        if len(rows) != 1 or not isinstance(rows[0], dict):
            raise ValueError("Container ownership lookup is ambiguous")
        found = rows[0]["ID"]
        if not isinstance(found, str) or re.fullmatch("[a-f0-9]{64}", found) is None:
            raise ValueError("Container lookup did not return a full identity")
        inspected = json_reply(self.call(["container", "inspect", found]), allow_list=True)
        if not isinstance(inspected, list) or len(inspected) != 1:
            raise ValueError("Container inspection is ambiguous")
        row = inspected[0]
        if (
            row["Id"] != found
            or (identifier is not None and found != identifier)
            or row["Name"] != "/" + owner["name"]
            or row["Config"]["Labels"].get(LABEL) != nonce
            or type(row["State"]["Running"]) is not bool
        ):
            raise ValueError("Refusing a foreign or changed container")
        return row

    def cleanup_container(self, active):
        row = self.owned_container(active)
        if row is None:
            # A daemon may still finish an unresolved create after an empty list.
            return dict(confirmed=active["owner"].get("create_completed") is True, attempts=[])
        attempts = []
        identifier = row["Id"]
        result = dict(confirmed=False, attempts=attempts)

        def finish_cleanup():
            remaining = self.owned_container(active)
            if remaining is not None and remaining["State"]["Running"]:
                killed = self.call(["container", "kill", identifier], check=False, timeout=15)
                attempts.append(dict(operation="kill", returncode=killed.returncode))
                remaining = self.owned_container(active)
                if remaining is not None and remaining["State"]["Running"]:
                    checked(killed)
                    raise RuntimeError("Owned container is still running after cleanup")
            if remaining is not None:
                self.call(["container", "rm", identifier], timeout=15)
            if self.owned_container(active) is not None:
                raise RuntimeError("Owned container removal is not confirmed")
            result["confirmed"] = True

        # A timed-out/failed stop client does not rule out later owner inspection
        # and forced cleanup. Preserve that original error if cleanup also fails.
        with finalizer(finish_cleanup):
            if row["State"]["Running"]:
                stopped = self.call(
                    ["container", "stop", "--time", "10", identifier], check=False, timeout=20
                )
                attempts.append(dict(operation="stop", returncode=stopped.returncode))
        return result


def recover(active, *, native=run):
    if active["kind"] != "docker":
        return False
    owner = active["owner"]
    docker = Docker(owner["endpoint"], owner["daemon_id"], native=native)
    docker.identity()
    if owner.get("resource") == "none" and active["phase"] == "prepared":
        return True
    if owner.get("resource") == "container":
        return docker.cleanup_container(active)["confirmed"]
    if (owner.get("resource"), active["phase"]) in (
        ("image", "image_lookup"),
        ("image", "image_complete"),
        ("image", "image_ready"),
        ("volume", "volume_lookup"),
        ("volume", "volume_ready"),
    ):
        return True
    # A successful container list cannot prove an interrupted build completed.
    return False
