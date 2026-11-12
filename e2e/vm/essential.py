"""The small VM profile: plain builds, mock traffic and stock native confinement."""

from e2e.ci.steps import essential_steps
from e2e.vm import checks
from e2e.vm.context import current, fail


def smoke(campaign):
    rows = []
    for name, command, _ in essential_steps():
        row = campaign.guest.run(
            "essential-" + name.lower().replace(" ", "-"),
            f'export PATH="{checks.CARGO_PATH}" RUST_TEST_THREADS=4 CARGO_BUILD_JOBS=4\n'
            f"cd {checks.SRC}\n{command}",
            timeout=5400,
        )[0]
        rows.append(dict(name=name, record=row))
    return dict(steps=rows)


def stock(campaign):
    guest = campaign.guest
    campaign.snapshot_baseline()
    probes = {
        context: checks.probe(guest, context, "stock-probe-" + context)
        for context in ("shell", "service")
    }
    campaign.state["initial_stock"] = probes["service"]
    done = guest.run(
        "transient-doctor",
        f'export PATH="{checks.CARGO_PATH}"\ncd {checks.SRC}\npython3 -m e2e.vm.daemon_probe',
        timeout=360,
    )[1]
    result = checks.marked_json(done.stdout)
    doctor = checks.doctor_result(result["returncode"], result["report"], "")
    acceptance = {
        provider: {
            context: campaign.acceptance("stock", provider, context, checks.CARGO_PATH)
            for context in ("shell", "service")
        }
        for provider in checks.ACCEPTANCE
    }
    noble = current().state["ubuntu"] == "24.04"
    observations = []
    for context, row in probes.items():
        if checks.probe_class(row) == "fail_other":
            fail("The stock probe failed without its recognized native denial")
        observations.append(
            dict(
                check="probe:" + context,
                passed=not noble or checks.probe_class(row) == "fail_rtm_newaddr",
            )
        )
    for provider, name in (("codex", "native_sandbox:daemon"), ("claude", "claude_sandbox:daemon")):
        observed = checks.diagnostic_status(doctor["sandbox"][name])
        for context, row in acceptance[provider].items():
            matches = observed == row["passed"] and not (noble and observed)
            observations.append(
                dict(
                    check=provider + ":" + context,
                    passed=False if not matches else True if observed else None,
                )
            )
    values = [row["passed"] for row in observations]
    passed = False if False in values else None if None in values else True
    return dict(
        probes=probes,
        doctor=doctor,
        acceptance=acceptance,
        observations=observations,
        passed=passed,
        reason="remedy not validated in essential mode" if passed is None else None,
    )


def evaluate(state):
    units = state["units"]
    return [
        dict(
            config="essential",
            check=name,
            expected=True,
            observed=row["passed"],
            match=row["passed"],
            side_check=False,
            evidence=row,
        )
        for name, row in units.items()
        if name == "native-stock"
    ]
