"""Audited graph scenario expectations, independent of model self-reporting."""

import secrets

from e2e.scripts.harness.checks import require


def facts():
    prefix = "concepts/graph-" + secrets.token_hex(4) + "/"
    slugs = [prefix + name for name in ("gateway", "journal", "team")]
    codes = [secrets.token_hex(12) for _ in slugs]
    return dict(
        prefix=prefix,
        slugs=slugs,
        codes=codes,
        next_code=secrets.token_hex(12),
        bad="review/graph-" + secrets.token_hex(4),
        query="Which journal preserves the gateway transactions and which team owns it?",
    )


def summary(f, index, *, changed=False):
    code = f["next_code"] if changed and index == 0 else f["codes"][index]
    text = [
        "Aurora gateway records durable transactions.",
        "Nimbus journal preserves gateway transactions after power loss.",
        "Cirrus team owns the durable journal.",
    ][index]
    relation = (
        ([f"related_to::{f['slugs'][2]}"] if changed else [f"depends_on::{f['slugs'][1]}"])
        if index == 0
        else ([f"owned_by::{f['slugs'][2]}"] if index == 1 else [])
    )
    return (
        text
        + "\nVerification code: "
        + code
        + (
            ("\n\n## Relations\n- [[" + relation[0] + "]] — fictional assertion")
            if relation
            else ""
        )
    )


def graph(f, **overrides):
    return dict(
        anchor=f["slugs"][0], predicates=["depends_on", "owned_by"], prefix=f["prefix"], **overrides
    )


def reader_operations(f, *, changed=False):
    a, b, c = f["slugs"]
    q = graph(f)
    direct = dict(q, predicates=["related_to" if changed else "depends_on"], depth=1)
    path = dict(q, to=c)
    unfiltered = dict(direct)
    unfiltered.pop("prefix")
    request = dict(direct if changed else path)
    return [
        ("graph_query", direct),
        ("graph_query", path),
        ("graph_query", dict(direct, reviewed_only=True)),
        ("graph_query", unfiltered),
        ("graph_query", dict(unfiltered, strict=True)),
        ("backlinks", dict(slug=c if changed else b)),
        ("search", dict(query="Aurora gateway", mode="text", limit=3, graph=request)),
        ("search", dict(query=f["query"], mode="vector", limit=3, graph=request)),
        *[("get", dict(slug=slug)) for slug in ([a] if changed else [a, b, c])],
    ]


def calls_for(rows, agent):
    return [row for row in rows if (row.get("claim") or {}).get("id") == agent]


def query_defaults(request):
    value = dict(
        to=None,
        direction="outgoing",
        depth=2,
        reviewed_only=False,
        strict=False,
        kind=None,
        tag=None,
        prefix=None,
        after=None,
        before=None,
        max_nodes=200,
        max_edges=1000,
        limit=50,
        max_evidence_bytes=65536,
        deadline_ms=100,
    )
    value.update(request)
    return value


def wire_operation(op, arguments):
    if op == "graph_query":
        return dict(op=op, request=query_defaults(arguments))
    if op == "search":
        request = dict(kind=None, tag=None, prefix=None, after=None, before=None, **arguments)
        mode = request.pop("mode")
        request["graph"] = query_defaults(request["graph"])
        return dict(op=op, request=request, mode=mode)
    if op == "backlinks":
        return dict(op="links", incoming=True, after=None, limit=100, **arguments)
    return dict(op=op, **arguments)


def verify_calls(f, agent, rows, *, changed=False):
    calls = calls_for(rows, agent)
    required = reader_operations(f, changed=changed)
    require(len(calls) == len(required), "Reader must make each prescribed call exactly once")
    results = []
    for row, (op, requested) in zip(calls, required, strict=True):
        require(
            row["operation"] == wire_operation(op, requested),
            "Reader changed prescribed calls, filters, budgets or ordering",
        )
        reply = row["reply"]
        if requested.get("strict"):
            require(
                reply["status"] == "error" and reply["error"]["code"] == "graph_incomplete",
                "Strict live query must refuse known partial extraction",
            )
            results.append(None)
        else:
            require(reply["status"] == "ok", "Unexpected live graph tool failure")
            results.append(reply["result"])
    return calls, results


