use super::*;
use crate::runtime::{
    api::Request,
    client::{Client, socket_path},
};
use std::{
    io::{BufRead, Write},
    os::unix::net::UnixListener,
    time::Duration,
};

fn payload(limit: usize, case: usize) -> Vec<u8> {
    let mut bytes = b"{}".to_vec();
    match case {
        0 => {
            bytes.resize(limit - 1, b' ');
            bytes.push(b'\n');
        }
        1 => {
            bytes.resize(limit, b' ');
            bytes.extend_from_slice(b"hidden\n");
        }
        2 => {
            bytes[0] = 0xff;
            bytes.resize(limit + 1, b' ');
        }
        3 => bytes = b"!\n".to_vec(),
        _ => bytes = vec![0xff, b'\n'],
    }
    bytes
}

#[test]
fn m5b_frame_admission_checks_bytes_before_utf8_in_both_adapters() {
    let runtime = crate::calls::executor().unwrap();
    for frame in [Frame::Request, Frame::Response] {
        for case in [0, 1, 2, 4] {
            let bytes = payload(usize::try_from(frame.limit()).unwrap(), case);
            for result in [
                frame.read(bytes.as_slice()),
                runtime.block_on(frame.read_async(bytes.as_slice())),
            ] {
                match case {
                    0 => assert_eq!(
                        result.unwrap().len(),
                        usize::try_from(frame.limit()).unwrap()
                    ),
                    4 => assert_eq!(result.unwrap_err().code(), "state_io"),
                    _ => assert!(
                        serde_json::to_string(&result.unwrap_err())
                            .unwrap()
                            .contains("exceeds")
                    ),
                }
            }
        }
    }
}

#[test]
fn m5b_all_three_client_paths_refuse_oversized_prefixes_and_malformed_frames() {
    let runtime = crate::calls::executor().unwrap();
    for mode in 0..3 {
        for case in 0..5 {
            let tmp = tempfile::tempdir_in("/tmp").unwrap();
            std::fs::create_dir(tmp.path().join("runtime")).unwrap();
            let listener = UnixListener::bind(socket_path(tmp.path())).unwrap();
            let bytes = payload(usize::try_from(Frame::Response.limit()).unwrap(), case);
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(stream);
                reader.read_until(b'\n', &mut Vec::new()).unwrap();
                if let Err(error) = reader.get_mut().write_all(&bytes) {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ));
                }
            });
            let client = Client::new(tmp.path());
            let result = match mode {
                0 => client.request("test", Request::Ping),
                1 => runtime
                    .block_on(client.observe::<serde_json::Value>(
                        "test",
                        &Request::Ping,
                        tokio::time::Instant::now() + Duration::from_secs(10),
                    ))
                    .map_err(crate::error::ObservationError::into_error),
                _ => runtime
                    .block_on(client.startup(tokio::time::Instant::now() + Duration::from_secs(10)))
                    .map(|value| value.unwrap()),
            };
            server.join().unwrap();
            if case == 0 {
                assert_eq!(result.unwrap(), serde_json::json!({}));
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.code(),
                    if case == 4 {
                        "state_io"
                    } else {
                        "daemon_protocol_error"
                    },
                    "mode={mode}, case={case}"
                );
                if case == 1 || case == 2 {
                    assert!(serde_json::to_string(&error).unwrap().contains("exceeds"));
                }
            }
        }
    }
}
