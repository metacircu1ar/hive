use super::*;
use serde::Serialize;

#[derive(Clone, Serialize)]
struct Report {
    z: f32,
    warnings: Vec<&'static str>,
}

#[test]
fn optional_availability_keeps_the_existing_canonical_report_bytes() {
    for availability in [None, Some(Vec::new())] {
        let report = Report {
            z: 0.1,
            warnings: vec!["future"],
        };
        let mut old = api::value(report.clone());
        if let Some(rows) = &availability {
            old.as_object_mut()
                .unwrap()
                .insert("availability".into(), api::value(rows));
        }
        let new = api::value(AvailabilityReport {
            report,
            availability,
        });
        assert_eq!(new.to_string(), old.to_string());
    }
}
