//! Typed test observations across Hive, setup and forwarded Comb error domains.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

#[track_caller]
pub fn observe<T: DeserializeOwned>(value: Value) -> T {
    assert!(value.is_object(), "reply must be an object: {value}");
    serde_json::from_value(value).expect("reply matches its public projection")
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Failure {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub fallback: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub request_id: Option<Value>,
    pub error: Problem,
    #[serde(flatten)]
    other: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Problem {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_committed: Option<bool>,
    pub code: String,
    pub message: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub field: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub expected: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub examples: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub received: Option<Value>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub request_id: Option<Value>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

impl Failure {
    #[track_caller]
    pub fn parse(value: Value) -> Self {
        observe(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failure_observation_preserves_extensions_and_forbidden_field_presence() {
        let wire = json!({"error":{"code":"invalid_value","message":"invalid",
            "field":"--name","expected":"a name","examples":["hivectl agents"],
            "extension":true},"schema_version":1});
        let failure = Failure::parse(wire.clone());
        assert!(failure.error.received.is_none());
        assert_eq!(serde_json::to_value(failure).unwrap(), wire);
        let mut explicit = wire.clone();
        explicit["error"]["received"] = Value::Null;
        let failure = Failure::parse(explicit.clone());
        assert!(failure.error.received.is_some());
        assert_eq!(serde_json::to_value(failure).unwrap(), explicit);
        for malformed in [
            json!({"error":null}),
            json!({"error":[]}),
            json!([]),
            json!({"error":{"code":-32602,"message":"JSON-RPC is a separate domain"}}),
        ] {
            assert!(serde_json::from_value::<Failure>(malformed).is_err());
        }
        for field in ["field", "expected", "examples"] {
            let mut malformed = wire.clone();
            malformed["error"][field] = Value::Null;
            assert!(serde_json::from_value::<Failure>(malformed).is_err());
        }
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failure_presence_keeps_outer_retry_id_and_committed_false_visible() {
        let wire = json!({"request_id":"retry","error":{"code":"conflict",
            "message":"Conflict","file_committed":false},"extension":true});
        let reply = Failure::parse(wire.clone());
        assert_eq!(reply.error.file_committed, Some(false));
        assert_eq!(reply.request_id.as_ref().unwrap(), "retry");
        assert_eq!(serde_json::to_value(reply).unwrap(), wire);
        let mut explicit = wire.clone();
        explicit["request_id"] = Value::Null;
        let reply = Failure::parse(explicit.clone());
        assert!(reply.request_id.is_some());
        assert_eq!(serde_json::to_value(reply).unwrap(), explicit);
        let mut malformed = wire;
        malformed["error"]["file_committed"] = Value::Null;
        assert!(serde_json::from_value::<Failure>(malformed).is_err());
    }
}
