//! Cleanup arguments retain their exact flat wire fields and validation order.
use super::{INPUT_STRING_BYTES, Input, Op, Result};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PurgeArgs {
    #[serde(default)]
    pub target: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub recursive: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub dry_run: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub keep_native_history: Input<bool>,
}
/// Validated cleanup policy shared by CLI paging and native-history planning.
/// Internal only: the request schemas retain their existing flat boolean fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CleanupOptions {
    pub dry_run: bool,
    pub keep_native_history: bool,
}

pub(crate) struct PurgeOptions {
    pub recursive: bool,
    pub dry_run: bool,
    pub keep_native_history: bool,
}
impl PurgeArgs {
    pub(crate) fn options(&self) -> Result<PurgeOptions> {
        Ok(PurgeOptions {
            recursive: self
                .recursive
                .optional(Op::Purge, "/recursive", "a boolean")?
                .copied()
                .unwrap_or(false),
            dry_run: self
                .dry_run
                .optional(Op::Purge, "/dry_run", "a boolean")?
                .copied()
                .unwrap_or(false),
            keep_native_history: self
                .keep_native_history
                .optional(Op::Purge, "/keep_native_history", "a boolean")?
                .copied()
                .unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GcCursor {
    pub ended_at: u64,
    pub id: String,
}
impl<'de> Deserialize<'de> for GcCursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            ended_at: u64,
            id: String,
        }
        let Fields { ended_at, id } = crate::decode::object(deserializer)?;
        Ok(Self { ended_at, id })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GcArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub before: Input<u64>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub after: Input<GcCursor>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub dry_run: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub keep_native_history: Input<bool>,
}
pub(crate) struct GcOptions<'a> {
    pub before: u64,
    pub after: Option<&'a GcCursor>,
    pub dry_run: bool,
    pub keep_native_history: bool,
}
impl GcArgs {
    pub(crate) fn options(&self) -> Result<GcOptions<'_>> {
        use hive_input::Code;
        let before = self
            .before
            .optional(
                Op::Gc,
                "/before",
                "an integer from 0 to 9223372036854775807",
            )?
            .copied()
            .ok_or_else(|| {
                Op::Gc.error(
                    Code::MissingArgument,
                    "/before",
                    "an integer from 0 to 9223372036854775807",
                )
            })?;
        if before > i64::MAX.unsigned_abs() {
            return Err(Op::Gc.error(
                Code::OutOfRange,
                "/before",
                "an integer from 0 to 9223372036854775807",
            ));
        }
        let after = self.after.optional(
            Op::Gc,
            "/after",
            "an object with ended_at (integer) and id (string)",
        )?;
        if let Some(cursor) = after {
            if cursor.ended_at >= before {
                return Err(Op::Gc.error(
                    Code::OutOfRange,
                    "/after/ended_at",
                    "an integer from 0 to before - 1",
                ));
            }
            hive_input::validate::text_bytes(&cursor.id, INPUT_STRING_BYTES)
                .map_err(|rule| Op::Gc.error(rule.code(), "/after/id", rule.expected()))?;
        }
        let dry_run = self
            .dry_run
            .optional(Op::Gc, "/dry_run", "a boolean")?
            .copied()
            .unwrap_or(false);
        Ok(GcOptions {
            before,
            after,
            dry_run,
            keep_native_history: self
                .keep_native_history
                .optional(Op::Gc, "/keep_native_history", "a boolean")?
                .copied()
                .unwrap_or(false),
        })
    }
}
