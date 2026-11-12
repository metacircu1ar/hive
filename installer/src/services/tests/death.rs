use super::*;
use serde::{Deserialize, Serialize};
use std::{os::unix::fs::DirBuilderExt, path::PathBuf, process::Command};

const ROOT: &str = "HIVE_INSTALL_POINTER_TEST_ROOT";
const POINT: &str = "HIVE_INSTALL_POINTER_TEST_POINT";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    loaded: BTreeSet<String>,
    starts: Vec<String>,
}

#[derive(Clone)]
struct DiskManager {
    manager: Manager,
    path: PathBuf,
}

impl DiskManager {
    fn new(root: &Path) -> Self {
        Self {
            manager: Manager::default(),
            path: root.join("native-manager.json"),
        }
    }

    fn read(&self) -> std::io::Result<Snapshot> {
        files::json(&self.path)?.ok_or_else(|| std::io::Error::other("Missing fixture manager"))
    }

    fn save(&self) -> std::io::Result<()> {
        files::write(
            &self.path,
            &files::encode(&Snapshot {
                loaded: self.manager.loaded.borrow().clone(),
                starts: self.manager.start_attempts.borrow().clone(),
            })?,
            true,
        )
    }

    fn services(&self, root: &Path) -> Services {
        let health = self.clone();
        Services {
            yes: true,
            runner: Box::new(self.clone()),
            health: Box::new(move |spec| {
                Ok(Health {
                    ready: health.read()?.loaded.contains(&spec.scope.label()?),
                    loaded_agents: Some(3),
                    active_turns: Some(1),
                })
            }),
            layout: Layout {
                home: root.into(),
                config: root.join("config"),
                state: root.join("state"),
                platform: Platform::Launchd,
            },
            held: vec![],
            journal: None,
        }
    }
}

