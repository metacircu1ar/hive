"""Current Comb graph contracts and release-scale observations.

A caller may select a separate release baseline. Each peer creates its own
root; deterministic HTTP embeddings require no model or provider login.
"""

from release.temporary import directory as temporary_directory
from release.process import run as run_process
from release.cancellation import check, pause, finish
from e2e.scripts.harness.hashing import sha256
import argparse
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

from e2e.scripts.harness.checks import require
from release.files import (
    create_directory,
    atomic_write_bytes,
    atomic_write_json,
    atomic_write_text,
    append_jsonl_record,
)
from release.progress import entry, phase, finalizer
from e2e.paths import product_root, require_paths, output_directory


GRAPH_P95_LIMIT_MS = 100
ORDINARY_RATIO_LIMIT = 1.05
ITERATIONS = 13
WARMUPS = 3
FULL_PAGES = 10000
FIXTURE_TIME = "2026-01-02T00:00:00Z"
FIXTURE_AFTER = "2000-01-01T02:00:00+02:00"
FUTURE_TIME = "2100-01-01T00:00:00Z"


def arms(args):
    return ([("baseline", args.baseline)] if args.baseline is not None else []) + [
        ("current", args.binary)
    ]


def percentile(values, fraction):
    values = sorted(values)
    position = (len(values) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(values) - 1)
    return values[lower] + (values[upper] - values[lower]) * (position - lower)


def timings(values):
    return dict(
        samples_ms=values,
        p50_ms=percentile(values, 0.5),
        p95_ms=percentile(values, 0.95),
        p99_ms=percentile(values, 0.99),
    )


def digest(path):
    return sha256(path)


class Peer:
    def __init__(self, binary, directory, socket):
        from release.acceptance import Cli

        self.binary, self.root, self.socket = binary, directory / "knowledge", socket
        directory.mkdir(mode=0o700)
        local = [str(binary), "--root", str(self.root)]
        self.cli = Cli(
            local + ["--socket", str(socket)], timeout=600, prefixes={"init": local, "embed": local}
        )
        self.cli("init")
        self.connection = directory / "reader.json"
        self.cli("credential", "create", "graph-benchmark", "--out", str(self.connection))
        self.credential = json.loads(self.connection.read_text())["credential"]
        self.sequence = 0

    def start(self, stack, embedding=None):
        from release.acceptance import running, wait_for

        command = self.cli.command + ["serve"]
        if embedding is not None:
            command += ["--embedding-config", str(embedding)]
        self.process = stack.enter_context(
            running(command, self.root.parent / "service.log", os.environ)
        )
        wait_for(
            lambda: self.socket.exists() and self.call(dict(op="stats"))["status"] == "ok",
            "graph acceptance service",
            self.process,
        )

    def call(self, operation):
        from release.compatibility_transcripts import exchange

        self.sequence += 1
        # Observational evidence only: keep filesystem barriers out of measured
        # requests. Product pages/state retain their usual durable publication.
        record = self.root.parent / "requests.ndjson"
        append_jsonl_record(
            record, dict(sequence=self.sequence, operation=operation), durable=False
        )
        began = time.perf_counter()
        reply = exchange(
            self.socket,
            dict(protocol=1, credential=self.credential, claim=None, operation=operation),
        )
        self.last_elapsed_ms = (time.perf_counter() - began) * 1000
        append_jsonl_record(
            record,
            dict(sequence=self.sequence, reply=reply, elapsed_ms=self.last_elapsed_ms),
            durable=False,
            max_record_bytes=2 * 1024 * 1024,
        )
        return reply

    def result(self, operation):
        reply = self.call(operation)
        require(reply["status"] == "ok", "Graph acceptance operation failed: " + operation["op"])
        return reply["result"]

    def rss(self):
        result = run_process(
            ["ps", "-o", "rss=", "-p", str(self.process.pid)],
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        )
        return int(result.stdout.strip()) * 1024

    def page(
        self,
        slug,
        title,
        body,
        *,
        reviewed=True,
        kind="concept",
        tag="focus",
        aliases=(),
        at=FIXTURE_TIME,
    ):
        raw = (
            "---\ntitle: "
            + title
            + "\ntype: "
            + kind
            + "\ntags: "
            + json.dumps([tag])
            + "\naliases: "
            + json.dumps(list(aliases))
            + "\nupdated_at: "
            + json.dumps(at)
            + "\n---\n"
            + body
            + "\n"
        )
        path = self.root / (slug + ".md")
        path.parent.mkdir(parents=True, exist_ok=True)
        if reviewed:
            source = self.root.parent / "input.md"
            atomic_write_text(source, raw)
            self.cli("put", slug, "--file", str(source), "--request-id", slug.replace("/", "-"))
        else:
            atomic_write_text(path, raw)


