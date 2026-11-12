"""Native-denial evidence is not an infrastructure or failed-control result."""

import copy
import errno
import json
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from e2e.scripts import vm_validation
from e2e.vm import checks, essential


def diagnostic(*, passed=False):
    return dict(
        status="ok" if passed else "error",
        detail=dict(
            context="daemon",
            diagnostic=dict(
                outcome="passed" if passed else "failed",
                code="native_command_completed" if passed else "sandbox_permission_denied",
                executable="/home/fixture/providers/codex",
            ),
        ),
    )


def report(*, passed=False):
    return dict(
        checks=[
            dict(name=name, **diagnostic(passed=passed))
            for name in ("native_sandbox:daemon", "claude_sandbox:daemon")
        ]
    )


def probe(*, passed=False):
    return dict(returncode=0 if passed else 1, stderr="" if passed else checks.RTM_NEWADDR)


def evaluation_state(config="stock"):
    passed = config == "user"
    doctor = checks.doctor_result(0 if passed else 1, report(passed=passed), "")
    result = dict(
        probes={name: probe(passed=passed) for name in ("shell", "service")},
        doctor=doctor,
        acceptance={
            name: {
                context: dict(passed=passed, returncode=0 if passed else 101)
                for context in ("shell", "service")
            }
            for name in checks.ACCEPTANCE
        },
    )
    if config == "user":
        result.update(
            helper_system_bwrap=dict(setgroups_denied=True),
            helper_scoped=dict(marker=True),
            controls=dict(
                returncode=0,
                controls=dict(write=errno.EROFS, unix_socket=errno.EPERM, userns=errno.EPERM),
            ),
        )
    return dict(
        ubuntu="24.04", initial_stock=probe(), configs={config: result}, units={}, finish=None
    )


