//! Native mock bridge fixture and deterministic result observations.
use super::*;
use ::comb::service::{auth::Grant, client};

pub(super) struct CombService {
    pub(super) directory: tempfile::TempDir,
    human: Connection,
    pub(super) credential: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
impl CombService {
    pub(super) fn new() -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        drop(::comb::Comb::init(directory.path()).unwrap());
        let root = directory.path().to_owned();
        let socket = root.join("comb.sock");
        let human = Connection::human(&root, socket.clone()).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (ready, prepared) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let server = Server::open(
                &root,
                Options {
                    socket: Some(socket),
                    ..Default::default()
                },
            )
            .unwrap();
            ready.send(()).unwrap();
            executor()
                .block_on(server.run(async {
                    let _ = stopped.await;
                }))
                .unwrap();
        });
        prepared.recv_timeout(Duration::from_secs(5)).unwrap();
        let issued = executor()
            .block_on(client::call(
                &human,
                &Operation::CredentialCreate {
                    grant: Grant::Bridge {
                        name: "mock-runtime".into(),
                        reviewer_roles: vec![
                            "implementor".into(),
                            "reviewer".into(),
                            "claude".into(),
                            "claude-reviewer".into(),
                        ],
                    },
                },
                None,
                Duration::from_secs(5),
            ))
            .unwrap();
        let credential = directory.path().join("bridge.json");
        Connection {
            format: hive_input::compatibility::Exact,
            socket: human.socket.clone(),
            credential: serde_json::from_value(issued).unwrap(),
        }
        .write(&credential)
        .unwrap();
        Self {
            directory,
            human,
            credential,
            stop: Some(stop),
            thread: Some(thread),
        }
    }
    pub(super) fn get(&self, slug: &str) -> Value {
        self.request(Operation::Get {
            slug: slug.parse().unwrap(),
        })
    }
    pub(super) fn request(&self, operation: Operation) -> Value {
        executor()
            .block_on(client::call(
                &self.human,
                &operation,
                None,
                Duration::from_secs(5),
            ))
            .unwrap()
    }
}

impl Drop for CombService {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}
pub(super) fn body(actions: Value) -> String {
    format!("comb:{actions}")
}
pub(super) fn spawn(f: &Harness, provider: Provider, reviewer: bool, actions: Value) -> String {
    let role = match (provider, reviewer) {
        (Provider::Codex, false) => "implementor",
        (Provider::Codex, true) => "reviewer",
        (Provider::Claude, false) => "claude",
        (Provider::Claude, true) => "claude-reviewer",
    };
    f.ok(
        json!({"op":"spawn","role":role,"lifetime":"independent","cwd":f.tmp.path(),
        "body":body(actions)}),
    )["agent_id"]
        .as_str()
        .unwrap()
        .into()
}
pub(super) fn results(f: &Harness, agent: &str, count: usize) -> Vec<Value> {
    support::wait::wait_until(
        Duration::from_secs(15),
        Duration::from_millis(25),
        "mock Comb output",
        || {
            let outputs: Vec<_> = f
                .events(agent)
                .events
                .iter()
                .filter_map(|e| {
                    let data = outputs::data(f, &e.data);
                    data["text"]
                        .as_str()
                        .and_then(|text| text.strip_prefix("comb-results:"))
                        .map(|text| serde_json::from_str::<Value>(text).unwrap())
                })
                .collect();
            (outputs.len() >= count).then(|| outputs.last().unwrap().as_array().unwrap().clone())
        },
    )
}
