use super::*;
use crate::external_mcp::{Active, RequestId, ResultBody, Update};

fn active(
    server: &mut Server,
    id: &RequestId,
    generation: &str,
    task: tokio::task::JoinHandle<()>,
) {
    server.active.insert(
        id.key(),
        Active {
            id: id.clone(),
            generation: generation.into(),
            task: Some(task),
        },
    );
}
fn finished(id: &RequestId, generation: &str) -> Update {
    Update {
        key: id.key(),
        generation: generation.into(),
        value: Message::reply(id.clone(), ResultBody::empty()),
        finished: true,
    }
}

#[tokio::test]
async fn either_join_update_ordering_delivers_one_result_and_releases_capacity() {
    for join_first in [false, true] {
        let mut server = Server::default();
        let (output, mut received) = mpsc::channel(2);
        let id = RequestId::Number(17.into());
        active(&mut server, &id, "current", tokio::spawn(async {}));
        if join_first {
            let completed = std::future::poll_fn(|cx| server.poll_owner(cx)).await;
            server.owner_finished(&output, completed).await.unwrap();
            assert!(server.active[&id.key()].task.is_none());
            assert!(received.try_recv().is_err());
        }
        server
            .progress(&output, finished(&id, "current"))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(received.recv().await.unwrap()).unwrap()["id"],
            17
        );
        assert!(server.active.is_empty());
        server
            .progress(&output, finished(&id, "current"))
            .await
            .unwrap();
        assert!(received.try_recv().is_err());
    }
}

#[tokio::test]
async fn failed_observer_releases_capacity_and_old_updates_cannot_finish_a_reused_id() {
    let mut server = Server::default();
    let (output, mut received) = mpsc::channel(2);
    let id = RequestId::Text("original-id".into());
    active(
        &mut server,
        &id,
        "old",
        tokio::spawn(async {
            panic!("private cause");
        }),
    );
    let completed = std::future::poll_fn(|cx| server.poll_owner(cx)).await;
    server.owner_finished(&output, completed).await.unwrap();
    assert!(server.active.is_empty());
    let value = serde_json::to_value(received.recv().await.unwrap()).unwrap();
    assert_eq!(value["id"], "original-id");
    assert_eq!(value["error"]["code"], -32603);
    assert!(!value.to_string().contains("private cause"));
    active(
        &mut server,
        &id,
        "new",
        tokio::spawn(std::future::pending()),
    );
    server
        .progress(&output, finished(&id, "old"))
        .await
        .unwrap();
    assert_eq!(server.active[&id.key()].generation, "new");
    assert!(received.try_recv().is_err());
}