def search(query, *, mode="text", **filters):
    return dict(
        op="search", mode=mode, request=dict(query=query, limit=10, before=FUTURE_TIME, **filters)
    )


def ordinary_cases():
    return [
        ("O01", search("Unique Atlas")),
        ("O02", search("CompassAlias")),
        ("O03", search("concepts/a")),
        ("O04", search("sharedneedle")),
        ("O05", search("durable transactions", mode="vector")),
        ("O06", search("sharedneedle", mode="hybrid")),
        ("O07", search("reviewtie")),
        ("O08", search("timelineprobe")),
        ("O09", search("sharedneedle", kind="concept")),
        ("O10", search("sharedneedle", tag="focus")),
        ("O11", search("sharedneedle", prefix="concepts/")),
        ("O12", search("sharedneedle", after=FIXTURE_AFTER)),
        (
            "O13",
            dict(
                op="search",
                mode="text",
                request=dict(query="sharedneedle", limit=1, before=FUTURE_TIME),
            ),
        ),
        ("O16", search("Unique Atlas")),
        ("O17", dict(op="graph", slug="concepts/a", depth=2)),
    ]


def seed_ordinary(peer):
    peer.page(
        "concepts/a",
        "Unique Atlas",
        "sharedneedle reviewtie. [[concepts/b]]",
        aliases=["CompassAlias"],
    )
    peer.page("concepts/b", "Other", "sharedneedle reviewtie.", reviewed=False)
    peer.page(
        "reports/c",
        "Distractor",
        "Unique Atlas sharedneedle timelineprobe.",
        kind="note",
        tag="other",
    )
    peer.page(
        "reports/old",
        "Old",
        "sharedneedle old observation.",
        reviewed=False,
        tag="other",
        at="1999-12-31T23:59:59Z",
    )
    peer.page(
        "concepts/x",
        "Unrelated",
        "[[concepts/a]]\n\n## Relations\n- [[depends_on::concepts/y]]\n- [[depends_on::concepts/y]]\n- [[related_to::concepts/a]]",
    )
    peer.page("concepts/y", "Endpoint", "Independent endpoint.")
    peer.cli("sync")
    peer.cli(
        "append",
        "concepts/a",
        "timelineprobe: immutable observation.",
        "--source",
        "fixture",
        "--request-id",
        "observation",
    )


def ordinary_control(case, reply):
    require(reply["status"] == "ok", "Ordinary fixture unexpectedly failed: " + case)
    if case == "O17":
        require(
            set(reply["result"]) == {"concepts/a", "concepts/b", "concepts/x"},
            "Classic fixture not exercised",
        )
        return
    hits = reply["result"]["results"]
    require(bool(hits), "Ordinary fixture produced no eligible hits: " + case)
    by_slug = {hit["slug"]: hit for hit in hits}
    if case in ("O05", "O06"):
        result = reply["result"]
        coverage = result["coverage"]
        require(
            result["fallback"] is None
            and coverage["total"] > 0
            and coverage["embedded"] == coverage["total"]
            and coverage["pending"] == coverage["rejected"] == 0,
            "Vector comparison requires complete deterministic embedding coverage",
        )
    if case == "O05":
        slugs = [hit["slug"] for hit in hits]
        require(slugs != sorted(slugs), "Vector fixture collapsed to lexical tie order")
    if case == "O07":
        require(
            "concepts/a" in by_slug
            and "concepts/b" in by_slug
            and not by_slug["concepts/a"]["unverified"]
            and by_slug["concepts/b"]["unverified"],
            "Review tie lacks reviewed and unreviewed participants",
        )
    if case == "O08":
        require(
            {h["zone"] for h in hits} == {"compiled", "timeline"},
            "Timeline/summary comparison not exercised",
        )
    if case == "O12":
        require(
            "concepts/b" in by_slug and "reports/old" not in by_slug,
            "Date interval did not include/exclude its controls",
        )


