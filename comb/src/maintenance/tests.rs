use super::*;
use crate::{
    Actor,
    service::{Operation, auth::Connection},
    test_support::write,
};
use std::{fs, os::unix::fs::MetadataExt as _};

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    bridge: PathBuf,
    receipt: PathBuf,
}
use std::path::PathBuf;

#[test]
fn m5c_bridge_revocation_remap_preserves_the_native_publication_boundary() {
    for step in [
        atomic_fs::Step::BeforePublish,
        atomic_fs::Step::AfterPublish,
    ] {
        let fixture = Fixture::new();
        fixture.plan(false).save(&fixture.receipt).unwrap();
        let registry = fixture.root.join(".comb/credentials.json");
        let before = fs::read(&registry).unwrap();
        let error = crate::disk::publication_hooks::with(
            move |path, at| {
                if path.file_name() == Some(std::ffi::OsStr::new("credentials.json")) && at == step
                {
                    Err(std::io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || apply(&fixture.receipt).err().unwrap(),
        );
        assert_eq!(error.file_committed, step == atomic_fs::Step::AfterPublish);
        assert_eq!(
            fs::read(&registry).unwrap() != before,
            step == atomic_fs::Step::AfterPublish
        );
        assert!(fixture.bridge.exists());
        assert!(error.message.contains("Revoke the exact credential"));
    }
}

#[test]
fn m5c_accepted_activation_recovery_keeps_commit_metadata_and_does_not_revoke_bridge() {
    let fixture = Fixture::new();
    fixture.plan(false).save(&fixture.receipt).unwrap();
    let mut settings =
        serde_json::to_value(crate::provision::settings(&fixture.root).unwrap().unwrap()).unwrap();
    settings["embeddings"] = serde_json::json!({"backend":"http", "model":"fixture", "revision":"pinned", "dimensions":2,
        "url":"http://127.0.0.1:9/v1/embeddings", "api_key_env":format!("HIVE_TEST_M5C_{}", crate::values::CombId::generate().as_str().replace('-', "_"))});
    fs::write(
        fixture.root.join(".comb/model-transition.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    let bridge = fs::read(&fixture.bridge).unwrap();
    let error = apply(&fixture.receipt).err().unwrap();
    assert!(error.file_committed);
    assert!(error.message.contains("Finish accepted activation"));
    assert_eq!(fs::read(&fixture.bridge).unwrap(), bridge);
    assert!(fixture.root.join(".comb/model-transition.json").is_file());
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let result = Self {
            root: base.join("root"),
            bridge: base.join("bridge.json"),
            receipt: base.join("purge.json"),
            temp,
        };
        crate::setup::run_offline(
            &result.root,
            None,
            crate::setup::Options {
                for_hive: true,
                bridge_file: Some(result.bridge.clone()),
                ..crate::setup::Options::default()
            },
        )
        .unwrap();
        result
    }
    fn plan(&self, remove_root: bool) -> Plan<Details> {
        prepare(
            &self.root,
            &Args {
                detach_comb: true,
                bridge_file: Some(self.bridge.clone()),
                purge_comb_root: remove_root.then(|| self.root.clone()),
                ..Args::default()
            },
        )
        .unwrap()
    }
}

#[test]
fn omitted_bridge_or_root_identity_refuses_purge_before_any_removal() {
    let fixture = Fixture::new();
    let plan = fixture.plan(false);
    let valid = serde_json::to_value(&plan).unwrap();
    let credential = fs::read(&fixture.bridge).unwrap();
    let missing = ["bridge", "root_id"].into_iter().map(|field| {
        let mut value = valid.clone();
        value["details"].as_object_mut().unwrap().remove(field);
        value
    });
    let mut positional = valid.clone();
    positional["details"] = serde_json::json!([
        valid["details"]["detach"],
        valid["details"]["remove_root"],
        valid["details"]["root_id"],
        valid["details"]["bridge"],
        valid["details"]["phase"]
    ]);
    for invalid in missing.chain(std::iter::once(positional)) {
        hive_setup_support::files::write(
            &fixture.receipt,
            &serde_json::to_vec(&invalid).unwrap(),
            true,
        )
        .unwrap();
        let before = fs::read(&fixture.receipt).unwrap();
        assert!(apply(&fixture.receipt).is_err());
        assert_eq!(fs::read(&fixture.receipt).unwrap(), before);
        assert_eq!(fs::read(&fixture.bridge).unwrap(), credential);
        assert!(fixture.root.join(".comb").is_dir());
    }
}
#[test]
fn default_purge_retains_trusted_pages_index_and_other_credentials() {
    let f = Fixture::new();
    let old = Connection::read(&f.bridge).unwrap();
    let mut comb = Comb::open(&f.root).unwrap();
    let slug = "facts/keep".parse().unwrap();
    let markdown = "---\ntitle: Retained fact\ntype: note\ntags: []\naliases: []\n---\nEvidence stays trusted.";
    comb.put_markdown(
        &slug,
        markdown,
        None,
        &write(&Actor::Human, "purge-fact", None).unwrap(),
    )
    .unwrap();
    service::offline(
        &mut comb,
        &Operation::CredentialCreate {
            grant: service::auth::Grant::Client {
                name: "other".into(),
                reviewer: false,
            },
        },
        None,
    )
    .unwrap();
    let identity = comb.identity();
    drop(comb);
    let plan = f.plan(false);
    plan.save(&f.receipt).unwrap();
    apply(&f.receipt).unwrap();
    assert!(!f.bridge.exists());
    assert!(!f.root.join(".comb/serve.json").exists());
    let mut comb = Comb::open(&f.root).unwrap();
    assert_eq!(comb.identity(), identity);
    assert!(!comb.get(&slug).unwrap().unverified);
    let listed = service::offline(&mut comb, &Operation::CredentialList, None).unwrap();
    let listed = serde_json::to_string(&listed).unwrap();
    assert!(listed.contains("other"));
    assert!(!listed.contains(&old.credential.id.to_string()));
    drop(comb);
    // A detached root needs an explicit selection before setup manages it again.
    provision::select_none(&f.root).unwrap();
    crate::setup::run_offline(&f.root, None, crate::setup::Options::default()).unwrap();
    assert!(!Comb::open(&f.root).unwrap().get(&slug).unwrap().unverified);
}
#[test]
fn full_root_purge_holds_stable_scope_and_never_deletes_recreated_root() {
    let f = Fixture::new();
    let plan = f.plan(true);
    plan.save(&f.receipt).unwrap();
    let lock = hive_setup_support::locks::path(&f.root, "comb-root", false).unwrap();
    assert!(apply(&f.receipt).is_err());
    drop(lock);
    apply(&f.receipt).unwrap();
    assert!(!f.root.exists());
    let recreated = Comb::init(&f.root).unwrap();
    let id = recreated.identity();
    drop(recreated);
    assert!(apply(&f.receipt).is_err());
    assert_eq!(Comb::open(&f.root).unwrap().identity(), id);
}
#[test]
fn changed_bridge_after_revocation_is_retained_even_with_same_inode() {
    let f = Fixture::new();
    let mut plan = f.plan(false);
    plan.details.bridge.as_mut().unwrap().revoked = true;
    plan.save(&f.receipt).unwrap();
    let inode = fs::metadata(&f.bridge).unwrap().ino();
    fs::write(&f.bridge, b"new credential contents").unwrap();
    assert_eq!(fs::metadata(&f.bridge).unwrap().ino(), inode);
    let error = apply(&f.receipt).err().unwrap();
    assert!(
        !error.file_committed,
        "an old revoked receipt is not a new mutation"
    );
    assert!(f.bridge.exists());
}

#[test]
fn m5c_successful_revocation_then_receipt_failure_keeps_the_current_change() {
    let f = Fixture::new();
    f.plan(false).save(&f.receipt).unwrap();
    let registry = f.root.join(".comb/credentials.json");
    let before = fs::read(&registry).unwrap();
    let receipt = f.receipt.clone();
    let error = crate::disk::publication_hooks::with(
        move |path, step| {
            if path.file_name() == Some(std::ffi::OsStr::new("credentials.json"))
                && step == atomic_fs::Step::AfterPublish
            {
                fs::remove_file(&receipt)?;
                fs::create_dir(&receipt)?;
            }
            Ok(())
        },
        || apply(&f.receipt).err().unwrap(),
    );
    assert!(error.file_committed);
    assert_ne!(fs::read(&registry).unwrap(), before);
    assert!(f.receipt.is_dir() && f.bridge.is_file());
    assert!(
        !error.message.contains("Revoke the exact credential"),
        "failure must be after successful revoke"
    );
}

#[test]
fn m5c_successful_activation_recovery_survives_a_later_bridge_refusal() {
    let f = Fixture::new();
    let mut plan = f.plan(false);
    plan.details.bridge.as_mut().unwrap().revoked = true;
    plan.save(&f.receipt).unwrap();
    fs::write(&f.bridge, b"new credential contents").unwrap();
    let settings = fs::read(f.root.join(".comb/serve.json")).unwrap();
    let journal = f.root.join(".comb/model-transition.json");
    fs::write(&journal, settings).unwrap();
    let error = apply(&f.receipt).err().unwrap();
    assert!(error.file_committed);
    assert!(!journal.exists());
    assert_eq!(fs::read(&f.bridge).unwrap(), b"new credential contents");
}

#[test]
fn m5c_detach_failure_after_unlink_is_committed_but_before_unlink_is_not() {
    for fail_before in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let admin = temp.path().join(".comb");
        fs::create_dir(&admin).unwrap();
        let path = admin.join("serve.json");
        if fail_before {
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, b"fixture wiring").unwrap();
        }
        let error = publication_scope(|changed| {
            detach_with(temp.path(), changed, |_| {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            })
        })
        .unwrap_err();
        assert_eq!(error.file_committed, !fail_before);
        assert_eq!(path.exists(), fail_before);
    }
}
#[test]
fn retry_refuses_new_root_wiring() {
    let f = Fixture::new();
    let mut plan = f.plan(false);
    plan.details.phase = Phase::Detached;
    plan.save(&f.receipt).unwrap();
    // A newly configured root is sufficient to refuse cleanup.
    assert!(apply(&f.receipt).is_err());
}
#[test]
fn absent_root_is_never_adopted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("new");
    let plan = prepare(
        &root,
        &Args {
            detach_comb: true,
            purge_comb_root: Some(root.clone()),
            ..Args::default()
        },
    )
    .unwrap();
    let receipt = temp.path().join("purge.json");
    plan.save(&receipt).unwrap();
    drop(Comb::init(&root).unwrap());
    assert!(apply(&receipt).is_err());
    assert!(root.join(".comb/identity.json").exists());
}

#[test]
fn sticky_temporary_parent_allows_init_and_contention_keeps_busy_code() {
    let root = std::env::temp_dir().join(format!(
        "comb-maintenance-{}",
        crate::values::CombId::generate()
    ));
    let comb = Comb::init(&root).unwrap();
    assert_eq!(
        Comb::open(&root).err().unwrap().code,
        crate::ErrorCode::CombBusy
    );
    drop(comb);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_fresh_setup_refuses_before_bridge_revocation() {
    let f = Fixture::new();
    f.plan(false).save(&f.receipt).unwrap();
    fs::write(f.root.join(".comb-setup.json"), b"pending").unwrap();
    assert!(
        apply(&f.receipt)
            .err()
            .unwrap()
            .message
            .contains("Finish fresh-root setup")
    );
    assert!(f.bridge.exists());
    assert!(f.root.join(".comb/serve.json").exists());
}

#[test]
fn reserved_receipt_does_not_revoke_bridge_or_remove_files() {
    for name in [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"] {
        let f = Fixture::new();
        let plan = f.plan(false);
        let reserved = f.temp.path().join(name);
        files::write(&reserved, &files::encode(&plan).unwrap(), false).unwrap();
        let before = fs::read(&f.bridge).unwrap();
        let connection = Connection::read(&f.bridge).unwrap();
        assert!(apply(&reserved).is_err());
        assert_eq!(fs::read(&f.bridge).unwrap(), before);
        service::auth::inspect_setup_bridge(&f.root, &connection, false).unwrap();
        assert!(f.root.join(".comb/serve.json").exists());
    }
}
