use super::*;
use crate::config::{Method, Profile};
use std::{fs, os::unix::fs::MetadataExt};

#[test]
fn duplicate_index_keys_including_decoded_uuid_collisions_refuse_without_rewriting() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let binding = state
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
    let index = serde_json::to_value(state.read_index().unwrap()).unwrap();
    let id = binding.id.to_string();
    let profile = &index["profiles"]["main"];
    let generation = &index["generations"][&id];
    let duplicate_profile = format!(
        r#"{{"profiles":{{"main":{profile},"main":{profile}}},"generations":{}}}"#,
        index["generations"]
    );
    let duplicate_generation = format!(
        r#"{{"profiles":{},"generations":{{"{id}":{generation},"{}":{generation}}}}}"#,
        index["profiles"],
        id.to_uppercase()
    );
    let path = state.root.join("index.json");
    for bytes in [duplicate_profile, duplicate_generation] {
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&path, bytes.as_bytes())
            .unwrap();
        assert_eq!(
            state.check_format().unwrap_err().code(),
            "unsupported_schema"
        );
        assert_eq!(fs::read(&path).unwrap(), bytes.as_bytes());
    }
}

#[test]
fn index_updates_preserve_explicit_durability_on_error() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let binding = state
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
    let path = state.root.join("index.json");
    let snapshot = || (fs::read(&path).unwrap(), fs::metadata(&path).unwrap().ino());
    let before = snapshot();
    let error = state
        .update_index(|index| {
            index.generation_mut(binding.id)?.possibly_authenticated = true;
            Err::<(), _>(corrupt())
        })
        .unwrap_err();
    assert_eq!(error.code(), "state_inconsistent");
    assert_eq!(snapshot(), before);
    state.update_index(|_| Ok(())).unwrap();
    assert_eq!(snapshot(), before);

    let error = state
        .update_index(|index| {
            index.generation_mut(binding.id)?.possibly_authenticated = true;
            index.save()?;
            Err::<(), _>(corrupt())
        })
        .unwrap_err();
    assert_eq!(error.code(), "state_inconsistent");
    assert!(
        state
            .read_index()
            .unwrap()
            .generation(binding.id)
            .unwrap()
            .possibly_authenticated
    );
    assert_ne!(snapshot(), before);
}