def embedding_fixture(stack, work, output):
    from release.acceptance import running, wait_for
    from release.compatibility_recipe import recipe

    data = recipe(product_root(), None, synthetic=True)["embeddings"]
    source = work / "hash-embeddings.py"
    atomic_write_bytes(source, data)
    log = output / "hash-embeddings.log"
    process = stack.enter_context(running([sys.executable, str(source)], log, os.environ))
    port = int(wait_for(lambda: log.read_text().strip(), "mock embeddings", process))
    require(0 < port < 65536, "Invalid mock embedding port")
    config = work / "embedding.json"
    atomic_write_json(
        config,
        dict(
            backend="http",
            url=f"http://127.0.0.1:{port}/v1/embeddings",
            model="mock-graph",
            revision=hashlib.sha256(data).hexdigest(),
            dimensions=8,
            query_prefix="query: ",
            document_prefix="passage: ",
        ),
    )
    return config


def regression(args, stack, work):
    embedding = embedding_fixture(stack, work, args.output_dir)
    servers = stack.enter_context(ExitStack())
    peers = [
        Peer(binary, args.output_dir / ("ordinary-" + name), work / (name + "-o.sock"))
        for name, binary in arms(args)
    ]
    results = []
    for peer in peers:
        seed_ordinary(peer)
        peer.start(servers)
        # No embedding configuration yet: exact fallback/error contract.
        rows = [
            ("O14", peer.call(search("sharedneedle", mode="hybrid"))),
            ("O15", peer.call(search("sharedneedle", mode="vector"))),
        ]
        rows += [
            (case, peer.call(op)) for case, op in ordinary_cases() if case not in ("O05", "O06")
        ]
        results.append(dict(rows))
    atomic_write_json(args.output_dir / "ordinary-before-embeddings.json", results)
    for rows in results:
        for case, reply in rows.items():
            if case not in ("O14", "O15"):
                ordinary_control(case, reply)
    require(results[0] == results[-1], "Ordinary peer results changed (no embedder)")
    require(
        results[-1]["O14"]["result"]["fallback"] is not None
        and results[-1]["O15"]["status"] == "error",
        "Fallback/vector-error controls were not exercised",
    )
    # Stop both services before activating embeddings through the local owner.
    servers.close()
    for peer in peers:
        peer.cli("embed", "--config", str(embedding))
        peer.start(servers, embedding)
    for case, operation in ordinary_cases():
        observations = [peer.call(operation) for peer in peers]
        before, after = observations[0], observations[-1]
        atomic_write_json(
            args.output_dir / (case + "-paired.json"),
            dict(request=operation, baseline=before if args.baseline else None, current=after),
        )
        ordinary_control(case, before)
        ordinary_control(case, after)
        require(before == after, "Ordinary baseline result changed: " + case)
        results[-1][case] = after
    current = peers[-1]
    links = current.result(dict(op="links", slug="concepts/x", incoming=False, limit=1))
    require(
        links["complete"]
        and len(links["references"]) == 1
        and links["references"][0]["slug"] == "concepts/a"
        and links["references"][0]["typed"]
        and links["references"][0]["ordinary"] == ["slug"]
        and links["next_after"],
        "O18 ordinary/typed union or pagination missing",
    )
    second = current.result(
        dict(op="links", slug="concepts/x", incoming=False, limit=1, after=links["next_after"])
    )
    require(
        len(second["references"]) == 1
        and second["references"][0]["slug"] == "concepts/y"
        and not second["references"][0]["ordinary"]
        and len(second["references"][0]["typed"]) == 1
        and second["next_after"] is None,
        "O18 duplicate relation or lost page",
    )
    incoming = current.result(dict(op="links", slug="concepts/y", incoming=True, limit=100))
    require(
        len(incoming["references"]) == 1 and incoming["references"][0]["slug"] == "concepts/x",
        "O18 source direction lost",
    )
    doctor = current.result(dict(op="doctor"))
    require(
        not any(f["slug"] == "concepts/y" and f["kind"] == "orphan" for f in doctor["findings"]),
        "O18 typed-only orphan",
    )
    results[-1]["O18"] = dict(links=links, next=second, incoming=incoming)
    atomic_write_json(args.output_dir / "ordinary-results.json", results[-1])
    return sorted(results[-1])


