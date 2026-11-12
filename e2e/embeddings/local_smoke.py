"""Opt-in release retrieval and indexing smoke against a local HTTP embedding server."""

from release.temporary import directory as temporary_directory
from release.process import run as run_process
from release.cancellation import check, pause, finish
from e2e.scripts.harness.hashing import sha256
from release.files import atomic_write_text, raw_log
import argparse
import hashlib
import json
import pathlib
import resource
import sys
import signal
import statistics
import subprocess
import tempfile
import time

from e2e.paths import product_default, product_root, require_paths
from release.progress import entry, finalizer
from release.owned_process import spawn
from release.process import json_reply


TOPICS = [
    (
        "writer",
        "SQLite commit durability",
        "The database writer serializes mutations. A successful commit makes messages durable before acknowledgements are returned. Read-only requests use a separate connection.",
        "How do we retain messages after a crash?",
    ),
    (
        "approval",
        "Human approval requests",
        "An agent asks a human for a decision using an approval request. The human can accept, decline or cancel. The answer wakes the same agent conversation.",
        "How can an agent ask a person for permission?",
    ),
    (
        "branch",
        "Conversation branching",
        "Branching starts an independent agent conversation from another agent's confirmed history. An active source finishes its current turn before the branch is materialized.",
        "How can I start another conversation with existing context?",
    ),
    (
        "lifetime",
        "Parent-bound lifetimes",
        "A child with the with_parent lifetime expires when its parent completes or is terminated. Independent children survive. Stopping or failing a parent does not expire its children.",
        "What happens to dependent children when a parent completes?",
    ),
    (
        "receipt",
        "Mutation replay receipts",
        "A write request has a stable request identifier. If its acknowledgement is lost, retrying the exact request returns the durable receipt without repeating the mutation.",
        "How can a caller retry after losing a write acknowledgement?",
    ),
    (
        "embedding",
        "Embedding space identity",
        "Document vectors belong to a model and preprocessing configuration. Changing the model retires the old vector space. Delayed results cannot overwrite vectors for changed inputs.",
        "Why must delayed vectors be checked against the current model?",
    ),
    (
        "evidence",
        "Atomic summaries and evidence",
        "Writers append sourced observations and update the summary in the same atomic operation. Existing timeline evidence is immutable. The summary cites evidence identifiers.",
        "How should a new observation change a knowledge page?",
    ),
    (
        "retention",
        "Agent garbage collection",
        "Garbage collection purges agents that ended permanently and released their resources. It removes owned output and Hive records while retaining native provider history and workspace files.",
        "What does purging a finished agent remove?",
    ),
    (
        "steering",
        "Same-turn guidance",
        "Steering adds guidance to an active provider turn at a supported boundary. A redirect interrupts that turn and queues guidance for the next turn on the same conversation.",
        "How does redirect differ from steering?",
    ),
    (
        "scheduler",
        "Resident conversation scheduling",
        "Loaded conversations consume provider capacity. Agents with waiting messages receive priority. Idle conversations can be unloaded and later resumed from their saved native history.",
        "Why do waiting replies need scheduling priority?",
    ),
]


