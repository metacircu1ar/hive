use super::{
    api::{Operation, Reply, Request},
    auth::{Claim, Connection},
    framing, limits,
};
use crate::{Error, Result};
use serde_json::Value;
use std::time::Duration;
use tokio::{io::BufReader, net::UnixStream};

pub async fn call(
    connection: &Connection,
    operation: &Operation,
    claim: Option<Claim>,
    deadline: Duration,
) -> Result<Value> {
    try_call(connection, operation, claim, deadline)
        .await?
        .ok_or_else(Error::service_not_listening)
}

/// The callback runs after connection, socket and frame admission, immediately
/// before the first write is polled. Submission may then be uncertain, not proven.
pub async fn call_observed(
    connection: &Connection,
    operation: &Operation,
    claim: Option<Claim>,
    deadline: Duration,
    on_submit: impl FnOnce(),
) -> Result<Value> {
    try_call_observed(connection, operation, claim, deadline, on_submit)
        .await?
        .ok_or_else(Error::service_not_listening)
}
/// None is returned only before submission, for not-found/refused connections.
/// Once connected, no error authorizes automatic offline execution.
pub async fn try_call(
    connection: &Connection,
    operation: &Operation,
    claim: Option<Claim>,
    deadline: Duration,
) -> Result<Option<Value>> {
    try_call_observed(connection, operation, claim, deadline, || {}).await
}

async fn try_call_observed(
    connection: &Connection,
    operation: &Operation,
    claim: Option<Claim>,
    deadline: Duration,
    on_submit: impl FnOnce(),
) -> Result<Option<Value>> {
    let stream = match tokio::time::timeout(
        deadline.min(limits::CONNECT),
        UnixStream::connect(&connection.socket),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None);
        }
        Ok(Err(cause)) => {
            return Err(Error::service_connection_failed().caused_by(cause));
        }
        Err(cause) => {
            return Err(Error::connection_timeout().caused_by(cause));
        }
    };
    super::check_socket(&connection.socket)?;
    let request = Request {
        protocol: crate::compatibility::SERVICE_PROTOCOL,
        credential: connection.credential.clone(),
        claim,
        operation: operation.clone(),
    };
    // Encode before submission; oversize requests are known not to be accepted.
    let bytes = framing::encode(&request, framing::REQUEST_BYTES)?;
    let result = tokio::time::timeout(deadline, async {
        use tokio::io::AsyncWriteExt;
        let mut stream = BufReader::new(stream);
        on_submit();
        stream.get_mut().write_all(&bytes).await?;
        let bytes = framing::read(&mut stream, framing::RESPONSE_BYTES)
            .await?
            .ok_or_else(Error::connection_lost)?;
        let reply: Reply = hive_input::decode::from_slice(&bytes)?;
        if let Reply::Ok { result, .. } = &reply {
            result_shape(operation, result)?;
        }
        Ok::<_, Error>(reply)
    })
    .await;
    match result {
        Ok(Ok(reply)) => reply.result().map(Some),
        Ok(Err(cause)) => Err(operation.lost_reply().caused_by(cause)),
        Err(cause) => Err(operation.lost_reply().caused_by(cause)),
    }
}

fn result_shape(operation: &Operation, value: &Value) -> serde_json::Result<()> {
    use serde::{Deserialize, de::Error as _};
    match operation {
        Operation::Graph { .. } => Vec::<crate::values::Slug>::deserialize(value).map(|_| ()),
        Operation::List { .. } | Operation::CredentialList => {
            if value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_object))
            {
                Ok(())
            } else {
                Err(serde_json::Error::custom(
                    "expected a list of named result objects",
                ))
            }
        }
        Operation::Health => hive_input::decode::object::<_, super::api::Health>(value).map(|_| ()),
        _ if value.is_object() => Ok(()),
        _ => Err(serde_json::Error::custom("expected a named result object")),
    }
}

#[cfg(test)]
mod tests;