def scale_graph_contract(report, pages):
    if not report["complete"]:
        require(
            report["incomplete_reasons"] == ["deadline"],
            "Scale query failed for an unexpected work bound",
        )
        return
    node = lambda i: f"concepts/p{i % pages:05d}"
    expected_nodes = {node(i) for i in range(21)}
    expected_edges = {
        (node(i), "depends_on", node(i + step)) for i in range(11) for step in range(1, 11)
    }
    nodes = [item["slug"] for item in report["nodes"]]
    edges = [(e["source"], e["predicate"], e["target"]) for e in report["relations"]]
    require(
        len(nodes) == len(expected_nodes) and set(nodes) == expected_nodes,
        "Complete scale traversal lost or substituted its expected nodes",
    )
    require(
        len(edges) == len(expected_edges) and set(edges) == expected_edges,
        "Complete scale traversal lost, duplicated or substituted its expected 110 edges",
    )


def tuning(args, stack, work):
    from e2e.scripts import comb_graph_tuning

    peer = Peer(args.binary, args.output_dir / "tuning", work / "tuning.sock")
    comb_graph_tuning.seed(peer)
    transport = work / "tuning"
    transport.mkdir(mode=0o700)
    config = embedding_fixture(stack, transport, peer.root.parent)
    peer.cli("embed", "--config", str(config))
    peer.start(stack, config)
    result = comb_graph_tuning.measure(peer)
    atomic_write_json(args.output_dir / "tuning-metrics.json", result)
    return result


def scale(args, stack, work):
    peers = [
        Peer(binary, args.output_dir / ("scale-" + name), work / (name + "-s.sock"))
        for name, binary in arms(args)
    ]
    for peer in peers:
        for index in range(args.pages):
            check()
            targets = [f"concepts/p{(index + step) % args.pages:05d}" for step in range(1, 11)]
            body = "loadprobe durable graph corpus.\n\n## Relations\n" + "\n".join(
                f"- [[depends_on::{s}]]" for s in targets
            )
            peer.page(f"concepts/p{index:05d}", f"Page {index:05d}", body, reviewed=False)
        peer.cli("sync")
        peer.start(stack)
        require(
            peer.result(dict(op="stats"))["pages"] == args.pages, "Scale corpus page count is wrong"
        )
    coverage = peers[-1].result(dict(op="doctor"))["graph"]
    require(
        coverage["current_reviewed"] + coverage["current_unreviewed"] == args.pages * 10
        and coverage["current_incomplete_sources"] == 0,
        "Scale corpus edge count or coverage is wrong",
    )
    ordinary = search("loadprobe")
    graph = dict(
        op="graph_query",
        request=dict(anchor="concepts/p00000", predicates=["depends_on"], depth=2, limit=200),
    )
    samples = [[] for _ in peers]
    graph_samples, graph_observations, rss, graph_report = [], [], [0 for _ in peers], None
    for iteration in range(ITERATIONS):
        check()
        replies = [None for _ in peers]
        for index in range(len(peers)) if iteration % 2 == 0 else reversed(range(len(peers))):
            replies[index] = peers[index].result(ordinary)
            elapsed = peers[index].last_elapsed_ms
            if iteration >= WARMUPS:
                samples[index].append(elapsed)
            rss[index] = max(rss[index], peers[index].rss())
        require(
            all(len(r["results"]) == 10 for r in replies),
            "Scale ordinary search did not exercise ten eligible hits",
        )
        require(replies[0] == replies[-1], "Ordinary baseline ranked results changed")
        graph_report = peers[-1].result(graph)
        elapsed = peers[-1].last_elapsed_ms
        if iteration >= WARMUPS:
            graph_samples.append(elapsed)
        rss[-1] = max(rss[-1], peers[-1].rss())
        graph_observations.append(
            dict(
                iteration=iteration,
                warmup=iteration < WARMUPS,
                elapsed_ms=elapsed,
                complete=graph_report["complete"],
                reasons=graph_report["incomplete_reasons"],
            )
        )
        atomic_write_json(
            args.output_dir / "scale-samples.json",
            dict(ordinary=samples, graph=graph_observations, sampled_peak_rss_bytes=rss),
            durable=False,
        )
        scale_graph_contract(graph_report, args.pages)
    baseline = timings(samples[0]) if args.baseline else None
    current, graph_times = timings(samples[-1]), timings(graph_samples)
    ratio = current["p95_ms"] / baseline["p95_ms"] if baseline else None
    incomplete = sum(not r["complete"] for r in graph_observations)
    return dict(
        pages=args.pages,
        edges=args.pages * 10,
        warmups=WARMUPS,
        repetitions=ITERATIONS - WARMUPS,
        baseline=baseline,
        current=current,
        graph=graph_times,
        sampled_peak_rss_bytes=rss,
        graph_work=graph_report["work"],
        graph_coverage=graph_report["coverage"],
        graph_observations=graph_observations,
        graph_incomplete_count=incomplete,
        current_over_baseline_p95=ratio,
        graph_p95_gate=incomplete == 0 and graph_times["p95_ms"] < GRAPH_P95_LIMIT_MS,
        ordinary_regression_gate=ratio <= ORDINARY_RATIO_LIMIT if ratio is not None else None,
    )