def digest(path):
    return sha256(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=pathlib.Path, default=product_default("target/release/comb")
    )
    parser.add_argument(
        "--embedding-config",
        type=pathlib.Path,
        default=product_default("examples/qwen3-embedding.json"),
    )
    parser.add_argument("--copies", type=int, default=6)
    parser.add_argument("--query-rounds", type=int, default=3)
    parser.add_argument(
        "--full",
        action="store_true",
        help="Include long inputs, dense splitting and background-indexing benchmark",
    )
    args = parser.parse_args()
    require_paths(args, parser, "binary", "embedding_config", product="Comb")
    binary, config = args.binary.resolve(), args.embedding_config.resolve()
    if not args.full:
        from e2e.embeddings.smoke import execute

        report = execute(binary, config)
        finish(lambda: print(json.dumps(report), flush=True))
        return
    if (
        "release" not in binary.parts
        or not 1 <= args.copies <= 20
        or not 1 <= args.query_rounds <= 10
    ):
        parser.error("Use a release binary, 1..20 copies and 1..10 query rounds")
    # The binary accepts YAML too. Use its JSON configurations for this driver,
    # so the harness itself has no optional parser dependency.
    configuration = json.loads(config.read_text())
    with temporary_directory(prefix="hb-model-", parent="/tmp") as temporary:
        root = pathlib.Path(temporary)
        comb, corpus, socket = root / "comb", root / "corpus", root / "comb.sock"
        corpus.mkdir()

        def call(*command, allow_error=False):
            prefix = [str(binary), "--root", str(comb)]
            if command[0] not in ("init", "embed"):
                prefix += ["--socket", str(socket)]
            result = run_process(
                [*prefix, *command], capture_output=True, text=True, timeout=180, check=False
            )
            if allow_error:
                # This explicit negative probe checks its expected exit and error below.
                if result.returncode != 2:
                    raise RuntimeError("Invalid-input probe returned unexpected exit status")
                return json.loads(result.stdout)
            return json_reply(result)

        call("init")
        for slug, title, text, _ in TOPICS:
            for number in range(args.copies):
                atomic_write_text(
                    corpus / f"{slug}-{number:02}.md",
                    f"---\ntitle: {title}\ntype: concept\n---\n{text}\n\nFixture observation {number}.\n",
                )
        # A dense body exceeds one Qwen slot but fits after one split. Its row
        # must be embedded through the documented length-weighted piece vector.
        dense = "0123456789" * 900
        atomic_write_text(
            corpus / "split-body.md", "---\ntitle: Split dense evidence\n---\n" + dense
        )
        # Every piece repeats its heading. This oversized heading with a short
        # body cannot be rescued, and must remain explicitly rejected.
        atomic_write_text(
            corpus / "token-limit.md",
            "---\ntitle: Oversized heading\n---\n# " + dense + "\n\nShort body.\n",
        )
        call("import", str(corpus), "--prefix", "concepts")
        started = time.monotonic()
        coverage = call("embed", "--config", str(config))
        embedding_seconds = time.monotonic() - started
        if coverage != dict(
            total=len(TOPICS) * args.copies + 2,
            embedded=len(TOPICS) * args.copies + 1,
            rejected=1,
            pending=0,
        ):
            raise RuntimeError({"unexpected_coverage": coverage})

        log = root / "server.log"
        with raw_log(log) as output:
            started = time.monotonic()
            with spawn(
                [
                    str(binary),
                    "--root",
                    str(comb),
                    "--socket",
                    str(socket),
                    "serve",
                    "--embedding-config",
                    str(config),
                ],
                stdout=output,
                stderr=output,
                stop_signal=signal.SIGINT,
                stop_grace=10,
                limit=64 * 1024**2,
            ) as server:
                deadline = started + 90
                while not socket.exists():
                    if server.exited() or time.monotonic() >= deadline:
                        raise RuntimeError(log.read_text())
                    pause(0.02)
                runtime = call("stats")["embedding_runtime"]
                if call("doctor")["stats"]["embedding_runtime"] != runtime:
                    raise AssertionError("Stats/doctor runtime selection differs")
                if runtime["provider"] not in ("cpu", "cuda", "http"):
                    raise AssertionError({"invalid_runtime": runtime})
                service_start_seconds = time.monotonic() - started
                samples, checks = [], []
                queries = [(slug, query) for slug, _, _, query in TOPICS]
                queries += [
                    ("approval", "Как агент может запросить разрешение у человека?"),
                    ("receipt", "Comment réessayer une écriture sans créer de doublon ?"),
                ]
                for _ in range(args.query_rounds):
                    for expected, query in queries:
                        one = time.monotonic()
                        response = call("search", query, "--mode", "vector", "--limit", "5")
                        if response["mode"] != "vector" or response["fallback"] is not None:
                            raise AssertionError("Requested vector search fell back")
                        samples.append((time.monotonic() - one) * 1000)
                        checks.append(
                            dict(
                                query=query,
                                expected_topic=expected,
                                top5=[hit["slug"] for hit in response["results"]],
                                match=any(
                                    hit["slug"].startswith(f"concepts/{expected}-")
                                    for hit in response["results"]
                                ),
                            )
                        )
                # Comb's public byte limit is independent of the model token limit.
                rejected = call("search", "x" * 4097, "--mode", "vector", allow_error=True)
                if (
                    rejected.get("error", {}).get("code") != "invalid_value"
                    or rejected["error"].get("field") != "QUERY"
                ):
                    raise RuntimeError("Oversized public query was not rejected")
                if not all(check["match"] for check in checks):
                    raise AssertionError(
                        {
                            "expected_topic_missing_from_top5": [
                                check for check in checks if not check["match"]
                            ]
                        }
                    )
                # Import fresh pages while serving queries against the original corpus.
                # Actual GPU contention is measured; a separate HTTP lane is not a
                # promise that the native server prioritizes queries over documents.
                background = root / "background"
                background.mkdir()
                for number in range(32):
                    atomic_write_text(
                        background / f"page-{number:02}.md",
                        f"---\ntitle: Background indexing measurement {number}\n---\n"
                        + (
                            "The weather station measures daily rainfall, air pressure and wind direction. "
                            * 12
                        )
                        + f"\nObservation identifier: {number}.\n",
                    )
                one = time.monotonic()
                call("import", str(background), "--prefix", "background")
                concurrent = []
                while True:
                    before = call("doctor")["embeddings"]
                    if before["pending"] == 0:
                        break
                    if time.monotonic() - one > 180:
                        raise AssertionError("Background indexing did not drain")
                    query_started = time.monotonic()
                    found = call("search", queries[0][1], "--mode", "vector", "--limit", "5")
                    if (
                        found["fallback"] is not None
                        or found["mode"] != "vector"
                        or not any(
                            hit["slug"].startswith("concepts/writer-") for hit in found["results"]
                        )
                    ):
                        raise AssertionError(
                            "Query during background indexing lost its expected result"
                        )
                    concurrent.append(
                        dict(
                            latency_ms=(time.monotonic() - query_started) * 1000,
                            pending_before=before["pending"],
                            pending_after=call("doctor")["embeddings"]["pending"],
                        )
                    )
                if (
                    not concurrent
                    or before["rejected"] != 1
                    or before["embedded"] != coverage["embedded"] + 32
                ):
                    raise AssertionError({"background_coverage": before, "queries": concurrent})
                if not any(query["pending_after"] > 0 for query in concurrent):
                    raise AssertionError(
                        "Searches only returned after the entire indexing backlog drained"
                    )
                background_seconds = time.monotonic() - one
                rss = run_process(
                    ["ps", "-o", "rss=", "-p", str(server.pid)],
                    capture_output=True,
                    text=True,
                    check=True,
                    timeout=5,
                )
                ordered = sorted(samples)
                report = dict(
                    profile="release",
                    backend=configuration["backend"],
                    device_requested="server-owned",
                    runtime=call("stats").get("embedding_runtime"),
                    embedding_config_sha256=digest(config),
                    binary_sha256=digest(binary),
                    corpus_pages=len(TOPICS) * args.copies + 2,
                    coverage=coverage,
                    embed_wall_seconds=embedding_seconds,
                    http_model_server_start_included=False,
                    embedded_chunks_per_second=coverage["embedded"] / embedding_seconds,
                    rejected_chunk_fraction=coverage["rejected"] / coverage["total"],
                    service_start_seconds=service_start_seconds,
                    service_rss_after_queries_bytes=int(rss.stdout.strip()) * 1024,
                    query_latency_includes_cli_and_socket=True,
                    query_samples_ms=samples,
                    query_median_ms=statistics.median(samples),
                    query_p95_ms=ordered[int(0.95 * (len(ordered) - 1))],
                    query_max_ms=max(samples),
                    expected_topic_top5=sum(check["match"] for check in checks),
                    query_count=len(checks),
                    query_checks=checks,
                    overlong_query_error=rejected["error"],
                    background_indexing_seconds=background_seconds,
                    background_final_coverage=before,
                    queries_during_background_indexing=concurrent,
                )
            # wait() above includes the server in RUSAGE_CHILDREN. This is the
            # highest individual child RSS, including native startup and backfill.
            report["max_child_peak_rss_bytes"] = resource.getrusage(
                resource.RUSAGE_CHILDREN
            ).ru_maxrss * (1 if sys.platform == "darwin" else 1024)
            finish(lambda: print(json.dumps(report, ensure_ascii=False), flush=True))


if __name__ == "__main__":
    raise SystemExit(entry(main, name="verify"))
