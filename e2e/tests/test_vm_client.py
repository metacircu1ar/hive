"""Native CLI custody preserves outcomes without leaking admission to daemons."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from e2e.campaign import Admission
from e2e.vm import client, proof
from release.process import ProcessFailure, run


class VmClientTests(unittest.TestCase):
    def test_native_child_has_no_admission_fd_and_keeps_exact_returncode(self):
        probe = """
import errno,json,os,sys
expected=tuple(map(int,sys.argv[1:3])); found=False
for fd in range(3,256):
    try: info=os.fstat(fd)
    except OSError as error:
        if error.errno!=errno.EBADF: raise
    else:
        found=found or (info.st_dev,info.st_ino)==expected
print(json.dumps(dict(inherited=found)))
raise SystemExit(int(sys.argv[3]))
"""
        with tempfile.TemporaryDirectory() as temporary:
            admission = Admission(Path(temporary) / "admission")
            with admission.held():
                admission.begin("native", "fixture-client", dict(pid=0))
                info = os.fstat(admission.lock_fd)
                for status in (0, 7, 124, 254):
                    evidence = proof.intent(admission.root)
                    result = client.run(
                        [
                            sys.executable,
                            "-c",
                            probe,
                            str(info.st_dev),
                            str(info.st_ino),
                            str(status),
                        ],
                        lock_fd=admission.lock_fd,
                        evidence=evidence,
                        text=True,
                    )
                    self.assertEqual(result.returncode, status)
                    self.assertEqual(proof.read(evidence)["returncode"], status)
                    self.assertEqual(proof.read(evidence)["kind"], "completed")
                    self.assertFalse(json.loads(result.stdout)["inherited"])
                with self.assertRaises(ProcessFailure) as failure:
                    client.run(
                        [sys.executable, "-c", "raise SystemExit(7)"],
                        lock_fd=admission.lock_fd,
                        check=True,
                    )
                self.assertEqual(failure.exception.result.returncode, 7)
                self.assertFalse(getattr(failure.exception, "owned_cleanup_uncertain", False))
                admission.complete()

    def test_timeout_and_launch_failure_are_infrastructure_not_native_outcomes(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            client.run([sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.1)
        with self.assertRaises(RuntimeError) as failure:
            client.run(["/missing-hive-fixture-executable"], check=False)
        self.assertFalse(getattr(failure.exception, "owned_cleanup_uncertain", False))

    def test_successful_helper_without_completion_never_counts_as_native_success(self):
        def missing_completion(argv, **options):
            return run([sys.executable, "-c", "pass"], **options)

        with self.assertRaises(RuntimeError) as failure:
            client.run(["unused"], native=missing_completion)
        self.assertTrue(failure.exception.owned_cleanup_uncertain)

    def test_malformed_completion_on_cancellation_preserves_uncertainty(self):
        from release.cancellation import Cancelled
        import signal

        with client.channel() as (completion, writer):
            data = b"not-json\n"
            self.assertEqual(os.write(writer, data), len(data))
            with self.assertRaises(Cancelled) as failure:
                with completion.protect():
                    raise Cancelled(signal.SIGTERM)
            self.assertTrue(failure.exception.owned_cleanup_uncertain)
            self.assertTrue(
                any(isinstance(error, ValueError) for error in failure.exception.cleanup_errors)
            )

    def test_private_completion_descriptor_refuses_non_pipe(self):
        with tempfile.TemporaryFile() as stream:
            done = run(
                [
                    sys.executable,
                    str(Path(client.__file__)),
                    "--timeout",
                    "1",
                    "--limit",
                    "1024",
                    "--status-fd",
                    str(stream.fileno()),
                    "--",
                    sys.executable,
                    "-c",
                    "pass",
                ],
                pass_fds=(stream.fileno(),),
            )
            self.assertNotEqual(done.returncode, 0)
            self.assertEqual(os.fstat(stream.fileno()).st_size, 0)


if __name__ == "__main__":
    unittest.main()
