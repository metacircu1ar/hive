use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Choices {
    provider: String,
}
fn private() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn saved(root: &std::path::Path) -> Saved<Choices> {
    Saved {
        format: hive_input::compatibility::Exact,
        product: Product::Hive,
        executable_sha256: "build".into(),
        resources: Resources {
            format: hive_input::compatibility::Exact,
            prefix: root.join("prefix"),
            hive_source: Some(root.join("hive")),
            comb_source: None,
            llama_source: None,
        },
        scope_sha256: "scope".into(),
        answers: Choices {
            provider: "codex".into(),
        },
    }
}
#[test]
fn saved_answers_bind_exact_private_bytes_and_build_scope() {
    let dir = private();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("answers");
    let checksum = write(&file, &saved(&root)).unwrap();
    let loaded: Saved<Choices> = read(&file, Some(&checksum)).unwrap();
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(loaded.validate(Product::Hive, "scope", "build").is_ok());
    assert!(loaded.validate(Product::Comb, "scope", "build").is_err());
    assert!(loaded.validate(Product::Hive, "other", "build").is_err());
    assert!(loaded.validate(Product::Hive, "scope", "other").is_err());
    let _: Saved<serde::de::IgnoredAny> = read(&file, Some(&checksum)).unwrap();
    assert!(write(&file, &saved(&root)).is_err());
    fs::write(&file, b"{}").unwrap();
    assert!(read::<Saved<Choices>>(&file, Some(&checksum)).is_err());
}
#[test]
fn preparation_rejects_nonobjects_missing_markers_duplicates_and_private_causes() {
    let dir = private();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("answers");
    let valid = serde_json::to_string(&saved(&root)).unwrap();
    let cases = [
        "[]".to_owned(),
        valid.replacen("\"format\":1,", "", 1),
        valid.replacen("\"format\":1", "\"format\":2", 1),
        valid.replacen("\"format\":1", "\"format\":1,\"format\":1", 1),
        valid.replace(",\"comb_source\":null", ""),
        valid.replace(",\"llama_source\":null", ""),
        valid.replace(
            "\"comb_source\":null",
            "\"comb_source\":null,\"extra\":true",
        ),
        valid.replace("{\"provider\":\"codex\"}", "[\"codex\"]"),
        valid.replace(
            "\"provider\":\"codex\"",
            "\"provider\":\"codex\",\"provider\":\"codex\"",
        ),
        valid.replace("\"product\":\"hive\"", "\"product\":\"PRIVATE_SENTINEL\""),
    ];
    for case in cases {
        fs::write(&file, case).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let error = read::<Saved<Choices>>(&file, None).err().unwrap();
        assert_eq!(error.to_string(), "Setup preparation record is malformed.");
        assert!(std::error::Error::source(error.get_ref().unwrap()).is_some());
    }
}
#[test]
fn preparation_rejects_aliases_unsafe_modes_and_oversize() {
    let dir = private();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("answers");
    write(&file, &saved(&root)).unwrap();
    let link = root.join("link");
    symlink(&file, &link).unwrap();
    assert!(read_private(&link).is_err());
    fs::remove_file(&link).unwrap();
    fs::hard_link(&file, &link).unwrap();
    assert!(read_private(&file).is_err());
    fs::remove_file(&link).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_private(&file).is_err());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&file, vec![b' '; 256 * 1024 + 1]).unwrap();
    assert!(read_private(&file).is_err());
    fs::write(&file, b"{}").unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(read_private(&file).is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
}
