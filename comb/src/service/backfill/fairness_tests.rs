use super::*;
use crate::test_support::embedding::FakeEmbedder;
use crate::{
    Actor,
    embedding::{Embedder, Space},
    store::Write,
    values::RequestId,
};
use std::sync::atomic::AtomicBool;

struct FailingChunk {
    inner: FakeEmbedder,
    failing: Arc<AtomicBool>,
}
impl Embedder for FailingChunk {
    fn space(&self) -> &Space {
        self.inner.space()
    }
    fn embed(&mut self, inputs: &[String]) -> Result<Vec<Result<Vec<f32>>>> {
        if self.failing.load(Ordering::SeqCst) && inputs.iter().any(|s| s.contains("blocked")) {
            return Err(Error::embedding_request_failed());
        }
        self.inner.embed(inputs)
    }
}

fn fails_transport(backfill: &mut Backfill, state: &State) {
    let Err(error) = backfill.step(&mut ServiceHost { state }, 1) else {
        panic!("expected transport failure");
    };
    assert_eq!(error.code, ErrorCode::EmbeddingTransport);
}

#[test]
fn failed_chunk_does_not_starve_later_chunks_and_is_retried_next_pass() -> Result<()> {
    let failing = Arc::new(AtomicBool::new(true));
    let space = serde_json::from_value(serde_json::json!({
        "backend":"fixture","model":"deterministic","revision":"fixed","dimensions":2,
        "document_prefix":"d:","query_prefix":"q:","preprocessing":"test","normalization":"l2-f32"
    }))?;
    let engine = FailingChunk {
        inner: FakeEmbedder::new(space, |_| Ok(vec![1.0, 0.0])),
        failing: failing.clone(),
    };
    let (_dir, server, _) = crate::service::tests::setup(Some(Box::new(engine)));
    let state = &server.state;
    for (slug, text) in [
        ("concepts/first", "blocked"),
        ("concepts/second", "healthy"),
    ] {
        state.writer.lock()?.put_markdown(
            &slug.parse()?,
            &format!("---\ntitle: Evidence\n---\n{text}"),
            None,
            &Write::new(&Actor::Human, &RequestId::generate(), None)?,
        )?;
    }
    let mut backfill = Backfill::default();
    fails_transport(&mut backfill, state);
    assert!(backfill.step(&mut ServiceHost { state }, 1)?.is_some());
    let count = state
        .snapshot()?
        .index
        .embedding_coverage(crate::search::Filter::default())?;
    assert_eq!((count.embedded, count.pending, count.rejected), (1, 1, 0));
    assert!(backfill.step(&mut ServiceHost { state }, 1)?.is_none());
    fails_transport(&mut backfill, state);
    failing.store(false, Ordering::SeqCst);
    assert!(backfill.step(&mut ServiceHost { state }, 1)?.is_none());
    assert!(backfill.step(&mut ServiceHost { state }, 1)?.is_some());
    let count = state
        .snapshot()?
        .index
        .embedding_coverage(crate::search::Filter::default())?;
    assert_eq!((count.embedded, count.pending, count.rejected), (2, 0, 0));
    Ok(())
}
