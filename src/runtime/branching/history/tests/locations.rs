//! Candidate-only lookup preserves native path ownership and ambiguity checks.
use super::*;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, fs::symlink},
};

fn write(path: &Path, values: &[Value]) {
    state::directory(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn home(root: &Path) -> PathBuf {
    let home = root.join("provider");
    state::directory(&home).unwrap();
    state::directory(&home.join("projects")).unwrap();
    home
}

#[test]
fn known_candidates_do_not_consult_unrelated_projects_and_refuse_ambiguity() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path());
    let session = uuid::Uuid::new_v4().to_string();
    let input = uuid::Uuid::new_v4().to_string();
    let relative = PathBuf::from(format!("projects/known/{session}.jsonl"));
    write(&home.join(&relative), &[row(&input, None, "source")]);
    let unrelated = home.join("projects/unrelated");
    state::directory(&unrelated).unwrap();
    write(
        &unrelated.join(format!("{session}.jsonl")),
        &[row(&input, None, "unrelated")],
    );
    fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(
        temp.path().join("missing"),
        home.join("projects/unrelated-link"),
    )
    .unwrap();
    assert!(
        read_optional(&home, &session, std::slice::from_ref(&relative))
            .unwrap()
            .is_some()
    );
    let duplicate = PathBuf::from(format!("projects/other/{session}.jsonl"));
    write(&home.join(&duplicate), &[row(&input, None, "source")]);
    assert!(read_optional(&home, &session, &[relative, duplicate]).is_err());
    for relative in [
        PathBuf::from("../outside"),
        PathBuf::from("projects/known/wrong.jsonl"),
        PathBuf::from(format!("projects/known/../{session}.jsonl")),
    ] {
        assert!(read_optional(&home, &session, &[relative]).is_err());
    }
}

#[test]
fn both_projects_and_the_known_project_directory_must_not_be_symlinks() {
    for link_projects in [true, false] {
        let temp = crate::runtime::test_directory();
        let home = temp.path().join("provider");
        state::directory(&home).unwrap();
        let target = temp.path().join("outside");
        state::directory(&target).unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let input = uuid::Uuid::new_v4().to_string();
        if link_projects {
            write(
                &target.join(format!("known/{session}.jsonl")),
                &[row(&input, None, "source")],
            );
            symlink(&target, home.join("projects")).unwrap();
        } else {
            write(
                &target.join(format!("{session}.jsonl")),
                &[row(&input, None, "source")],
            );
            state::directory(&home.join("projects")).unwrap();
            symlink(&target, home.join("projects/known")).unwrap();
        }
        assert!(
            read_optional(
                &home,
                &session,
                &[PathBuf::from(format!("projects/known/{session}.jsonl"))]
            )
            .is_err()
        );
    }
}

#[test]
fn a_fork_can_be_verified_in_its_different_known_working_directory() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path());
    let source = uuid::Uuid::new_v4().to_string();
    let child = uuid::Uuid::new_v4().to_string();
    let cutoff = uuid::Uuid::new_v4().to_string();
    let tail = uuid::Uuid::new_v4().to_string();
    let input = uuid::Uuid::new_v4().to_string();
    let relative = PathBuf::from(format!("projects/source/{source}.jsonl"));
    let before = vec![
        row(&cutoff, None, "source"),
        json!({"type":"assistant","uuid":tail,
        "parentUuid":cutoff,"message":{"role":"assistant","content":"done"}}),
    ];
    write(&home.join(&relative), &before);
    let cwd = temp.path().join("different-workspace");
    state::directory(&cwd).unwrap();
    let locations = Locations::new(&[relative], &cwd, &child).unwrap();
    let snapshot = Snapshot::capture(&home, &source, &child, &cutoff, locations).unwrap();
    assert!(snapshot.verify(&input).unwrap().is_none());
    let child_path =
        home.join(crate::runtime::store::Store::claude_transcript(&cwd, &child).unwrap());
    let mut copied = before;
    copied.push(row(&input, Some(&tail), "child"));
    write(&child_path, &copied);
    assert_eq!(snapshot.verify(&input).unwrap(), Some(child_path));
}

#[test]
fn cutoff_and_first_child_input_use_exact_uuid_spelling() {
    let temp = crate::runtime::test_directory();
    let home = home(temp.path());
    let source = uuid::Uuid::new_v4().to_string();
    let child = uuid::Uuid::new_v4().to_string();
    let cutoff = "12345678-abcd-4234-8234-123456789abc";
    let input = "abcdef01-1234-4234-8234-123456789abc";
    let relative = PathBuf::from(format!("projects/source/{source}.jsonl"));
    write(&home.join(&relative), &[row(cutoff, None, "source")]);
    let locations = Locations::new(std::slice::from_ref(&relative), temp.path(), &child).unwrap();
    assert!(Snapshot::capture(&home, &source, &child, &cutoff.to_uppercase(), locations).is_err());
    let locations = Locations::new(std::slice::from_ref(&relative), temp.path(), &child).unwrap();
    let snapshot = Snapshot::capture(&home, &source, &child, cutoff, locations).unwrap();
    let child_path = home.join(relative.with_file_name(format!("{child}.jsonl")));
    write(
        &child_path,
        &[
            row(cutoff, None, "source"),
            row(input, Some(cutoff), "child"),
        ],
    );
    assert!(snapshot.verify(&input.to_uppercase()).is_err());
    assert_eq!(snapshot.verify(input).unwrap(), Some(child_path));
}
