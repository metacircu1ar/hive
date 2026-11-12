"""Shared lifetime for live checks that own one daemon and a task lineage."""

from pathlib import Path
import time
import uuid

from e2e.live.ownership import (
    Daemon,
    campaign,
    cancel_descendants,
    configured_cli,
    default_state,
    opt_in,
    owned_agents,
    preflight,
    verify_tasks,
)
from e2e.paths import output_directory, product_default, require_paths
from e2e.scripts.harness.cli import call_deadline, event_pages
from release.acceptance import event_data
from release.cancellation import deadline, finish
from release.files import atomic_write_json, create_directory
from release.progress import cleanup_all, finalizer, phase


def options(parser, *, cap):
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--binary", type=Path, default=product_default("target/release/hivectl"))
    parser.add_argument("--state-dir", type=Path, default=default_state())
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--timeout", type=int, default=cap, help="Absolute work limit in seconds")


def checked(parser, args, *, label, agents, models, cap):
    if args.timeout < 1 or (cap is not None and args.timeout > cap):
        parser.error("Use a positive timeout within this scenario's work limit")
    opt_in(parser, args, agents=agents, models=models)
    require_paths(args, parser, "binary")
    args.state_dir = args.state_dir.expanduser().resolve()
    args.output_dir = args.output_dir or output_directory("live-" + label)
    return args


class Hosted:
    def __init__(self, args, *, label, roles, runtime=None):
        self.args, self.label, self.roles = args, label, roles
        self.output = args.output_dir.expanduser().resolve()
        create_directory(self.output, mode=0o700)
        self.work = self.output / "work"
        create_directory(self.work, mode=0o700)
        self.roots, self.pending = [], {}
        self.admission = None
        self.started = time.monotonic()
        self.config = self.output / "runtime.json"
        atomic_write_json(
            self.config,
            dict(
                auth_profiles={
                    p + "-main": dict(provider=p, method="subscription")
                    for p in ("codex", "claude")
                },
                roles=roles,
                runtime=runtime or {},
            ),
        )
        self.cli = configured_cli(
            [str(args.binary), "--state-dir", str(args.state_dir)], self.config
        )
        self.owner = Daemon(self.cli, args.state_dir, self.config, self.output)

    def events(self, agent):
        return [
            dict(row, data=event_data(self.cli, row["data"]))
            for row in event_pages(self.cli, agent)
        ]

    def spawn_task(self, role, body, *, cwd=None, extra=()):
        request = self.label + "-" + uuid.uuid4().hex
        self.pending[request] = dict(role=role, cwd=str(cwd or self.work), body=body)
        atomic_write_json(self.output / "pending-spawns.json", self.pending)
        accepted = self.cli(
            "spawn",
            *([role] if role else []),
            "--cwd",
            str(cwd or self.work),
            "--body",
            body,
            "--request-id",
            request,
            *extra,
        )
        self.roots.append(accepted["agent_id"])
        del self.pending[request]
        atomic_write_json(self.output / "pending-spawns.json", self.pending)
        atomic_write_json(self.output / "roots.json", self.roots)
        return accepted

    def discover(self):
        return owned_agents(self.cli, self.work, self.roots)

    def prerequisites(self):
        preflight(self.args.state_dir)
        for auth in sorted({role["auth"] for role in self.roles.values()}):
            if self.cli("auth", "status", auth).get("state") != "authenticated":
                raise RuntimeError("An existing authenticated subscription login is required")

    def prepare(self):
        pass

    def observe(self):
        pass

    def release(self):
        pass

    def stop_observers(self):
        pass

    def retain(self):
        for agent in self.discover():
            atomic_write_json(self.output / (agent["id"] + "-status.json"), dict(agent=agent))
            atomic_write_json(
                self.output / (agent["id"] + "-events.json"), self.events(agent["id"])
            )

    def cleanup(self):
        def hosted():
            if self.owner.ready:
                with call_deadline(time.monotonic() + 90):
                    cleanup_all(
                        self.release,
                        lambda: cancel_descendants(
                            self.cli, self.roots[0] if self.roots else "", self.discover
                        ),
                        lambda: verify_tasks(self.cli),
                        self.retain,
                    )

        cleanup_all(hosted, self.owner.close, self.stop_observers)
        if self.pending:
            raise RuntimeError("Spawn acceptance unresolved; exact intent and admission retained")
        self.admission.advance("cleaned")

    def run(self):
        try:
            with (
                campaign(self.args, "live-" + self.label) as admission,
                deadline(self.args.timeout),
            ):
                self.admission = admission
                with finalizer(self.cleanup), phase("scenario"):
                    self.prerequisites()
                    self.prepare()
                    self.daemon = self.owner.start(admission)
                    self.observe()
                    result = self.execute()
                finish(
                    lambda: atomic_write_json(
                        self.output / "result.json",
                        dict(
                            result="passed",
                            profile="release",
                            seconds=time.monotonic() - self.started,
                            work_limit_seconds=self.args.timeout,
                            checks=result,
                        ),
                    )
                )
        except BaseException as error:
            with finalizer(
                lambda failure=error: atomic_write_json(
                    self.output / "retained.json",
                    dict(
                        result="unaccepted",
                        error_type=type(failure).__name__,
                        roots=self.roots,
                        unresolved_spawns=self.pending,
                    ),
                )
            ):
                raise
        return 0
