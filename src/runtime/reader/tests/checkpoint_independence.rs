//! Checkpoint independence regressions.
use super::*;

#[tokio::test]
async fn committed_reply_and_publication_do_not_wait_for_idle_checkpoint() {
    reply_and_publication_precede_checkpoint(false).await;
}

#[tokio::test]
async fn committed_reply_and_publication_do_not_wait_for_deferred_checkpoint() {
    reply_and_publication_precede_checkpoint(true).await;
}
