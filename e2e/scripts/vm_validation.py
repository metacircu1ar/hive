"""Opt-in native Mac VZ checks with one exact owned Ubuntu guest and no host mounts."""

import argparse
from contextlib import contextmanager
from pathlib import Path
import shlex
import os
import time
import uuid

from e2e.cache import cache_path
from e2e.campaign import Admission, describe_blocked, read_receipt, recover
from e2e.paths import output_directory, product_root
from e2e.scripts.local_ci import selected_head, staging
from e2e.vm import assets, checks, essential, state as ledger
from e2e.vm.folder import Folder, default_home, delete as delete_vm
from e2e.vm.context import activity, bind, emit, free_bytes, guard, lima_facts
from e2e.vm.images import MIN_FREE, VERSIONS, admit_host, image_path
from e2e.vm.lima import Lima, globals_clean, owner_state
from release.cancellation import check, deadline, finish, pause
from release.files import atomic_write_bytes, atomic_write_json, create_directory, record_lock
from release.progress import cleanup_all, finalizer, phase

ESSENTIAL = ("provision", "rust", "providers", "sources", "smoke", "test-build", "native-stock")
ESSENTIAL_SECONDS = 2 * 3600
FULL_SECONDS = 24 * 3600


def parse_args(argv=None):
    import sys

    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0].startswith("--") and argv[0] not in ("--help",):
        argv.insert(0, "run")
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("guard", help="Show active admission, host activity and free space")
    delete = sub.add_parser(
        "delete", help="Stop and delete a marked VM from the reserved private folder"
    )
    delete.add_argument("name", help="Marked VM name from vm guard")
    for name in ("create", "probe", "run"):
        command = sub.add_parser(name)
        command.add_argument(
            "--output-dir", type=Path, help="Fresh output root; required with --resume"
        )
        command.add_argument("--ubuntu", choices=VERSIONS, default="24.04")
        command.add_argument("--memory-gib", type=int, choices=range(8, 25), default=8)
        command.add_argument("--resume", action="store_true")
        command.add_argument(
            "--keep", action="store_true", help="Keep the proven stopped guest even on success"
        )
        command.add_argument("--cache", type=Path, help="Prepared pinned VM image directory")
        if name == "probe":
            command.add_argument("--distribution-check", action="store_true")
        if name == "run":
            command.add_argument("--full", action="store_true")
            command.add_argument("--through", choices=checks.SEQUENCE)
            command.add_argument("--fallback-assets", type=Path)
            command.add_argument("--suite-comparison", action="store_true")
            command.add_argument("--embedding-smoke", action="store_true")
    args = parser.parse_args(argv)
    if args.command in ("guard", "delete"):
        return args
    if args.resume and args.output_dir is None:
        parser.error("--resume requires --output-dir with an owned ledger")
    args.full = getattr(args, "full", False)
    args.fallback_assets = getattr(args, "fallback_assets", None)
    args.suite_comparison = getattr(args, "suite_comparison", False)
    args.embedding_smoke = getattr(args, "embedding_smoke", False)
    args.through = getattr(args, "through", None)
    if not args.full and (
        args.through or args.fallback_assets or args.suite_comparison or args.embedding_smoke
    ):
        parser.error("--through, fallback assets and extra suites require --full")
    if args.embedding_smoke and args.memory_gib < 20:
        parser.error("--embedding-smoke requires --memory-gib 20 or more")
    args.mode = args.command if args.command != "run" else "full" if args.full else "essential"
    args.output_dir = (args.output_dir or output_directory("vm")).expanduser().absolute()
    args.through = args.through or "finish"
    return args


def retained(owner):
    if not os.path.lexists(owner.home / owner.name):
        return None
    try:
        Folder(home=owner.home).marker(owner.name)
    except (OSError, ValueError):
        owned = False
    else:
        owned = True
    return dict(
        name=owner.name,
        home=str(owner.home),
        owned=owned,
        delete_command=shlex.join(["python3", "e2e/run.py", "vm", "delete", owner.name]),
    )


@contextmanager
def reporting(admission, owner):
    def report():
        atomic_write_json(
            owner.output / "retained.json",
            dict(format=1, accepted=False, retained=retained(owner), cleanup=owner.cleanup),
        )

    with finalizer(report):
        yield


def cleanup_guest(campaign):
    if campaign.user is None:
        return

    # Even an interrupted installer may have installed a unit. Absence is allowed;
    # a service that exists must stop/uninstall successfully before normal completion.
    def service():
        campaign.guest.run(
            "cleanup-service",
            f"""set -euo pipefail
if [ -x "$HOME/hive-prefix/bin/hivectl" ]; then
  {checks.HIVECTL} service uninstall
fi""",
            timeout=300,
        )

    cleanup_all(service, lambda: campaign.revert_policy("cleanup"))


