//! Version-only projections precede the full strict receipt decoder.
use hive_input::compatibility::record::{self, Error};
use serde::Deserialize;
use serde_json::Value;

// Unknown fields belong to the later exact decoder, or a newer nested format.
#[derive(Deserialize)]
struct Version {
    format: u64,
}
#[derive(Deserialize)]
struct Job {
    #[serde(deserialize_with = "hive_input::decode::object")]
    plan: Version,
}
#[derive(Deserialize)]
struct Scope {
    #[serde(deserialize_with = "hive_input::decode::object")]
    spec: Version,
}
#[derive(Deserialize)]
struct Services {
    #[serde(default, deserialize_with = "hive_input::decode::objects")]
    scopes: Vec<Scope>,
}
#[derive(Deserialize)]
struct Nested {
    #[serde(default, deserialize_with = "hive_input::decode::objects")]
    jobs: Vec<Job>,
    #[serde(default, deserialize_with = "hive_input::decode::optional_object")]
    services: Option<Services>,
}
pub(super) fn classify(value: &Value) -> Result<(), Error> {
    record::classify(value, &crate::compatibility::INSTALLATION)?;
    let nested = Nested::deserialize(value).map_err(Error::Malformed)?;
    for job in nested.jobs {
        record::require(
            job.plan.format,
            &hive_setup_support::compatibility::MAINTENANCE,
        )?;
    }
    if let Some(services) = nested.services {
        for scope in services.scopes {
            record::require(
                scope.spec.format,
                &hive_setup_support::compatibility::SERVICE,
            )?;
        }
    }
    Ok(())
}
