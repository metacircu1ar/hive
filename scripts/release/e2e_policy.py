"""Explicit source-only opt-in checks and their documentation boundary."""

import ast
import json
import re

FILES = {
    "e2e/README.md",
    "e2e/__init__.py",
    "e2e/paths.py",
    "e2e/prepare.py",
    "e2e/run.py",
    "e2e/cache.py",
    "e2e/campaign.py",
    "e2e/live/__init__.py",
    "e2e/live/ownership.py",
    "e2e/live/scenario.py",
    "e2e/live/runner.py",
    "e2e/live/prepare.py",
    "e2e/scripts/run_conversation_controls.py",
    "e2e/scripts/run_factorial_demo.py",
    "e2e/scripts/run_commit_review_demo.py",
    "e2e/scripts/commit_review_fixture.py",
    "e2e/scripts/run_shop_team.py",
    "e2e/tests/test_conversation_controls.py",
    "e2e/tests/test_live_defaults.py",
    "e2e/tests/test_scenarios.py",
    "e2e/tests/test_live_scenario.py",
    "e2e/tests/test_live_set.py",
    "e2e/tests/test_live_preparation.py",
    "e2e/examples/runtime.yaml",
    "e2e/examples/factorial-task.md",
    "e2e/examples/roles/implementor.md",
    "e2e/examples/roles/reviewer.md",
    "e2e/examples/roles/commit-implementor.md",
    "e2e/examples/roles/commit-reviewer.md",
    "e2e/examples/shop-team/coordinator.md",
    "e2e/examples/shop-team/repo/inventory.py",
    "e2e/examples/shop-team/repo/test_inventory.py",
    "e2e/examples/shop-team/repo/README.md",
    "e2e/live/proof.py",
    "e2e/live/audit.py",
    "e2e/scripts/run_live_comb.py",
    "e2e/scripts/comb_live_fixture.py",
    "e2e/scripts/run_live_comb_graph.py",
    "e2e/scripts/run_live_comb_upkeep.py",
    "e2e/scripts/comb_graph_live_fixture.py",
    "e2e/tests/test_live_comb.py",
    "e2e/tests/test_live_comb_graph.py",
    "e2e/tests/test_live_comb_upkeep.py",
    "e2e/tests/test_live_comb_audit.py",
    "e2e/examples/roles/ping.md",
    "e2e/examples/roles/pong.md",
    "e2e/examples/ping-pong-all-to-one-task.md",
    "e2e/scripts/run_live_ping_pong_pair.py",
    "e2e/scripts/verify_live_ping_pong_pair.py",
    "e2e/scripts/live_pair_memory.py",
    "e2e/scripts/run_ping_pong_all_to_one.py",
    "e2e/tests/test_live_ping_pong_pair.py",
    "e2e/tests/test_live_all_to_one_verifier.py",
    "e2e/tests/test_live_ownership.py",
    "e2e/tests/test_live_retirement.py",
    "e2e/tests/fixtures/live_daemon.py",
    "e2e/docker.py",
    "e2e/ci/__init__.py",
    "e2e/ci/Dockerfile",
    "e2e/ci/container.py",
    "e2e/ci/prepare.py",
    "e2e/ci/runner.py",
    "e2e/ci/steps.py",
    "e2e/vm/__init__.py",
    "e2e/vm/assets.py",
    "e2e/vm/checks.py",
    "e2e/vm/checks_account.py",
    "e2e/vm/client.py",
    "e2e/vm/context.py",
    "e2e/vm/daemon_probe.py",
    "e2e/vm/embedding.py",
    "e2e/vm/essential.py",
    "e2e/vm/folder.py",
    "e2e/vm/guest.py",
    "e2e/vm/host-policy.sh",
    "e2e/vm/images.py",
    "e2e/vm/lima.py",
    "e2e/vm/native.py",
    "e2e/vm/prepare.py",
    "e2e/vm/proof.py",
    "e2e/vm/provision.sh",
    "e2e/vm/state.py",
    "e2e/dependencies.json",
    "e2e/models/__init__.py",
    "e2e/models/llama.py",
    "e2e/models/memory.py",
    "e2e/models/qwen3.py",
    "e2e/embeddings/__init__.py",
    "e2e/embeddings/append.py",
    "e2e/embeddings/paired_append.py",
    "e2e/embeddings/local_smoke.py",
    "e2e/embeddings/qwen_contract.py",
    "e2e/embeddings/smoke.py",
    "e2e/embeddings/http.py",
    "e2e/scripts/comb_graph_tuning.py",
    "e2e/scripts/local_ci.py",
    "e2e/scripts/vm_validation.py",
    "e2e/scripts/run_comb_graph_acceptance.py",
    "e2e/scripts/__init__.py",
    "e2e/scripts/harness/__init__.py",
    "e2e/scripts/harness/checks.py",
    "e2e/scripts/harness/audit.py",
    "e2e/scripts/harness/hashing.py",
    "e2e/scripts/harness/scenario.py",
    "e2e/scripts/harness/cli.py",
    "e2e/scripts/harness/clock.py",
    "e2e/scripts/harness/daemon.py",
    "e2e/scripts/harness/evidence.py",
    "e2e/scripts/harness/io.py",
    "e2e/scripts/harness/stats.py",
    "e2e/scripts/harness/traces.py",
    "e2e/scripts/harness/worker.py",
    "e2e/scripts/product_constants.py",
    "e2e/scripts/run_arms.py",
    "e2e/scripts/run_mock_ping_pong_all_to_one.py",
    "e2e/scripts/run_mock_ping_pong_pair.py",
    "e2e/scripts/runtime_timings.py",
    "e2e/scripts/scenario_arrivals.py",
    "e2e/scripts/scenario_control.py",
    "e2e/scripts/scenario_host.py",
    "e2e/scripts/scenario_memory.py",
    "e2e/scripts/scenario_observers.py",
    "e2e/scripts/scenario_progress.py",
    "e2e/scripts/status_schema.py",
    "e2e/scripts/storage_probe.py",
    "e2e/scripts/verify_ping_pong_pair.py",
    "e2e/tests/__init__.py",
    "e2e/tests/fixtures/scale_provider.py",
    "e2e/tests/fixtures/campaign_service.py",
    "e2e/tests/test_campaign.py",
    "e2e/tests/test_campaign_processes.py",
    "e2e/tests/test_campaign_reports.py",
    "e2e/tests/test_container_contracts.py",
    "e2e/tests/test_runner_processes.py",
    "e2e/tests/test_local_ci.py",
    "e2e/tests/fixtures/vm_client.py",
    "e2e/tests/test_vm_client.py",
    "e2e/tests/test_vm_classification.py",
    "e2e/tests/test_vm_cleanup.py",
    "e2e/tests/test_vm_contracts.py",
    "e2e/tests/test_vm_processes.py",
    "e2e/tests/test_vm_workflows.py",
    "e2e/tests/test_vm_accounts.py",
    "e2e/tests/test_vm_preparation.py",
    "e2e/tests/status_fixture.py",
    "e2e/tests/test_audit_errors.py",
    "e2e/tests/test_cli_prefixes.py",
    "e2e/tests/test_complete_evidence.py",
    "e2e/tests/test_comparison.py",
    "e2e/tests/test_audit_cancellation.py",
    "e2e/tests/test_completion.py",
    "e2e/tests/test_entry.py",
    "e2e/tests/test_mock_ping_pong_all_to_one.py",
    "e2e/tests/test_mock_ping_pong_pair.py",
    "e2e/tests/test_pair_arrivals.py",
    "e2e/tests/test_pair_provider_audit.py",
    "e2e/tests/test_runtime_timings.py",
    "e2e/tests/test_scenario_control.py",
    "e2e/tests/test_scenario_diagnostics.py",
    "e2e/tests/test_scenario_observers.py",
    "e2e/tests/test_scale_provider_retry.py",
    "e2e/tests/test_scenario_progress.py",
    "e2e/tests/test_storage_probe.py",
    "e2e/tests/test_worker.py",
    "e2e/tests/test_comb_graph_acceptance.py",
    "e2e/tests/test_embedding_smoke.py",
    "e2e/tests/test_llama.py",
    "e2e/tests/test_memory.py",
    "e2e/tests/test_qwen3.py",
}

