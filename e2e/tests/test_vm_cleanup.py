"""Folder-owned cleanup works without output records and preserves active fences."""

import contextlib
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from e2e import campaign
from e2e.campaign import Admission, read_receipt
from e2e.tests.test_vm_contracts import actor_fixture
from e2e.vm.folder import Folder, delete as delete_vm
from e2e.scripts import vm_validation as vm
from release.files import atomic_write_bytes, create_directory
from release.progress import entry
from release.process import ProcessFailure


class FolderCleanup(unittest.TestCase):
    def test_cleanup_uses_current_saved_marker_while_resume_keeps_its_strict_binding(self):
        with actor_fixture() as (actor, _):
            actor.create()
            original = (actor.output / "owner.json").read_bytes()
            changed = dict(actor.state["config"], message="Hive E2E owner: " + "c" * 32)
            atomic_write_bytes(actor.home / actor.name / "lima.yaml", json.dumps(changed).encode())
            with self.assertRaises(ValueError):
                actor.inspect()
            self.assertTrue(vm.retained(actor)["owned"])
            self.assertTrue(Folder(actor.admission, actor.home).listing()[0]["owned"])
            self.assertTrue(delete_vm(actor.admission, actor.name, home=actor.home)["deleted"])
            self.assertEqual((actor.output / "owner.json").read_bytes(), original)

    def test_cli_delete_name_uses_the_existing_guard_without_an_output_argument(self):
        with actor_fixture() as (actor, _):
            actor.create()
            shutil.rmtree(actor.output)
            fd = actor.admission.lock_fd

            @contextlib.contextmanager
            def held(*args, **options):
                yield fd

            args = vm.parse_args(["delete", actor.name])
            with (
                patch.object(vm, "admit_host"),
                patch.object(vm, "Admission", return_value=actor.admission),
                patch.object(vm, "record_lock", side_effect=held),
                patch("e2e.vm.folder.default_home", return_value=actor.home),
                contextlib.redirect_stdout(io.StringIO()) as output,
            ):
                self.assertEqual(entry(lambda: vm.delete(args), name="verify"), 0)
            self.assertTrue(json.loads(output.getvalue())["deleted"])
            self.assertFalse((actor.home / actor.name).exists())
            self.assertIsNone(read_receipt(actor.admission.root))

    def test_an_unrelated_record_is_unchanged_by_delete(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            actor.admission.advance("running", name="other-instance")
            before = read_receipt(actor.admission.root)
            calls = len(transport.calls)
            with self.assertRaises(RuntimeError):
                delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertEqual(read_receipt(actor.admission.root), before)
            self.assertEqual(len(transport.calls), calls)

    def test_orphan_with_marker_is_listed_and_deleted_without_output_or_sidecar(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            actor.start()
            shutil.rmtree(actor.output)
            folder = Folder(actor.admission, actor.home)
            rows = folder.listing()
            self.assertEqual(
                [(row["name"], row["state"], row["owned"]) for row in rows],
                [(actor.name, "Running", True)],
            )
            result = delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertTrue(result["confirmed"])
            self.assertTrue(result["deleted"])
            self.assertIsNone(read_receipt(actor.admission.root))
            self.assertFalse((actor.home / actor.name).exists())
            self.assertEqual(transport.row, None)
            self.assertFalse((actor.admission.root / "vm-owners").exists())

    def test_broken_reaches_force_then_only_stopped_can_be_deleted(self):
        for status, sticks in (("Broken", False), ("Broken", True), ("", False), ("", True)):
            with self.subTest(status=status, sticks=sticks), actor_fixture() as (actor, transport):
                actor.create()
                transport.row["status"] = status
                transport.force_sticks = sticks
                if sticks:
                    with self.assertRaises(RuntimeError):
                        delete_vm(actor.admission, actor.name, home=actor.home)
                    self.assertIsNotNone(read_receipt(actor.admission.root))
                    self.assertTrue((actor.home / actor.name).exists())
                    self.assertFalse(any(row[0] == "delete" for row in transport.calls))
                else:
                    result = delete_vm(actor.admission, actor.name, home=actor.home)
                    self.assertTrue(result["deleted"])
                    self.assertIsNone(read_receipt(actor.admission.root))
                self.assertIn(["stop", "--force", actor.name], transport.calls)

    def test_pending_work_fields_are_required_and_typed_before_any_native_call(self):
        for key, value, missing in (
            ("client_file", None, True),
            ("native_uncertain", None, True),
            ("native_uncertain", None, False),
            ("native_uncertain", 0, False),
            ("native_uncertain", "", False),
            ("native_uncertain", [], False),
            ("client_file", False, False),
            ("client_file", "/outside/proof.json", False),
        ):
            with (
                self.subTest(key=key, value=value, missing=missing),
                actor_fixture() as (actor, transport),
            ):
                actor.create()
                if missing:
                    del actor.admission.active["owner"][key]
                else:
                    actor.admission.active["owner"][key] = value
                actor.admission.save()
                before = read_receipt(actor.admission.root)
                count = len(transport.calls)
                with self.assertRaises(ValueError):
                    delete_vm(actor.admission, actor.name, home=actor.home)
                self.assertEqual(len(transport.calls), count)
                self.assertEqual(read_receipt(actor.admission.root), before)

    def test_start_success_is_recorded_before_failing_post_start_inspection(self):
        with actor_fixture() as (actor, _):
            actor.create()
            with patch.object(
                actor,
                "inspect",
                side_effect=[
                    dict(status="Stopped"),
                    RuntimeError("fixture post-start observation"),
                ],
            ):
                with self.assertRaises(RuntimeError):
                    actor.start()
            self.assertEqual(read_receipt(actor.admission.root)["phase"], "started")

    def test_start_pre_dispatch_failure_does_not_leave_a_starting_fence(self):
        for target in ("home", "proof"):
            with self.subTest(target=target), actor_fixture() as (actor, transport):
                actor.create()
                failure = (
                    patch.object(actor, "check_home", side_effect=OSError("fixture home check"))
                    if target == "home"
                    else patch("e2e.vm.native.proof.intent", side_effect=OSError("fixture proof"))
                )
                with patch.object(actor, "inspect", return_value=dict(status="Stopped")), failure:
                    with self.assertRaises(OSError):
                        actor.start()
                self.assertEqual(read_receipt(actor.admission.root)["phase"], "created")
                self.assertFalse(any(row[0] == "start" for row in transport.calls))
                actor.retire()
                self.assertIsNone(read_receipt(actor.admission.root))

    def test_nonzero_start_stays_fenced_until_a_hostagent_is_observed(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            transport.start_returncode = 1
            with self.assertRaises(ProcessFailure):
                actor.start()
            self.assertTrue(actor.transport.confirmed())
            self.assertEqual(read_receipt(actor.admission.root)["phase"], "starting")
            with self.assertRaises(RuntimeError):
                delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertEqual(read_receipt(actor.admission.root)["phase"], "starting")
            # A delayed hostagent can still publish after a normal failed CLI.
            transport.row.update(status="Broken", hostAgentPID=12345)
            self.assertTrue(delete_vm(actor.admission, actor.name, home=actor.home)["deleted"])
            self.assertIsNone(read_receipt(actor.admission.root))

    def test_unmarked_and_symlink_entries_are_visible_but_never_deleted(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            config = actor.home / actor.name / "lima.yaml"
            atomic_write_bytes(config, b'{"message":"not a Hive marker"}\n')
            folder = Folder(actor.admission, actor.home)
            self.assertFalse(folder.listing()[0]["owned"])
            actor.admission.complete()
            with self.assertRaises(ValueError):
                delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertIsNone(actor.admission.active)
            outside = actor.home.parent / "outside"
            create_directory(outside, mode=0o700)
            atomic_write_bytes(outside / "lima.yaml", json.dumps(actor.state["config"]).encode())
            (actor.home / "foreign").symlink_to(outside, target_is_directory=True)
            with self.assertRaises(ValueError):
                delete_vm(actor.admission, "foreign", home=actor.home)
            self.assertTrue(outside.exists())
            self.assertFalse(any(row[0] == "delete" for row in transport.calls))

    def test_local_uncertainty_is_only_in_active_record_and_blocks_cleanup(self):
        with actor_fixture() as (actor, transport):
            actor.create()
            actor.start()
            transport.stop_raises = transport.stop_unknown = True
            folder = Folder(actor.admission, actor.home)
            with self.assertRaises(TimeoutError):
                folder.stop(actor.name)
            active = read_receipt(actor.admission.root)
            self.assertTrue(active["owner"]["native_uncertain"])
            self.assertNotIn(
                "native_uncertain", json.loads((actor.output / "owner.json").read_text())
            )
            before = len(transport.calls)
            with self.assertRaises(RuntimeError):
                delete_vm(actor.admission, actor.name, home=actor.home)
            self.assertEqual(len(transport.calls), before)

    def test_manual_clear_ignores_invalid_sidecar_and_is_one_atomic_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            invalid = root / "unsupported-owner.json"
            atomic_write_bytes(invalid, b'{"unsupported":"changed pins or format"}\n')
            admission = Admission(root / "admission")
            with contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(RuntimeError), admission.held():
                    active = admission.begin(
                        "lima",
                        "uncertain-fixture",
                        dict(
                            native_uncertain=True,
                            client_file="/missing/proof.json",
                            sidecar=str(invalid),
                        ),
                    )
            argv = ["campaign", "clear", "--nonce", active["nonce"], "--manual-cleanup-confirmed"]
            before = invalid.read_bytes()
            with (
                patch.object(campaign, "default_root", return_value=admission.root),
                patch("sys.argv", argv),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                with patch.object(
                    campaign, "atomic_write_json", side_effect=OSError("fixture publication failed")
                ):
                    with self.assertRaises(OSError):
                        campaign.main()
                self.assertEqual(read_receipt(admission.root), active)
                with patch.object(
                    campaign, "atomic_write_json", wraps=campaign.atomic_write_json
                ) as publication:
                    campaign.main()
                    self.assertEqual(publication.call_count, 1)
            self.assertIsNone(read_receipt(admission.root))
            self.assertEqual(invalid.read_bytes(), before)
            with admission.held():
                self.assertIsNone(admission.active)


if __name__ == "__main__":
    unittest.main()
