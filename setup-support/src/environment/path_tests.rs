use super::*;

#[test]
fn implicit_path_skips_unusable_entries_while_explicit_and_io_failures_stay_errors() {
    for errno in [libc::EACCES, libc::ENOTDIR, libc::EIO, libc::EMFILE] {
        for explicit in [false, true] {
            let result = path_entries(std::ffi::OsStr::new("/denied:/bin"), explicit, |path| {
                if path == Path::new("/denied") {
                    Err(std::io::Error::from_raw_os_error(errno))
                } else {
                    std::fs::metadata(path)
                }
            });
            if !explicit && matches!(errno, libc::EACCES | libc::ENOTDIR) {
                assert_eq!(result.unwrap(), "/bin");
            } else {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(errno));
            }
        }
    }
}
