"""Eight disjoint graph tuning shapes; report metrics without changing weights."""

from release.process import run as run_process
from release.cancellation import check, pause
from e2e.scripts.harness.hashing import sha256
from e2e.scripts.harness.checks import require


SHAPES = {
    "one-edge": [("a", "depends_on", "b")],
    "chain": [("a", "depends_on", "b"), ("b", "depends_on", "c"), ("c", "depends_on", "d")],
    "star": [("a", "depends_on", "b"), ("a", "depends_on", "c"), ("a", "depends_on", "d")],
    "diamond": [
        ("a", "depends_on", "b"),
        ("a", "depends_on", "c"),
        ("b", "depends_on", "d"),
        ("c", "depends_on", "d"),
    ],
    "cycle": [("a", "depends_on", "b"), ("b", "depends_on", "c"), ("c", "depends_on", "a")],
    "mixed": [("a", "depends_on", "b"), ("b", "owned_by", "c"), ("a", "related_to", "d")],
    "duplicates": [("a", "depends_on", "b"), ("a", "depends_on", "b")],
    "disconnected": [("a", "depends_on", "b"), ("c", "depends_on", "d")],
}


def cases():
    result = []
    for name, edges in SHAPES.items():
        prefix = "tuning/" + name + "/"
        all_edges = [(prefix + a, p, prefix + b) for a, p, b in edges]
        # The disconnected component is deliberately indexed but not reachable.
        expected = {edge for edge in all_edges if name != "disconnected" or edge[0] == prefix + "a"}
        relevant = {s for a, _, b in expected for s in (a, b)} - {prefix + "a"}
        result.append(
            dict(
                name=name,
                prefix=prefix,
                anchor=prefix + "a",
                authored=all_edges,
                expected=expected,
                relevant=relevant,
            )
        )
    return result


def seed(peer):
    for case in cases():
        nodes = sorted({s for a, _, b in case["authored"] for s in (a, b)})
        for node in nodes:
            outgoing = [
                f"- [[{predicate}::{target}]]"
                for source, predicate, target in case["authored"]
                if source == node
            ]
            body = "tuningneedle synthetic document " + node
            if outgoing:
                body += "\n\n## Relations\n" + "\n".join(outgoing)
            peer.page(node, "Tuning " + node, body)


def rank_metrics(slugs, relevant, cutoff):
    found = len(set(slugs[:cutoff]) & relevant)
    return dict(
        cutoff=cutoff,
        relevant_retrieved=found,
        relevant_total=len(relevant),
        precision=found / cutoff,
        recall=found / len(relevant),
    )


def measure(peer):
    reports = []
    for case in cases():
        operation = dict(
            op="search",
            mode="hybrid",
            request=dict(
                query="tuningneedle",
                limit=10,
                prefix=case["prefix"],
                before="2100-01-01T00:00:00Z",
                graph=dict(
                    anchor=case["anchor"],
                    predicates=sorted({p for _, p, _ in case["authored"]}),
                    prefix=case["prefix"],
                    depth=3,
                    limit=200,
                ),
            ),
        )
        result = peer.result(operation)
        coverage = result["coverage"]
        require(
            result["mode"] == "hybrid"
            and result["fallback"] is None
            and coverage["total"] > 0
            and coverage["embedded"] == coverage["total"]
            and coverage["pending"] == coverage["rejected"] == 0,
            "Tuning comparison needs complete deterministic embedding coverage",
        )
        graph = result["graph"]
        edges = [(e["source"], e["predicate"], e["target"]) for e in graph["relations"]]
        require(
            graph["complete"]
            and len(edges) == len(case["expected"])
            and set(edges) == case["expected"],
            "Tuning graph shape was not retrieved correctly: " + case["name"],
        )
        slugs = [hit["slug"] for hit in result["results"]]
        reports.append(
            dict(
                case=case["name"],
                request=operation,
                reply=result,
                relevance=sorted(case["relevant"]),
                correctness_passed=True,
                metrics=[rank_metrics(slugs, case["relevant"], k) for k in (3, 10)],
            )
        )
    return dict(set="disjoint_tuning", weights="unchanged_defaults", cases=reports)