def execute(args):
    result = dict(
        status="failed",
        host=platform.platform(),
        machine=platform.machine(),
        profile="release",
        cache="warm",
        baseline_sha256=digest(args.baseline) if args.baseline else None,
        binary_sha256=digest(args.binary),
        qualifying_size=args.pages == FULL_PAGES,
        ordinary_cases=[],
    )
    # Always leave a failed receipt on exceptions; native causes remain at entry.
    # Requests and responses are already retained before any assertion.
    with finalizer(lambda: atomic_write_json(args.output_dir / "result.json", result)):
        with temporary_directory(prefix="hx-graph-", parent="/tmp") as directory:
            work = Path(directory)
            with ExitStack() as stack, phase("verify"):
                result["ordinary_cases"] = regression(args, stack, work)
            with ExitStack() as stack, phase("verify"):
                result["tuning"] = tuning(args, stack, work)
            with ExitStack() as stack, phase("verify"):
                result.update(scale(args, stack, work))
        passed = (
            result["graph_p95_gate"]
            and (args.baseline is None or result["ordinary_regression_gate"])
            and result["qualifying_size"]
        )
        smoke = getattr(args, "smoke", False) and result["graph_incomplete_count"] == 0
        if passed or smoke:

            def publish():
                result["status"] = "passed" if passed else "smoke_passed"
                atomic_write_json(args.output_dir / "result.json", result)
                print(json.dumps(result, indent=2))

            finish(publish)
        else:
            print(json.dumps(result, indent=2))
        return 0 if passed or smoke else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, help="Optional caller-selected release baseline")
    parser.add_argument("--binary", type=Path, default=product_root() / "target/release/comb")
    parser.add_argument("--output-dir", type=Path, default=output_directory("graph"))
    parser.add_argument("--pages", type=int, default=FULL_PAGES)
    parser.add_argument(
        "--smoke", action="store_true", help="Reduced contract check, not a scale-performance pass"
    )
    args = parser.parse_args()
    require(args.pages >= 11, "Need at least eleven pages for distinct degree-ten targets")
    require_paths(args, parser, "binary", *(["baseline"] if args.baseline else []), product="Comb")
    args.output_dir = args.output_dir.resolve()
    create_directory(args.output_dir)
    raise SystemExit(execute(args))


def cli():
    return entry(main, name="scenario")


if __name__ == "__main__":
    raise SystemExit(cli())
