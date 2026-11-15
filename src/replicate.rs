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
/// The model a request with text goes to: Bria's FIBO Edit, which takes
/// `image`, `instruction` ("Text description of the desired edit") and
/// `mask` ("White areas will be edited; black areas preserved"), from
/// <https://replicate.com/bria/fibo-edit> (checked 2026-11).
pub(crate) const EDIT_MODEL: &str = "bria/fibo-edit";

/// How long `Prefer: wait` holds the create call: Replicate's maximum.
const WAIT: u32 = 60;

/// The shortest `Cancel-After` Replicate takes
/// (<https://replicate.com/docs/reference/http>): with less of the deadline
/// left, no prediction is started.
const LEAST_CANCEL_AFTER: Duration = Duration::from_secs(5);

/// Replicate, with a token and a model.
pub(crate) struct Replicate {
    base: String,
    token: String,
    /// `owner/name`.
    model: String,
    /// The version id, for a model named `owner/name:version`.
    version: Option<String>,
    /// The `instruction` input, for [`EDIT_MODEL`].
    instruction: Option<String>,
    deadline: Duration,
    poll_every: Duration,
}

impl Replicate {
    /// Checks `model` is `owner/name` or `owner/name:version`. With an
    /// `instruction`, it goes to the model as its `instruction` input.
    pub(crate) fn new(
        base: &str,
        token: &str,
        model: &str,
        instruction: Option<String>,
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
        // Never the value itself: a token pasted here by mistake must not
        // reach a message.
        if !shaped {
            return Err(
                "the Replicate model setting isn't owner/name or owner/name:version".into(),
            );
        }
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            model: name.to_string(),
            version: version.map(str::to_string),
            instruction,
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
        until: Until<'_>,
        name: &str,
        png: &[u8],
    ) -> Result<(String, String), ProviderError> {
        let agent = until.agent()?;
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
        until: Until<'_>,
        uploaded: &mut Vec<String>,
        image_png: &[u8],
        mask_png: &[u8],
    ) -> Result<Vec<u8>, ProviderError> {
        let (id, image) = self.upload(until, "image", image_png)?;
        uploaded.push(id);
        let (id, mask) = self.upload(until, "mask", mask_png)?;
        uploaded.push(id);
        // Checked again before the paid call: it never starts once cancelled,
        // past the deadline, or with too little of it left to bound it.
        let agent = until.agent()?;
        // Read once: the check and the header must agree.
        let left = until.left();
        if left < LEAST_CANCEL_AFTER {
            return Err(ProviderError::Unavailable(
                "the request timed out before the prediction could start".into(),
            ));
        }
        let mut input = json!({ "image": image, "mask": mask });
        if let Some(instruction) = &self.instruction {
            input["instruction"] = json!(instruction);
        }
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
                // What is left of the request's own deadline, not a fresh one.
                .header("Cancel-After", format!("{}s", left.as_secs()))
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
        let prediction = reply.json();
        let id = prediction["id"]
            .as_str()
            .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric()) && !id.is_empty())
            .ok_or_else(|| ProviderError::Invalid("Replicate's answer has no prediction".into()))?
            .to_string();
        // From here every way out but success, or the prediction's own end,
        // stops it on Replicate: a paid prediction is never left running.
        let result = self.follow(until, &id, prediction);
        if let Err(Ended::Abandoned(_)) = &result {
            self.cancel(&id);
        }
        let prediction = result.map_err(Ended::into_error)?;
        let agent = until.agent()?;
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
        http::fetch_image(&agent, output, on_api.then_some(authorization.as_str()))
    }

    /// Polls prediction `id` from `prediction` until it ends.
    fn follow(&self, until: Until<'_>, id: &str, mut prediction: Value) -> Result<Value, Ended> {
        loop {
            match prediction["status"].as_str().unwrap_or("") {
                "succeeded" => return Ok(prediction),
                "failed" => return Err(Ended::Over(failed(&prediction))),
                "canceled" => {
                    return Err(Ended::Over(ProviderError::Unavailable(
                        "the prediction was cancelled on Replicate".into(),
                    )));
                }
                _ => {}
            }
            self.wait(until).map_err(Ended::Abandoned)?;
            let agent = until.agent().map_err(Ended::Abandoned)?;
            let reply = http::read(
                agent
                    .get(format!("{}/v1/predictions/{id}", self.base))
                    .header("Authorization", self.authorization())
                    .call(),
            )
            .map_err(Ended::Abandoned)?;
            if !(200..=299).contains(&reply.status) {
                return Err(Ended::Abandoned(error(&reply, "reading the prediction")));
            }
            prediction = reply.json();
        }
    }

    /// Sleeps one polling interval, watching the cancel and the deadline.
    fn wait(&self, until: Until<'_>) -> Result<(), ProviderError> {
        let next = Instant::now() + self.poll_every;
        loop {
            until.check()?;
            let now = Instant::now();
            if now >= next {
                return Ok(());
            }
            std::thread::sleep((next - now).min(Duration::from_millis(20)));
        }
    }
}

/// How following a prediction ended, when not in success.
enum Ended {
    /// It ended by itself on Replicate (failed, cancelled there).
    Over(ProviderError),
    /// This program stopped following it (cancelled, past the deadline, a
    /// poll that failed): it may still be running there.
    Abandoned(ProviderError),
}
impl Ended {
    fn into_error(self) -> ProviderError {
        match self {
            Self::Over(e) | Self::Abandoned(e) => e,
        }
    }
}

/// One request's cancel and its absolute deadline, shared by every step, so
/// no step starts once either has passed and none outlives the deadline.
#[derive(Clone, Copy)]
struct Until<'a> {
    cancel: &'a dyn Fn() -> bool,
    at: Instant,
}
impl Until<'_> {
    fn left(&self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }
    /// Fails when cancelled or past the deadline.
    fn check(&self) -> Result<(), ProviderError> {
        if (self.cancel)() {
            return Err(ProviderError::Cancelled);
        }
        if self.left().is_zero() {
            return Err(ProviderError::Unavailable("the request timed out".into()));
        }
        Ok(())
    }
    /// An agent for the next call, bounded by what is left of the deadline.
    fn agent(&self) -> Result<Agent, ProviderError> {
        self.check()?;
        Ok(http::agent(self.left()))
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
        let until = Until {
            cancel,
            at: Instant::now() + self.deadline,
        };
        let mut uploaded = Vec::new();
        let result = self.predict(until, &mut uploaded, image_png, mask_png);
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
        .map(|n| format!("rawmakase-ai-retouch-{seed:x}-{n}"))
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
