use super::*;

#[test]
fn memory_below_the_floor_is_refused_with_the_needed_amount() {
    assert!(admit_memory(MEMORY_FLOOR_BYTES).is_ok());
    let error = admit_memory(MEMORY_FLOOR_BYTES - 1).unwrap_err();
    assert_eq!(error.code, ErrorCode::EmbeddingModel);
    assert!(error.message.contains(&MEMORY_FLOOR_BYTES.to_string()));
    assert!(error.message.contains("16 GiB"));
}

fn tree(root: &Path, meminfo: &str, cgroup: &str) -> (PathBuf, PathBuf) {
    let proc = root.join("proc");
    let groups = root.join("cgroup");
    std::fs::create_dir_all(proc.join("self")).unwrap();
    std::fs::write(proc.join("meminfo"), meminfo).unwrap();
    std::fs::write(proc.join("self/cgroup"), cgroup).unwrap();
    std::fs::create_dir_all(&groups).unwrap();
    let (kind, point, options) = if cgroup.contains(":memory:") {
        ("cgroup", groups.join("memory"), "rw,memory")
    } else {
        ("cgroup2", groups.clone(), "rw")
    };
    std::fs::create_dir_all(&point).unwrap();
    std::fs::write(point.join("cgroup.controllers"), "memory cpu\n").unwrap();
    std::fs::write(
        proc.join("self/mountinfo"),
        format!(
            "1 0 0:1 / {} rw - {kind} cgroup {options}\n",
            point.display()
        ),
    )
    .unwrap();
    (proc, groups)
}

fn empty_file_cache(directory: &Path) {
    std::fs::write(
        directory.join("memory.stat"),
        "active_file 0\ninactive_file 0\ntotal_active_file 0\ntotal_inactive_file 0\n",
    )
    .unwrap();
}

#[test]
fn linux_floor_checks_available_memory_instead_of_nominal_host_capacity() {
    let directory = tempfile::tempdir().unwrap();
    // Representative /proc shape on a nominal 16 GiB guest: kernel-reserved
    // pages reduce MemTotal, and existing use reduces MemAvailable further.
    let (proc, _groups) = tree(
        directory.path(),
        "MemTotal:       16336800 kB\nMemFree:        14200000 kB\nMemAvailable:   15000000 kB\nBuffers:          100000 kB\nCached:           800000 kB\n",
        "0::/\n",
    );
    let available = memory::linux(&proc).unwrap();
    assert_eq!(available, 15_000_000 * 1024);
    assert!(admit_memory(available).is_err());
    // A larger guest passes only when its actually available memory also does.
    std::fs::write(
        proc.join("meminfo"),
        "MemTotal: 20500000 kB\nMemAvailable: 18000000 kB\n",
    )
    .unwrap();
    assert!(admit_memory(memory::linux(&proc).unwrap()).is_ok());
}

#[test]
fn linux_available_memory_honors_every_cgroup_limit() {
    let directory = tempfile::tempdir().unwrap();
    let gib = 1024 * 1024 * 1024_u64;
    let meminfo = "MemTotal: 67108864 kB\nMemAvailable: 33554432 kB\n";
    let (proc, groups) = tree(directory.path(), meminfo, "0::/user.slice/app\n");
    assert_eq!(
        memory::linux(&proc).unwrap_err().code,
        ErrorCode::EmbeddingModel
    );
    let app = groups.join("user.slice/app");
    std::fs::create_dir_all(&app).unwrap();
    assert_eq!(memory::linux(&proc).unwrap(), 32 * gib);
    std::fs::write(app.join("memory.max"), "max\n").unwrap();
    std::fs::write(groups.join("user.slice/memory.max"), (20 * gib).to_string()).unwrap();
    std::fs::write(
        groups.join("user.slice/memory.current"),
        (8 * gib).to_string(),
    )
    .unwrap();
    empty_file_cache(&groups.join("user.slice"));
    assert_eq!(memory::linux(&proc).unwrap(), 12 * gib);
    let (proc, groups) = tree(
        &directory.path().join("v1"),
        meminfo,
        "4:memory:/docker/box\n3:cpu:/docker/box\n",
    );
    let box_group = groups.join("memory/docker/box");
    std::fs::create_dir_all(&box_group).unwrap();
    std::fs::write(
        box_group.join("memory.limit_in_bytes"),
        (4 * gib).to_string(),
    )
    .unwrap();
    std::fs::write(box_group.join("memory.usage_in_bytes"), gib.to_string()).unwrap();
    empty_file_cache(&box_group);
    let available = memory::linux(&proc).unwrap();
    assert_eq!(available, 3 * gib);
    assert!(admit_memory(available).is_err());
    let (proc, _groups) = tree(&directory.path().join("escape"), meminfo, "0::/../../etc\n");
    assert!(memory::linux(&proc).is_err());
    let (proc, _groups) = tree(&directory.path().join("bad"), "MemAvailable: x kB\n", "");
    assert!(memory::linux(&proc).is_err());
}

