#!/usr/bin/env python3
"""Exercise the packaged curator skill against stale, linked and duplicate pages."""

import time
import uuid

from e2e.paths import product_default
from e2e.scripts.harness.checks import require
from e2e.scripts.product_constants import LIVE_CODEX_MODEL, LIVE_CODEX_EFFORT
from e2e.scripts.run_live_comb import Scenario, arguments, check_execution, save
from release.files import atomic_write_text
from release.progress import entry

SELECTED = ("components/retry", "components/duplicate", "operations/watcher")
PROPOSAL = "review/retry-merge"
UNTOUCHED = "decisions/outside-scope"


def verify(before, after, report, current, extra, agent):
    """Require observable repairs, retained evidence and unmodified authority."""
    require(not before[SELECTED[0]]["unverified"], "Fixture did not start with a reviewed summary")
    for slug in SELECTED:
        old = before[slug]["page"]["timeline"]
        new = after[slug]["page"]["timeline"]
        require(
            len(new) > len(old) and new[-len(old) :] == old,
            "Upkeep lost old evidence or did not append its repair",
        )
        require(
            all(e["metadata"]["actor"].get("id") == agent for e in new[: -len(old)]),
            "Repair evidence was not attributed to the live curator",
        )
        require("[evidence:" in after[slug]["page"]["compiled"], "Repair summary is uncited")
        require(slug in report["page"]["compiled"], "Report omitted a changed page")
        if slug in SELECTED[:2]:
            require(
                any(PROPOSAL in e["metadata"]["source"] for e in new[: -len(old)]),
                "Merge evidence did not name its accepted proposal",
            )
    text = after[SELECTED[0]]["page"]["compiled"]
    require(
        current in text and extra in text and "OBSOLETE_VALUE" not in text,
        "Canonical summary did not adopt current and merged evidence",
    )
    require(after[SELECTED[0]]["unverified"], "Changed reviewed summary retained review trust")
    require(
        "[[components/retry]]" in after[SELECTED[1]]["page"]["compiled"],
        "Duplicate did not become a retained pointer",
    )
    watcher = after[SELECTED[2]]["page"]["compiled"]
    require(
        "[[components/retry]]" in watcher and "[[components/retry-old]]" not in watcher,
        "Broken current link was not repaired",
    )
    for slug in (PROPOSAL, UNTOUCHED):
        require(after[slug]["hash"] == before[slug]["hash"], "Upkeep changed protected content")
    require(
        "components/retired" in report["page"]["compiled"],
        "Immutable broken timeline link was not reported",
    )


def verify_repeat(after, again, report):
    require(
        all(again[s]["hash"] == after[s]["hash"] for s in after),
        "Fresh repeat curator changed repaired pages or duplicated merge evidence",
    )
    require(
        "components/retired" in report["page"]["compiled"],
        "Repeat report lost the unresolved historical link",
    )


def verify_tools(events):
    # Hive-side refusals never reach the Comb proxy; successful proxy replies
    # alone cannot establish that every hosted tool call succeeded.
    for event in events:
        if event["type"] == "tool_completed":
            item = event["data"]["item"]
            require(
                item.get("success") is True and item.get("status") == "completed",
                "Curator had a failed or incomplete hosted tool call",
            )


