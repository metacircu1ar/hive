"""Reject fabricated graph acceptance without starting a model or provider."""

import copy
from pathlib import Path
import unittest
from unittest.mock import patch
from types import SimpleNamespace

from e2e.scripts.comb_graph_live_fixture import (
    facts,
    summary,
    reader_operations,
    wire_operation,
    verify_reader,
)


def fixture(changed=False):
    f = facts()
    a, b, c = f["slugs"]
    docs = {
        slug: dict(
            hash="hash-" + str(i), page=dict(compiled=summary(f, i, changed=changed), timeline=[])
        )
        for i, slug in enumerate(f["slugs"])
    }
    docs[a]["page"]["timeline"] = [dict(text=f"## Relations\n- [[depends_on::{b}]]")]

    def edge(source, target, predicate):
        text = f"[[{predicate}::{target}]]"
        compiled = docs[source]["page"]["compiled"].encode()
        start = compiled.index(text.encode())
        return dict(
            source=source,
            target=target,
            source_hash=docs[source]["hash"],
            predicate=predicate,
            page_unverified=True,
            declaration=dict(
                zone="compiled",
                entry=None,
                start=start,
                end=start + len(text.encode()),
                text=text,
                unverified=True,
            ),
        )

    direct_edge = edge(a, c if changed else b, "related_to" if changed else "depends_on")
    direct = dict(complete=True, relations=[direct_edge])
    connection = (
        None
        if changed
        else dict(
            nodes=[a, b, c],
            shortest=True,
            unverified=True,
            edges=[direct_edge, edge(b, c, "owned_by")],
        )
    )
    path = dict(complete=True, connection=connection)
    reviewed = dict(complete=True, relations=[])
    partial = dict(direct, complete=False, incomplete_reasons=["extraction"])
    backlinks = dict(
        complete=False,
        references=[
            dict(
                slug=a,
                ordinary=[],
                typed=[dict(predicate=direct_edge["predicate"], unverified=True)],
            )
        ],
    )
    searches = [
        dict(
            mode=mode,
            fallback=None,
            graph=direct if changed else path,
            results=[dict(slug=c)],
            coverage=dict(total=3, embedded=3, pending=0, rejected=0),
        )
        for mode in ("text", "vector")
    ]
    gets = [docs[a]] if changed else [docs[a], docs[b], docs[c]]
    outputs = [direct, path, reviewed, partial, None, backlinks, *searches, *gets]
    rows = []
    for i, ((op, args), result) in enumerate(
        zip(reader_operations(f, changed=changed), outputs, strict=True)
    ):
        reply = (
            dict(status="error", error=dict(code="graph_incomplete"))
            if result is None
            else dict(status="ok", result=result)
        )
        rows.append(
            dict(
                start=2 * i,
                end=2 * i + 1,
                claim=dict(id="reader"),
                operation=wire_operation(op, args),
                reply=reply,
            )
        )
    answer = (
        dict(codes={a: f["next_code"]})
        if changed
        else dict(codes=dict(zip(f["slugs"], f["codes"], strict=True)))
    )
    return f, rows, answer, docs