def host_truth(campaign):
    result = {}
    profiles = checks.preflight(campaign.guest, result)
    checks.lima_additions(campaign.guest, result)
    if result["preflight"]["version_id"] != campaign.state["ubuntu"]:
        raise ValueError("The running guest is not the requested Ubuntu release")
    if not result["lima_additions"]["clean"]:
        raise ValueError("The fresh plain guest has host mounts or policy contamination")
    memory = campaign.guest.value("guest-memory", "grep -E '^Mem(Total|Available):' /proc/meminfo")
    result["guest_memory"] = memory.splitlines()
    result["lima"] = lima_facts(current_status(campaign))
    result["profiles"] = profiles
    return result


def current_status(campaign):
    row = campaign.guest.owner.inspect()
    if row is None or row["status"] != "Running":
        raise ValueError("The inspected VM is not running")
    return row


def probe_work(campaign):
    report = {}
    user, baseline = checks.provision(campaign.guest, report)
    campaign.user = user
    report.update(host_truth(campaign))
    if report["profiles"] != baseline:
        raise ValueError("AppArmor profile set changed after provisioning")
    report["probes"] = {
        context: checks.probe(campaign.guest, context, "probe-" + context)
        for context in ("shell", "service")
    }
    observed = [checks.probe_class(row) for row in report["probes"].values()]
    report["passed"] = all(value != "fail_other" for value in observed) and (
        campaign.state["ubuntu"] != "24.04"
        or all(value == "fail_rtm_newaddr" for value in observed)
    )
    if campaign.args.distribution_check:
        checks.distribution_check(campaign.guest, report, user, baseline)
        report["passed"] = report["passed"] and report["distribution_check"]["passed"]
    return report


def policy_skip(state, name):
    policy = checks.stock_policy(state)
    if policy in (None, "unknown"):
        raise ValueError("The stock Bubblewrap policy could not be classified")
    stock = state["configs"]["stock"]
    denied = any(
        not checks.diagnostic_status(stock["doctor"]["sandbox"][key])
        for key in ("native_sandbox:daemon", "claude_sandbox:daemon")
    )
    if not denied:
        return "no stock denial"
    if name == "distribution" and policy == "restricted_distribution":
        return "provided by stock"
    return None


def execute(owner, args, state):
    args.instance = owner.name
    campaign = checks.Campaign(args, owner.output, state)
    attempt = state["attempts"][-1]
    started = time.monotonic()
    attempt.update(through=args.through, free_bytes_before=free_bytes())
    with finalizer(campaign.save):
        with finalizer(lambda: cleanup_guest(campaign)):
            owner.start()
            campaign.user = campaign.guest.value("whoami", "id -un").strip()
            if not campaign.user or not campaign.user.replace("_", "").isalnum():
                raise ValueError("Unexpected guest user")
            if args.mode == "probe":
                campaign.unit("probe", lambda: probe_work(campaign))
            else:
                sequence = (
                    checks.SEQUENCE[: checks.SEQUENCE.index(args.through) + 1]
                    if args.full
                    else ESSENTIAL
                )
                for name in sequence:
                    check()
                    skip = policy_skip(state, name) if name in ("distribution", "user") else None
                    if skip is not None:
                        campaign.unit(
                            "policy-" + name,
                            lambda: dict(
                                observed_stock=checks.stock_policy(state),
                                skipped=[name],
                                reason=skip,
                            ),
                        )
                        continue
                    with phase(checks.SEQUENCE_PHASES.get(name.split(":")[0], "verify")):
                        if name == "smoke":
                            campaign.unit(name, lambda: essential.smoke(campaign))
                        elif name == "native-stock":
                            campaign.unit(name, lambda: essential.stock(campaign))
                        else:
                            campaign.step(name)
                        if name == "provision":
                            campaign.unit("host-truth", lambda: host_truth(campaign))
                if args.full and args.through == "finish":
                    if args.suite_comparison:
                        campaign.unit("suite-comparison", campaign.suite_comparison)
                    if args.embedding_smoke:
                        campaign.unit("embedding-smoke", campaign.embedding_smoke)
    attempt.update(
        outcome="complete",
        finished_at=checks.iso_now(),
        seconds=round(time.monotonic() - started, 2),
        free_bytes_after=free_bytes(),
    )
    campaign.save()


def evaluate(args, state):
    if args.mode == "probe":
        row = state["units"]["probe"]
        return False, row["passed"], [row]
    if args.full:
        partial = args.through != "finish"
        rows = checks.evaluate(state)
        if not partial and state["finish"] is None:
            raise ValueError("A complete campaign has no finish evidence")
    else:
        partial, rows = False, essential.evaluate(state)
        if not rows:
            raise ValueError("The essential campaign has no native stock result")
    failed = any(row["match"] is False and not row["side_check"] for row in rows)
    if not args.full and any(row["match"] is None and not row["side_check"] for row in rows):
        partial = not failed
    passed = False if failed else None if partial else True
    return partial, passed, rows


