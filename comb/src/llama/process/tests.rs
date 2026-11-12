use super::*;
use std::{io::Write as _, net::TcpListener};

fn shared(state: State) -> Shared {
    Shared {
        state: Mutex::new(state),
        tail: Mutex::new(VecDeque::new()),
        drained: AtomicBool::new(true),
        stopping: AtomicBool::new(false),
    }
}

#[test]
fn uncertain_cleanup_remains_an_error_without_retrying_signals() {
    let failure = Error::llama_server_stop_uncertain();
    let shared = shared(State::Lost(failure.clone()));
    for _ in 0..2 {
        assert_eq!(retire(&shared).unwrap_err().message, failure.message);
        assert_eq!(exit_status(&shared).unwrap_err().message, failure.message);
    }
    assert!(matches!(*lock(&shared.state), State::Lost(_)));
}

#[test]
fn lost_observation_fences_the_child_instead_of_signalling_it_again() {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    child.wait().unwrap();
    // Simulate a child reaped by another native owner. Its numeric PID must
    // never authorize a later signal, even if the OS has already reused it.
    let shared = shared(State::Live(child));
    assert!(exit_status(&shared).is_err());
    assert!(matches!(*lock(&shared.state), State::Lost(_)));
    assert!(retire(&shared).is_err());
}

fn health_reply(body: &[u8], declared: usize) -> Result<Health> {
    let listener = TcpListener::bind((HOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let bytes = body.to_vec();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        stream.write_all(&bytes).unwrap();
    });
    let result = health(&health_client().unwrap(), port);
    server.join().unwrap();
    result
}

#[test]
fn readiness_requires_a_bounded_complete_json_object_with_top_level_status() {
    let ready = br#"{ "status": "ok", "extra": true }"#;
    assert!(matches!(
        health_reply(ready, ready.len()).unwrap(),
        Health::Ready
    ));
    for body in [
        br#"{"nested":{"status":"ok"}}"#.as_slice(),
        br#"{"status":"ok"} invalid"#,
    ] {
        assert!(health_reply(body, body.len()).is_err());
    }
    let oversized = vec![b' '; health::HEALTH_BYTES + 1];
    assert!(health_reply(&oversized, oversized.len()).is_err());
    let truncated = health_reply(br#"{"status":"ok"}"#, 100).err().unwrap();
    assert!(std::error::Error::source(&truncated).is_some());
}
