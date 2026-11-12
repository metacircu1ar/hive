"""Source attribution includes modes, deletions, links and untracked product inputs."""

from pathlib import Path
import tempfile
import unittest
from .source import Snapshot


class Git:
    def __init__(self, root, tracked):
        self.root, self.tracked = root, tracked

    def __call__(self, args, *, strip=True, text=True):
        if args[1] == "ls-files" and "--others" not in args:
            output = "\0".join(self.tracked) + "\0"
        elif args[1] == "ls-files":
            names = [
                str(path.relative_to(self.root))
                for path in self.root.rglob("*")
                if (path.is_file() or path.is_symlink())
                and str(path.relative_to(self.root)) not in self.tracked
                and not path.relative_to(self.root).parts[0] in {"target", "dist"}
            ]
            output = "\0".join(sorted(names)) + "\0"
        elif args[1] == "rev-parse":
            output = "a" * 40
        else:
            output = ""
        output = output if text else output.encode()
        return output.strip() if strip else output


class BuildSourceTests(unittest.TestCase):
    def test_mode_deletion_and_untracked_markdown_change_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            file = root / "README.md"
            file.write_text("same contents")
            file.chmod(0o644)
            git = Git(root, ["README.md"])
            before = Snapshot(root, git)
            file.chmod(0o755)
            self.assertNotEqual(
                before.public["tree_sha256"], Snapshot(root, git).public["tree_sha256"]
            )
            with self.assertRaises(ValueError):
                before.check()
            file.unlink()
            deleted = Snapshot(root, git)
            self.assertEqual(deleted.records["README.md"], {"kind": "deleted"})
            (root / "docs").mkdir()
            (root / "docs/setup.md").write_text("new untracked product guide")
            self.assertIn("docs/setup.md", Snapshot(root, git).records)

    def test_source_directory_symlink_is_not_adopted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "project"
            root.mkdir()
            outside = Path(directory) / "outside"
            outside.mkdir()
            (outside / "lib.rs").write_text("external input")
            (root / "src").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "directories must not be symlinks"):
                Snapshot(root, Git(root, ["src/lib.rs"]))

    def test_link_identity_is_hashed_and_external_content_is_not_adopted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "project"
            root.mkdir()
            (root / "one").write_bytes(b"same")
            (root / "two").write_bytes(b"same")
            link = root / "logo.png"
            link.symlink_to("one")
            git = Git(root, ["logo.png", "one", "two"])
            before = Snapshot(root, git)
            link.unlink()
            link.symlink_to("two")
            self.assertNotEqual(before.public, Snapshot(root, git).public)
            private = Path(directory) / "outside"
            private.write_bytes(b"private input")
            link.unlink()
            link.symlink_to(private)
            with self.assertRaises(ValueError):
                Snapshot(root, git)

    def test_owned_output_is_excluded_and_nul_filename_lists_are_not_stripped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / " README.md").write_bytes(b"leading space")
            git = Git(root, [" README.md"])
            output = root / "scripts/release/output"
            before = Snapshot(root, git, exclude=(output,))
            self.assertEqual(before.records[" README.md"]["kind"], "file")
            output.mkdir(parents=True)
            (output / "generated.py").write_text("generated output")
            before.check()
            (root / "src").mkdir()
            (root / "src/new.rs").write_text("new source")
            with self.assertRaises(ValueError):
                before.check()