def run_campaign(args):
    admit_host()
    admission = Admission()
    admission.recover = lambda active: recover(active, admission=admission)
    with admission.held():
        if args.resume:
            owner = Lima.load(admission, args.output_dir)
            if owner.home != default_home().resolve():
                raise ValueError("Resume requires Hive's reserved private Lima folder")
            if owner.state["deleted"]:
                raise ValueError("The owned VM has already been deleted")
            with staging(admission, "vm-resume") as native:
                commit = selected_head(product_root(), native)
            expected = (args.ubuntu, args.memory_gib, commit)
            actual = tuple(owner.state[key] for key in ("ubuntu", "memory_gib", "source_commit"))
            if actual != expected:
                raise ValueError("Resume arguments or committed source differ from the owned VM")
        else:
            if args.output_dir.exists():
                raise ValueError("Use a fresh VM output directory, or explicit --resume")
            create_directory(args.output_dir, mode=0o700)
            with staging(admission, "vm-inputs") as native:
                commit = selected_head(product_root(), native)
                directory = args.cache or cache_path() / "vm-images"
                image = image_path(directory, args.ubuntu)
                value = owner_state(
                    args.output_dir,
                    default_home(),
                    nonce=uuid.uuid4().hex,
                    version=args.ubuntu,
                    memory_gib=args.memory_gib,
                    commit=commit,
                    mode=args.mode,
                    image=image,
                )
                owner = Lima(admission, args.output_dir, value)
                owner.save()
        globals_clean(owner.home)
        extras = [
            name
            for name in ("suite-comparison", "embedding-smoke")
            if getattr(args, name.replace("-", "_"))
        ]
        state = (
            ledger.resume(owner.output / "state.json", owner.state, args.mode, extras)
            if args.resume
            else ledger.fresh(owner.state, extras=extras)
        )
        atomic_write_json(owner.output / "state.json", state)
        with reporting(admission, owner), bind(owner):
            with deadline(FULL_SECONDS if args.full else ESSENTIAL_SECONDS):
                guard()
                with owner.held("vm-" + owner.name):
                    if args.resume and owner.state["create_completed"] and owner.inspect() is None:
                        raise ValueError("The completed VM is missing; resume was refused")
                    if not owner.state["create_completed"]:
                        owner.create()
                    if args.command != "create":
                        execute(owner, args, state)
            # Evaluation is host-only, after stop proof. Failed/partial runs retain
            # their stopped guest; accepted publication is after optional deletion.
            partial_run, passed, rows = (
                (True, None, []) if args.command == "create" else evaluate(args, state)
            )
            state.update(evaluation=rows, passed=passed)
            atomic_write_json(owner.output / "state.json", state)
            if passed is True and not partial_run and not args.keep:
                owner.cleanup = delete_vm(admission, owner.name, home=owner.home)
                owner.state["deleted"] = True
                owner.save()
        result = dict(
            profile="release",
            validation="vm-" + args.mode,
            commit=commit,
            ubuntu=args.ubuntu,
            platform="linux-arm64"
            if owner.state["config"]["arch"] == "aarch64"
            else "linux-amd64",
            memory_gib=args.memory_gib,
            passed=passed,
            partial=partial_run,
            host_completed=True,
            cleanup=owner.cleanup,
            retained=retained(owner),
            source="committed HEAD",
            evaluation=rows,
            deferred_commands=[]
            if args.embedding_smoke
            else [
                "python3 e2e/run.py vm run --full --ubuntu "
                + args.ubuntu
                + " --memory-gib 20 --embedding-smoke"
            ],
        )

    workflow_report = assets.prepare_workflow(owner.output, state, result)

    def publish():
        atomic_write_json(owner.output / "results.json", result)
        if workflow_report is not None:
            path, raw = workflow_report
            atomic_write_bytes(path, raw, mode=0o600)
        emit(**result)
        if passed is False:
            raise SystemExit(1)

    finish(publish)


def delete(args):
    admit_host()
    admission = Admission()
    # Cleanup may target the very record blocking normal campaign admission.
    # Hold the same global lock, but do not require its recovery to finish first.
    with record_lock(admission.root / "admission", check=check, pause=pause, timeout=30) as fd:
        admission.lock_fd = fd
        with finalizer(lambda: setattr(admission, "lock_fd", None)):
            admission.active = read_receipt(admission.root)
            with finalizer(
                lambda: describe_blocked(admission.active) if admission.active is not None else None
            ):
                result = delete_vm(admission, args.name)
    finish(lambda: emit(**result))


def main():
    args = parse_args()
    if args.command == "guard":
        admission = Admission()
        active, busy, free = read_receipt(admission.root), activity(), free_bytes()
        emit(
            active=active,
            activity=busy,
            vms=Folder().listing(),
            free_bytes=free,
            clear=active is None and not busy and free >= MIN_FREE,
        )
        if active is not None or busy or free < MIN_FREE:
            raise SystemExit(1)
    elif args.command == "delete":
        delete(args)
    else:
        run_campaign(args)
