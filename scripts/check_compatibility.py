#!/usr/bin/env python3
"""Exercise plain release binaries across control, hosted knowledge and MCP."""

import argparse
import json
import os
from pathlib import Path
import sys

from release.acceptance import ready
from release.acceptance import json_command, running, wait_for
from release.acceptance import Cli, mock_login, require
from release.common import ROOT, digest, native_target
from release.compatibility_peers import catalog, declarations, materialize, fetch, fetch_local
from release.compatibility_recipe import recipe, previous_line
from release.compatibility_skew import protocol, overlap, probe
from release.files import atomic_write_bytes, atomic_write_json, create_directory
from release.progress import entry, phase
from release.cancellation import finish
from release.temporary import directory as private_directory
from release.native_evidence import evidence
from release.compatibility_readers import identity
from release.compatibility_pair import Pair
from release.mock_provider import CODEX_MODULES


class Matrix:
    def __init__(self, output, work, fixtures, current):
        self.current = current
        self.output, self.work = output, work
        self.env = dict(os.environ)
        self.inputs = json.loads(fixtures["scenario"])
        self.transcripts = json.loads(fixtures["transcripts"])
        self.provider = work / "mock-provider"
        create_directory(work)
        os.chmod(work, 0o700)
        atomic_write_bytes(
            self.provider, f"#!{sys.executable}\n".encode() + fixtures["provider"], mode=0o700
        )
        for key, name in [
            ("provider_schema", "codex-schema.json"),
            ("provider_common", "mock_common.py"),
            ("publication", "mock_files.py"),
            ("embeddings", "embeddings.py"),
        ]:
            atomic_write_bytes(work / name, fixtures[key], mode=0o600)
        for module in CODEX_MODULES:
            atomic_write_bytes(work / (module + ".py"), fixtures[module], mode=0o600)

    def pair(self, label, cli, daemon, comb_client, comb_server):
        work = self.work / label
        create_directory(work)
        os.chmod(work, 0o700)
        model_log = self.output / (label + "-embeddings.log")
        with running(
            [sys.executable, str(self.work / "embeddings.py")], model_log, self.env
        ) as model:
            port = wait_for(lambda: model_log.read_text().strip(), "mock embedding startup", model)
            config = work / "embedding.json"
            atomic_write_json(
                config, dict(self.inputs["embedding"], url=f"http://127.0.0.1:{port}/v1/embeddings")
            )
            return Pair(self, label, work, config).run(cli, daemon, comb_client, comb_server)

    def outside_window(self, label, cli, daemon, comb_client, comb_server):
        work = self.work / label
        create_directory(work)
        os.chmod(work, 0o700)
        root, socket = work / "knowledge", work / "comb.sock"
        producer = [str(comb_server), "--root", str(root), "--socket", str(socket)]
        json_command([str(comb_server), "--root", str(root), "init"], env=self.env)
        service_shared = overlap(
            protocol(identity(comb_client), "service_protocol"),
            protocol(identity(comb_server), "service_protocol"),
        )
        with running(producer + ["serve"], self.output / (label + "-comb.log"), self.env) as server:
            wait_for(
                lambda: socket.exists() and ready(json_command, producer + ["stats"], env=self.env),
                "old-peer service startup",
                server,
            )
            service = probe(
                [str(comb_client), "--root", str(root), "--socket", str(socket), "stats"],
                service_shared,
                "Comb",
                self.env,
            )
        config = work / "config.json"
        atomic_write_json(
            config,
            {
                "auth_profiles": {"mock": {"provider": "codex", "method": "subscription"}},
                "roles": {},
            },
        )
        state = work / "state"
        args = [
            str(daemon),
            "--state-dir",
            str(state),
            "--config",
            str(config),
            "--codex-bin",
            str(self.provider),
        ]
        mock_login(args, env=self.env)
        control_shared = overlap(
            protocol(identity(cli), "control_protocol"),
            protocol(identity(daemon), "control_protocol"),
        )
        with running(
            args + ["daemon", "run"], self.output / (label + "-hive.log"), self.env
        ) as process:
            native = self.control(daemon, state, config)
            wait_for(
                lambda: (
                    (state / "runtime/control.sock").exists() and ready(native, "daemon", "status")
                ),
                "old-peer daemon startup",
                process,
            )
            control = probe(
                [str(cli), "--state-dir", str(state), "agents"], control_shared, "Hive", self.env
            )
            native("daemon", "stop")
            require(process.wait(timeout=20) == 0, "Old-peer daemon failed to stop")
        return {"control": control, "service": service}

    def control(self, binary, state, config):
        control = [str(binary), "--state-dir", str(state), "--config", str(config)]
        return Cli(
            [str(binary), "--state-dir", str(state)],
            prefixes={"daemon": control, "auth": control},
            timeout=15,
        )


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--current-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--peers", type=Path, default=ROOT / "compat/releases.json")
    parser.add_argument(
        "--peer-assets", type=Path, help="Local immutable assets for the selected current release"
    )
    parser.add_argument(
        "--local-peer-version", help="Current release read locally; all other peers still use HTTPS"
    )
    return parser.parse_args()


