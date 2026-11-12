"""Synthetic mount namespaces for the auth-free CPU memory admission probe."""

from pathlib import Path
import tempfile
import unittest

from e2e.models.memory import available
from release.files import atomic_write_text

GIB = 1024**3


class MemoryMounts(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.proc = self.root / "proc"
        self.write(self.proc / "meminfo", "MemAvailable: 33554432 kB\n")

    def write(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.name in ("memory.current", "memory.usage_in_bytes"):
            atomic_write_text(path.parent / "memory.stat",
                              "active_file 0\ninactive_file 0\n"
                              "total_active_file 0\ntotal_inactive_file 0\n")
        atomic_write_text(path, text)

    def mount(self, root, point, kind):
        def escape(text):
            return str(text).replace("\\", "\\134").replace(" ", "\\040")
        options = "rw,memory" if kind == "cgroup" else "rw"
        return f"1 0 0:1 {escape(root)} {escape(point)} rw - {kind} cgroup {options}\n"

    def test_hybrid_v2_without_memory_does_not_hide_the_v1_limit(self):
        unified, memory = self.root / "unified", self.root / "memory"
        self.write(unified / "cgroup.controllers", "cpu io\n")
        self.write(memory / "docker/box/memory.limit_in_bytes", str(20 * GIB))
        self.write(memory / "docker/box/memory.usage_in_bytes", str(2 * GIB))
        self.write(self.proc / "self/cgroup", "0::/user.slice/missing\n4:memory:/docker/box\n")
        v1 = self.mount("/", memory, "cgroup")
        self.write(self.proc / "self/mountinfo", self.mount("/", unified, "cgroup2") + v1)
        self.assertEqual(available(self.proc), 18 * GIB)
        self.write(self.proc / "self/mountinfo", v1)
        self.assertEqual(available(self.proc), 18 * GIB)

    def test_v1_bind_mount_strips_the_container_leaf_root(self):
        memory = self.root / "memory"
        self.write(memory / "memory.limit_in_bytes", str(4 * GIB))
        self.write(memory / "memory.usage_in_bytes", str(GIB))
        self.write(self.proc / "self/cgroup", "4:memory:/docker/box\n")
        self.write(self.proc / "self/mountinfo", self.mount("/docker/box", memory, "cgroup"))
        self.assertEqual(available(self.proc), 3 * GIB)
        self.write(self.proc / "self/cgroup", "4:memory:/docker/other\n")
        with self.assertRaisesRegex(ValueError, "Cannot resolve"):
            available(self.proc)

    def test_escaped_mount_paths_resolve(self):
        memory = self.root / "with space"
        self.write(memory / "cgroup.controllers", "memory\n")
        self.write(memory / "memory.max", str(20 * GIB))
        self.write(memory / "memory.current", str(3 * GIB))
        self.write(self.proc / "self/cgroup", "0::/\n")
        self.write(self.proc / "self/mountinfo", self.mount("/", memory, "cgroup2"))
        self.assertEqual(available(self.proc), 17 * GIB)

    def test_cached_model_lru_pages_are_available_but_shmem_is_not(self):
        for v1 in (False, True):
            with self.subTest(v1=v1):
                point = self.root / ("v1" if v1 else "v2")
                kind = "cgroup" if v1 else "cgroup2"
                prefix = "total_" if v1 else ""
                maximum = "memory.limit_in_bytes" if v1 else "memory.max"
                used = "memory.usage_in_bytes" if v1 else "memory.current"
                self.write(self.proc / "self/cgroup", "4:memory:/\n" if v1 else "0::/\n")
                self.write(self.proc / "self/mountinfo", self.mount("/", point, kind))
                self.write(point / "cgroup.controllers", "memory\n")
                self.write(point / maximum, str(20 * GIB))
                self.write(point / used, str(10 * GIB))
                self.write(point / "memory.stat",
                           f"{prefix}active_file {4 * GIB}\n"
                           f"{prefix}inactive_file {4 * GIB}\n")
                self.assertEqual(available(self.proc), 18 * GIB)
                self.write(point / "memory.stat", f"file {8 * GIB}\nshmem {8 * GIB}\n"
                           f"{prefix}active_file 0\n{prefix}inactive_file 0\n")
                self.assertEqual(available(self.proc), 10 * GIB)
                self.write(point / "memory.stat", "file 0\n")
                with self.assertRaisesRegex(ValueError, "Missing cgroup"):
                    available(self.proc)
