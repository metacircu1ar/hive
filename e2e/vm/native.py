"""Lima CLI custody; the active admission is the sole pending-work fence."""

from contextlib import contextmanager
from pathlib import Path

from e2e.campaign import cleanup_uncertain
from e2e.ci.steps import ci_env
from e2e.vm import client, proof
from release.process import run
from release.progress import finalizer


class Native:
    def __init__(self, admission, home, *, native=run):
        self.admission, self.home, self.native = admission, Path(home).absolute(), native
        self.env = dict(ci_env(), LIMA_HOME=str(self.home))

    def confirmed(self):
        active = self.admission.active
        if active is None:
            return True
        owner = active["owner"]
        if (
            "client_file" not in owner
            or "native_uncertain" not in owner
            or type(owner["native_uncertain"]) is not bool
        ):
            raise ValueError("Unsupported pending-work fields in the active VM record")
        path = owner["client_file"]
        if path is not None and (
            not isinstance(path, str) or Path(path).parent != self.admission.root / "clients"
        ):
            raise ValueError("Native CLI proof is outside this admission directory")
        return not owner["native_uncertain"] and proof.confirmed(path)

    def request(self, *, phase=None):
        if (
            self.admission is None
            or self.admission.active is None
            or self.admission.active["kind"] != "lima"
        ):
            raise RuntimeError("Native Lima work requires an active VM admission")
        if not self.confirmed():
            error = RuntimeError("The preceding native CLI has no confirmed cleanup receipt")
            error.owned_cleanup_uncertain = True
            raise error
        path = proof.intent(self.admission.root)
        self.admission.advance(
            self.admission.active["phase"] if phase is None else phase, client_file=str(path)
        )
        return path

    @contextmanager
    def protect(self):
        try:
            yield
        except BaseException as error:
            if (
                cleanup_uncertain(error)
                and self.admission is not None
                and self.admission.active is not None
            ):
                with finalizer(
                    lambda: self.admission.advance(
                        self.admission.active["phase"], native_uncertain=True
                    )
                ):
                    raise
            raise

    def call(self, args, *, phase=None, **options):
        if self.admission is None:
            if args[:1] not in (["list"], ["--version"]):
                raise RuntimeError("Only read-only Lima discovery may run without admission")
            lock_fd, evidence = None, None
        else:
            lock_fd = self.admission.lock_fd
            if lock_fd is None:
                raise RuntimeError("Native Lima work requires the held admission lock")
            evidence = self.request(phase=phase)
        with self.protect():
            return client.run(
                ["limactl", *args],
                lock_fd=lock_fd,
                evidence=evidence,
                native=self.native,
                env=self.env,
                check=options.pop("check", True),
                text=True,
                **options,
            )
