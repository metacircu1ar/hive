//! Shared bounds for accepted control-plane input.
// Idempotency keys are bounded before durable request lookup or insertion.
pub(crate) const REQUEST_ID_BYTES: usize = hive_input::validate::REQUEST_ID_BYTES;
// Bound each user-supplied control text field and each retained-output page to 64 KiB.
pub(crate) const CONTROL_INPUT_BYTES: usize = 64 * 1024;
// A maximum join is ~4 MiB of text; nested JSON escaping and headers need bounded headroom.
pub(crate) const JOIN_TRANSPORT_BYTES: usize = 64 * 1024 * 1024;
// Offline retention parameters must fit SQLite's signed integer domain exactly.
pub(crate) const COMPACT_BEFORE: std::ops::RangeInclusive<u64> = 0..=i64::MAX.unsigned_abs();
pub(crate) const COMPACT_KEEP: std::ops::RangeInclusive<u64> = 1..=i64::MAX.unsigned_abs();

// Bound one atomic batch before request replay storage or dispatch.
pub(crate) const BATCH_OPERATION_LIMIT: usize = 64;
// Bound serialized batch input and its durable request fingerprint to 512 KiB.
pub(crate) const BATCH_INPUT_BYTES: usize = 512 * 1024;
// Cursor identities have the same bounded width across approval and stall reads.
pub(crate) const CURSOR_ID_BYTES: usize = 128;
// SQL-backed public pages accept at least one row and at most 200 rows.
pub(crate) const MIN_PAGE_ROWS: u64 = 1;
pub(crate) const MAX_PAGE_ROWS: u64 = 200;
// Four bytes always fit one UTF-8 scalar; byte pages share the control-text cap.
pub(crate) const MIN_OUTPUT_BYTES: u64 = 4;
pub(crate) const MAX_OUTPUT_BYTES: u64 = crate::limits::CONTROL_INPUT_BYTES as u64;
// History and approval reads default to half a maximum page.
pub(crate) const DEFAULT_HISTORY_PAGE_ROWS: u64 = MAX_PAGE_ROWS / 2;
// GC considers at most this many agent candidates per acknowledged page.
pub(crate) const GC_CANDIDATES: usize = 128;
const _: () = assert!(MIN_PAGE_ROWS <= DEFAULT_HISTORY_PAGE_ROWS);
const _: () = assert!(DEFAULT_HISTORY_PAGE_ROWS <= MAX_PAGE_ROWS);
const _: () = assert!(BATCH_OPERATION_LIMIT < BATCH_INPUT_BYTES);

// Maximum accepted delayed-message interval: seven days in seconds.
pub(crate) const MAX_DELAY: u64 = 604_800;

pub(crate) const MAX_FILES: usize = 10;
