//! Command runner regressions.
use super::*;

#[test]
fn bounded_runner_handles_flood_hang_and_descendants_without_hanging() {
    let run = |body: &str, timeout| {
        runner::capture(
            std::process::Command::spawn,
            "/bin/sh",
            &["-c".into(), body.into()],
            timeout,
        )
    };
    // Success verifies capture/descendant cleanup, not sub-100ms OS scheduling.
    let success = Duration::from_secs(1);
    let timeout = Duration::from_millis(100);
    assert_eq!(run("printf ready", success).unwrap().stdout, "ready");
    let start = Instant::now();
    assert!(run("sleep 30", timeout).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    let start = Instant::now();
    assert_eq!(run("sleep 30 & exit 0", success).unwrap().code, 0);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(run("while :; do printf abcdefghijklmnop; done", timeout).is_err());
}

#[test]
fn attached_runner_bounds_time_cancellation_and_owns_descendants() {
    use std::process::{Command, Stdio};
    let run = |script: &str, timeout, cancelled: bool| {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        runner::attached_command(&mut command, None, timeout, &|| cancelled)
    };
    assert_eq!(
        run("exit 2", Duration::from_secs(1), false).unwrap().code,
        2
    );
    assert!(run("sleep 30", Duration::from_millis(100), false).is_err());
    assert_eq!(
        run("sleep 30", Duration::from_secs(1), true)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    assert_eq!(
        run("sleep 30 & exit 0", Duration::from_secs(1), false)
            .unwrap()
            .code,
        0
    );
    assert!(
        run(
            "while :; do printf abcdefghijklmnop; done",
            Duration::from_secs(1),
            false
        )
        .is_err()
    );
}

#[test]
fn cancellation_before_spawn_has_no_child_side_effect_and_stopped_children_are_reaped() {
    use std::process::{Command, Stdio};
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("must-not-exist");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "touch \"$1\"", "fixture"])
        .arg(&marker)
        .stdin(Stdio::null());
    let error =
        runner::attached_command(&mut command, None, Duration::from_secs(1), &|| true).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(!marker.exists());
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "kill -STOP $$; exit 99"])
        .stdin(Stdio::null());
    let start = Instant::now();
    let error = runner::attached_command(&mut command, None, Duration::from_secs(30), &|| false)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn stopped_setup_can_reap_its_separately_grouped_child_before_returning() {
    use std::process::{Command, Stdio};
    let directory = tempfile::tempdir().unwrap();
    let script = r"
import os,signal,subprocess,sys,time
from pathlib import Path
root=Path(sys.argv[1])
child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)'],start_new_session=True)
def cancelled(signum,frame):
    child.terminate()
    child.wait(timeout=3)
    (root/'reaped').write_text(str(child.pid))
    sys.exit(0)
signal.signal(signal.SIGTERM,cancelled)
(root/'child').write_text(str(child.pid))
os.kill(os.getpid(),signal.SIGSTOP)
while True: time.sleep(1)
";
    let mut command = Command::new("python3");
    command
        .args(["-c", script])
        .arg(directory.path())
        .stdin(Stdio::null());
    let error = runner::attached_command(&mut command, None, Duration::from_secs(10), &|| false)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    let reaped = fs::read_to_string(directory.path().join("reaped")).unwrap();
    assert_eq!(
        reaped,
        fs::read_to_string(directory.path().join("child")).unwrap()
    );
    let pid: libc::pid_t = reaped.parse().unwrap();
    // SAFETY: signal 0 only observes existence; it never mutates a possibly reused PID.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[test]
fn failed_pipe_initialization_cannot_leave_blocking_streams_in_cleanup() {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    let mut command = Command::new("/bin/sleep");
    command.arg("60").stdin(Stdio::null());
    // SAFETY: before exec, only initialized stack signal structures and
    // async-signal-safe libc calls are used. Ignored TERM survives exec.
    unsafe {
        command.pre_exec(|| {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_IGN;
            if libc::sigemptyset(&raw mut action.sa_mask) != 0
                || libc::sigaction(libc::SIGTERM, &raw const action, std::ptr::null_mut()) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let started = Instant::now();
    let error = runner::with_nonblocking_failure(|| {
        runner::attached_command(&mut command, None, Duration::from_secs(30), &|| false)
    })
    .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EIO));
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn progress_is_visible_before_exit_and_owns_no_global_download_deadline() {
    use std::{
        path::PathBuf,
        process::{Child, Command, Stdio},
    };
    thread_local! { static PROGRESS_FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) }; }
    fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
        let path = PROGRESS_FILE.with(|p| p.borrow().clone().unwrap());
        cmd.stderr(Stdio::from(fs::File::create(path)?));
        cmd.spawn()
    }
    let temp = tempfile::tempdir().unwrap();
    let progress = temp.path().join("progress");
    let ack = temp.path().join("ack");
    PROGRESS_FILE.with(|p| *p.borrow_mut() = Some(progress.clone()));
    let start = Instant::now();
    let seen = Cell::new(false);
    let output = runner::progress(spawn, "/bin/sh", &["-c".into(), "echo downloading >&2; sleep 0.1; while [ ! -f \"$1\" ]; do sleep 0.01; done; echo '{}'".into(), "fixture".into(), ack.to_string_lossy().into_owned()], &|| {
        if fs::read_to_string(&progress).is_ok_and(|s| s.contains("downloading")) {
            seen.set(true);
            fs::write(&ack, "seen before exit").unwrap();
        }
        start.elapsed() > Duration::from_secs(3)
    }).unwrap();
    assert!(seen.get());
    assert!(start.elapsed() >= Duration::from_millis(100));
    assert_eq!(output.stdout.trim(), "{}");
    assert_eq!(output.code, 0);
    assert!(output.stderr.is_empty());
    let cancel = runner::progress(
        Command::spawn,
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        &|| true,
    )
    .unwrap_err();
    assert_eq!(cancel.kind(), std::io::ErrorKind::Interrupted);
}
