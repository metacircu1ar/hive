//! Bounded native-server readiness, separate from process ownership.
use super::{Error, HEALTH_TIMEOUT, HOST, Result};
use std::io::Read as _;
pub(super) const HEALTH_BYTES: usize = 4096;

pub(super) fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(HEALTH_TIMEOUT)
        .connect_timeout(HEALTH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|cause| Error::http_transport_initialization().caused_by(cause))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Health {
    Ready,
    Loading,
}

#[derive(serde::Deserialize)]
struct HealthReply {
    status: String,
}

pub(super) fn health(client: &reqwest::blocking::Client, port: u16) -> Result<Health> {
    let Ok(mut response) = client.get(format!("http://{HOST}:{port}/health")).send() else {
        // Not listening yet, or busy loading: readiness has its own deadline.
        return Ok(Health::Loading);
    };
    match response.status().as_u16() {
        200 => {
            let mut body = [0; HEALTH_BYTES + 1];
            let mut length = 0;
            while length < body.len() {
                let count = response
                    .read(&mut body[length..])
                    .map_err(|cause| Error::llama_server_unhealthy().caused_by(cause))?;
                if count == 0 {
                    break;
                }
                length += count;
            }
            if length > HEALTH_BYTES {
                return Err(Error::llama_server_unhealthy());
            }
            let reply: HealthReply = serde_json::from_slice(&body[..length])
                .map_err(|cause| Error::llama_server_unhealthy().caused_by(cause))?;
            if reply.status == "ok" {
                Ok(Health::Ready)
            } else {
                Err(Error::llama_server_unhealthy())
            }
        }
        503 => Ok(Health::Loading),
        _ => Err(Error::llama_server_unhealthy()),
    }
}
