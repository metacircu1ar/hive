use super::*;
use hive_setup_support::child;
use rustix::process::{Pid, Signal};
use std::{io, os::unix::process::CommandExt, process::ExitStatus};

pub(super) struct Native {
    directory: Option<tempfile::TempDir>,
    child: Option<Child>,
    pid: Pid,
    lost: bool,
    clean: bool,
    pub connection: Connection,
}

impl Native {
    pub fn new() -> Self {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let root = directory.path().canonicalize().unwrap();
        drop(Comb::init(&root).unwrap());
        let connection = Connection::human(&root, root.join(".comb/service.sock")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
        command
            .args(["--root", root.to_str().unwrap(), "serve"])
            .env_remove("COMB_LOG")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only the async-signal-safe native limit syscall runs before exec.
        unsafe {
            command.pre_exec(|| {
                rustix::process::setrlimit(
                    rustix::process::Resource::Nofile,
                    rustix::process::Rlimit {
                        current: Some(64),
                        maximum: Some(64),
                    },
                )
                .map_err(io::Error::from)
            });
        }
        eprintln!("native FD fixture evidence: {}", directory.path().display());
        let child = command.spawn().unwrap();
        let pid = Pid::from_child(&child);
        Self {
            directory: Some(directory),
            child: Some(child),
            pid,
            lost: false,
            clean: false,
            connection,
        }
    }

    pub fn root(&self) -> &std::path::Path {
        self.directory.as_ref().unwrap().path()
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn live(&mut self) {
        let observed = child::has_exited(self.child.as_ref().unwrap());
        if observed.is_err() {
            self.lost = true;
        }
        assert!(
            !observed.unwrap(),
            "limited native server exited unexpectedly"
        );
    }

    pub fn stop(&mut self) -> Instant {
        self.live();
        rustix::process::kill_process(self.pid, Signal::TERM).unwrap();
        let sent = Instant::now();
        let status = self.reap(Instant::now() + Duration::from_secs(10)).unwrap();
        assert!(status.unwrap().success(), "native shutdown failed");
        assert!(self.group_absent(), "native group remains after reap");
        if self.connection.socket.exists() {
            self.lost = true;
            panic!("native socket ownership was not released");
        }
        let reopened = Comb::open(self.root());
        if reopened.is_err() {
            self.lost = true;
        }
        drop(reopened.unwrap());
        self.clean = true;
        sent
    }

    fn reap(&mut self, deadline: Instant) -> io::Result<Option<ExitStatus>> {
        let result = child::reap_until(self.child.as_mut().unwrap(), deadline);
        match &result {
            Ok(Some(_)) => drop(self.child.take()),
            Err(_) => self.lost = true,
            Ok(None) => {}
        }
        result
    }

    fn group_absent(&self) -> bool {
        match rustix::process::test_kill_process_group(self.pid) {
            Err(rustix::io::Errno::SRCH) => true,
            Ok(()) => false,
            Err(error) => {
                tracing::warn!(name: "test.native_group_probe_failed",
                    pid = self.pid.as_raw_pid(),
                    error = &error as &(dyn std::error::Error + 'static),
                    decision = "retain_evidence");
                false
            }
        }
    }

    fn cleanup(&mut self) -> bool {
        if self.lost {
            return false;
        }
        if let Some(owned) = self.child.as_mut() {
            let attempt =
                child::kill_owned(owned, |owned| {
                    match rustix::process::kill_process_group(Pid::from_child(owned), Signal::KILL)
                    {
                        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                        Err(error) => Err(error.into()),
                    }
                });
            for (phase, error) in attempt.errors() {
                tracing::warn!(name: "test.native_cleanup_failed", phase = phase,
                    error = error as &(dyn std::error::Error + 'static),
                    decision = "preserve_primary_failure");
            }
            if attempt.ownership_error.is_some() {
                self.lost = true;
                return false;
            }
            match self.reap(Instant::now() + child::FORCED_REAP_GRACE) {
                Ok(Some(_)) => {}
                Ok(None) => return false,
                Err(error) => {
                    tracing::warn!(name: "test.native_reap_failed",
                        error = &error as &(dyn std::error::Error + 'static),
                        decision = "retain_evidence");
                    return false;
                }
            }
        }
        self.group_absent()
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        let confirmed = self.clean || self.cleanup();
        if !confirmed || std::thread::panicking() {
            if let Some(directory) = self.directory.take() {
                drop(directory.keep());
            }
            tracing::warn!(name: "test.native_evidence_retained", pid = self.pid.as_raw_pid(),
                confirmed = confirmed,
                decision = "retain_evidence");
        }
    }
}
