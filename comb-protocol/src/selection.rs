//! Shared page-selection values and validation for search and graph requests.
use crate::{
    Error, Result,
    limits::{KIND_BYTES, LABEL_BYTES},
    values::{Timestamp, limits::SLUG_BYTES},
};
use serde::{Deserialize, Serialize};

/// Borrowed page filters shared by retrieval and embedding coverage.
#[derive(Clone, Copy, Debug, Default)]
pub struct Filter<'a> {
    pub kind: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub prefix: Option<&'a str>,
    pub after: Option<&'a Timestamp>,
    pub before: Option<&'a Timestamp>,
}

/// Owned page selection echoed by graph-assisted search, in addition to the
/// graph request's own filters. Both selections apply at every hop.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PageFilters {
    pub kind: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    pub after: Option<Timestamp>,
    pub before: Option<Timestamp>,
}
impl From<Filter<'_>> for PageFilters {
    fn from(filter: Filter<'_>) -> Self {
        Self {
            kind: filter.kind.map(str::to_owned),
            tag: filter.tag.map(str::to_owned),
            prefix: filter.prefix.map(str::to_owned),
            after: filter.after.cloned(),
            before: filter.before.cloned(),
        }
    }
}
impl Filter<'_> {
    pub fn validate(self) -> Result<()> {
        for (value, bound) in [
            (self.kind, KIND_BYTES),
            (self.tag, LABEL_BYTES),
            (self.prefix, SLUG_BYTES),
        ] {
            if let Some(value) = value {
                filter_text(value, bound)?;
            }
        }
        date_range(self.after, self.before)
    }
}

/// A literal metadata filter; unlike a slug, a prefix may end in a slash.
pub fn filter_text(value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(Error::invalid_search());
    }
    Ok(())
}

/// Compare instants, not the lexical spellings of timezone offsets.
pub fn date_range(after: Option<&Timestamp>, before: Option<&Timestamp>) -> Result<()> {
    if let (Some(after), Some(before)) = (after, before) {
        let parse = |value: &Timestamp| {
            chrono::DateTime::parse_from_rfc3339(value.as_str())
                .map_err(|cause| Error::invalid_search_dates().caused_by(cause))
        };
        if parse(after)? > parse(before)? {
            return Err(Error::reversed_search_dates());
        }
    }
    Ok(())
}
