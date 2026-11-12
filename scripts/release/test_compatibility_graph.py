import copy
import unittest

from .compatibility_graph import without_generation
from .compatibility_transcripts import expected
from .installed_graph import connectivity


class GraphTranscriptTests(unittest.TestCase):
    def test_installed_connectivity_requires_a_disconnected_control(self):
        for complete in (True, False):
            kind = "orphan" if complete else "reference_coverage_incomplete"
            report = dict(
                graph=dict(current_incomplete_sources=0 if complete else 1),
                findings=[dict(slug="concepts/installed", kind=kind)],
            )
            connectivity(report, "concepts/target", complete=complete)
            with self.assertRaises(RuntimeError):
                connectivity(dict(report, findings=[]), "concepts/target", complete=complete)
            with self.assertRaises(RuntimeError):
                connectivity(
                    dict(
                        report,
                        findings=report["findings"] + [dict(slug="concepts/target", kind=kind)],
                    ),
                    "concepts/target",
                    complete=complete,
                )

    def test_array_pointers_and_escaped_keys(self):
        expected({"a/b": [{"~key": ["value"]}]}, {"/a~1b/0/~0key/0": "value"})
        for pointer in ["/a/00", "/a/-1", "/a/١", "/a/2"]:
            with self.subTest(pointer=pointer), self.assertRaises((RuntimeError, IndexError)):
                expected({"a": [1]}, {pointer: 1})
        with self.assertRaises(RuntimeError):
            expected({"a": [False]}, {"/a/0": True})

    def test_only_generation_is_normalized_and_source_evidence_is_preserved(self):
        report = {
            "generation": {"identity": "a" * 32, "revision": 3},
            "relations": [{"source_hash": "hash", "declaration": {"start": 4, "end": 10}}],
        }
        original = copy.deepcopy(report)
        normalized = without_generation(report)
        self.assertEqual(report, original)
        self.assertNotIn("generation", normalized)
        self.assertEqual(normalized["relations"], report["relations"])
        for generation in [
            {"identity": "x" * 32, "revision": 0},
            {"identity": "a" * 32, "revision": True},
            {"identity": "a" * 32, "revision": -1},
        ]:
            with self.subTest(generation=generation), self.assertRaises(RuntimeError):
                without_generation(dict(report, generation=generation))
