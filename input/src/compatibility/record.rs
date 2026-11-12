//! Classify explicit shared-file versions before decoding the current exact shape.
use super::Entry;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{error::Error as StdError, fmt, io};

#[derive(Debug)]
pub enum Error {
    Malformed(serde_json::Error),
    Version { found: u64, current: u32 },
}
impl Error {
    pub fn newer(&self) -> bool {
        matches!(self, Self::Version {found, current} if *found > u64::from(*current))
    }
}
impl fmt::Display for Error {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(_) => output.write_str("Unsupported or malformed shared record."),
            Self::Version { .. } if self.newer() => output.write_str(concat!(
                "This shared record uses a newer format. Keep it unchanged and use ",
                "its producing build or a newer supported build."
            )),
            Self::Version { .. } => {
                output.write_str("Unsupported shared-file format. Keep the record unchanged.")
            }
        }
    }
}
impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Malformed(cause) => Some(cause),
            Self::Version { .. } => None,
        }
    }
}
/// `io::Error` can hide its custom payload behind `get_ref`, so inspect both forms.
pub fn is_newer(error: &(dyn StdError + 'static)) -> bool {
    std::iter::successors(Some(error), |&cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(|cause| cause.get_ref())
            .map(|cause| cause as &(dyn StdError + 'static))
            .or_else(|| cause.source())
    })
    .take(32)
    .any(|cause| cause.downcast_ref::<Error>().is_some_and(Error::newer))
}
pub fn classify(value: &Value, entry: &Entry) -> Result<(), Error> {
    use serde::de::Error as _;
    let found = value
        .as_object()
        .and_then(|object| object.get("format"))
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            Error::Malformed(serde_json::Error::custom(
                "expected an unsigned record format",
            ))
        })?;
    require(found, entry)
}
pub fn require(found: u64, entry: &Entry) -> Result<(), Error> {
    if found != u64::from(entry.current) {
        return Err(Error::Version {
            found,
            current: entry.current,
        });
    }
    Ok(())
}
pub fn decode<T: DeserializeOwned>(bytes: &[u8], entry: &Entry) -> Result<T, Error> {
    decode_with(bytes, |value| classify(value, entry))
}
/// The caller names each nested versioned boundary; unversioned data is not guessed.
pub fn decode_with<T: DeserializeOwned>(
    bytes: &[u8],
    inspect: impl FnOnce(&Value) -> Result<(), Error>,
) -> Result<T, Error> {
    let value = crate::decode::json_value(bytes).map_err(Error::Malformed)?;
    inspect(&value)?;
    crate::decode::from_slice(bytes).map_err(Error::Malformed)
}

#[cfg(test)]
mod tests;