HISTORY = re.compile(
    r"common_document_storage|hive-tools|parallax|known[- ]issue\s*#?\d+"
    r"|storage plan\s+[A-Z]\d+|(?:plan|handoff)\.md|\b20\d{2}-\d{2}-\d{2}\b"
    r"|(?<![A-Za-z0-9])(?=[a-f0-9]{7,40}(?![A-Za-z0-9]))"
    r"(?=[a-f0-9]*[a-f])[a-f0-9]{7,40}|/Users/[^/\s]+",
    re.I,
)


def dependency_text(text):
    """Only declared image identity fields carry pins; the model and llama.cpp pins
    live once, in the product's shared pin, never in the E2E suite."""
    value = json.loads(text)
    if set(value) != {"ubuntu"}:
        raise ValueError("Unexpected E2E dependency input fields")
    if set(value["ubuntu"]) != {"22.04", "24.04", "26.04"}:
        raise ValueError("Unexpected Ubuntu dependency matrix")
    for row in value["ubuntu"].values():
        if set(row) != {"codename", "release", "arm64", "amd64"}:
            raise ValueError("Unexpected Ubuntu image fields")
        for arch in ("arm64", "amd64"):
            pin = row[arch]
            if (
                not isinstance(pin, dict)
                or set(pin) != {"sha256", "bytes"}
                or not isinstance(pin["sha256"], str)
                or re.fullmatch("[a-f0-9]{64}", pin["sha256"]) is None
                or type(pin["bytes"]) is not int
                or pin["bytes"] <= 0
            ):
                raise ValueError("Invalid Ubuntu image identity")
            pin["sha256"] = "PIN"
    return json.dumps(value)


def synthetic_times(text):
    allowed = {"FIXTURE_TIME": "2026-01-02T00:00:00Z", "FIXTURE_AFTER": "2000-01-01T02:00:00+02:00"}
    lines = text.splitlines(keepends=True)
    for node in ast.parse(text).body:
        if (
            isinstance(node, ast.Assign)
            and len(node.targets) == 1
            and isinstance(node.targets[0], ast.Name)
        ):
            name = node.targets[0].id
            if name in allowed:
                if not isinstance(node.value, ast.Constant) or node.value.value != allowed[name]:
                    raise ValueError("Changed deterministic graph timestamp input")
                lines[node.lineno - 1] = lines[node.lineno - 1].replace(
                    allowed.pop(name), "SYNTHETIC_TIME", 1
                )
    if allowed:
        raise ValueError("Missing deterministic graph timestamp input")
    return "".join(lines)


def check_text(root):
    for name in sorted(FILES):
        path = root / name
        try:
            text = path.read_text()
        except FileNotFoundError:
            continue  # Source-only suite is absent from installed documentation fixtures.
        if name == "e2e/dependencies.json":
            text = dependency_text(text)
        elif name == "e2e/scripts/run_comb_graph_acceptance.py":
            text = synthetic_times(text)
        match = HISTORY.search(text)
        if match:
            line = text.count("\n", 0, match.start()) + 1
            raise ValueError(f"Internal history or private path in {name}:{line}")
