use super::*;
use crate::binary::BinaryIdentity;

#[test]
fn failed_or_cancelled_canary_revokes_a_device_renumbered_prior_proof() {
    for (cancelled, untrusted_parent) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let (root, binary, old) = fixture(untrusted_parent);
        let identity = BinaryIdentity::read(&binary).unwrap();
        let mut value = serde_json::to_value(&old).unwrap();
        value["binary"]["device"] =
            serde_json::json!(value["binary"]["device"].as_u64().unwrap() ^ 1);
        let old: proof::Proof = serde_json::from_value(value).unwrap();
        proof::write(&proof::path(root.path()), Some(old.clone())).unwrap();
        assert_eq!(
            proof::load(root.path(), &binary)
                .unwrap()
                .selection(Some(&old.model), &old.effort)
                .mode()
                .is_some(),
            !untrusted_parent
        );
        let state = State::new(root.path().into()).unwrap();
        let config = claude_config();
        let binding = state
            .register("claude-main", config.profile("claude-main").unwrap())
            .unwrap();
        let owner = super::super::super::maintenance_lock(root.path()).unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let cwd = root.path().canonicalize().unwrap();
        std::fs::write(cwd.join("fixed.txt"), "fixed harmless content").unwrap();
        let launcher = crate::launch::test_cache().unwrap();
        let mut rpc = mock_rpc(&launcher, &session, &cwd, cancelled);
        let cancellation = Cancellation::default();
        let (finished, done) = std::sync::mpsc::channel::<()>();
        let observer = cancelled.then(|| cancel_after_submission(&cancellation, &cwd, done));
        let canary = Canary {
            launcher: &launcher,
            state: &state,
            binding: &binding,
            binary: &binary,
            identity: &identity,
            digest: &old.digest,
            model: "sonnet",
            effort: "low",
            cwd: &cwd,
            owner: &owner,
            cancellation: &cancellation,
            old: Some(&old),
        };
        let result = canary.exercise(&mut rpc, session, "2.1.test".into());
        drop(finished);
        if let Some(observer) = observer {
            assert!(observer.join().unwrap());
        }
        let shutdown = rpc.close();
        assert!(
            shutdown.confirmed,
            "cancelled={cancelled} result={:?} cleanup={}",
            result.as_ref().err(),
            shutdown.method.as_str()
        );
        if cancelled {
            assert!(matches!(result, Err(Error::Cancelled { .. })));
        } else {
            assert!(matches!(result, Err(Error::UnsupportedOperation { .. })));
        }
        assert!(proof::read(&proof::path(root.path())).unwrap().is_none());
        assert!(
            proof::load(root.path(), &binary)
                .unwrap()
                .selection(Some(&old.model), &old.effort)
                .mode()
                .is_none()
        );
    }
}

fn mock_rpc(launcher: &LauncherCache, session: &str, cwd: &Path, cancelled: bool) -> Rpc {
    let config = Arc::new(SessionConfig {
        steering: Some(SteeringPermit::Canary),
        ..SessionConfig::for_test(session, cwd.into())
    });
    let mut command = std::process::Command::new(
        crate::process::executable(std::ffi::OsStr::new("python3")).unwrap(),
    );
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_canary.py"))
        .arg(if cancelled {
            "cancel-after-submit"
        } else {
            "missing"
        });
    Rpc::start_authorized(launcher, command, Backend::Claude(config), vec![], |_| {
        Ok(())
    })
    .unwrap()
}

fn cancel_after_submission(
    cancellation: &Cancellation,
    cwd: &Path,
    done: std::sync::mpsc::Receiver<()>,
) -> std::thread::JoinHandle<bool> {
    let signal = cancellation.clone();
    let path = cwd.to_path_buf();
    std::thread::spawn(move || {
        while !path.join("inference-submitted").exists() {
            if done.recv_timeout(Duration::from_millis(5))
                != Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            {
                return false;
            }
        }
        signal.request();
        std::fs::write(path.join("release-inference"), "").unwrap();
        true
    })
}

fn fixture(untrusted_parent: bool) -> (tempfile::TempDir, std::path::PathBuf, proof::Proof) {
    let (root, mut binary, mut old) = proof::tests::fixture();
    if untrusted_parent {
        use std::os::unix::fs::PermissionsExt;
        let parent = root.path().join("shared-bin");
        state::directory(&parent).unwrap();
        let moved = parent.join("claude");
        std::fs::rename(&binary, &moved).unwrap();
        binary = moved;
        old.binary = BinaryIdentity::read(&binary).unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o777)).unwrap();
    }
    (root, binary, old)
}
