"""Reject false-positive upkeep acceptance without launching providers."""

import copy
import unittest
from e2e.scripts.run_live_comb_upkeep import (
    SELECTED,
    PROPOSAL,
    UNTOUCHED,
    verify,
    verify_repeat,
    verify_tools,
)


def evidence():
    before = {
        slug: dict(
            hash="old",
            unverified=False,
            page=dict(
                compiled="old",
                timeline=[dict(metadata=dict(actor=dict(id="human")), text="original")],
            ),
        )
        for slug in (*SELECTED, PROPOSAL, UNTOUCHED)
    }
    after = copy.deepcopy(before)
    summaries = (
        "CURRENT_TOKEN EXTRA_TOKEN [evidence:id]",
        "Merged into [[components/retry]] [evidence:id]",
        "Use [[components/retry]] [evidence:id]",
    )
    for slug, summary in zip(SELECTED, summaries, strict=True):
        after[slug]["page"]["compiled"] = summary
        after[slug]["page"]["timeline"].insert(
            0, dict(metadata=dict(actor=dict(id="curator"), source=PROPOSAL), text="repair")
        )
        after[slug]["unverified"] = True
    report = dict(page=dict(compiled=", ".join(SELECTED) + " unresolved components/retired"))
    return before, after, report


class UpkeepVerifierTests(unittest.TestCase):
    def test_hive_side_tool_failure_is_rejected_without_any_proxy_receipt(self):
        for success, status in (
            (False, "failed"),
            (False, "completed"),
            (True, "failed"),
            (None, None),
        ):
            with self.subTest(success=success, status=status):
                events = [
                    dict(
                        type="tool_completed",
                        data=dict(
                            item=dict(
                                type="dynamicToolCall",
                                tool="comb_links",
                                success=success,
                                status=status,
                            )
                        ),
                    )
                ]
                with self.assertRaises(RuntimeError):
                    verify_tools(events)
        verify_tools(
            [dict(type="tool_completed", data=dict(item=dict(success=True, status="completed")))]
        )

    def check(self, before, after, report):
        verify(before, after, report, "CURRENT_TOKEN", "EXTRA_TOKEN", "curator")

    def test_complete_repair_is_accepted(self):
        self.check(*evidence())

    def test_repeat_rejects_duplicate_writes_and_missing_unresolved_findings(self):
        _, after, report = evidence()
        verify_repeat(after, copy.deepcopy(after), report)
        for defect in ("duplicate", "report"):
            again = copy.deepcopy(after)
            repeated = copy.deepcopy(report)
            if defect == "duplicate":
                again[SELECTED[0]]["hash"] = "changed"
            if defect == "report":
                repeated["page"]["compiled"] = "All fixed"
            with self.assertRaises(RuntimeError):
                verify_repeat(after, again, repeated)

    def test_lost_history_wrong_actor_stale_claim_and_false_trust_are_rejected(self):
        for defect in ("history", "actor", "claim", "trust", "citation", "pointer", "link"):
            with self.subTest(defect=defect):
                before, after, report = evidence()
                page = after[SELECTED[0]]
                if defect == "history":
                    page["page"]["timeline"][-1]["text"] = "rewritten"
                if defect == "actor":
                    page["page"]["timeline"][0]["metadata"]["actor"]["id"] = "human"
                if defect == "claim":
                    page["page"]["compiled"] = "OBSOLETE_VALUE [evidence:id]"
                if defect == "trust":
                    page["unverified"] = False
                if defect == "citation":
                    page["page"]["compiled"] = "CURRENT_TOKEN EXTRA_TOKEN"
                if defect == "pointer":
                    after[SELECTED[1]]["page"]["compiled"] = "old [evidence:id]"
                if defect == "link":
                    after[SELECTED[2]]["page"]["compiled"] = (
                        "[[components/retry-old]] [evidence:id]"
                    )
                with self.assertRaises(RuntimeError):
                    self.check(before, after, report)

    def test_changed_authority_outside_scope_and_incomplete_reports_are_rejected(self):
        for defect in (PROPOSAL, UNTOUCHED, "report", "unresolved"):
            with self.subTest(defect=defect):
                before, after, report = evidence()
                if defect in after:
                    after[defect]["hash"] = "changed"
                if defect == "report":
                    report["page"]["compiled"] = "components/retired"
                if defect == "unresolved":
                    report["page"]["compiled"] = ", ".join(SELECTED)
                with self.assertRaises(RuntimeError):
                    self.check(before, after, report)
