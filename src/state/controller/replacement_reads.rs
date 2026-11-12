//! Replacement index discovery tolerates publication; locked readers refuse it.
use super::*;
use crate::{
    config::{Method, Profile},
    state::sync_tests::with_checkpoint,
};
use hive_setup_support::checkpoints::Checkpoint;
use std::cell::Cell;

fn replacing_index<T>(state: &State, run: impl FnOnce() -> T) -> T {
    let target = state.root.join("index.json");
    let first = Cell::new(true);
    with_checkpoint(
        move |path, at| {
            if path == target && at == Checkpoint::RecordOpened && first.replace(false) {
                let index: crate::state::profiles::Index =
                    serde_json::from_slice(&fs::read(path)?)?;
                write_json(path, &index).unwrap();
            }
            Ok(())
        },
        run,
    )
}

#[test]
fn replacement_metadata_reopens_index_while_locked_readers_refuse_it() {
    let temporary = tempfile::tempdir().unwrap();
    let state = State::new(temporary.path().join("state")).unwrap();
    state
        .register(
            "main",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    for provider in [Provider::Codex, Provider::Claude] {
        let metadata = replacing_index(&state, || state.replacement_metadata(provider)).unwrap();
        assert!(metadata.metadata_unavailable.is_none());
    }
    let _lock = crate::state::Lock::index(&state.root.join("index.lock")).unwrap();
    assert_eq!(
        replacing_index(&state, || state.read_index())
            .err()
            .unwrap()
            .code(),
        "unsafe_state"
    );
    assert_eq!(
        replacing_index(&state, || state.migration_index())
            .err()
            .unwrap()
            .code(),
        "unsafe_state"
    );
}
