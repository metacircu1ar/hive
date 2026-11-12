"""Native installed tools exercise typed assertions without a provider account."""

import json

from .acceptance import require


def tool(name, **arguments):
    return dict(name="comb_" + name, arguments=arguments)


def replies(turn, actions):
    text = turn("comb:" + json.dumps(actions))
    require(
        text.startswith("comb-results:"), "Installed native graph turn returned no tool results"
    )
    values = json.loads(text.removeprefix("comb-results:"))
    require(len(values) == len(actions), "Installed graph turn lost a tool reply")
    return values


def connectivity(doctor, target, *, complete):
    require(
        doctor["graph"]["current_incomplete_sources"] == (0 if complete else 1),
        "Installed extraction coverage changed",
    )
    kinds = {"orphan", "reference_coverage_incomplete"}
    require(
        not any(f["slug"] == target and f["kind"] in kinds for f in doctor["findings"]),
        "Typed-only installed target was reported disconnected",
    )
    expected = "orphan" if complete else "reference_coverage_incomplete"
    require(
        any(
            f["slug"] == "concepts/installed" and f["kind"] == expected for f in doctor["findings"]
        ),
        "Disconnected control did not exercise the installed orphan check",
    )


def check_graph(invoke, turn):
    source, target = "concepts/installed-graph", "concepts/installed-target"
    query = dict(anchor=source, predicates=["depends_on"])
    values = replies(
        turn,
        [
            {"catalog": True},
            tool("put", slug=target, markdown="---\ntitle: Target\n---\nIndependent target."),
            tool(
                "put",
                slug=source,
                markdown=(
                    f"---\ntitle: Graph\n---\n## Relations\n- [[depends_on::{target}]]"
                    " — installed declaration"
                ),
            ),
            tool("graph_query", **query),
            tool("search", query="unmatched-query", mode="text", graph=query),
            tool("backlinks", slug=target),
            tool("graph", slug=source, depth=2),
        ],
    )
    doc = check_current(invoke, values, source, target)
    invoke(
        "promote", source, "--expected-hash", doc["hash"], "--request-id", "installed-graph-review"
    )
    values = replies(
        turn,
        [
            tool("graph_query", **query, reviewed_only=True),
            tool(
                "put",
                slug="concepts/installed-typo",
                markdown=(
                    "---\ntitle: Typo\n---\n## Relations\n- [[unknown::concepts/installed-target]]"
                ),
            ),
            tool("graph_query", **query),
            tool("graph_query", **query, strict=True),
        ],
    )
    require(all("error" not in v for v in values[:3]), "Installed review/partial graph tool failed")
    require(
        values[0]["complete"]
        and len(values[0]["relations"]) == 1
        and not values[0]["relations"][0]["declaration"]["unverified"],
        "Installed review transition failed",
    )
    require(
        not values[2]["complete"]
        and len(values[2]["relations"]) == 1
        and values[2]["incomplete_reasons"] == ["extraction"],
        "Incomplete extraction hid a valid installed assertion",
    )
    require(
        values[3].get("error", {}).get("code") == "graph_incomplete",
        "Installed strict traversal accepted partial coverage",
    )
    connectivity(invoke("doctor"), target, complete=False)
    return dict(
        native_discovery=True,
        current_unreviewed=True,
        graph_search=True,
        typed_backlinks=True,
        human_review=True,
        partial=True,
        strict=True,
        classic_unchanged=True,
    )


def check_current(invoke, values, source, target):
    require(all("error" not in v for v in values), "Installed graph tool failed")
    catalog = {
        t["name"]: t["inputSchema"] for t in values[0]["tools"] if t["name"].startswith("comb_")
    }
    require(
        catalog["comb_graph_query"]["required"] == ["anchor", "predicates"]
        and "graph" in catalog["comb_search"]["properties"],
        "Native graph discovery lost its schema",
    )
    graph = values[3]
    require(
        graph["complete"]
        and len(graph["relations"]) == 1
        and graph["relations"][0]["declaration"]["unverified"],
        "Installed agent assertion must be visible and unreviewed by default",
    )
    search = values[4]
    require(
        search["mode"] == "text"
        and len(search["results"]) == 1
        and search["results"][0]["slug"] == target
        and search["results"][0]["create_safety"] == "unknown"
        and search["graph"]["complete"],
        "Installed graph search did not supply its independent target",
    )
    require(
        values[5]["complete"]
        and len(values[5]["references"]) == 1
        and not values[5]["references"][0]["ordinary"]
        and values[5]["references"][0]["typed"][0]["unverified"],
        "Installed typed backlink missing",
    )
    require(values[6] == [source], "Installed classic graph traversed typed declarations")
    doc = invoke("get", source)
    span = graph["relations"][0]["declaration"]
    require(
        graph["relations"][0]["source_hash"] == doc["hash"]
        and doc["page"]["compiled"].encode()[span["start"] : span["end"]].decode() == span["text"],
        "Installed declaration citation changed",
    )
    connectivity(invoke("doctor"), target, complete=True)
    return doc