class UpkeepScenario(Scenario):
    name = "comb-upkeep"
    collector_capable = False

    def __init__(self, args):
        super().__init__(args)
        self.run_name = "upkeep-" + uuid.uuid4().hex[:12]
        self.current, self.extra = "CURRENT_" + uuid.uuid4().hex, "MERGED_" + uuid.uuid4().hex
        instructions = self.output / "agent-instructions.md"
        atomic_write_text(
            instructions,
            instructions.read_text()
            + "\n\n"
            + product_default("skills/comb-curator/SKILL.md").read_text()
            + "\nFor this acceptance run, call tools sequentially. Never issue parallel "
            "tool calls. Report any tool failure; this scenario does not allow retries.\n",
        )

    def put(self, slug, summary):
        page = self.output / (slug.replace("/", "-") + ".md")
        atomic_write_text(page, "---\ntitle: " + slug + "\n---\n" + summary)
        return self.hb("put", slug, "--file", str(page))

    def documents(self):
        return {slug: self.hb("get", slug) for slug in (*SELECTED, PROPOSAL, UNTOUCHED)}

    def seed(self):
        self.put(SELECTED[0], "The current retry token is OBSOLETE_VALUE.")
        self.hb(
            "append",
            SELECTED[0],
            "The current retry token is " + self.current + ".",
            "--source",
            "fixture:current-canonical-evidence",
        )
        doc = self.hb("get", SELECTED[0])
        self.hb("promote", SELECTED[0], "--expected-hash", doc["hash"])
        self.put(SELECTED[1], "Additional retry information.")
        self.hb(
            "append",
            SELECTED[1],
            "The retry diagnostic marker is " + self.extra + ".",
            "--source",
            "fixture:duplicate-evidence",
        )
        self.put(SELECTED[2], "Use [[components/retry-old]] for the current retry setting.")
        self.hb(
            "append",
            SELECTED[2],
            "The current retry page is [[components/retry]].\n\n"
            "Historical docs referenced [[components/retired]], which no longer exists.",
            "--source",
            "fixture:watcher-routing",
        )
        self.put(UNTOUCHED, "Out-of-scope content must remain unchanged.")
        proposal = self.put(
            PROPOSAL,
            "Merge [[components/duplicate]] into [[components/retry]]. "
            "Preserve its diagnostic marker and leave a pointer on the duplicate.",
        )
        self.hb(
            "decide",
            PROPOSAL,
            "accept",
            "--expected-hash",
            proposal["hash"],
            "--reason",
            "Merge the supported diagnostic marker into components/retry; "
            "retain components/duplicate as a pointer. Preserve both timelines.",
        )
        return self.documents()

    def curate(self, label):
        report = "reports/" + self.run_name + "-" + label
        task = product_default("examples/comb-maintain.md").read_text() + (
            "\nFor this run override the example scope and budget: inspect exactly these three "
            "ordinary pages: "
            + ", ".join(SELECTED)
            + ". You may read "
            + PROPOSAL
            + " to establish merge authority. Other ordinary pages are outside scope. "
            "Use their existing timeline evidence as the supplied authoritative test sources; "
            "there are no external files to check. Preserve every old timeline entry. "
            "Report the missing historical link rather than repairing history. "
            "Use run="
            + self.run_name
            + " on every mutation and write the report to "
            + report
            + ". On a repeat pass, leave correct pages unchanged and do not duplicate merge "
            "evidence. Finish with hive_complete."
        )
        started = time.monotonic()
        agent = self.spawn("comb-curator", task, label)
        state = self.wait([agent], complete=True)[0]
        events = self.events(agent)
        check_execution(state, events, self.args.model, self.args.effort)
        verify_tools(events)
        for row in self.proxy.rows():
            if (row.get("claim") or {}).get("id") != agent:
                continue
            operation = row["operation"]
            require(row["reply"]["status"] == "ok", "Curator had a failed tool operation")
            require(
                operation["op"]
                in ("get", "search", "list", "links", "graph", "graph_query", "append", "put"),
                "Curator attempted an operation outside upkeep",
            )
            if operation["op"] in ("append", "put"):
                require(operation["run"] == self.run_name, "Curator mutation omitted its run name")
            if operation["op"] == "put":
                require(operation["slug"] == report, "Curator created an unexpected page")
            if operation["op"] == "append":
                require(operation["slug"] in SELECTED, "Curator wrote outside its scope")
        self.timings[label + "_seconds"] = time.monotonic() - started
        return agent, self.hb("get", report)

    def execute(self):
        before = self.seed()
        save(self.output / "before.json", before)
        agent, report = self.curate("first")
        after = self.documents()
        verify(before, after, report, self.current, self.extra, agent)
        doctor = self.hb("doctor")
        require(
            not any(
                f["slug"] in SELECTED and f["kind"] in ("uncited_paragraph", "dangling_citation")
                for f in doctor["findings"]
            ),
            "Selected summaries still have uncited paragraphs",
        )
        save(self.output / "after.json", after)
        save(self.output / "doctor.json", doctor)
        repeat = None
        if self.args.full:
            repeat, repeat_report = self.curate("repeat")
            again = self.documents()
            verify_repeat(after, again, repeat_report)
            save(self.output / "repeat.json", again)
        return dict(
            curator=agent,
            repeat_curator=repeat,
            repaired=list(SELECTED),
            immutable_history=True,
            protected_pages=True,
            repeat_idempotent=bool(repeat),
        )


def main():
    raise SystemExit(
        UpkeepScenario(
            arguments(
                __doc__,
                label="comb-upkeep",
                full_cap=600,
                model_default=LIVE_CODEX_MODEL,
                effort_default=LIVE_CODEX_EFFORT,
            )
        ).run()
    )


if __name__ == "__main__":
    raise SystemExit(entry(main, name="scenario"))
