//! fal.ai: Bria's FIBO Eraser, `bria/fibo-edit-1.5/eraser`, on the
//! synchronous endpoint `https://fal.run/<model>`.
//!
//! What this relies on, from fal's documentation (checked 2026-11):
//!
//! - The model's input is `image_url` and `mask_url`, "a binary mask the same
//!   size as the image. White marks what to remove, black what to keep", sent
//!   lossless. `composite: true` is its "pixel-perfect mode": only the masked
//!   area is rebuilt and the rest stays as it was, at full resolution. The
//!   output is `{"image": {"url", "width", "height", ...}, "seed"}` at the
//!   input's size. $0.04 per image.
//!   <https://fal.ai/models/bria/fibo-edit-1.5/eraser/api>,
//!   <https://fal.ai/models/bria/fibo-edit-1.5/eraser>
//! - File inputs may be base64 `data:` URIs: the model's API page says you can
//!   "pass a Base64 data URI as a file input. The API will handle the file
//!   decoding for you" (fal's CDN guide recommends hosted URLs for large
//!   files, <https://fal.ai/docs/documentation/model-apis/fal-cdn>). A region
//!   is at most 2048 px a side, a few MiB as PNG, well inside the 64 MiB
//!   request limit, so this program sends data URIs and uploads nothing to
//!   fal's storage first.
//! - `sync_mode: true` "returns the image directly in the response" (as a
//!   data URI), so nothing is fetched afterwards. A URL is fetched all the
//!   same, without the key, should fal return one.
//! - `Authorization: Key <key>`; `X-Fal-Store-IO: 0` turns off storing the
//!   request's payloads. Statuses: 401 bad credentials, 403 the key can't use
//!   this model (permissions, a spending limit, a locked account), 422 the
//!   model rejected the input, 429 the concurrency limit, 500/503/504 the
//!   platform. <https://fal.ai/docs/api-reference/inference-api/sync/run>
//! - A content refusal is a 422 whose `detail` holds an item of `type`
//!   `content_policy_violation`.
//!   <https://fal.ai/docs/documentation/model-apis/errors>
use crate::http::{self, Reply};
use crate::provider::{Provider, ProviderError, reads_as_refusal};
use serde_json::{Value, json};
use std::time::Duration;

/// The model this program calls.
pub(crate) const MODEL: &str = "bria/fibo-edit-1.5/eraser";

/// fal.ai, with a key.
pub(crate) struct Fal {
    base: String,
    key: String,
    timeout: Duration,
}

impl Fal {
    /// `base` is the synchronous endpoint (`https://fal.run`).
    pub(crate) fn new(base: &str, key: &str, timeout: Duration) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
            timeout,
        }
    }
}

impl Provider for Fal {
    fn name(&self) -> &'static str {
        crate::FAL
    }

    fn model(&self) -> String {
        MODEL.into()
    }

    /// The last part is this program's mask policy (a threshold and a
    /// grow); raise it when what is sent for the same pixels changes.
    fn model_key(&self) -> String {
        format!("fal|{MODEL}|composite|1")
    }

    fn erase(
        &self,
        image_png: &[u8],
        mask_png: &[u8],
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, ProviderError> {
        if cancel() {
            return Err(ProviderError::Cancelled);
        }
        let body = json!({
            "image_url": http::data_uri("image/png", image_png),
            "mask_url": http::data_uri("image/png", mask_png),
            "composite": true,
            "sync_mode": true,
        });
        let agent = http::agent(self.timeout);
        let reply = http::read(
            agent
                .post(format!("{}/{MODEL}", self.base))
                .header("Authorization", format!("Key {}", self.key))
                .header("X-Fal-Store-IO", "0")
                .content_type("application/json")
                .send(body.to_string().as_bytes()),
        )?;
        if !(200..=299).contains(&reply.status) {
            return Err(error(&reply));
        }
        if cancel() {
            return Err(ProviderError::Cancelled);
        }
        let answer = reply.json();
        let url = answer["image"]["url"]
            .as_str()
            .ok_or_else(|| ProviderError::Invalid("fal.ai's answer has no image".into()))?;
        // A URL fal hosts is public; the key stays with fal.run.
        http::fetch_image(&agent, url, None)
    }
}

/// What a failed call means.
fn error(reply: &Reply) -> ProviderError {
    let json = reply.json();
    let message = message(&json).unwrap_or_else(|| reply.text());
    match reply.status {
        401 | 403 => ProviderError::Auth {
            status: reply.status,
            message,
        },
        422 if refused(&json) || reads_as_refusal(&message) => ProviderError::Refused(message),
        408 | 429 | 500..=599 => {
            ProviderError::Unavailable(format!("HTTP {}: {message}", reply.status))
        }
        status => ProviderError::Invalid(format!("HTTP {status}: {message}")),
    }
}

/// Whether a 422's `detail` names a content policy violation.
fn refused(json: &Value) -> bool {
    json["detail"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["type"].as_str() == Some("content_policy_violation"))
    })
}

/// The human part of a fal error body: `detail` as text, or each of its
/// items' `msg`.
fn message(json: &Value) -> Option<String> {
    match &json["detail"] {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let messages: Vec<&str> = items.iter().filter_map(|i| i["msg"].as_str()).collect();
            (!messages.is_empty()).then(|| messages.join("; "))
        }
        _ => json["error"]
            .as_str()
            .or_else(|| json["message"].as_str())
            .map(str::to_string),
    }
}