def verify_edges(edges, expected, documents):
    require(
        [(e["source"], e["predicate"], e["target"]) for e in edges] == expected,
        "Missing, disconnected, reordered or substituted path edge",
    )
    for edge in edges:
        doc = documents[edge["source"]]
        span = edge["declaration"]
        token = f"[[{edge['predicate']}::{edge['target']}]]"
        require(
            edge["source_hash"] == doc["hash"]
            and edge["page_unverified"]
            and span["zone"] == "compiled"
            and span["entry"] is None
            and span["unverified"]
            and span["text"] == token
            and doc["page"]["compiled"].encode()[span["start"] : span["end"]].decode() == token,
            "Live graph citation token, span, hash or trust mismatch",
        )


def verify_path(report, nodes, edges, documents):
    path = report["connection"]
    require(
        path is not None and path["nodes"] == nodes and path["shortest"] and path["unverified"],
        "Live two-hop connection is wrong",
    )
    verify_edges(path["edges"], edges, documents)


def verify_reader(f, agent, rows, answer, documents, *, changed=False):
    calls, results = verify_calls(f, agent, rows, changed=changed)
    direct, path, reviewed, partial, _, backlinks, literal, semantic, *gets = results
    a, b, c = f["slugs"]
    target = c if changed else b
    require(direct["complete"] and len(direct["relations"]) == 1, "Direct relation missing")
    direct_edges = [(a, "related_to" if changed else "depends_on", target)]
    path_edges = [(a, "depends_on", b), (b, "owned_by", c)]
    verify_edges(direct["relations"], direct_edges, documents)
    require(path["complete"], "Filtered connection incomplete")
    if changed:
        require(path["connection"] is None, "Retracted dependency resurrected from past evidence")
    else:
        verify_path(path, [a, b, c], path_edges, documents)
    require(
        reviewed["complete"] and not reviewed["relations"], "Unreviewed source passed reviewed-only"
    )
    require(
        not partial["complete"]
        and partial["incomplete_reasons"] == ["extraction"]
        and len(partial["relations"]) == 1,
        "Partial coverage hid a valid relation",
    )
    verify_edges(partial["relations"], direct_edges, documents)
    require(
        not backlinks["complete"]
        and any(
            r["slug"] == a
            and not r["ordinary"]
            and r["typed"]
            == [dict(predicate="related_to" if changed else "depends_on", unverified=True)]
            for r in backlinks["references"]
        ),
        "Typed-only incoming reference missing",
    )
    for mode, result in [("text", literal), ("vector", semantic)]:
        require(
            result["mode"] == mode and result["fallback"] is None and result["graph"]["complete"],
            "Requested graph search fell back or lost its report",
        )
        if changed:
            verify_edges(result["graph"]["relations"], direct_edges, documents)
        else:
            verify_path(result["graph"], [a, b, c], path_edges, documents)
        require(result["results"], "Live search returned no ranked pages")
    coverage = semantic["coverage"]
    require(
        coverage["total"] > 0 and coverage["pending"] == coverage["rejected"] == 0,
        "Real model embedding coverage incomplete",
    )
    # All gets are after both actual search replies, never just a guessed answer.
    require(
        all(row["start"] >= calls[7]["end"] for row in calls[8:]),
        "Source was read before searches finished",
    )
    expected_slugs = [a] if changed else [a, b, c]
    expected_codes = [f["next_code"]] if changed else f["codes"]
    require(
        answer == dict(codes=dict(zip(expected_slugs, expected_codes, strict=True))),
        "Reader did not recover hidden current verification codes",
    )
    for slug, doc in zip(expected_slugs, gets, strict=True):
        require(
            doc == documents[slug], "Native source read differs from independent current document"
        )
    if changed:
        require(
            any(f"[[depends_on::{b}]]" in e["text"] for e in gets[0]["page"]["timeline"]),
            "Reader did not observe preserved past assertion",
        )
    return dict(
        direct=True,
        connection=True,
        trust=True,
        partial=True,
        strict=True,
        backlinks=True,
        text=True,
        vector=True,
        source_spans=True,
        hidden_codes=True,
    )