impl Runner for DiskManager {
    fn run(&self, program: &str, args: &[String]) -> std::io::Result<Output> {
        let before = self.read()?;
        *self.manager.loaded.borrow_mut() = before.loaded;
        *self.manager.start_attempts.borrow_mut() = before.starts;
        let output = self.manager.run(program, args)?;
        self.save()?;
        Ok(output)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Death {
    point: String,
    pid: u32,
}

#[test]
#[ignore = "owned native fixture exits without unwinding at pointer publication"]
fn pointer_death_fixture() {
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let point = std::env::var(POINT).unwrap();
    assert!(matches!(point.as_str(), "before_replace" | "after_replace"));
    let part = fixture(&root, "hive-core");
    let manager = DiskManager::new(&root);
    publish::install_with(
        &root.join("prefix"),
        &root,
        &[part],
        &mut manager.services(&root),
        |at| {
            if at == point {
                files::write(
                    &root.join("native-death.json"),
                    &files::encode(&Death {
                        point: point.clone(),
                        pid: std::process::id(),
                    })?,
                    false,
                )?;
                std::process::exit(73);
            }
            Ok(())
        },
    )
    .unwrap();
    panic!("native pointer death point was not reached");
}

fn die_at(root: &Path, point: &str) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env(ROOT, root).env(POINT, point).args([
        "--exact",
        "services::tests::death::pointer_death_fixture",
        "--ignored",
    ]);
    let result = hive_setup_support::runner::capture_command(
        Command::spawn,
        &mut command,
        Duration::from_secs(30),
        &|| false,
    )
    .unwrap();
    assert_eq!(result.code, 73, "{}", result.stdout);
    let death: Death = files::json(&root.join("native-death.json"))
        .unwrap()
        .unwrap();
    assert_eq!(death.point, point);
    let pid = rustix::process::Pid::from_raw(i32::try_from(death.pid).unwrap()).unwrap();
    assert_eq!(
        rustix::process::test_kill_process_group(pid),
        Err(rustix::io::Errno::SRCH)
    );
}

fn recover_and_check(manager: &DiskManager, root: &Path, prefix: &Path, expected: &[Spec]) {
    let installation = prefix.join("lib/hive");
    let pointer = publish::current(&installation).unwrap();
    manager
        .services(root)
        .recover(prefix, &installation)
        .unwrap();
    for spec in expected {
        assert_eq!(fs::read(&spec.unit).unwrap(), spec.unit_contents.as_bytes());
        assert_eq!(
            fs::read(&spec.descriptor).unwrap(),
            files::encode(spec).unwrap()
        );
    }
    assert!(!installation.join(PENDING).exists());
    let before = files::read(&manager.path).unwrap().unwrap();
    manager
        .services(root)
        .recover(prefix, &installation)
        .unwrap();
    assert_eq!(files::read(&manager.path).unwrap().unwrap(), before);
    assert_eq!(publish::current(&installation).unwrap(), pointer);
}

fn selected_specs(installation: &Path, previous: Option<&Path>, before: bool) -> Vec<Spec> {
    let journal = record::read(&installation.join(PENDING)).unwrap().unwrap();
    let expected = if before {
        journal.change.previous.clone()
    } else {
        Some(journal.change.next.clone())
    };
    assert_eq!(publish::current(installation).unwrap(), expected);
    assert_eq!(expected.as_deref() == previous, before);
    let installed = publish::load(
        installation,
        expected.as_deref(),
        &crate::manifest::native_target().unwrap(),
    )
    .unwrap();
    assert_eq!(
        installed.parts.get("hive").unwrap().part.as_str(),
        if before { "hive" } else { "hive-core" }
    );
    journal
        .services
        .into_iter()
        .map(|saved| if before { saved.spec } else { saved.next })
        .collect()
}

fn unknown_is_unchanged(path: &Path, identity: &atomic_fs::DirectoryIdentity) {
    assert_eq!(&atomic_fs::DirectoryIdentity::of(path).unwrap(), identity);
    assert_eq!(
        files::read(&path.join("sentinel")).unwrap().unwrap(),
        b"unclaimed"
    );
    let entries = fs::read_dir(path)
        .unwrap()
        .try_fold(0, |count, entry| entry.map(|_| count + 1))
        .unwrap();
    assert_eq!(entries, 1);
}

fn interrupted_pointer(point: &str) {
    let World {
        temp,
        prefix,
        layout,
        manager,
        running,
        stopped,
    } = World::new();
    // Reuse the checked owner: every panic retains the root, including capture
    // or group-observation failure, before any potentially live writer is removed.
    let _root = crate::publication::Owned::new(temp).unwrap();
    let root = &layout.home;
    eprintln!("native pointer-death fixture evidence: {}", root.display());
    let disk = DiskManager::new(root);
    *disk.manager.loaded.borrow_mut() = manager.loaded.borrow().clone();
    disk.save().unwrap();
    let installation = prefix.join("lib/hive");
    let previous = publish::current(&installation).unwrap();
    let unknown = installation.join("generations/.preparing-unclaimed-native");
    fs::DirBuilder::new().mode(0o700).create(&unknown).unwrap();
    files::write(&unknown.join("sentinel"), b"unclaimed", false).unwrap();
    let identity = atomic_fs::DirectoryIdentity::of(&unknown).unwrap();
    die_at(root, point);
    assert!(disk.read().unwrap().loaded.is_empty());
    let expected = selected_specs(
        &installation,
        previous.as_deref(),
        point == "before_replace",
    );
    recover_and_check(&disk, root, &prefix, &expected);
    let recovered = disk.read().unwrap();
    assert_eq!(recovered.loaded, BTreeSet::from([running.clone()]));
    assert_eq!(recovered.starts, vec![running]);
    assert!(!recovered.loaded.contains(&stopped));
    unknown_is_unchanged(&unknown, &identity);
    let part = fixture(root, "hive-core");
    publish::install_with(&prefix, root, &[part], &mut disk.services(root), |_| Ok(())).unwrap();
    unknown_is_unchanged(&unknown, &identity);
}

#[test]
fn native_pointer_death_recovers_original_running_set_and_keeps_unknown_preparations() {
    for point in ["before_replace", "after_replace"] {
        interrupted_pointer(point);
    }
}