#[test]
fn hybrid_hierarchy_uses_the_v1_memory_mount_and_ignores_unrelated_v2() {
    let directory = tempfile::tempdir().unwrap();
    let (proc, groups) = tree(
        directory.path(),
        "MemAvailable: 33554432 kB\n",
        "0::/user.slice/missing\n4:memory:/docker/box\n",
    );
    let unified = groups.join("unified");
    std::fs::create_dir_all(&unified).unwrap();
    std::fs::write(unified.join("cgroup.controllers"), "cpu io\n").unwrap();
    let memory = groups.join("memory");
    let member = memory.join("docker/box");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(
        member.join("memory.limit_in_bytes"),
        (20_u64 << 30).to_string(),
    )
    .unwrap();
    std::fs::write(
        member.join("memory.usage_in_bytes"),
        (2_u64 << 30).to_string(),
    )
    .unwrap();
    let v1 = format!(
        "2 0 0:2 / {} rw - cgroup cgroup rw,memory\n",
        memory.display()
    );
    empty_file_cache(&member);
    std::fs::write(
        proc.join("self/mountinfo"),
        format!(
            "1 0 0:1 / {} rw - cgroup2 cgroup rw\n{v1}",
            unified.display()
        ),
    )
    .unwrap();
    assert_eq!(memory::linux(&proc).unwrap(), 18_u64 << 30);
    // Some hybrid environments expose no cgroup2 mount at all.
    std::fs::write(proc.join("self/mountinfo"), v1).unwrap();
    assert_eq!(memory::linux(&proc).unwrap(), 18_u64 << 30);
}

#[test]
fn bind_mounted_v1_container_leaf_uses_its_mount_root() {
    let directory = tempfile::tempdir().unwrap();
    let (proc, groups) = tree(
        directory.path(),
        "MemAvailable: 33554432 kB\n",
        "4:memory:/docker/box\n",
    );
    let leaf = groups.join("memory");
    std::fs::write(
        leaf.join("memory.limit_in_bytes"),
        (4_u64 << 30).to_string(),
    )
    .unwrap();
    std::fs::write(
        leaf.join("memory.usage_in_bytes"),
        (1_u64 << 30).to_string(),
    )
    .unwrap();
    std::fs::write(
        proc.join("self/mountinfo"),
        format!(
            "1 0 0:1 /docker/box {} rw - cgroup cgroup rw,memory\n",
            leaf.display()
        ),
    )
    .unwrap();
    empty_file_cache(&leaf);
    assert_eq!(memory::linux(&proc).unwrap(), 3_u64 << 30);
    std::fs::write(proc.join("self/cgroup"), "4:memory:/docker/another\n").unwrap();
    assert!(memory::linux(&proc).is_err());
}

#[test]
fn mountinfo_escaped_mount_point_is_resolved() {
    let directory = tempfile::tempdir().unwrap();
    let (proc, groups) = tree(directory.path(), "MemAvailable: 33554432 kB\n", "0::/\n");
    let mounted = groups.join("with space");
    std::fs::create_dir_all(&mounted).unwrap();
    std::fs::write(mounted.join("cgroup.controllers"), "memory\n").unwrap();
    std::fs::write(mounted.join("memory.max"), (20_u64 << 30).to_string()).unwrap();
    std::fs::write(mounted.join("memory.current"), (3_u64 << 30).to_string()).unwrap();
    empty_file_cache(&mounted);
    std::fs::write(
        proc.join("self/mountinfo"),
        format!(
            "1 0 0:1 / {} rw - cgroup2 cgroup rw\n",
            mounted.to_string_lossy().replace(' ', "\\040")
        ),
    )
    .unwrap();
    assert_eq!(memory::linux(&proc).unwrap(), 17_u64 << 30);
}

#[test]
fn cached_model_pages_are_reclaimable_but_anonymous_and_shmem_usage_are_not() {
    for v1 in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let membership = if v1 { "4:memory:/\n" } else { "0::/\n" };
        let (proc, groups) = tree(directory.path(), "MemAvailable: 33554432 kB\n", membership);
        let (point, maximum, used, prefix) = if v1 {
            (
                groups.join("memory"),
                "memory.limit_in_bytes",
                "memory.usage_in_bytes",
                "total_",
            )
        } else {
            (groups, "memory.max", "memory.current", "")
        };
        std::fs::write(point.join(maximum), (20_u64 << 30).to_string()).unwrap();
        std::fs::write(point.join(used), (10_u64 << 30).to_string()).unwrap();
        std::fs::write(
            point.join("memory.stat"),
            format!(
                "{prefix}active_file {}\n{prefix}inactive_file {}\n",
                4_u64 << 30,
                4_u64 << 30
            ),
        )
        .unwrap();
        let available = memory::linux(&proc).unwrap();
        assert_eq!(available, 18_u64 << 30);
        assert!(admit_memory(available).is_ok());
        // A broad file count can include tmpfs: only file-LRU counters count.
        std::fs::write(
            point.join("memory.stat"),
            format!(
                "file {}\nshmem {}\n{prefix}active_file 0\n{prefix}inactive_file 0\n",
                8_u64 << 30,
                8_u64 << 30
            ),
        )
        .unwrap();
        let available = memory::linux(&proc).unwrap();
        assert_eq!(available, 10_u64 << 30);
        assert!(admit_memory(available).is_err());
        std::fs::write(point.join("memory.stat"), "file 0\n").unwrap();
        assert!(memory::linux(&proc).is_err());
    }
}
