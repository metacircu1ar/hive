//! Fake servers and downloads only: no model, network or real llama-server.
use super::*;
use crate::{ErrorCode, embedding::HttpConfig};
use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::TcpListener,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn pin_text(tree: &str, sha256: &str, bytes: usize) -> String {
    serde_json::json!({
        "llama_cpp": {
            "repository": "https://github.com/ggml-org/llama.cpp.git",
            "commit": COMMIT,
            "tree": tree,
        },
        "model": {
            "repository": "Example/model",
            "revision": "e".repeat(40),
            "file": "model.gguf",
            "sha256": sha256,
            "bytes": bytes,
            "license": "Apache-2.0",
        },
    })
    .to_string()
}

fn pin_for(body: &[u8]) -> Pin {
    use sha2::Digest as _;
    let sha256 = format!("{:x}", sha2::Sha256::digest(body));
    Pin::parse(&pin_text(&"d".repeat(40), &sha256, body.len())).unwrap()
}

fn free_port() -> u16 {
    TcpListener::bind((HOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

mod pins;

mod arguments;

mod memory_floor;

mod downloads;

/// A fake `llama-server`: `--version`, `/health` and normalized embeddings. The
/// trailing user arguments select a behavior; real servers never see them.
const FAKE_SERVER: &str = r#"#!/usr/bin/env python3
import http.server, json, math, signal, socketserver, sys
args = sys.argv[1:]
if "--version" in args:
    sys.stderr.write("version: 0.6.0-dev (build 1, commit REPORTED)\n")
    sys.exit(0)
def value(name, default=None):
    return args[args.index(name) + 1] if name in args else default
mode = value("--fake-mode", "ok")
record = value("--fake-record")
if record:
    with open(record, "w") as output:
        json.dump(args, output)
sys.stderr.write("PRIVATE loading tail\n")
sys.stderr.flush()
if mode == "exit":
    sys.exit(3)
dims = 8 if mode == "dims" else 4096
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *ignored):
        pass
    def reply(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        if self.path != "/health":
            self.reply(404, {})
        elif mode == "never_ready":
            self.reply(503, {"error": {"message": "Loading model"}})
        else:
            self.reply(200, {"status": "ok"})
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        vector = [1 / math.sqrt(dims)] * dims
        rows = [{"index": i, "embedding": vector} for i, _ in enumerate(request["input"])]
        self.reply(200, {"data": rows})
class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True
Server(("127.0.0.1", int(value("--port"))), Handler).serve_forever()
"#;

fn fake_server(directory: &Path, reported: &str) -> PathBuf {
    let path = directory.join("llama-server");
    std::fs::write(&path, FAKE_SERVER.replace("REPORTED", reported)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn launch(program: &Path, mode: &str, record: Option<&Path>) -> (Launch, HttpConfig) {
    let pin = Pin::parse(&pin_text(&"d".repeat(40), &"f".repeat(64), 5)).unwrap();
    let port = free_port();
    let mut extra = vec!["--fake-mode".to_owned(), mode.to_owned()];
    if let Some(record) = record {
        extra.extend(["--fake-record".to_owned(), record.display().to_string()]);
    }
    let launch = Launch {
        program: program.to_owned(),
        arguments: argv(Path::new("/private/model.gguf"), port, Device::Own, &extra),
        port,
        pin: pin.clone(),
    };
    (launch, config(&pin, Device::Own, port))
}

mod binaries;

mod lifecycle;
