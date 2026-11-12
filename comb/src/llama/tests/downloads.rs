use super::*;

fn make_private(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// One fake origin: serves `body`, honoring Range unless told otherwise.
struct Origin {
    url: String,
    ranges: Arc<Mutex<Vec<Option<String>>>>,
}
fn origin(body: Vec<u8>, honor_range: bool, connections: usize) -> Origin {
    let listener = TcpListener::bind((HOST, 0)).unwrap();
    let url = format!("http://{}/model.gguf", listener.local_addr().unwrap());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let seen = ranges.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(connections) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut range = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: ") {
                    range = Some(value.to_owned());
                }
            }
            seen.lock().unwrap().push(range.clone());
            let start = range
                .as_deref()
                .filter(|_| honor_range)
                .and_then(|r| r.strip_prefix("bytes="))
                .and_then(|r| r.strip_suffix('-'))
                .and_then(|r| r.parse::<usize>().ok());
            let head = match start {
                Some(start) => format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{}\r\n\
                        Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - 1,
                    body.len(),
                    body.len() - start
                ),
                None => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
            };
            // A cancelled client closes early; keep serving the next request.
            if let Err(error) = stream
                .write_all(head.as_bytes())
                .and_then(|()| stream.write_all(&body[start.unwrap_or(0)..]))
            {
                assert!(
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ),
                    "unexpected fixture write failure: {error}"
                );
            }
        }
    });
    Origin { url, ranges }
}

fn fetch(cache: &Path, pin: &Pin, url: &str) -> crate::Result<PathBuf> {
    download::ensure(cache, &Source::pinned(pin, url), &|| false, &mut |_| ())
}

#[test]
fn corrupt_cache_cleanup_failure_preserves_the_original_asset_error() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let pin = pin_for(b"model");
    let target = cache.join(&pin.model.file);
    std::fs::create_dir_all(&target).unwrap();
    let error = fetch(&cache, &pin, "http://127.0.0.1:1/not-contacted").unwrap_err();
    assert_eq!(error.message, crate::Error::model_size_mismatch().message);
    assert!(crate::service::auth::has_cleanup_failure(&error));
    assert!(target.is_dir());
}

#[test]
fn fresh_download_is_verified_and_published_without_a_partial_file() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let body = b"pinned model bytes".to_vec();
    let pin = pin_for(&body);
    let origin = origin(body.clone(), true, 1);
    let path = fetch(&cache, &pin, &origin.url).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), body);
    assert!(!cache.join("model.gguf.part").exists());
    assert_eq!(*origin.ranges.lock().unwrap(), [None]);
    // A cached file is rehashed and reused without any request.
    assert_eq!(
        fetch(&cache, &pin, "http://127.0.0.1:9/unused").unwrap(),
        path
    );
}

#[test]
fn partial_download_resumes_with_a_range_request() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    let body = b"0123456789abcdefghij".to_vec();
    std::fs::write(cache.join("model.gguf.part"), &body[..8]).unwrap();
    make_private(&cache.join("model.gguf.part"));
    let pin = pin_for(&body);
    let origin = origin(body.clone(), true, 1);
    let path = fetch(&cache, &pin, &origin.url).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), body);
    assert_eq!(
        *origin.ranges.lock().unwrap(),
        [Some("bytes=8-".to_owned())]
    );
}

#[test]
fn an_ignored_range_restarts_the_partial_file_from_zero() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
    let body = b"0123456789abcdefghij".to_vec();
    std::fs::write(cache.join("model.gguf.part"), b"stale!!!").unwrap();
    make_private(&cache.join("model.gguf.part"));
    let pin = pin_for(&body);
    let origin = origin(body.clone(), false, 1);
    assert_eq!(
        std::fs::read(fetch(&cache, &pin, &origin.url).unwrap()).unwrap(),
        body
    );
}

#[test]
fn corrupt_or_oversized_downloads_are_removed_and_never_published() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let body = b"expected model".to_vec();
    let pin = pin_for(&body);
    let corrupt = origin(b"corrupt model!".to_vec(), true, 1);
    let error = fetch(&cache, &pin, &corrupt.url).unwrap_err();
    assert_eq!(
        error.message,
        crate::Error::model_checksum_mismatch().message
    );
    assert!(!cache.join("model.gguf").exists());
    assert!(!cache.join("model.gguf.part").exists());
    let oversized = origin(b"expected model plus more".to_vec(), true, 1);
    let error = fetch(&cache, &pin, &oversized.url).unwrap_err();
    assert_eq!(error.message, crate::Error::model_size_mismatch().message);
    assert!(!cache.join("model.gguf.part").exists());
    // A corrupted cached file is discarded and downloaded again.
    std::fs::write(cache.join("model.gguf"), b"tampered model").unwrap();
    let good = origin(body.clone(), true, 1);
    assert_eq!(
        std::fs::read(fetch(&cache, &pin, &good.url).unwrap()).unwrap(),
        body
    );
}

#[test]
fn cancellation_keeps_the_partial_file_for_a_later_resume() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("cache");
    let body = vec![7_u8; 3 * 1024 * 1024];
    let pin = pin_for(&body);
    let origin = origin(body.clone(), true, 2);
    let received = std::cell::Cell::new(false);
    let error = download::ensure(
        &cache,
        &Source::pinned(&pin, &origin.url),
        &|| received.get(),
        &mut |event| {
            if matches!(event, Progress::Received { .. }) {
                received.set(true);
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IoError);
    let partial = std::fs::metadata(cache.join("model.gguf.part"))
        .unwrap()
        .len();
    assert!(partial > 0 && partial < 3 * 1024 * 1024);
    assert_eq!(
        std::fs::read(fetch(&cache, &pin, &origin.url).unwrap()).unwrap(),
        body
    );
    assert_eq!(
        origin.ranges.lock().unwrap()[1],
        Some(format!("bytes={partial}-"))
    );
}

#[test]
fn a_selected_model_must_still_have_its_pinned_size() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("model.gguf");
    assert_eq!(
        download::present(&path, 5).unwrap_err().code,
        ErrorCode::InvalidEmbeddingAsset
    );
    std::fs::write(&path, b"12345").unwrap();
    make_private(&path);
    assert!(download::present(&path, 5).is_ok());
    assert!(download::present(&path, 6).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660)).unwrap();
    assert!(download::present(&path, 5).is_err());
}
