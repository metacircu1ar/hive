"""In-tree notices must cover actual compiled sources and included headers."""

from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from . import llama_notices
from .native_evidence import evidence


class CompiledNotices(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="llama notice ")
        self.addCleanup(temporary.cleanup)
        self.source = Path(temporary.name) / "source"
        self.build = Path(temporary.name) / "build"
        self.source.mkdir()
        self.build.mkdir()
        self.unit = self.source / "main.cpp"
        self.header = self.source / "helper.h"
        self.unit.write_text('#include "helper.h"\n')
        self.header.write_text("// Copyright 2026 Another Author\n// MIT licensed.\n")
        quote = lambda path: str(path).replace(" ", "\\ ")
        (self.build / "main.cpp.o.d").write_text(
            "main.o: " + quote(self.unit) + " \\\n " + quote(self.header) + "\n"
        )

    def declaration(self):
        return (("helper", "MIT", (("helper.h", "Copyright 2026 Another Author"),)),)

    def test_included_header_with_foreign_attribution_requires_a_notice(self):
        with self.assertRaisesRegex(ValueError, "helper.h:1"):
            llama_notices.check(self.source, self.build, (), metal=False)
        llama_notices.check(self.source, self.build, self.declaration(), metal=False)

    def test_license_wording_and_adaptation_references_require_review(self):
        for line in [
            "// adapted from X [MIT License]", "// licensed under MIT",
            "// (c) 2026 Someone", "// Permission is hereby granted",
            "// GPL", "// LGPL", "// AGPL", "// All rights reserved",
            "// Ported from another project", "// porting from another project",
            "// port of another function", "// borrowed from another project",
            "// porting Koboldcpp implementation authored by pi6am",
        ]:
            with self.subTest(line=line):
                self.header.write_text(line + "\n")
                with self.assertRaisesRegex(ValueError, "helper.h:1"):
                    llama_notices.check(self.source, self.build, (), metal=False)

    def test_repeated_credit_does_not_hide_other_later_attributions(self):
        self.header.write_text("// Copyright 2026 Another Author\n" + "\n" * 70
                               + "// Copyright 2026 Another Author\n// Copyright 2026 Stranger\n")
        with self.assertRaisesRegex(ValueError, "Unreviewed in-tree"):
            llama_notices.check(self.source, self.build, self.declaration(), metal=False)

    def test_repeated_credit_with_extra_attribution_requires_its_own_notice(self):
        self.header.write_text("// Copyright 2026 Another Author\n" + "\n" * 70
                               + "// Copyright 2026 Another Author; Copyright 2026 Stranger\n")
        with self.assertRaisesRegex(ValueError, "Unreviewed in-tree"):
            llama_notices.check(self.source, self.build, self.declaration(), metal=False)

    def test_exact_repeated_credit_is_covered(self):
        self.header.write_text("// Copyright 2026 Another Author\n" + "\n" * 70
                               + "  // Copyright 2026 Another Author\n")
        llama_notices.check(self.source, self.build, self.declaration(), metal=False)

    def test_internal_language_conversion_is_not_an_external_port_exemption(self):
        native = self.source / "ggml/src/ggml-backend.cpp"
        native.parent.mkdir(parents=True)
        native.write_text("// Note: porting this file to C++ is a work in progress\n")
        escaped = str(native).replace(" ", "\\ ")
        (self.build / "main.cpp.o.d").write_text("main.o: " + escaped + "\n")
        llama_notices.check(self.source, self.build, (), metal=False)
        native.write_text(native.read_text() + "// port of Some External Project\n")
        with self.assertRaisesRegex(ValueError, "ggml-backend.cpp:2"):
            llama_notices.check(self.source, self.build, (), metal=False)

    def test_a_later_notice_is_not_hidden_by_an_earlier_excerpt(self):
        with self.header.open("a") as stream:
            stream.write("\n" * 70 + "// Copyright 2026 A Different Author\n")
        with self.assertRaisesRegex(ValueError, "Unreviewed in-tree"):
            llama_notices.check(self.source, self.build, self.declaration(), metal=False)

    def test_public_domain_header_is_checked_even_without_a_copyright_line(self):
        self.header.write_text("This software is released into the public domain.\n")
        with self.assertRaisesRegex(ValueError, "helper.h:1"):
            llama_notices.check(self.source, self.build, (), metal=False)

    def test_embedded_metal_is_checked_beyond_compiler_depfiles(self):
        directory = self.source / "ggml/src/ggml-metal"
        directory.mkdir(parents=True)
        (directory / "kernel.metal").write_text("// Copyright 2026 Another Shader Author\n")
        with self.assertRaisesRegex(ValueError, "kernel.metal:1"):
            llama_notices.check(self.source, self.build, self.declaration(), metal=True)

    def test_missing_or_malformed_compiler_dependencies_refuse(self):
        depfile = self.build / "main.cpp.o.d"
        depfile.write_text("malformed dependency evidence")
        with self.assertRaisesRegex(ValueError, "Invalid native compiler"):
            llama_notices.check(self.source, self.build, (), metal=False)
        depfile.unlink()
        with self.assertRaisesRegex(ValueError, "Missing compiled-source"):
            llama_notices.check(self.source, self.build, (), metal=False)

    def test_failure_keeps_relative_notice_locations_in_private_evidence(self):
        with evidence(self.build / "diagnostics") as sink:
            with self.assertRaisesRegex(ValueError, "helper.h:1"):
                llama_notices.check(self.source, self.build, (), metal=False)
        content = sink.path.read_text()
        self.assertIn("helper.h:1", content)
        self.assertNotIn(str(self.source), content)
        self.assertEqual(sink.path.stat().st_mode & 0o777, 0o600)

    def test_evidence_failure_preserves_the_notice_failure(self):
        with evidence(self.build / "diagnostics") as sink:
            with patch.object(sink, "retain", side_effect=OSError("fixture")):
                with self.assertRaisesRegex(ValueError, "helper.h:1") as result:
                    llama_notices.check(self.source, self.build, (), metal=False)
        self.assertEqual(len(result.exception.cleanup_errors), 1)
        self.assertIsInstance(result.exception.cleanup_errors[0], OSError)
