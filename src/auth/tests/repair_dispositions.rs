//! Repair disposition changes only the existing report outcome and retirement.
use super::*;
use crate::auth::report::RepairDisposition;

#[test]
fn repair_dispositions_preserve_verification_and_native_causes() {
    for (disposition, expected) in [
        (
            RepairDisposition::Activated,
            AuthOutcome::GenerationReplaced,
        ),
        (RepairDisposition::Superseded, AuthOutcome::RepairSuperseded),
        (RepairDisposition::Retired, AuthOutcome::RepairRetired),
    ] {
        for pending in [false, true] {
            let mut value = report();
            if pending {
                value.verification = Some(Verification::Pending);
                value.diagnostic = Some(Error::state_io_from(std::io::Error::from_raw_os_error(
                    libc::ENOSPC,
                )));
            }
            value.repaired(disposition);
            assert_eq!(value.outcome, Some(expected));
            assert!(value.verification == pending.then_some(Verification::Pending));
            if pending && !matches!(disposition, RepairDisposition::Activated) {
                let diagnostic = value.diagnostic.as_ref().unwrap();
                assert_eq!(diagnostic.code(), "recovery_required");
                let original = std::error::Error::source(diagnostic).unwrap();
                assert_eq!(original.downcast_ref::<Error>().unwrap().code(), "state_io");
                assert_eq!(value.state, Status::RecoveryRequired);
            } else if pending {
                assert_eq!(value.diagnostic.as_ref().unwrap().code(), "state_io");
            } else {
                assert!(value.diagnostic.is_none());
                assert_eq!(value.state, Status::Missing);
            }
        }
    }
}
