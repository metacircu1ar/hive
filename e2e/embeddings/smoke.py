"""Short-input retrieval through real Comb, suitable for baseline CPU inference."""

from release.temporary import directory as temporary_directory
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
import time

from release.acceptance import running, wait_for
from release.cancellation import deadline
from release.files import atomic_write_json
from release.process import json_reply, run

INPUT_BYTES = 512
BODY_BYTES = 64
TITLE_BYTES = 32
HTTP_SECONDS = 120
CLI_SECONDS = 180
SMOKE_SECONDS = 900
READY_SECONDS = 180
SERVICE_SECONDS = 60
FACTS = (
    ("france", "France", "Paris is the capital of France.", "What is the capital of France?"),
    ("china", "China", "Beijing is the capital of China.", "What is the capital of China?"),
    (
        "storage",
        "Storage",
        "Committed SQLite messages survive a restart.",
        "How do committed messages survive a restart?",
    ),
)


def execute(binary, configuration, *, cpu=None):
    cpu = sys.platform.startswith("linux") if cpu is None else cpu
    with (
        deadline(SMOKE_SECONDS),
        temporary_directory(prefix="comb-smoke-", parent="/tmp") as temporary,
    ):
        directory = Path(temporary)
        root, socket = directory / "knowledge", directory / "comb.sock"
        config = json.loads(configuration.read_text())
        if cpu:
            config["timeout_seconds"] = HTTP_SECONDS
        selected = directory / "embedding.json"
        atomic_write_json(selected, config)
        serving = False

        def call(*args, body=None):
            command = [str(binary), "--root", str(root)]
            if serving:
                command += ["--socket", str(socket)]
            return json_reply(
                run(
                    [*command, *args], input=body, text=True, timeout=CLI_SECONDS, limit=4 * 1024**2
                )
            )

        call("init")
        started = time.monotonic()
        samples, queries = [], []
        for slug, title, text, query in FACTS:
            if (
                len(title.encode()) > TITLE_BYTES
                or len(text.encode()) > BODY_BYTES
                or len((title + "\n\n" + text).encode()) > INPUT_BYTES
                or len((config["query_prefix"] + query).encode()) > INPUT_BYTES
            ):
                raise ValueError("Small embedding fixture exceeds its input budget")
            call(
                "put",
                "concepts/" + slug,
                "--request-id",
                slug,
                body=f"---\ntitle: {title}\ntype: concept\n---\n{text}\n",
            )
            began = time.monotonic()
            coverage = call("embed", "--config", str(selected))
            samples.append(time.monotonic() - began)
            if coverage["pending"] or coverage["rejected"]:
                raise RuntimeError("Small embedding input was rejected or left pending")
        if coverage != dict(total=len(FACTS), embedded=len(FACTS), rejected=0, pending=0):
            raise RuntimeError("Small embedding coverage is incomplete")
        with ExitStack() as resources:
            if not cpu:
                owner = resources.enter_context(
                    running(
                        [
                            str(binary),
                            "--root",
                            str(root),
                            "--socket",
                            str(socket),
                            "serve",
                            "--embedding-config",
                            str(selected),
                        ],
                        directory / "service.log",
                        os.environ,
                    )
                )
                wait_for(socket.exists, "small Comb service", owner, seconds=READY_SECONDS)
                serving = True
            for slug, _, _, query in FACTS:
                began = time.monotonic()
                args = ["search", query, "--mode", "vector", "--limit", "1"]
                if cpu:
                    args += ["--embedding-config", str(selected)]
                reply = call(*args)  # A failure ends the check immediately; no retries.
                if (
                    reply["mode"] != "vector"
                    or reply["fallback"] is not None
                    or not reply["results"]
                    or reply["results"][0]["slug"] != "concepts/" + slug
                ):
                    raise RuntimeError("Small semantic retrieval did not return its expected fact")
                queries.append(time.monotonic() - began)
        return dict(
            status="passed",
            profile="release",
            route="offline" if cpu else "service",
            cpus=os.cpu_count(),
            document_request_input_limit=1,
            input_bytes_limit=INPUT_BYTES,
            query_deadline_seconds=HTTP_SECONDS if cpu else SERVICE_SECONDS,
            cli_timeout_seconds=CLI_SECONDS,
            smoke_timeout_seconds=SMOKE_SECONDS,
            model=config["model"],
            revision=config["revision"],
            coverage=coverage,
            embedding_seconds=samples,
            chunks_per_second=len(FACTS) / sum(samples),
            query_seconds=queries,
            elapsed_seconds=time.monotonic() - started,
        )
