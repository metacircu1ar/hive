use super::*;

#[derive(Default)]
struct Recorder {
    calls: Vec<Vec<String>>,
    fail_at: Option<usize>,
}
impl Transport for Recorder {
    fn request(
        &mut self,
        texts: &[&str],
        deadline: Instant,
    ) -> std::result::Result<Vec<Vec<f32>>, Classified> {
        assert!(deadline > Instant::now());
        self.calls
            .push(texts.iter().map(|text| (*text).to_owned()).collect());
        if self.fail_at == Some(self.calls.len()) {
            return Err(Error::embedding_request_failed().into());
        }
        Ok(texts
            .iter()
            .map(|text| {
                vec![crate::convert::f64_f32(crate::convert::usize_f64(
                    text.len(),
                ))]
            })
            .collect())
    }
}

#[test]
fn bounds_total_utf8_bytes_and_preserves_rows_around_rejected_inputs() -> Result<()> {
    let mut recorder = Recorder::default();
    let wide = "é".repeat(32);
    let too_large = "x".repeat(65);
    let rows = budgeted(
        &mut recorder,
        &[&wide, "a", &too_large, "bc", "", "d"],
        "",
        Duration::from_secs(2),
        64,
    )?;
    assert_eq!(
        recorder.calls,
        [
            vec![wide],
            vec!["a".into()],
            vec!["bc".into(), String::new(), "d".into()]
        ]
    );
    assert_eq!(rows.len(), 6);
    assert!(rows[2].as_ref().is_err_and(Error::is_input_rejection));
    for (i, length) in [(0, 64.0), (1, 1.0), (3, 2.0), (4, 0.0), (5, 1.0)] {
        assert_eq!(rows[i].as_ref().unwrap(), &[length]);
    }
    assert!(
        recorder
            .calls
            .iter()
            .all(|call| call.iter().map(String::len).sum::<usize>() <= 64)
    );
    Ok(())
}

#[test]
fn a_later_request_failure_discards_partial_results_and_stops() {
    let mut recorder = Recorder {
        fail_at: Some(2),
        ..Recorder::default()
    };
    let input = "x".repeat(64);
    let error = budgeted(
        &mut recorder,
        &[&input, &input, &input],
        "",
        Duration::from_secs(2),
        64,
    )
    .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::EmbeddingTransport);
    assert_eq!(recorder.calls.len(), 2);
}

#[test]
fn cpu_piece_requests_are_bounded_in_aggregate() -> Result<()> {
    let mut recorder = Recorder::default();
    let pieces = ["x".repeat(300), "y".repeat(300), "z".repeat(210)];
    let texts = pieces.iter().map(String::as_str).collect::<Vec<_>>();
    let rows = budgeted(
        &mut recorder,
        &texts,
        "",
        Duration::from_secs(2),
        crate::llama::CPU_REQUEST_BYTES,
    )?;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        recorder.calls.iter().map(Vec::len).collect::<Vec<_>>(),
        [1, 2]
    );
    Ok(())
}
