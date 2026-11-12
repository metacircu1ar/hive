"""Graph preservation observations from native current corpus producers."""

import copy
import json
import re

from .acceptance import require


def without_generation(report):
    value = copy.deepcopy(report)
    generation = value.pop("generation")
    require(
        re.fullmatch("[0-9a-f]{32}", generation["identity"]) is not None
        and type(generation["revision"]) is int
        and generation["revision"] >= 0,
        "Graph generation must remain well formed",
    )
    return value


class GraphCorpus:
    def __init__(self, inputs):
        self.inputs = inputs

    def seed(self, invoke):
        anchor = self.inputs["anchor"]
        invoke(
            "append",
            anchor,
            self.inputs["past_evidence"],
            "--source",
            "compatibility",
            "--expected-hash",
            invoke("get", anchor)["hash"],
            "--request-id",
            "past-declaration",
        )

    def observe(self, invoke):
        q = self.inputs
        args = ("graph-query", q["anchor"], "--predicate", "depends_on", "--to", q["target"])
        graph = invoke(*args)
        self.check_connection(invoke, graph)
        reviewed = invoke(*args, "--reviewed-only")
        require(
            reviewed["complete"] and reviewed["connection"] is None,
            "Unreviewed bridge was traversed",
        )
        backlinks = invoke("backlinks", q["target"])
        refs = backlinks["references"]
        require(
            backlinks["complete"]
            and len(refs) == 1
            and refs[0]["slug"] == q["via"]
            and not refs[0]["ordinary"]
            and refs[0]["typed"] == [{"predicate": "depends_on", "unverified": True}],
            "Typed-only backlink classification changed",
        )
        links = invoke("links", q["anchor"])
        require(
            links["complete"] and [r["slug"] for r in links["references"]] == [q["via"]],
            "Past declaration leaked into current links",
        )
        doctor = invoke("doctor")
        require(
            not any(f["kind"] == "orphan" and f["slug"] == q["target"] for f in doctor["findings"]),
            "Typed-only reference did not suppress an orphan",
        )
        past = doctor["graph"]["past_occurrences"]
        require(
            any(
                p["location"]["source"] == q["anchor"]
                and p["target"] == q["past_target"]
                and not p["unverified"]
                for p in past
            ),
            "Reviewed past declaration was lost",
        )
        request = dict(anchor=q["anchor"], predicates=["depends_on"], to=q["target"])
        search = invoke(
            "search", "unmatched-query", "--mode", "text", "--graph", json.dumps(request)
        )
        require(
            search["mode"] == "text"
            and search["fallback"] is None
            and search["graph"]["complete"]
            and all(
                h["matched_by"] == "graph" and h["create_safety"] == "unknown"
                for h in search["results"]
            )
            and {h["slug"] for h in search["results"]} == {q["via"], q["target"]},
            "Graph search did not supply independent pages",
        )
        search["graph"] = without_generation(search["graph"])
        return dict(
            graph=without_generation(graph),
            reviewed=without_generation(reviewed),
            links=without_generation(links),
            backlinks=without_generation(backlinks),
            diagnostics=without_generation(doctor["graph"]),
            search=search,
        )

    def check_connection(self, invoke, graph):
        q = self.inputs
        require(
            graph["complete"]
            and graph["connection"]["shortest"]
            and graph["connection"]["unverified"],
            "Corpus needs a complete connection with mixed source trust",
        )
        edges = graph["connection"]["edges"]
        require(
            graph["connection"]["nodes"] == [q["anchor"], q["via"], q["target"]]
            and len(edges) == 2,
            "Corpus current connection changed",
        )
        require(
            not edges[0]["declaration"]["unverified"] and edges[1]["declaration"]["unverified"],
            "Corpus declaration trust missing",
        )
        require(
            edges[0]["support_complete"]
            and len(edges[0]["support"]) == 1
            and not edges[0]["support"][0]["unverified"],
            "Corpus annotated evidence was not resolved with independent trust",
        )
        for edge in edges:
            source = invoke("get", edge["source"])
            span = edge["declaration"]
            require(
                source["hash"] == edge["source_hash"]
                and source["page"]["compiled"].encode()[span["start"] : span["end"]].decode()
                == span["text"],
                "Corpus declaration span/hash changed",
            )

    def capture(self, invoke):
        self.before = self.observe(invoke)

    def verify(self, invoke):
        require(
            self.observe(invoke) == self.before,
            "Rebuild/upgrade changed graph provenance or reports",
        )
