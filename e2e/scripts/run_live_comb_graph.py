#!/usr/bin/env python3
"""Real Codex writer/reader turns with local Qwen graph retrieval and retraction.

Requires an authenticated current state with no
running daemon, matched release binaries and the selected Qwen embedding server.
No model retries, fallback retrieval, copied login or review promotion is used.
"""

import json
import time
from release.cancellation import pause

from e2e.scripts.comb_graph_live_fixture import (
    facts,
    summary,
    reader_operations,
    calls_for,
    verify_reader,
)
from e2e.scripts.harness.checks import require
from e2e.scripts.product_constants import LIVE_CODEX_EFFORT, LIVE_CODEX_MODEL
from release.files import atomic_write_text
from release.progress import entry
from e2e.scripts.run_live_comb import (
    Scenario,
    arguments,
    check_execution,
    read_json_output,
    save,
)


class GraphScenario(Scenario):
    name = "comb-graph"
    collector_capable = False

    def __init__(self, args):
        super().__init__(args)
        self.graph_facts = facts()
        save(self.output / "expected-graph.json", self.graph_facts)

    def documents(self):
        return {slug: self.hb("get", slug) for slug in self.graph_facts["slugs"]}

    def next_turn(self, agent, body, label):
        task = self.output / (label + "-task.md")
        atomic_write_text(task, body)
        sent = self.cli(
            "send", agent, "--body-file", str(task), "--request-id", self.private.name + "-" + label
        )
        return self.wait([agent], complete=False, message=sent["message_id"])[0]

    def writer(self):
        f = self.graph_facts
        pages = [
            dict(slug=slug, markdown="---\ntitle: " + title + "\n---\n" + summary(f, i))
            for i, (slug, title) in enumerate(
                zip(f["slugs"], ["Aurora gateway", "Nimbus journal", "Cirrus team"], strict=True)
            )
        ]
        pages.append(
            dict(
                slug=f["bad"],
                markdown="---\ntitle: Deliberate malformed fixture\n---\n## Relations\n- [[unknown::"
                + f["slugs"][0]
                + "]]",
            )
        )
        body = (
            "Sequentially call comb_put once for each exact page: "
            + json.dumps(pages)
            + ". Then comb_append once to "
            + f["slugs"][0]
            + ' with source="graph-fixture" and text='
            + json.dumps(
                "Initial fictional assertion.\n## Relations\n- [[depends_on::"
                + f["slugs"][1]
                + "]]"
            )
            + '. Preserve the summary. Return {"written":true}, without hive_complete; wait for a later message.'
        )
        started = time.monotonic()
        writer = self.spawn("comb-writer", body, "graph-writer")
        state = self.wait([writer], complete=False)[0]
        check_execution(state, self.events(writer), self.args.model, self.args.effort)
        rows = calls_for(self.proxy.rows(), writer)
        require(
            [r["operation"]["op"] for r in rows] == ["put"] * 4 + ["append"]
            and all(r["reply"]["status"] == "ok" for r in rows),
            "Writer did not publish its exact assigned fixtures",
        )
        docs = self.documents()
        for i, slug in enumerate(f["slugs"]):
            require(
                docs[slug]["page"]["compiled"].strip() == summary(f, i),
                "Writer changed assigned summary or hidden code",
            )
        require(
            len(docs[f["slugs"][0]]["page"]["timeline"]) == 1,
            "Initial evidence was not appended once",
        )
        self.timings["writer_seconds"] = time.monotonic() - started
        return state, docs

    def embeddings(self):
        deadline = time.monotonic() + 120
        while True:
            coverage = self.hb("search", "embedding readiness", "--mode", "vector", "--limit", "1")[
                "coverage"
            ]
            require(coverage["rejected"] == 0, "Graph fixture embedding rejected")
            if coverage["total"] > 0 and coverage["pending"] == 0:
                return coverage
            require(
                time.monotonic() < deadline, "Graph embeddings did not finish within 120 seconds"
            )
            pause(0.25)

    def read(self, docs, reader=None, *, changed=False):
        f = self.graph_facts
        body = (
            "Use the native tools in this exact sequential order, with these arguments: "
            + json.dumps(
                [
                    dict(tool="comb_" + op, arguments=args)
                    for op, args in reader_operations(f, changed=changed)
                ]
            )
            + ". The strict query intentionally returns graph_incomplete: record it and continue, without retry. "
            "After the searches read the requested current pages, including their past evidence. "
            'Return only {"codes":{SLUG: CURRENT_VERIFICATION_CODE}} for each requested get. '
            "Do not guess from earlier context. Do not call hive_complete yet."
        )
        cutoff = time.monotonic()
        if reader is None:
            agent_id = self.spawn("comb-reader", body, "graph-reader")
            state = self.wait([agent_id], complete=False)[0]
        else:
            agent_id = reader["id"]
            state = self.next_turn(
                agent_id, body, "graph-reader-updated" if changed else "graph-reader-initial"
            )
            require(
                state["thread_id"] == reader["thread_id"],
                "Reader conversation changed across correction",
            )
        events = self.events(agent_id)
        check_execution(state, events, self.args.model, self.args.effort)
        answer = read_json_output(events)
        rows = [r for r in self.proxy.rows() if r["start"] >= cutoff]
        checks = verify_reader(f, agent_id, rows, answer, docs, changed=changed)
        self.timings["reader_updated_seconds" if changed else "reader_initial_seconds"] = (
            time.monotonic() - cutoff
        )
        save(
            self.output / ("reader-updated.json" if changed else "reader-initial.json"),
            dict(answer=answer, checks=checks),
        )
        return state, checks

    def update(self, writer, before):
        f = self.graph_facts
        a = f["slugs"][0]
        body = (
            "Call comb_get for "
            + a
            + ". Then call comb_append exactly once, using that expected_hash, "
            'source="graph-retraction", text="The dependency was retracted; the gateway is now associated with the team.", '
            "and this exact summary: "
            + json.dumps(summary(f, 0, changed=True))
            + '. Return {"updated":true}. Do not call hive_complete yet.'
        )
        cutoff = time.monotonic()
        state = self.next_turn(writer["id"], body, "graph-writer-update")
        require(state["thread_id"] == writer["thread_id"], "Writer conversation changed")
        check_execution(state, self.events(state["id"]), self.args.model, self.args.effort)
        rows = calls_for([r for r in self.proxy.rows() if r["start"] >= cutoff], writer["id"])
        require(
            [r["operation"]["op"] for r in rows] == ["get", "append"]
            and all(r["reply"]["status"] == "ok" for r in rows),
            "Retraction was not one agent-authored append",
        )
        docs = self.documents()
        require(
            docs[a]["page"]["compiled"].strip() == summary(f, 0, changed=True),
            "Writer did not revise the current relation",
        )
        require(
            docs[a]["page"]["timeline"][1:] == before[a]["page"]["timeline"],
            "Writer changed old immutable evidence",
        )
        self.timings["retraction_seconds"] = time.monotonic() - cutoff
        return docs

    def execute(self):
        writer, docs = self.writer()
        initial_coverage = self.embeddings()
        reader, first = self.read(docs, None if self.args.full else writer)
        require(
            writer["thread_id"]
            and reader["thread_id"]
            and (
                (writer["thread_id"] != reader["thread_id"])
                if self.args.full
                else (writer["thread_id"] == reader["thread_id"] and writer["id"] == reader["id"])
            ),
            "Graph scenario conversation topology did not match its selected mode",
        )
        docs = self.update(writer, docs)
        updated_coverage = self.embeddings()
        reader, second = self.read(docs, reader, changed=True)
        require(not self.proxy.errors, "Native call audit was incomplete")
        ids = list(dict.fromkeys((writer["id"], reader["id"])))
        for agent in ids:
            self.cli(
                "send",
                agent,
                "--body",
                'Call hive_complete now with body="Graph acceptance finished".',
                "--request-id",
                self.private.name + "-complete-" + agent,
            )
        states = self.wait(ids, complete=True)
        for state in states:
            check_execution(state, self.events(state["id"]), self.args.model, self.args.effort)
        return dict(
            writer=writer["id"],
            reader=reader["id"],
            first=first,
            updated=second,
            initial_coverage=initial_coverage,
            updated_coverage=updated_coverage,
            same_reader_conversation=True,
            past_evidence_preserved=True,
        )


def main():
    args = arguments(
        __doc__,
        model_default=LIVE_CODEX_MODEL,
        effort_default=LIVE_CODEX_EFFORT,
        label="comb-graph",
        full_cap=600,
    )
    require(
        args.effort == LIVE_CODEX_EFFORT and args.model == LIVE_CODEX_MODEL,
        "Graph live acceptance uses Terra 6.1 at low effort",
    )
    raise SystemExit(GraphScenario(args).run())


def cli():
    return entry(main, name="scenario")


if __name__ == "__main__":
    raise SystemExit(cli())
