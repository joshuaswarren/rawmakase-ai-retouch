//! Blocking HTTP with `ureq`, and the data URIs images travel as.
use crate::provider::ProviderError;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use std::time::Duration;
use ureq::Agent;
use ureq::http::Response;

/// The largest answer read from a service: a 2048 px PNG is well inside it.
const MAX_BODY: u64 = 64 << 20;

/// An agent whose every call ends within `timeout`, connecting within 15 s,
/// and which hands back every HTTP status rather than failing on one.
/// Redirects never carry the `Authorization` header (ureq's default).
pub(crate) fn agent(timeout: Duration) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(Duration::from_secs(15).min(timeout)))
        .http_status_as_error(false)
        .user_agent(concat!("rawmakase-ai-retouch/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// An answer: its status and body.
pub(crate) struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as JSON, or [`Value::Null`] when it isn't.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    /// The body as text, lossily.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Reads a response whole, or says why the call failed.
pub(crate) fn read(
    response: Result<Response<ureq::Body>, ureq::Error>,
) -> Result<Reply, ProviderError> {
    let mut response = response.map_err(transport)?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(transport)?;
    Ok(Reply { status, body })
}

/// A call that got no answer: offline, a name that doesn't resolve, a
/// timeout. All are worth trying again later, by the user.
fn transport(error: ureq::Error) -> ProviderError {
    ProviderError::Unavailable(match error {
        ureq::Error::Timeout(_) => "the request timed out".to_string(),
        ureq::Error::HostNotFound => "the service's address doesn't resolve (offline?)".into(),
        ureq::Error::ConnectionFailed => "couldn't connect to the service (offline?)".into(),
        ureq::Error::BodyExceedsLimit(_) => "the service's answer is too large".into(),
        other => format!("the request failed: {other}"),
    })
}

/// `bytes` as a `data:` URI of type `mime`.
pub(crate) fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// The bytes of a base64 `data:` URI; `None` when `uri` isn't one.
pub(crate) fn from_data_uri(uri: &str) -> Option<Result<Vec<u8>, ProviderError>> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if !meta.ends_with(";base64") {
        return Some(Err(ProviderError::Invalid(
            "the service returned an image that isn't base64".into(),
        )));
    }
    Some(STANDARD.decode(data.trim()).map_err(|_| {
        ProviderError::Invalid("the service returned an image that doesn't decode".into())
    }))
}

/// The image at `url`: a `data:` URI, or an `http(s)` URL fetched with
/// `authorization` when one is given.
pub(crate) fn fetch_image(
    agent: &Agent,
    url: &str,
    authorization: Option<&str>,
) -> Result<Vec<u8>, ProviderError> {
    if let Some(bytes) = from_data_uri(url) {
        return bytes;
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(ProviderError::Invalid(
            "the service returned an image address this program can't read".into(),
        ));
    }
    let mut request = agent.get(url);
    if let Some(authorization) = authorization {
        request = request.header("Authorization", authorization);
    }
    let reply = read(request.call())?;
    match reply.status {
        200..=299 => Ok(reply.body),
        status => Err(ProviderError::Unavailable(format!(
            "downloading the result failed (HTTP {status})"
        ))),
    }
}