class GraphLiveVerifier(unittest.TestCase):
    def test_cli_propagates_failed_scenario_through_the_real_diagnostic_wrapper(self):
        from e2e.scripts.run_live_comb_graph import cli

        with (
            patch(
                "e2e.scripts.run_live_comb_graph.arguments",
                return_value=SimpleNamespace(model="gpt-6.1-terra", effort="low"),
            ),
            patch("e2e.scripts.run_live_comb_graph.GraphScenario") as scenario,
        ):
            for status in (0, 1):
                scenario.return_value.run.return_value = status
                self.assertEqual(cli(), status)
        self.assertEqual(scenario.return_value.run.call_count, 2)

    def test_live_canary_refuses_expensive_models_and_efforts_before_starting(self):
        from e2e.scripts.run_live_comb_graph import cli

        for model, effort in [("gpt-6-terra", "low"), ("gpt-6-sol", "low"), ("gpt-6.1-terra", "high")]:
            with (
                self.subTest(model=model, effort=effort),
                patch(
                    "e2e.scripts.run_live_comb_graph.arguments",
                    return_value=SimpleNamespace(model=model, effort=effort),
                ),
                patch("e2e.scripts.run_live_comb_graph.GraphScenario") as scenario,
            ):
                self.assertEqual(cli(), 1)
                scenario.assert_not_called()

    def test_reused_reader_gets_distinct_initial_and_updated_request_ids(self):
        from e2e.scripts.run_live_comb_graph import GraphScenario
        from unittest.mock import Mock

        scenario = object.__new__(GraphScenario)
        scenario.graph_facts = facts()
        scenario.args = SimpleNamespace(model="gpt-6-luna", effort="medium")
        scenario.private, scenario.output = Path("run"), Path("unused")
        scenario.timings = {}
        scenario.events = Mock(return_value=[])
        scenario.proxy = SimpleNamespace(rows=lambda: [])
        reader = dict(id="same", thread_id="conversation")
        scenario.next_turn = Mock(return_value=reader)
        with (
            patch("e2e.scripts.run_live_comb_graph.read_json_output", return_value={}),
            patch("e2e.scripts.run_live_comb_graph.check_execution"),
            patch("e2e.scripts.run_live_comb_graph.verify_reader", return_value={}),
            patch("e2e.scripts.run_live_comb_graph.save"),
        ):
            scenario.read({}, reader=reader)
            scenario.read({}, reader=reader, changed=True)
        self.assertEqual(
            [call.args[2] for call in scenario.next_turn.call_args_list],
            ["graph-reader-initial", "graph-reader-updated"],
        )

    def test_both_turns_and_fresh_hidden_codes(self):
        self.assertNotEqual(facts()["codes"], facts()["codes"])
        for changed in (False, True):
            f, rows, answer, docs = fixture(changed)
            self.assertTrue(
                all(verify_reader(f, "reader", rows, answer, docs, changed=changed).values())
            )

    def test_retries_wrong_arguments_fallback_and_guessed_answers_fail(self):
        f, source, answer, docs = fixture()
        for kind in [
            "retry",
            "wrong-agent",
            "budget",
            "mode",
            "fallback",
            "code",
            "span",
            "trust",
            "early-get",
            "strict-success",
        ]:
            rows, value = copy.deepcopy(source), copy.deepcopy(answer)
            if kind == "retry":
                rows.append(copy.deepcopy(rows[-1]))
            elif kind == "wrong-agent":
                rows[0]["claim"]["id"] = "writer"
            elif kind == "budget":
                rows[0]["operation"]["request"]["deadline_ms"] = 500
            elif kind == "mode":
                rows[7]["operation"]["mode"] = "hybrid"
            elif kind == "fallback":
                rows[7]["reply"]["result"]["fallback"] = "text"
            elif kind == "code":
                value["codes"][f["slugs"][0]] = "guessed"
            elif kind == "span":
                rows[0]["reply"]["result"]["relations"][0]["declaration"]["start"] += 1
            elif kind == "trust":
                rows[0]["reply"]["result"]["relations"][0]["declaration"]["unverified"] = False
            elif kind == "early-get":
                rows[8]["start"] = 0
            else:
                rows[4]["reply"] = dict(status="ok", result={})
            with self.subTest(kind=kind), self.assertRaises(RuntimeError):
                verify_reader(f, "reader", rows, value, docs)

    def test_retracted_current_relation_and_retained_evidence_are_required(self):
        f, source, answer, docs = fixture(True)
        for kind in ["resurrection", "lost-history", "stale-read"]:
            rows, current = copy.deepcopy(source), copy.deepcopy(docs)
            if kind == "resurrection":
                rows[1]["reply"]["result"]["connection"] = {"nodes": f["slugs"]}
            elif kind == "lost-history":
                current[f["slugs"][0]]["page"]["timeline"] = []
                rows[8]["reply"]["result"] = current[f["slugs"][0]]
            else:
                rows[8]["reply"]["result"]["hash"] = "old"
            with self.subTest(kind=kind), self.assertRaises(RuntimeError):
                verify_reader(f, "reader", rows, answer, current, changed=True)

    def test_every_independent_path_and_search_citation_must_be_complete(self):
        f, source, answer, docs = fixture()
        for row_index in (1, 6, 7):
            for kind in (
                "empty",
                "disconnected",
                "wrong-predicate",
                "forged-token",
                "forged-hash",
                "trust",
            ):
                rows = copy.deepcopy(source)
                report = copy.deepcopy(rows[row_index]["reply"]["result"])
                path = report["connection"] if row_index == 1 else report["graph"]["connection"]
                if kind == "empty":
                    path["edges"] = []
                elif kind == "disconnected":
                    path["edges"][1]["source"] = f["slugs"][0]
                elif kind == "wrong-predicate":
                    path["edges"][1]["predicate"] = "depends_on"
                elif kind == "forged-token":
                    path["edges"][1]["declaration"]["text"] = "made up"
                elif kind == "forged-hash":
                    path["edges"][1]["source_hash"] = "made up"
                else:
                    path["edges"][1]["declaration"]["unverified"] = False
                rows[row_index]["reply"]["result"] = report
                with self.subTest(row=row_index, kind=kind), self.assertRaises(RuntimeError):
                    verify_reader(f, "reader", rows, answer, docs)
