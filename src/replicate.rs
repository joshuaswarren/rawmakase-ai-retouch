//! Replicate: a mask-based eraser through the predictions API.
//!
//! The default model is Bria's `bria/eraser`, an official Replicate model
//! (always on, versionless, kept up by Bria; $0.04 per output image). It is
//! the same vendor's eraser as the fal.ai default, trained on licensed data,
//! and takes exactly an image and a mask, with no prompt to write. Its input
//! schema, from <https://replicate.com/bria/eraser> (checked 2026-11):
//! `image` and `mask` (file URIs), `image_url`, `mask_url`, `sync` (default
//! true), `content_moderation` (default false), `preserve_alpha` (default
//! true) and a deprecated `mask_type`; the output is one image URI. Another
//! model may be named in the `replicate-model` setting if it takes `image`
//! and `mask` the same way.
//!
//! The flow, from <https://replicate.com/docs/reference/http>:
//!
//! 1. The image and the mask are uploaded with the files API
//!    (`POST /v1/files`, multipart `content`), and their `urls.get` passed as
//!    inputs, as Replicate's own clients do. Data URIs are "only recommended
//!    if the file is less than 1MB"
//!    (<https://replicate.com/docs/topics/predictions/input-files>), and a
//!    region can be several. Both are deleted once the prediction ends.
//! 2. `POST /v1/models/{owner}/{name}/predictions` (or `/v1/predictions` with
//!    a `version`) with `Prefer: wait=60`, which holds the request up to 60 s,
//!    and `Cancel-After`, so Replicate stops it on its own past the deadline.
//! 3. A prediction still `starting` or `processing` is polled with
//!    `GET /v1/predictions/{id}` until it ends, the deadline passes or the app
//!    cancels; the last two cancel it (`POST /v1/predictions/{id}/cancel`).
//!
//! Replicate has no error code for a content refusal (its codes,
//! <https://replicate.com/docs/reference/error-codes>, are infrastructure
//! ones): a model that declines fails the prediction with its own message,
//! which is matched on its words ([`reads_as_refusal`]).
use crate::http::{self, Reply};
use crate::provider::{Provider, ProviderError, reads_as_refusal};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use ureq::Agent;

/// The `replicate-model` setting's default.
pub(crate) const DEFAULT_MODEL: &str = "bria/eraser";

/// How long `Prefer: wait` holds the create call: Replicate's maximum.
const WAIT: u32 = 60;

/// Replicate, with a token and a model.
pub(crate) struct Replicate {
    base: String,
    token: String,
    /// `owner/name`.
    model: String,
    /// The version id, for a model named `owner/name:version`.
    version: Option<String>,
    deadline: Duration,
    poll_every: Duration,
}

