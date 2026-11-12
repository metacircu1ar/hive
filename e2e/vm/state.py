"""Strict draft VM execution ledger, separate from the stable instance owner."""

from release.build_record import decode, read

FIELDS = {
    "format",
    "instance_nonce",
    "commit",
    "ubuntu",
    "memory_gib",
    "mode",
    "created_at",
    "attempts",
    "units",
    "configs",
    "baseline_profiles",
    "baseline_policy_files",
    "initial_stock",
    "policy_owned",
    "prerequisites",
    "finish",
    "evaluation",
    "passed",
    "extras",
}


def fresh(owner, *, extras=(), mode=None):
    from e2e.vm.checks import iso_now

    return dict(
        format=1,
        instance_nonce=owner["instance_nonce"],
        commit=owner["source_commit"],
        ubuntu=owner["ubuntu"],
        memory_gib=owner["memory_gib"],
        mode=owner["mode"] if mode is None else mode,
        created_at=iso_now(),
        attempts=[],
        units={},
        configs={},
        baseline_profiles=None,
        baseline_policy_files=None,
        initial_stock=None,
        policy_owned=dict(distribution=False, user=False),
        prerequisites=[],
        finish=None,
        evaluation=[],
        passed=None,
        extras=list(extras),
    )


def load(path, owner, extras, *, mode=None):
    value = decode(read(path))
    expected = fresh(owner, extras=extras, mode=mode)
    if (
        not isinstance(value, dict)
        or set(value) != FIELDS
        or type(value["format"]) is not int
        or value["format"] != 1
        or any(
            value[key] != expected[key]
            for key in ("instance_nonce", "commit", "ubuntu", "memory_gib", "mode", "extras")
        )
        or any(not isinstance(value[key], dict) for key in ("units", "configs", "policy_owned"))
        or any(
            not isinstance(value[key], list) for key in ("attempts", "prerequisites", "evaluation")
        )
        or set(value["policy_owned"]) != {"distribution", "user"}
        or any(type(item) is not bool for item in value["policy_owned"].values())
        or value["passed"] is not None
        and type(value["passed"]) is not bool
    ):
        raise ValueError("Unsupported or differently bound VM execution ledger")
    return value


def resume(path, owner, mode, extras):
    """A create-only owner chooses one execution mode in one ledger publication."""
    if owner["mode"] != "create":
        if owner["mode"] != mode:
            raise ValueError("Resume cannot change the owned VM execution mode")
        return load(path, owner, extras)
    value = decode(read(path))
    if isinstance(value, dict) and value.get("mode") == "create" and mode != "create":
        value = load(path, owner, [])
        if value["attempts"] or value["units"] or value["configs"]:
            raise ValueError("A create-only guest already contains execution evidence")
        return fresh(owner, mode=mode, extras=extras)
    return load(path, owner, extras, mode=mode)
