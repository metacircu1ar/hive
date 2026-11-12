//! Mutation recording helpers retain the exact golden traffic.
use super::*;

pub(super) fn record_capture_and_put(cli: &Cli, mutation: &[(&str, &str)]) -> TestResult {
    cli.record(
        "capture-blank-title",
        &["capture", "\nObservation", "--request-id", "blank"],
        None,
        mutation,
    )?;
    cli.record(
        "put",
        &["put", "concepts/new", "--request-id", "put"],
        Some("---\ntitle: New\n---\nContent"),
        mutation,
    )?;
    cli.record(
        "put-replay",
        &["put", "concepts/new", "--request-id", "put"],
        Some("---\ntitle: New\n---\nContent"),
        mutation,
    )?;
    cli.record(
        "put-conflict",
        &["put", "concepts/new", "--request-id", "conflict"],
        Some("---\ntitle: Changed\n---\nContent"),
        &[],
    )?;
    Ok(())
}

pub(super) fn record_append_and_capture(cli: &Cli, mutation: &[(&str, &str)]) -> TestResult {
    cli.record(
        "append",
        &[
            "append",
            "concepts/new",
            "Evidence",
            "--source",
            "test",
            "--request-id",
            "append",
        ],
        None,
        mutation,
    )?;
    cli.record(
        "capture",
        &["capture", "Captured note", "--request-id", "capture"],
        None,
        mutation,
    )?;
    Ok(())
}

pub(super) fn record_reviews(cli: &Cli, mutation: &[(&str, &str)]) -> TestResult {
    cli.record(
        "promote",
        &[
            "promote",
            "concepts/page",
            "--expected-hash",
            &cli.hash("concepts/page")?,
            "--request-id",
            "promote",
        ],
        None,
        mutation,
    )?;
    cli.record(
        "decide",
        &[
            "decide",
            "review/proposal",
            "reject",
            "--reason",
            "Needs evidence",
            "--expected-hash",
            &cli.hash("review/proposal")?,
            "--request-id",
            "decide",
        ],
        None,
        mutation,
    )?;
    Ok(())
}

pub(super) fn record_consolidation(cli: &Cli) -> TestResult {
    let summary = cli.root.path().join("summary.txt");
    std::fs::write(&summary, "New summary. [evidence:new]")?;
    cli.record(
        "consolidate",
        &[
            "--run",
            "curate",
            "consolidate",
            "inbox/note",
            "concepts/target",
            "--note-hash",
            &cli.hash("inbox/note")?,
            "--target-hash",
            &cli.hash("concepts/target")?,
            "--summary-file",
            summary.to_str().ok_or("summary path is not UTF-8")?,
            "--recopy",
            "--request-id",
            "consolidate",
        ],
        None,
        &[("/target/hash", "hash"), ("/note/hash", "hash")],
    )?;
    Ok(())
}

pub(super) fn record_removal_and_import(cli: &Cli) -> TestResult {
    cli.record(
        "rm",
        &[
            "rm",
            "concepts/new",
            "--expected-hash",
            &cli.hash("concepts/new")?,
            "--request-id",
            "remove",
        ],
        None,
        &[],
    )?;
    let imported = cli.root.path().join("import");
    std::fs::create_dir(&imported)?;
    std::fs::write(imported.join("note.md"), "Imported text.")?;
    cli.record(
        "import",
        &[
            "import",
            imported.to_str().ok_or("import path is not UTF-8")?,
            "--prefix",
            "concepts",
        ],
        None,
        &[("/batch_id", "uuid")],
    )?;
    Ok(())
}
