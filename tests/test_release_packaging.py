"""Release artifact provenance and native dependency boundaries, without builds."""

import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from release.acceptance import Cli
from release.policy import (
    PACKAGES,
    allowed_path,
    check_product_text,
    check_text,
    check_tracked,
    rust_text,
)


class ProductBoundaryTests(unittest.TestCase):
    def test_dependency_and_synthetic_input_exceptions_do_not_admit_history(self):
        import json
        from release.e2e_policy import check_text as check_suite_text

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "e2e/scripts").mkdir(parents=True)
            dependency = root / "e2e/dependencies.json"
            original = json.loads((ROOT / "e2e/dependencies.json").read_text())
            dependency.write_text(json.dumps(original))
            check_suite_text(root)
            for field in ("codename", "extra"):
                bad = json.loads(json.dumps(original))
                bad["ubuntu"]["22.04"][field] = "common_document_storage"
                dependency.write_text(json.dumps(bad))
                with self.assertRaises(ValueError):
                    check_suite_text(root)
            # Model and llama.cpp pins have one copy, outside the E2E suite.
            duplicate = json.loads(json.dumps(original)) | {"qwen": {"sha256": "0" * 64}}
            dependency.write_text(json.dumps(duplicate))
            with self.assertRaises(ValueError):
                check_suite_text(root)
            dependency.unlink()
            graph = root / "e2e/scripts/run_comb_graph_acceptance.py"
            source = (ROOT / "e2e/scripts/run_comb_graph_acceptance.py").read_text()
            graph.write_text(source)
            check_suite_text(root)
            graph.write_text(source + "\n# known-issue 42\n")
            with self.assertRaises(ValueError):
                check_suite_text(root)

    def test_end_to_end_suite_is_explicit_and_never_discovered_in_github_ci(self):
        workflow = (ROOT / ".github/workflows/rust.yml").read_text()
        self.assertNotIn("e2e/", workflow)
        self.assertNotIn("e2e.", workflow)
        self.assertTrue(allowed_path("e2e/run.py"))
        self.assertFalse(allowed_path("e2e/results.json"))
        self.assertFalse(allowed_path("e2e/old-run.tar.gz"))
        self.assertFalse(allowed_path("e2e/implementation-plan.md"))

    def test_suite_source_rejects_history_without_rejecting_agent_counts(self):
        from release.e2e_policy import check_text as check_suite_text

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "e2e").mkdir()
            path = root / "e2e/run.py"
            for text in ("known-issue 7", "storage plan P3", "handoff.md", "commit a1b2c3d4"):
                path.write_text(text)
                with self.assertRaises(ValueError):
                    check_suite_text(root)
            path.write_text("agents = 1000000")
            check_suite_text(root)

    def test_installed_cli_uses_explicit_prefixes_for_consumers(self):
        remote = ["hivectl", "--state-dir", "/state"]
        control = remote + ["--config", "/config"]
        call = Cli(remote, prefixes={"daemon": control, "auth": control})
        for args, prefix in [
            (("spawn", "--body", "work"), remote),
            (("evidence", "correction", "--summary", "fact"), remote),
            (("collectors", "status"), remote),
            (("daemon", "status"), control),
            (("daemon", "stop"), control),
            (("auth", "list"), control),
        ]:
            self.assertEqual(call.argv(*args), prefix + list(args))
        self.assertEqual(remote, ["hivectl", "--state-dir", "/state"])

    def test_current_product_text_and_tracked_categories(self):
        check_product_text(ROOT)
        check_tracked(ROOT)

    def test_internal_material_cannot_enter_a_product_category(self):
        for name in (
            "docs/new-plan.md",
            "IMPL_PLAN.md",
            "scripts/run_demo.py",
            "examples/generate_icons.rs",
            "icons/generated/parallax.png",
            "comb/models/helper.py",
            "tests/test_scale.py",
            "binaries/model.gguf",
            "binaries/build.log",
            "binaries/hive-aarch64-apple-darwin-debug.tar.gz",
            "binaries/target/release/hivectl",
            "src/notes.md",
            "../outside.rs",
            "compat/0.1.0/unreviewed.py",
            "compat/0.2.0/provider",
            *(
                f"scripts/release/{prefix}{gate}.py"
                for prefix in ("", "test_")
                for gate in ("error_gate", "write_gate", "log_gate", "python_error_gate")
            ),
            *(
                f"scripts/release/{name}.json"
                for name in ("error-sites", "write-sites", "python-errors", "log-canaries")
            ),
        ):
            with self.subTest(name=name):
                self.assertFalse(allowed_path(name))
        for name in (
            "setup-support/src/new.rs",
            "tests/runtime/new.rs",
            "tests/fixtures/pty_setup.py",
            "examples/qwen3-embedding.json",
            "binaries/aarch64-apple-darwin/hive/release/hivectl",
            "binaries/aarch64-unknown-linux-gnu/hive-core/release/hive-daemon",
            "binaries/x86_64-unknown-linux-gnu/hive-install/build.json",
            "binaries/x86_64-unknown-linux-gnu/comb/THIRD_PARTY_LICENSES",
            "binaries/SHA256SUMS",
            "compat/0.1.0/corpus.json",
            "compat/0.1.0/provider",
            "logo.png",
            "comb/logo.png",
            "icons/generated/hive-64.data-uri",
            "scripts/release/policy.py",
        ):
            with self.subTest(name=name):
                self.assertTrue(allowed_path(name))

    def test_prose_policy_rejects_internal_text_including_code_examples(self):
        for text in (
            "Parallax",
            "scenarios",
            "benchmark",
            "historical",
            "research",
            "validation leg",
            "target/release/hivectl",
            "cargo test --release",
            "cargo clippy",
            "cargo fmt",
            "B16",
            "V2",
            "P3",
            "unreleased",
            "run_live_comb.py",
            "scripts/run_example.py",
            "scenario-runs/output",
            "dist/assets/install.sh",
        ):
            for body in (text, f"```sh\n{text}\n```"):
                with self.subTest(body=body), self.assertRaises(ValueError):
                    check_text("guide.md", body, markdown=True)
        with self.assertRaisesRegex(ValueError, "Internal date"):
            check_text("guide.md", "Updated on 2026-10-13.", markdown=True)
        check_text(
            "guide.md",
            'Use `2026-10-13` as the date.\n\n```json\n{"at":"2026-10-13"}\n```',
            markdown=True,
        )
        check_text(
            "guide.md", "Use /v1/embeddings and e5-small-v2; validation errors stop a request."
        )

    def test_rust_text_includes_help_attributes_docs_raw_and_continued_strings(self):
        source = "// internal benchmark comment\n/// Run persistent agents.\n"
        source += (
            '#[command(about = "Run \\\n   agents")]\nconst S: &str = r##"Read \\"facts\\"."##;'
        )
        texts = [text for text, _ in rust_text(source)]
        self.assertEqual(texts, [" Run persistent agents.", "Run agents", r"Read \"facts\"."])

    def test_embedded_help_instructions_and_cargo_description_are_enforced(self):
        with tempfile.TemporaryDirectory() as temp:
            # A checkout's ancestor name must not disable embedded-text checks.
            root = Path(temp) / "tests" / "checkout"
            for name in (
                "Cargo.toml",
                *(f"{package}/Cargo.toml" for package in PACKAGES),
                "packaging/install.sh",
            ):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("")
            cases = (
                ("src/cli/new.rs", "/// P3 setup\n"),
                ("src/cli/new.rs", "/** B16 setup help */\n"),
                ("src/runtime/api/tools.rs", 'const HELP: &str = r#"research tools"#;'),
                ("src/runtime/api/request.rs", "/// B16 tool field\n"),
                ("comb/src/setup.rs", "/// Unreleased setup\n"),
                ("src/external_mcp/catalog.rs", 'const HELP: &str = "Updated 2026-10-13";'),
                (
                    "src/runtime/worker/session/startup.rs",
                    'const HELP: &str = "scripts/run_demo.py";',
                ),
                ("comb/src/provision/serve.rs", 'const HELP: &str = "run_live_comb.py";'),
                ("installer/src/options.rs", '#[command(about = "benchmark installer")]'),
                ("Cargo.toml", 'description = "Unreleased tool"'),
                ("packaging/install.sh", "echo 'scenario runner'"),
            )
            with patch("release.policy.product_files", return_value={}):
                for name, source in cases:
                    path = root / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(source)
                    with self.subTest(name=name), self.assertRaises(ValueError):
                        check_product_text(root)
                    path.write_text("")


if __name__ == "__main__":
    unittest.main()