def admit(args):
    current = args.current_dir.resolve()
    require(current.name == "release", "Current binaries must come from a plain release build")
    require(
        all(
            (current / name).is_file() and (current / name).resolve().parent == current
            for name in ("hivectl", "hive-daemon", "comb")
        ),
        "Current release directory must contain every expected native executable",
    )
    require(not args.output_dir.exists(), "Compatibility output must be new")
    require(
        (args.peer_assets is None) == (args.local_peer_version is None),
        "--peer-assets and --local-peer-version must be supplied together",
    )
    create_directory(args.output_dir)
    os.chmod(args.output_dir, 0o700)
    rows = catalog(args.peers.read_bytes())
    target = native_target()
    declarations(ROOT, rows, target)
    peers = [row for row in rows if row["target"] == target]
    require(
        not rows or bool(peers), "Declared releases lack checksum-pinned assets for this target"
    )
    results = {
        "status": "incomplete",
        "current": {},
        "synthetic": None,
        "released": [],
        "historical_status": "not_declared" if not rows else "pending",
    }
    for name in ("hivectl", "hive-daemon", "comb"):
        results["current"][name] = {
            "sha256": digest(current / name),
            "identity": identity(current / name),
        }
    if args.local_peer_version is not None:
        require(
            args.local_peer_version == results["current"]["hivectl"]["identity"]["product_version"],
            "Local peer version must match current binaries",
        )
        require(
            any(row["version"] == args.local_peer_version for row in peers),
            "Selected local peer version and target must exist in the catalog",
        )
    results["local_peer_version"] = args.local_peer_version
    return current, rows, peers, results


def synthetic_case(args, current, results, directory):
    matrix = Matrix(
        args.output_dir,
        Path(directory) / "synthetic",
        recipe(ROOT, None, synthetic=True),
        current,
    )
    with phase("verify"):
        results["synthetic"] = matrix.pair(
            "synthetic-0",
            current / "hivectl",
            current / "hivectl",
            current / "comb",
            current / "comb",
        )


def released_cases(args, current, results, directory, peers):
    skew = previous_line(
        [row["version"] for row in peers],
        results["current"]["hivectl"]["identity"]["product_version"],
    )
    for row in peers:
        destination = Path(directory) / row["version"] / "release"
        with phase("fetch"):
            download = (
                (lambda url, expected: fetch_local(args.peer_assets, url, expected))
                if row["version"] == args.local_peer_version
                else fetch
            )
            materialize(row, destination, download)
        for name in ("hivectl", "hive-daemon", "comb"):
            require(
                identity(destination / name)["product_version"] == row["version"],
                "Downloaded peer version mismatch",
            )
        historical = Matrix(
            args.output_dir,
            Path(directory) / (row["version"] + "-corpus"),
            recipe(ROOT, row["version"], expected_sha256=row["corpus_sha256"]),
            current,
        )
        verify_peer(historical, row, destination, skew, results)


def verify_peer(historical, row, destination, skew, results):
    with phase("verify"):
        upgraded = historical.pair(
            row["version"] + "-upgrade",
            destination / "hivectl",
            destination / "hivectl",
            destination / "comb",
            destination / "comb",
        )
        if row["version"] not in skew:
            # All historical corpora still upgrade; unsupported protocol
            # directions are checked separately, never required to work.
            old_new = historical.outside_window(
                row["version"] + "-old-protocol",
                destination / "hivectl",
                historical.current / "hivectl",
                historical.current / "comb",
                destination / "comb",
            )
            new_old = historical.outside_window(
                row["version"] + "-new-protocol",
                historical.current / "hivectl",
                destination / "hivectl",
                destination / "comb",
                historical.current / "comb",
            )
            results["released"].append(
                {
                    "version": row["version"],
                    "upgrade": upgraded,
                    "skew": "outside_window",
                    "old_cli_new_daemon": old_new,
                    "new_cli_old_daemon": new_old,
                }
            )
            return
        old_new = historical.pair(
            row["version"] + "-old-cli",
            destination / "hivectl",
            historical.current / "hivectl",
            historical.current / "comb",
            destination / "comb",
        )
        new_old = historical.pair(
            row["version"] + "-new-cli",
            historical.current / "hivectl",
            destination / "hivectl",
            destination / "comb",
            historical.current / "comb",
        )
    results["released"].append(
        {
            "version": row["version"],
            "upgrade": upgraded,
            "old_cli_new_daemon": old_new,
            "new_cli_old_daemon": new_old,
        }
    )


def main():
    args = arguments()
    with evidence(args.output_dir.with_name(args.output_dir.name + ".diagnostics")):
        return execute(args)


def execute(args):
    current, rows, peers, results = admit(args)
    with private_directory(prefix="hive-compat-", parent="/tmp") as directory:
        synthetic_case(args, current, results, directory)
        released_cases(args, current, results, directory, peers)
        results["historical_status"] = "executed" if rows else "not_declared"
    results["status"] = "passed"

    def complete():
        atomic_write_json(args.output_dir / "results.json", results, mode=384)
        print(
            json.dumps(
                {
                    "synthetic": "passed",
                    "released_pairs": len(peers),
                    "historical_status": results["historical_status"],
                    "local_peer_version": args.local_peer_version,
                }
            )
        )

    finish(complete)
    return 0


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