impl Replicate {
    /// Checks `model` is `owner/name` or `owner/name:version`.
    pub(crate) fn new(
        base: &str,
        token: &str,
        model: &str,
        deadline: Duration,
        poll_every: Duration,
    ) -> Result<Self, String> {
        let (name, version) = match model.split_once(':') {
            Some((name, version)) => (name, Some(version)),
            None => (model, None),
        };
        let part = |p: &str| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        let shaped = name
            .split_once('/')
            .is_some_and(|(o, n)| part(o) && part(n))
            && version
                .is_none_or(|v| v.chars().all(|c| c.is_ascii_alphanumeric()) && !v.is_empty())
            && model.len() <= 100;
        if !shaped {
            return Err(format!(
                "the Replicate model setting {model:?} isn't owner/name or owner/name:version"
            ));
        }
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            model: name.to_string(),
            version: version.map(str::to_string),
            deadline,
            poll_every,
        })
    }

    fn authorization(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// Uploads a PNG with the files API and returns its id and URL.
    fn upload(
        &self,
        agent: &Agent,
        name: &str,
        png: &[u8],
    ) -> Result<(String, String), ProviderError> {
        let boundary = boundary(png);
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"content\"; \
             filename=\"{name}.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(png);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let reply = http::read(
            agent
                .post(format!("{}/v1/files", self.base))
                .header("Authorization", self.authorization())
                .content_type(format!("multipart/form-data; boundary={boundary}"))
                .send(&body[..]),
        )?;
        if !(200..=299).contains(&reply.status) {
            return Err(error(&reply, "uploading the image"));
        }
        let file = reply.json();
        match (file["id"].as_str(), file["urls"]["get"].as_str()) {
            (Some(id), Some(url)) => Ok((id.to_string(), url.to_string())),
            _ => Err(ProviderError::Invalid(
                "Replicate's answer to the upload has no file URL".into(),
            )),
        }
    }

    /// Deletes uploaded files; failures are ignored, as the files expire.
    fn delete(&self, files: &[String]) {
        let agent = http::agent(Duration::from_secs(10));
        for id in files {
            let _ = agent
                .delete(format!("{}/v1/files/{id}", self.base))
                .header("Authorization", self.authorization())
                .call();
        }
    }

    /// Asks Replicate to stop a prediction; failures are ignored, as
    /// `Cancel-After` stops it anyway.
    fn cancel(&self, id: &str) {
        let agent = http::agent(Duration::from_secs(10));
        let _ = agent
            .post(format!("{}/v1/predictions/{id}/cancel", self.base))
            .header("Authorization", self.authorization())
            .send_empty();
    }

    fn predict(
        &self,
        agent: &Agent,
        uploaded: &mut Vec<String>,
        image_png: &[u8],
        mask_png: &[u8],
        cancel: &dyn Fn() -> bool,
        started: Instant,
    ) -> Result<Vec<u8>, ProviderError> {
        let (id, image) = self.upload(agent, "image", image_png)?;
        uploaded.push(id);
        let (id, mask) = self.upload(agent, "mask", mask_png)?;
        uploaded.push(id);
        if cancel() {
            return Err(ProviderError::Cancelled);
        }
        let input = json!({ "image": image, "mask": mask });
        let (url, body) = match &self.version {
            Some(version) => (
                format!("{}/v1/predictions", self.base),
                json!({ "version": version, "input": input }),
            ),
            None => (
                format!("{}/v1/models/{}/predictions", self.base, self.model),
                json!({ "input": input }),
            ),
        };
        let reply = http::read(
            agent
                .post(url)
                .header("Authorization", self.authorization())
                .header("Prefer", format!("wait={WAIT}"))
                .header(
                    "Cancel-After",
                    format!("{}s", self.deadline.as_secs().max(5)),
                )
                .content_type("application/json")
                .send(body.to_string().as_bytes()),
        )?;
        if reply.status == 404 {
            return Err(ProviderError::Invalid(format!(
                "Replicate has no model {:?}; check the Replicate model setting",
                self.model
            )));
        }
        if !(200..=299).contains(&reply.status) {
            return Err(error(&reply, "starting the prediction"));
        }
        let mut prediction = reply.json();
        let id = prediction["id"]
            .as_str()
            .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric()) && !id.is_empty())
            .ok_or_else(|| ProviderError::Invalid("Replicate's answer has no prediction".into()))?
            .to_string();
        loop {
            match prediction["status"].as_str().unwrap_or("") {
                "succeeded" => break,
                "failed" => return Err(failed(&prediction)),
                "canceled" => {
                    return Err(ProviderError::Unavailable(
                        "the prediction was cancelled on Replicate".into(),
                    ));
                }
                _ => {}
            }
            if !self.wait(cancel, started) {
                self.cancel(&id);
                return Err(if cancel() {
                    ProviderError::Cancelled
                } else {
                    ProviderError::Unavailable("the prediction timed out".into())
                });
            }
            let reply = http::read(
                agent
                    .get(format!("{}/v1/predictions/{id}", self.base))
                    .header("Authorization", self.authorization())
                    .call(),
            )?;
            if !(200..=299).contains(&reply.status) {
                return Err(error(&reply, "reading the prediction"));
            }
            prediction = reply.json();
        }
        if cancel() {
            return Err(ProviderError::Cancelled);
        }
        let output = match &prediction["output"] {
            Value::String(url) => Some(url.as_str()),
            Value::Array(items) => items.iter().find_map(Value::as_str),
            _ => None,
        }
        .ok_or_else(|| ProviderError::Invalid("the prediction has no image".into()))?;
        // Files on Replicate's API need the token; its delivery URLs don't,
        // and nothing else gets it.
        let authorization = self.authorization();
        let on_api = output.starts_with(&format!("{}/", self.base));
        http::fetch_image(agent, output, on_api.then_some(authorization.as_str()))
    }

    /// Sleeps one polling interval, watching `cancel`; false when cancelled
    /// or past the deadline.
    fn wait(&self, cancel: &dyn Fn() -> bool, started: Instant) -> bool {
        let until = Instant::now() + self.poll_every;
        loop {
            if cancel() || started.elapsed() >= self.deadline {
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(20)));
        }
    }
}

impl Provider for Replicate {
    fn name(&self) -> &'static str {
        crate::REPLICATE
    }

    fn model(&self) -> String {
        match &self.version {
            Some(version) => format!("{}:{version}", self.model),
            None => self.model.clone(),
        }
    }

    /// The last part is this program's mask policy, as for fal.ai.
    fn model_key(&self) -> String {
        format!("replicate|{}|1", self.model())
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
        let started = Instant::now();
        let agent = http::agent(self.deadline);
        let mut uploaded = Vec::new();
        let result = self.predict(&agent, &mut uploaded, image_png, mask_png, cancel, started);
        self.delete(&uploaded);
        result
    }
}

/// A multipart boundary that doesn't occur in `data`.
fn boundary(data: &[u8]) -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0u32..)
        .map(|n| format!("rawmakase-ai-remove-{seed:x}-{n}"))
        .find(|b| !data.windows(b.len()).any(|w| w == b.as_bytes()))
        .expect("some boundary is free")
}

/// A failed prediction: refused on its content, or unavailable.
fn failed(prediction: &Value) -> ProviderError {
    let message = match &prediction["error"] {
        Value::String(text) => text.clone(),
        Value::Null => "the prediction failed".into(),
        other => other.to_string(),
    };
    if reads_as_refusal(&message) {
        ProviderError::Refused(message)
    } else {
        ProviderError::Unavailable(message)
    }
}

/// What a failed call means; `doing` names the step for the message.
fn error(reply: &Reply, doing: &str) -> ProviderError {
    let json = reply.json();
    let message = json["detail"]
        .as_str()
        .or_else(|| json["title"].as_str())
        .map_or_else(|| reply.text(), str::to_string);
    match reply.status {
        401 | 403 => ProviderError::Auth {
            status: reply.status,
            message,
        },
        422 if reads_as_refusal(&message) => ProviderError::Refused(message),
        408 | 429 | 500..=599 => {
            ProviderError::Unavailable(format!("{doing} failed (HTTP {}): {message}", reply.status))
        }
        status => ProviderError::Invalid(format!("{doing} failed (HTTP {status}): {message}")),
    }
}
