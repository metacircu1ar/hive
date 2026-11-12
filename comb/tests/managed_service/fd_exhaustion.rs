use super::*;
use comb::service::{
    Operation,
    auth::{Connection, ConnectionExt},
    client,
};
use std::{
    fs,
    io::{ErrorKind, Read},
};
use tokio::net::UnixStream;

#[path = "fd_exhaustion/owner.rs"]
mod owner;
use owner::Native;

const PRESSURE: Duration = Duration::from_secs(3);
const CONNECT: Duration = Duration::from_millis(100);
// Pressure fixtures share the parent descriptor table, despite private children.
static PRESSURE_OWNER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn pressure_owner() -> std::sync::MutexGuard<'static, ()> {
    PRESSURE_OWNER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn parent_headroom() {
    let limit = rustix::process::getrlimit(rustix::process::Resource::Nofile);
    let used = fs::read_dir("/dev/fd")
        .unwrap()
        .try_fold(0_u64, |count, entry| entry.map(|_| count + 1))
        .unwrap();
    assert!(
        limit.current.unwrap_or(u64::MAX).saturating_sub(used) >= 128,
        "native FD fixture requires 128 spare parent descriptors"
    );
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn ready(native: &mut Native) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        native.live();
        assert!(tokio::time::Instant::now() < deadline);
        let reply = tokio::time::timeout_at(
            deadline,
            client::try_call(
                &native.connection,
                &Operation::Stats,
                None,
                Duration::from_secs(2),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        if reply.is_some() {
            return;
        }
        // None proves no submission: retry bind-to-listen without replaying.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn exhausted(native: &Native) -> bool {
    let path = native.root().join(".comb/logs/comb.jsonl");
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(
        bytes.len() <= 1024 * 1024,
        "native diagnostics exceeded fixture bound"
    );
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(&[][..], |at| &bytes[..=at]);
    complete
        .split(|byte| *byte == b'\n')
        .filter(|row| !row.is_empty())
        .any(|row| {
            let record: serde_json::Value = serde_json::from_slice(row).unwrap();
            record["event"] == "comb.accept_retry"
                && record["error"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|cause| cause["errno"] == libc::EMFILE)
        })
}

struct Pressure {
    clients: Vec<UnixStream>,
    deadline: tokio::time::Instant,
}
impl Pressure {
    async fn begin(native: &mut Native) -> Self {
        let deadline = tokio::time::Instant::now() + PRESSURE;
        let mut clients = Vec::new();
        while !exhausted(native) {
            native.live();
            assert!(tokio::time::Instant::now() < deadline);
            if clients.len() == 96 {
                tokio::time::sleep(Duration::from_millis(1)).await;
                continue;
            }
            clients.push(
                tokio::time::timeout_at(
                    deadline.min(tokio::time::Instant::now() + CONNECT),
                    UnixStream::connect(&native.connection.socket),
                )
                .await
                .unwrap()
                .unwrap(),
            );
        }
        Self { clients, deadline }
    }

    fn check(&self) {
        assert!(
            tokio::time::Instant::now() < self.deadline,
            "pressure proof cannot rely on five-second idle frame expiry"
        );
        for stream in &self.clients {
            let mut bytes = [0_u8; 4096];
            match stream.try_read(&mut bytes) {
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Ok(0) if cfg!(target_os = "macos") => {}
                result => panic!("unexpected reply before explicit pressure release: {result:?}"),
            }
        }
    }

    fn release(self) {
        self.check();
        let deadline = self.deadline;
        let count = self.clients.len();
        drop(self);
        assert!(tokio::time::Instant::now() < deadline);
        eprintln!("native EMFILE recovery: child_nofile=64 held_connections={count}");
    }
}

#[test]
fn native_emfile_accept_recovers_after_explicit_pressure_release() {
    let _pressure_owner = pressure_owner();
    parent_headroom();
    let mut native = Native::new();
    runtime().block_on(async {
        ready(&mut native).await;
        let pressure = Pressure::begin(&mut native).await;
        pressure.check();
        #[cfg(target_os = "linux")]
        same_queued_request(&native, pressure).await;
        #[cfg(not(target_os = "linux"))]
        pressure.release();
        client::call(
            &native.connection,
            &Operation::Stats,
            None,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    });
    native.stop();
}

#[cfg(target_os = "linux")]
async fn same_queued_request(native: &Native, pressure: Pressure) {
    let submitted = std::sync::atomic::AtomicBool::new(false);
    let request = client::call_observed(
        &native.connection,
        &Operation::Stats,
        None,
        Duration::from_secs(2),
        || {
            submitted.store(true, std::sync::atomic::Ordering::Release);
        },
    );
    tokio::pin!(request);
    let hold = async {
        while !submitted.load(std::sync::atomic::Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    tokio::select! {
        result = &mut request => panic!("queued request replied before release: {result:?}"),
        outcome = tokio::time::timeout_at(pressure.deadline, hold) => outcome.unwrap(),
    }
    pressure.release();
    request.await.unwrap();
}

#[test]
fn native_emfile_backoff_still_stops_and_releases_ownership() {
    let _pressure_owner = pressure_owner();
    parent_headroom();
    let mut native = Native::new();
    let rt = runtime();
    let pressure = rt.block_on(async {
        ready(&mut native).await;
        let pressure = Pressure::begin(&mut native).await;
        pressure.check();
        pressure
    });
    // Keep clients while TERM reaches the resource-exhausted server.
    let sent = native.stop();
    assert!(sent < pressure.deadline.into_std());
    eprintln!(
        "native EMFILE stop: child_nofile=64 held_connections={}",
        pressure.clients.len()
    );
    drop(pressure);
}

#[test]
fn native_pressure_assertion_keeps_primary_failure_and_reaps_before_root_removal() {
    let _pressure_owner = pressure_owner();
    parent_headroom();
    let mut native = Native::new();
    let rt = runtime();
    let pressure = rt.block_on(async {
        ready(&mut native).await;
        Pressure::begin(&mut native).await
    });
    let pid = native.pid();
    let root = native.root().to_owned();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owned = native;
        panic!("native-pressure-primary");
    }))
    .unwrap_err();
    assert_eq!(
        failure.downcast_ref::<&str>(),
        Some(&"native-pressure-primary")
    );
    assert_eq!(
        rustix::process::test_kill_process_group(pid),
        Err(rustix::io::Errno::SRCH)
    );
    assert!(root.join(".comb/logs/comb.jsonl").is_file());
    drop(Comb::open(&root).unwrap());
    // The injected failure retained its evidence; ownership is now proven gone.
    fs::remove_dir_all(&root).unwrap();
    assert!(!root.exists());
    drop(pressure);
}