class Classifiers(unittest.TestCase):
    def test_setup_records_expected_sandbox_denial_and_rejects_other_failures(self):
        for passed in (True, False):
            doctor = report(passed=passed)
            doctor["checks"] += [
                dict(name="services", status="ok", detail=dict(ready=True)),
                dict(name="auth:codex-main", status="error", detail=dict(state="missing")),
                dict(name="auth:claude-main", status="error", detail=dict(state="missing")),
                dict(name="provider:codex-main", status="warning", detail={}),
            ]
            payload = dict(configured=True, ready=False, doctor=doctor)
            result = checks.setup_result(0 if passed else 1, payload)
            self.assertEqual(result["outcome"], "success" if passed else "sandbox_denied")
            for mutation in ("extra_error", "bad_auth", "service", "duplicate", "bad_exit"):
                bad = copy.deepcopy(payload)
                rows = bad["doctor"]["checks"]
                code = 0 if passed else 1
                if mutation == "extra_error":
                    rows.append(dict(name="config", status="error", detail={}))
                elif mutation == "bad_auth":
                    next(row for row in rows if row["name"] == "auth:codex-main")["detail"]["state"] = "invalid"
                elif mutation == "service":
                    next(row for row in rows if row["name"] == "services")["detail"]["ready"] = False
                elif mutation == "duplicate":
                    rows.append(copy.deepcopy(rows[0]))
                elif mutation == "bad_exit":
                    code = 2
                with self.subTest(passed=passed, mutation=mutation), self.assertRaises(RuntimeError):
                    checks.setup_result(code, bad)
        mixed = dict(configured=True, doctor=report())
        mixed["doctor"]["checks"][0].update(diagnostic(passed=True))
        mixed["doctor"]["checks"].append(dict(name="services", status="ok", detail=dict(ready=True)))
        self.assertEqual(checks.setup_result(1, mixed)["sandbox"],
                         {"native_sandbox:daemon": True, "claude_sandbox:daemon": False})
        with self.assertRaises(RuntimeError):
            checks.setup_result(0, mixed)

    def test_setup_outcome_is_checked_against_measured_stock_restriction(self):
        for ubuntu, stock_passed, setup_passed, expected in (
            ("22.04", True, True, True),
            ("22.04", True, False, False),
            ("24.04", False, False, True),
            ("24.04", False, True, False),
            ("26.04", False, False, True),
            ("26.04", True, False, False),
            ("26.04", True, True, True),
        ):
            saved = evaluation_state()
            stock = saved["configs"]["stock"]
            stock["doctor"] = checks.doctor_result(0 if stock_passed else 1, report(passed=stock_passed), "")
            saved.update(ubuntu=ubuntu, initial_stock=probe(passed=stock_passed))
            saved["units"]["install"] = dict(setup=dict(sandbox={
                name: setup_passed for name in ("native_sandbox:daemon", "claude_sandbox:daemon")
            }))
            row = next(row for row in checks.evaluate(saved) if row["check"] == "setup sandbox agreement")
            with self.subTest(ubuntu=ubuntu, stock=stock_passed, setup=setup_passed):
                self.assertEqual(row["match"], expected)

    def test_distribution_stock_requires_consistent_results_and_working_remedy(self):
        saved = evaluation_state()
        saved["ubuntu"] = "26.04"
        saved["units"]["host-truth"] = dict(
            preflight=dict(sysctls={"kernel.apparmor_restrict_unprivileged_userns": "1"},
                           profiles={"bwrap": "enforce", "unpriv_bwrap": "enforce"}),
            lima_additions=dict(unowned_apparmor_files=[]),
        )
        stock = saved["configs"]["stock"]
        doctor = report()
        doctor["checks"][0].update(diagnostic(passed=True))
        stock["doctor"] = checks.doctor_result(1, doctor, "")
        stock["probes"] = {context: probe(passed=True) for context in ("shell", "service")}
        stock["helper_system_bwrap"] = dict(marker=False, setgroups_denied=True)
        stock["acceptance"]["codex"] = {context: dict(passed=True) for context in ("shell", "service")}
        saved["initial_stock"] = probe(passed=True)
        saved["units"]["install"] = dict(setup=dict(sandbox={
            "native_sandbox:daemon": True, "claude_sandbox:daemon": False,
        }))
        saved["finish"] = dict(profiles_restored=True, leftover_policy_files=[],
                               service_uninstall_returncode=0,
                               restored_probe_class="pass", stock_probe_class="pass")
        saved["policy_owned"] = dict(distribution=False, user=False)
        self.assertEqual(checks.stock_policy(saved), "restricted_distribution")
        self.assertEqual(vm_validation.policy_skip(saved, "distribution"), "provided by stock")
        self.assertIsNone(vm_validation.policy_skip(saved, "user"))
        rows = checks.evaluate(saved)
        self.assertFalse(next(row for row in rows if row["check"] == "remedy required")["match"])
        saved["configs"]["user"] = evaluation_state("user")["configs"]["user"]
        saved["configs"]["user"]["helper_system_bwrap"] = dict(marker=False, setgroups_denied=True)
        saved["units"]["policy-distribution"] = dict(reason="provided by stock")
        rows = checks.evaluate(saved)
        self.assertFalse([row for row in rows if row["match"] is False])
        saved["units"]["install"]["setup"]["sandbox"]["claude_sandbox:daemon"] = True
        rows = checks.evaluate(saved)
        self.assertFalse(next(row for row in rows if row["check"] == "setup sandbox agreement")["match"])

    def test_stock_policy_refuses_unowned_or_partial_profiles_and_allows_partial_state(self):
        saved = evaluation_state()
        saved.update(ubuntu="26.04", initial_stock=None, configs={})
        self.assertEqual(checks.evaluate(saved), [])
        saved["units"]["host-truth"] = dict(
            preflight=dict(sysctls={"kernel.apparmor_restrict_unprivileged_userns": "1"},
                           profiles={"bwrap": "enforce", "unpriv_bwrap": "enforce"}),
            lima_additions=dict(unowned_apparmor_files=["/etc/apparmor.d/custom"]),
        )
        self.assertEqual(checks.stock_policy(saved), "unknown")
        saved["units"]["host-truth"]["lima_additions"]["unowned_apparmor_files"] = []
        saved["units"]["host-truth"]["preflight"]["profiles"].pop("unpriv_bwrap")
        self.assertEqual(checks.stock_policy(saved), "unknown")

    def test_stock_pins_and_host_observations_cannot_be_silently_waived(self):
        saved = evaluation_state()
        saved["ubuntu"] = "22.04"
        saved["units"]["host-truth"] = dict(
            preflight=dict(sysctls={"kernel.apparmor_restrict_unprivileged_userns": "0"}, profiles={}),
            lima_additions=dict(unowned_apparmor_files=[]),
        )
        stock = saved["configs"]["stock"]
        stock["helper_system_bwrap"] = dict(marker=True, setgroups_denied=False)
        rows = checks.evaluate(saved)
        self.assertFalse(next(row for row in rows if row["check"] == "provider sandboxes")["match"])
        self.assertFalse(next(row for row in rows if row["check"] == "host_acceptance:claude")["match"])
        saved["units"]["host-truth"]["preflight"]["sysctls"]["kernel.apparmor_restrict_unprivileged_userns"] = "1"
        rows = checks.evaluate(saved)
        self.assertFalse(next(row for row in rows if row["check"] == "policy class")["match"])

    def test_vscode_profile_transition_failure_is_a_side_outcome_only(self):
        done = SimpleNamespace(returncode=1, stdout="", stderr="aa-exec: ERROR: insufficient permissions to change to the profile\n")
        guest = SimpleNamespace(
            value=lambda *args, **kwargs: "vscode (enforce)\n",
            run=lambda *args, **kwargs: ({}, done),
        )
        campaign = SimpleNamespace(guest=guest)
        result = checks.Campaign.acceptance(campaign, "stock", "codex", "vscode", "/usr/bin")
        self.assertTrue(result["unavailable"])
        self.assertIsNone(result["passed"])
        with self.assertRaises(RuntimeError):
            checks.Campaign.acceptance(campaign, "stock", "codex", "shell", "/usr/bin")

    def test_optional_info_rows_are_valid_but_do_not_hide_other_errors(self):
        payload = report(passed=True)
        payload["checks"] += [
            dict(name="comb_binary", status="info", detail="Comb is not installed."),
            dict(name="comb", status="info", detail="Not configured; optional."),
            dict(name="auth:codex-main", status="error", detail=dict(state="missing")),
        ]
        value = checks.doctor_result(1, payload, "")
        self.assertEqual(value["errors"], ["auth:codex-main"])
        self.assertEqual(value["sandbox"]["native_sandbox:daemon"]["status"], "ok")
        with self.assertRaises(RuntimeError):
            checks.doctor_result(0, payload, "")
        for mutate in ("unknown-status", "duplicate", "missing-daemon"):
            bad = copy.deepcopy(payload)
            if mutate == "unknown-status":
                bad["checks"][2]["status"] = "unknown"
            elif mutate == "duplicate":
                bad["checks"].append(copy.deepcopy(bad["checks"][0]))
            else:
                bad["checks"].pop(0)
            with self.subTest(case=mutate), self.assertRaises(RuntimeError):
                checks.doctor_result(1, bad, "")

    def test_doctor_caller_has_the_installed_application_and_provider_path(self):
        guest = SimpleNamespace(run=lambda *a, **kw: (
            {}, SimpleNamespace(returncode=0, stdout=json.dumps(report(passed=True)), stderr="")
        ))
        with patch.object(guest, "run", wraps=guest.run) as invoke:
            value = checks.Campaign.doctor(SimpleNamespace(guest=guest), "stock")
        self.assertTrue(value["exit_matches_errors"])
        script = invoke.call_args.args[1]
        self.assertIn('$HOME/hive-prefix/bin:', script)
        self.assertIn('$HOME/providers/bin', script)
        self.assertTrue(script.rstrip().endswith('"$HOME/hive-prefix/bin/hivectl" doctor'))
        self.assertFalse(invoke.call_args.kwargs["require"])

    def test_doctor_requires_exact_verified_daemon_native_outcome(self):
        for passed in (False, True):
            self.assertTrue(
                checks.doctor_result(0 if passed else 1, report(passed=passed), "")[
                    "exit_matches_errors"
                ]
            )
        for name in ("native_sandbox:daemon", "claude_sandbox:daemon"):
            for code, outcome in (
                ("timed_out", "failed"),
                ("probe_io_failed", "failed"),
                ("provider_unavailable", "unavailable"),
            ):
                bad = report()
                row = next(row for row in bad["checks"] if row["name"] == name)
                row["detail"]["diagnostic"].update(code=code, outcome=outcome)
                with self.subTest(name=name, code=code), self.assertRaises(RuntimeError):
                    checks.doctor_result(1, bad, "")
            for detail in (
                "unverified",
                dict(context="caller_local", diagnostic=diagnostic()["detail"]["diagnostic"]),
            ):
                bad = report()
                next(row for row in bad["checks"] if row["name"] == name).update(
                    status="warning", detail=detail
                )
                with self.subTest(name=name, detail=detail), self.assertRaises(RuntimeError):
                    checks.doctor_result(1, bad, "")

    def test_full_evaluator_rejects_bad_diagnostics_and_keeps_guest_stock_baseline(self):
        saved = evaluation_state()
        self.assertFalse(any(row["match"] is False for row in checks.evaluate(saved)))
        for code in ("timed_out", "probe_io_failed", "provider_unavailable"):
            bad = copy.deepcopy(saved)
            bad["configs"]["stock"]["doctor"]["sandbox"]["native_sandbox:daemon"]["detail"][
                "diagnostic"
            ]["code"] = code
            with self.assertRaises(RuntimeError):
                checks.evaluate(bad)
        for ubuntu, baseline in (
            ("22.04", "pass"),
            ("26.04", "pass"),
            ("24.04", "fail_rtm_newaddr"),
        ):
            saved = evaluation_state()
            saved.update(
                ubuntu=ubuntu,
                finish=dict(
                    profiles_restored=True,
                    leftover_policy_files=[],
                    service_uninstall_returncode=0,
                    restored_probe_class=baseline,
                    stock_probe_class=baseline,
                ),
            )
            rows = checks.evaluate(saved)
            self.assertTrue(rows[-1]["match"])
            saved["finish"]["restored_probe_class"] = "other"
            self.assertFalse(checks.evaluate(saved)[-1]["match"])

    def test_essential_rejects_unverified_doctor_and_enforces_noble_failure_status(self):
        for passed in (False, True):
            payload = dict(returncode=0 if passed else 1, report=report(passed=passed))
            campaign = SimpleNamespace(
                snapshot_baseline=lambda: None,
                state={},
                guest=SimpleNamespace(
                    run=lambda *a, **kw: (
                        None,
                        SimpleNamespace(stdout="HIVE_VM_JSON " + json.dumps(payload)),
                    )
                ),
                acceptance=lambda *a: dict(passed=False, returncode=101),
            )
            with (
                patch.object(checks, "probe", return_value=probe()),
                patch.object(
                    essential, "current", return_value=SimpleNamespace(state=dict(ubuntu="24.04"))
                ),
            ):
                result = essential.stock(campaign)
                if passed:
                    self.assertIs(result["passed"], False)
                else:
                    self.assertIsNone(result["passed"])
                    self.assertEqual(result["reason"], "remedy not validated in essential mode")
            payload["report"]["checks"][0].update(status="warning", detail="daemon unavailable")
            with (
                patch.object(checks, "probe", return_value=probe()),
                patch.object(
                    essential, "current", return_value=SimpleNamespace(state=dict(ubuntu="24.04"))
                ),
            ):
                with self.assertRaises(RuntimeError):
                    essential.stock(campaign)

    def test_essential_indeterminate_remedy_cannot_be_a_pass(self):
        args = SimpleNamespace(mode="run", full=False)
        state = dict(units={"native-stock": dict(passed=None, reason="remedy not validated in essential mode")})
        partial, passed, rows = vm_validation.evaluate(args, state)
        self.assertTrue(partial)
        self.assertIsNone(passed)
        self.assertIsNone(rows[0]["match"])
        state["units"]["native-stock"]["passed"] = False
        partial, passed, _ = vm_validation.evaluate(args, state)
        self.assertFalse(partial)
        self.assertIs(passed, False)

    def test_controls_require_zero_exit_one_record_and_integer_errnos(self):
        valid = dict(write=errno.EROFS, unix_socket=errno.EPERM, userns=errno.EPERM)
        records = [json.dumps(valid)]
        self.assertEqual(checks.controls_result(0, records, "fixture"), valid)
        for code, rows in (
            (1, records),
            (0, records * 2),
            (0, []),
            (0, [json.dumps(dict(valid, unix_socket=True))]),
        ):
            with self.subTest(code=code, rows=rows):
                self.assertIsNone(checks.controls_result(code, rows, "fixture"))
        for code, values in ((1, valid), (0, dict(valid, userns=True))):
            saved = evaluation_state("user")
            saved["configs"]["user"]["controls"] = dict(returncode=code, controls=values)
            row = next(row for row in checks.evaluate(saved) if row["check"] == "helper:controls")
            self.assertFalse(row["match"])


if __name__ == "__main__":
    unittest.main()
