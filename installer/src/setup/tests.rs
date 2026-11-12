use super::*;
use clap::Parser;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    process::Command,
};

const SCRIPT: &str = r"#!/usr/bin/env python3
import hashlib,json,os,sys
from pathlib import Path
args=sys.argv[1:]
def value(flag): return args[args.index(flag)+1]
if '--raw-block' in args:
    import signal,time,tty
    tty.setraw(0)
    signal.signal(signal.SIGTERM,signal.SIG_IGN)
    print('RAW_READY',flush=True)
    os.kill(os.getpid(),signal.SIGSTOP)
    while True: time.sleep(1)
if '--refuse' in args: sys.exit(2)
if '--prepare-output' in args:
    if '--fail-publication' in args:
        for path in [value('--prepare-output'),str(Path(value('--prepare-output')).parent/'.hive-tmp-retained')]:
            Path(path).write_bytes(b'uncertain publication evidence')
        sys.exit(1)
    resources=json.loads(Path(value('--prepare-resources')).read_bytes())
    assert not Path(resources['prefix']).exists(), 'activated before planning'
    if '--ask' in args:
        fd=os.open('/dev/tty',os.O_RDWR)
        try:
            os.write(fd,b'PREPARE_QUESTION: ')
            assert os.read(fd,1024).strip() == b'codex'
        finally: os.close(fd)
    record={'format':1,'product':'hive',
            'executable_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            'resources':resources,'scope_sha256':'fixture','answers':{'provider':'codex'}}
    descriptor=os.open(value('--prepare-output'),os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(descriptor,'w') as output:
        json.dump(record,output); output.flush(); os.fsync(output.fileno())
    print('PREPARE_BEFORE_ACTIVATION',flush=True)
else:
    data=Path(value('--apply-prepared')).read_bytes()
    assert hashlib.sha256(data).hexdigest() == value('--prepared-sha256')
    assert json.loads(data)['answers'] == {'provider':'codex'}
    if '--fail-apply' in args: sys.exit(1)
    print('APPLIED_WITHOUT_PROMPT',flush=True)
";

fn stage(root: &Path) -> Manifest {
    let mut manifest = crate::tests::fixture(root, "hive-core");
    let program = root.join("hive-core/bin/hivectl");
    fs::write(&program, SCRIPT).unwrap();
    let entry = manifest.files.get_mut("bin/hivectl").unwrap();
    entry.sha256 = crate::manifest::hash(&program).unwrap();
    entry.bytes = SCRIPT.len() as u64;
    manifest
}
fn args(root: &Path, prefix: &Path, flag: &str) -> Args {
    Args::try_parse_from([
        "hive-install".as_ref(),
        "--from".as_ref(),
        root.as_os_str(),
        "--prefix".as_ref(),
        prefix.as_os_str(),
        "--core".as_ref(),
        "--".as_ref(),
        flag.as_ref(),
    ])
    .unwrap()
}
fn directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn verified_llama_component_is_available_to_pre_activation_setup() {
    let root = directory();
    let staging = root.path().canonicalize().unwrap();
    let manifests = [
        crate::tests::fixture(&staging, "comb"),
        crate::tests::fixture(&staging, "comb-llama"),
    ];
    let resources = resources(&staging.join("prefix"), &staging, &manifests).unwrap();
    assert_eq!(resources.llama_source, Some(staging.join("comb-llama")));
    assert_eq!(resources.comb_source, Some(staging.join("comb")));
}

#[test]
fn setup_refusal_preserves_status_without_creating_prefix() {
    let root = directory();
    let prefix = root.path().canonicalize().unwrap().join("prefix");
    let manifest = stage(root.path());
    let result = Preparation::new(
        &prefix,
        root.path(),
        &[manifest],
        &args(root.path(), &prefix, "--refuse"),
    )
    .unwrap();
    assert!(matches!(result, Preparation::Exit(2)));
    assert!(!prefix.exists());
    assert!(!root.path().join("setup-answers.json").exists());
}

#[test]
fn setup_refusal_keeps_an_existing_active_pointer_and_payload_unchanged() {
    let root = directory();
    let prefix = root.path().canonicalize().unwrap().join("prefix");
    let manifest = stage(root.path());
    fs::create_dir_all(prefix.join("lib/hive/generations/old")).unwrap();
    let payload = prefix.join("lib/hive/generations/old/payload");
    fs::write(&payload, "Previous bytes").unwrap();
    let pointer = prefix.join("lib/hive/current");
    symlink("generations/old", &pointer).unwrap();
    let result = Preparation::new(
        &prefix,
        root.path(),
        &[manifest],
        &args(root.path(), &prefix, "--refuse"),
    )
    .unwrap();
    assert!(matches!(result, Preparation::Exit(2)));
    assert_eq!(
        fs::read_link(&pointer).unwrap(),
        Path::new("generations/old")
    );
    assert_eq!(fs::read_to_string(&payload).unwrap(), "Previous bytes");
}

#[test]
fn operational_child_failure_retains_evidence_through_the_outer_staging_owner() {
    let owned = crate::publication::Owned::new(directory()).unwrap();
    let root = owned.path().to_owned();
    let prefix = root.canonicalize().unwrap().join("prefix");
    let manifest = stage(&root);
    let result = Preparation::new(
        &prefix,
        &root,
        &[manifest],
        &args(&root, &prefix, "--fail-publication"),
    );
    assert!(owned.finish(result).is_err());
    assert!(!prefix.exists());
    assert_eq!(
        fs::read(root.join(".hive-tmp-retained")).unwrap(),
        b"uncertain publication evidence"
    );
    assert_eq!(
        fs::read(root.join("setup-answers.json")).unwrap(),
        b"uncertain publication evidence"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_setup_after_activation_reports_stage_and_retains_evidence() {
    let owned = crate::publication::Owned::new(directory()).unwrap();
    let root = owned.path().to_owned();
    let prefix = root.canonicalize().unwrap().join("prefix");
    let manifest = stage(&root);
    let prepared = Preparation::new(
        &prefix,
        &root,
        &[manifest],
        &args(&root, &prefix, "--fail-apply"),
    )
    .unwrap();
    fs::create_dir_all(prefix.join("bin")).unwrap();
    symlink(
        root.join("hive-core/bin/hivectl"),
        prefix.join("bin/hivectl"),
    )
    .unwrap();
    let error = owned.finish(prepared.apply(&prefix)).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("Binaries are installed, but setup failed (exit 1)."),
        "{message}"
    );
    assert!(
        !message.contains("Publication failed before its namespace change"),
        "{message}"
    );
    assert!(root.join("setup-answers.json").is_file());
    assert!(prefix.join("bin/hivectl").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "Subprocess fixture with a controlling pseudo-terminal"]
fn interactive_preparation_child() {
    let root = directory();
    let prefix = root.path().canonicalize().unwrap().join("prefix");
    let manifest = stage(root.path());
    let result = Preparation::new(
        &prefix,
        root.path(),
        &[manifest],
        &args(root.path(), &prefix, "--ask"),
    )
    .unwrap();
    assert!(matches!(result, Preparation::Ready(_)));
    assert!(!prefix.exists());
    fs::create_dir_all(prefix.join("bin")).unwrap();
    symlink(
        root.path().join("hive-core/bin/hivectl"),
        prefix.join("bin/hivectl"),
    )
    .unwrap();
    assert_eq!(result.apply(&prefix).unwrap(), 0);
}

#[test]
fn private_process_group_keeps_interactive_setup_in_foreground_and_restores_terminal() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pty_setup.py");
    let output = Command::new("python3")
        .arg(fixture)
        .arg(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "setup::tests::interactive_preparation_child",
            "--nocapture",
        ])
        .env("HIVE_SETUP_ANSWERS", r#"[["PREPARE_QUESTION", "codex"]]"#)
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{text} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(text.matches("PREPARE_QUESTION").count(), 1);
    assert!(text.contains("PREPARE_BEFORE_ACTIVATION"));
    assert!(text.contains("APPLIED_WITHOUT_PROMPT"));
    assert!(text.contains("SETUP_PIPE_PRESERVED"));
}

#[test]
#[ignore = "Subprocess fixture for terminal stop and confirmation cancellation"]
fn terminal_cancellation_child() {
    let (backend, diagnostics) = hive_log::start_stderr(hive_log::Config::default()).unwrap();
    backend.install_global().unwrap();
    let root = PathBuf::from(std::env::var_os("HIVE_INSTALL_TEST_ROOT").unwrap());
    let confirmation = std::env::var_os("HIVE_INSTALL_TEST_CONFIRM").is_some();
    let prefix = root.join("prefix");
    let manifest = stage(&root);
    let modes = terminal_modes();
    let raw = std::env::var_os("HIVE_INSTALL_TEST_RAW").is_some();
    let flag = if confirmation {
        "--plain"
    } else if raw {
        "--raw-block"
    } else {
        "--ask"
    };
    let result = Preparation::new(&prefix, &root, &[manifest], &args(&root, &prefix, flag));
    if confirmation {
        let prepared = result.unwrap();
        assert!(matches!(&prepared, Preparation::Ready(_)));
        crate::services::confirm(false, "CONFIRM_CANCEL").unwrap();
        drop(prepared);
        panic!("Ctrl-C at confirmation must preserve its original signal behavior");
    } else {
        assert!(result.is_err());
        assert!(!prefix.exists());
        diagnostics.finish(std::time::Duration::from_millis(250));
        let after = terminal_modes();
        // PENDIN is kernel-maintained when buffered input is reprocessed after
        // restoring canonical mode. All user-controlled modes must match.
        assert_eq!(
            (
                modes.c_iflag,
                modes.c_oflag,
                modes.c_cflag,
                modes.c_lflag & !libc::PENDIN,
                modes.c_cc
            ),
            (
                after.c_iflag,
                after.c_oflag,
                after.c_cflag,
                after.c_lflag & !libc::PENDIN,
                after.c_cc
            )
        );
        println!("STOP_CANCELLED");
    }
}
fn terminal_modes() -> libc::termios {
    // SAFETY: initialized C output storage for this fixture's live pseudo-terminal.
    let mut modes = unsafe { std::mem::zeroed() };
    // SAFETY: the isolated fixture owns stdin as its controlling pseudo-terminal.
    assert_eq!(unsafe { libc::tcgetattr(0, &raw mut modes) }, 0);
    modes
}

const TERMINAL_CANCEL: &str = r"
import errno,os,pty,select,signal,sys,time
pid,fd=pty.fork()
if pid==0: os.execv(sys.argv[1],sys.argv[1:])
data=bytearray(); sent=False; status=None; deadline=time.monotonic()+60
confirmation='HIVE_INSTALL_TEST_CONFIRM' in os.environ
raw='HIVE_INSTALL_TEST_RAW' in os.environ
try:
    while time.monotonic()<deadline:
        if select.select([fd],[],[],0.05)[0]:
            try: chunk=os.read(fd,65536)
            except OSError as error:
                if error.errno!=errno.EIO: raise
                chunk=b''
            data.extend(chunk)
        needle=b'RAW_READY' if raw else b'CONFIRM_CANCEL' if confirmation else b'PREPARE_QUESTION'
        if not sent and needle in data:
            if not raw: os.write(fd,b'\x03' if confirmation else b'\x1a')
            sent=True; deadline=time.monotonic()+20
        done,status=os.waitpid(pid,os.WNOHANG)
        if done:
            pid=None
            while select.select([fd],[],[],0)[0]:
                try: chunk=os.read(fd,65536)
                except OSError as error:
                    if error.errno!=errno.EIO: raise
                    break
                if not chunk: break
                data.extend(chunk)
            break
    else: raise TimeoutError('foreground terminal was not released')
    assert sent,data
    if confirmation: assert os.WIFSIGNALED(status) and os.WTERMSIG(status)==signal.SIGINT,status
    else: assert os.waitstatus_to_exitcode(status)==0 and b'STOP_CANCELLED' in data,data
finally:
    os.close(fd)
    if pid is not None:
        try: os.killpg(pid,signal.SIGKILL)
        except ProcessLookupError: pass
        os.waitpid(pid,0)
";

#[test]
fn terminal_stop_cancels_child_and_confirmation_ctrl_c_keeps_original_behavior() {
    for (confirmation, raw) in [(false, false), (true, false), (false, true)] {
        let root = directory();
        let mut command = Command::new("python3");
        command
            .args(["-c", TERMINAL_CANCEL])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "setup::tests::terminal_cancellation_child",
                "--nocapture",
            ])
            .env(
                "HIVE_INSTALL_TEST_ROOT",
                root.path().canonicalize().unwrap(),
            );
        if confirmation {
            command.env("HIVE_INSTALL_TEST_CONFIRM", "1");
        }
        if raw {
            command.env("HIVE_INSTALL_TEST_RAW", "1");
        }
        let output = command.output().unwrap();
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("setup.child_cancel_failed"),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
