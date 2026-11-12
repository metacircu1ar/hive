//! Standalone daemon process entry point.
fn main() -> std::process::ExitCode {
    hive::run(true)
}
