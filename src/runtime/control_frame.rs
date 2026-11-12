//! Admit bounded control bytes before UTF-8 or JSON can accept a truncated prefix.
use crate::error::{Error, Result};
use std::io::{BufRead, Read};

const REQUEST_BYTES: u64 = 1024 * 1024;
const INITIAL_CAPACITY: usize = 8192;

#[derive(Clone, Copy)]
pub(super) enum Frame {
    Request,
    Response,
}
impl Frame {
    fn limit(self) -> u64 {
        match self {
            Self::Request => REQUEST_BYTES,
            Self::Response => crate::limits::JOIN_TRANSPORT_BYTES as u64,
        }
    }
    fn text(self, bytes: Vec<u8>) -> Result<String> {
        if bytes.len() as u64 > self.limit() {
            return Err(match self {
                Self::Request => Error::invalid_request("Control request exceeds 1 MiB."),
                Self::Response => Error::daemon_protocol_error("Daemon response exceeds 64 MiB."),
            });
        }
        String::from_utf8(bytes).map_err(|cause| {
            Error::state_io_from(std::io::Error::new(std::io::ErrorKind::InvalidData, cause))
        })
    }
    pub(super) fn read(self, reader: impl Read) -> Result<String> {
        let mut bytes = Vec::with_capacity(INITIAL_CAPACITY);
        std::io::BufReader::new(reader.take(self.limit() + 1))
            .read_until(b'\n', &mut bytes)
            .map_err(Error::state_io_from)?;
        self.text(bytes)
    }
    pub(super) async fn read_async(
        self,
        reader: impl tokio::io::AsyncRead + Unpin,
    ) -> Result<String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};
        let mut bytes = Vec::with_capacity(INITIAL_CAPACITY);
        tokio::io::BufReader::new(reader.take(self.limit() + 1))
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(Error::state_io_from)?;
        self.text(bytes)
    }
}

#[cfg(test)]
mod tests;
