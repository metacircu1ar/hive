//! Scheduler admission class bounds.
// Admission's coordinator and startup SQL yields exactly four classes, numbered 0..3.
pub(super) const READY_CLASS_COUNT: usize = 4;
pub(super) const MAX_READY_CLASS: usize = READY_CLASS_COUNT - 1;
