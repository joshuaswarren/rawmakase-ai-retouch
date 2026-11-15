//! fal.ai, on the synchronous endpoint `https://fal.run/<model>`, with one
//! of the models [`FalModel`] names: Bria's FIBO Eraser for a plain removal,
//! Bria's FIBO Edit for a removal with an instruction, and Nano Banana, Bria
//! GenFill, FLUX.1 Fill \[pro] or FIBO Edit for a replacement.
//!
//! What this relies on, from fal's documentation (checked 2026-11):
//!
//! - `bria/fibo-edit-1.5/eraser` takes `image_url` and `mask_url`, "a binary
//!   mask the same size as the image. White marks what to remove, black what
//!   to keep", sent lossless. `composite: true` is its "pixel-perfect mode":
//!   only the masked area is rebuilt and the rest stays as it was, at full
//!   resolution. The output is `{"image": {"url", "width", "height", ...},
//!   "seed"}` at the input's size. $0.04 per image.
//!   <https://fal.ai/models/bria/fibo-edit-1.5/eraser/api>
//! - `bria/fibo-edit-1.5/edit` takes `image_urls` ("1-4 reference images"),
//!   `instruction`, and `mask_url`: "white where the model should edit, black
//!   elsewhere. Single-reference requests only, and it must be the same size
//!   as that image. A masked edit comes back at the reference's own
//!   resolution." $0.04 per image.
//!   <https://fal.ai/models/bria/fibo-edit-1.5/edit/api>
//! - `bria/genfill/v2` takes `image_url`, `mask_url` and `instruction`, all
//!   required ("Instruct what elements you would like to fill in your image
//!   based on the mask"). $0.04 per megapixel.
//!   <https://fal.ai/models/bria/genfill/v2>
//! - `fal-ai/flux-pro/v1/fill` takes `image_url`, `mask_url` and `prompt`
//!   ("The prompt to fill the masked part of the image"), and
//!   `output_format` (`jpeg` by default; `png` here). $0.05 per megapixel,
//!   rounded up. <https://fal.ai/models/fal-ai/flux-pro/v1/fill/api>
//! - `fal-ai/nano-banana/edit` (Gemini 2.5 Flash Image) takes `prompt` and
//!   `image_urls`, no mask, an `aspect_ratio` from a list (`auto`, `21:9`,
//!   `16:9`, `3:2`, `4:3`, `5:4`, `1:1`, `4:5`, `3:4`, `2:3`, `9:16`) and
//!   `output_format` (`png` here), and answers with `images`. The painted area
//!   goes as a second image (white where to change, at the padded size), and
//!   the prompt asks for the change to stay inside it: RAWmakase keeps only
//!   what is inside it, so what the model draws past it would be cut off. Its
//!   page doesn't
//!   say it keeps the input's size, so the region goes padded (its edges
//!   mirrored) to the nearest ratio, and its own area is cut back out of the
//!   answer resized to the padded size. $0.0398 per image.
//!   <https://fal.ai/models/fal-ai/nano-banana/edit/api>
//! - The Bria models answer with `image.url`; FLUX.1 Fill with `images`, a
//!   list of `{url, width, height, content_type}` (one by default). White in
//!   the mask is what to change in all four; the app keeps its own pixels
//!   outside it anyway.
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
use crate::pixels;
use crate::provider::{Provider, ProviderError, reads_as_refusal};
use serde_json::{Value, json};
use std::time::Duration;

/// The eraser, for a removal without an instruction.
pub(crate) const ERASER: &str = "bria/fibo-edit-1.5/eraser";
/// FIBO Edit, for a removal with an instruction and a replacement.
pub(crate) const EDIT: &str = "bria/fibo-edit-1.5/edit";
/// Bria GenFill, for a replacement.
pub(crate) const GENFILL: &str = "bria/genfill/v2";
/// FLUX.1 Fill \[pro], for a replacement.
pub(crate) const FLUX_FILL: &str = "fal-ai/flux-pro/v1/fill";
/// Nano Banana (Gemini 2.5 Flash Image) edit, for a replacement.
pub(crate) const NANO_BANANA: &str = "fal-ai/nano-banana/edit";

/// The aspect ratios Nano Banana takes (`aspect_ratio`), besides `auto`.
const NANO_BANANA_RATIOS: [&str; 10] = [
    "21:9", "16:9", "3:2", "4:3", "5:4", "1:1", "4:5", "3:4", "2:3", "9:16",
];

/// What a request asks of fal, with its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FalModel {
    /// Remove what is masked: [`ERASER`].
    Eraser,
    /// Edit what is masked as told: [`EDIT`].
    Edit(String),
    /// Fill what is masked with what is described: [`GENFILL`].
    GenFill(String),
    /// Fill what is masked with what is described: [`FLUX_FILL`].
    FluxFill(String),
    /// Edit the region as told, without a mask: [`NANO_BANANA`]. The app
    /// keeps only what is painted.
    NanoBanana(String),
}

impl FalModel {
    fn id(&self) -> &'static str {
        match self {
            Self::Eraser => ERASER,
            Self::Edit(_) => EDIT,
            Self::GenFill(_) => GENFILL,
            Self::FluxFill(_) => FLUX_FILL,
            Self::NanoBanana(_) => NANO_BANANA,
        }
    }

    /// The request's body, with the image and mask as data URIs.
    fn body(&self, image: String, mask: String) -> Value {
        match self {
            Self::Eraser => json!({
                "image_url": image,
                "mask_url": mask,
                "composite": true,
                "sync_mode": true,
            }),
            Self::Edit(instruction) => json!({
                "image_urls": [image],
                "mask_url": mask,
                "instruction": instruction,
                "sync_mode": true,
            }),
            Self::GenFill(instruction) => json!({
                "image_url": image,
                "mask_url": mask,
                "instruction": instruction,
                "sync_mode": true,
            }),
            Self::FluxFill(prompt) => json!({
                "image_url": image,
                "mask_url": mask,
                "prompt": prompt,
                "output_format": "png",
                "sync_mode": true,
            }),
            // No mask: it edits the region as told; the ratio is filled in by
            // `erase`, which pads the region to it.
            // It takes no mask: the painted area goes as a second image, white
            // where to change, and the prompt says to keep the change there,
            // as RAWmakase keeps only what is inside it.
            Self::NanoBanana(prompt) => json!({
                "prompt": format!(
                    "In the first image, change only the white area of the second image: \
                     {prompt}. Keep anything you add inside that white area, and leave \
                     everything outside it exactly as it is."
                ),
                "image_urls": [image, mask],
                "num_images": 1,
                "output_format": "png",
                "sync_mode": true,
            }),
        }
    }
}

/// fal.ai, with a key and the model a request asks for.
pub(crate) struct Fal {
    base: String,
    key: String,
    model: FalModel,
    timeout: Duration,
}

impl Fal {
    /// `base` is the synchronous endpoint (`https://fal.run`).
    pub(crate) fn new(base: &str, key: &str, model: FalModel, timeout: Duration) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
            model,
            timeout,
        }
    }
}

impl Provider for Fal {
    fn name(&self) -> &'static str {
        crate::FAL
    }

    fn model(&self) -> String {
        self.model.id().into()
    }

    /// The model and its options; the last part is this program's mask
    /// policy (a threshold and a grow). Raise it when what is sent for the
    /// same pixels changes. What the user wrote isn't in it: the app sends
    /// and keeps that itself.
    fn model_key(&self) -> String {
        match self.model {
            FalModel::Eraser => format!("fal|{ERASER}|composite|1"),
            FalModel::FluxFill(_) => format!("fal|{FLUX_FILL}|png|1"),
            FalModel::NanoBanana(_) => format!("fal|{NANO_BANANA}|png|padded|1"),
            _ => format!("fal|{}|1", self.model.id()),
        }
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
        // Nano Banana answers at an aspect ratio of its list, not the
        // region's: the region goes padded to the nearest, and its own area
        // is cut back out of the answer.
        let padded = match self.model {
            FalModel::NanoBanana(_) => {
                let (rgb, w, h) = pixels::decode(image_png)?;
                let padded = pixels::pad_to_aspect(&rgb, w, h, &NANO_BANANA_RATIOS);
                let png = pixels::encode_rgb_png(&padded.rgb, padded.width, padded.height)?;
                // The mask at the padded size, black in the padding.
                let (mask_rgb, ..) = pixels::decode(mask_png)?;
                let mut mask = vec![0u8; padded.width * padded.height];
                for y in 0..h {
                    for x in 0..w {
                        mask[(y + padded.y) * padded.width + x + padded.x] =
                            mask_rgb[(y * w + x) * 3];
                    }
                }
                let mask = pixels::encode_mask_png(&mask, padded.width, padded.height)?;
                Some((padded, png, mask, w, h))
            }
            _ => None,
        };
        let (sent, mask_sent) = padded
            .as_ref()
            .map_or((image_png, mask_png), |(_, png, mask, ..)| {
                (png.as_slice(), mask.as_slice())
            });
        let mut body = self.model.body(
            http::data_uri("image/png", sent),
            http::data_uri("image/png", mask_sent),
        );
        if let Some((padded, ..)) = &padded {
            body["aspect_ratio"] = json!(padded.ratio);
        }
        let agent = http::agent(self.timeout);
        let reply = http::read(
            agent
                .post(format!("{}/{}", self.base, self.model.id()))
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
        // Bria's models answer with `image`, FLUX's with `images`.
        let url = answer["image"]["url"]
            .as_str()
            .or_else(|| answer["images"][0]["url"].as_str())
            .ok_or_else(|| ProviderError::Invalid("fal.ai's answer has no image".into()))?;
        // A URL fal hosts is public; the key stays with fal.run.
        let image = http::fetch_image(&agent, url, None)?;
        let Some((padded, _, _, w, h)) = padded else {
            return Ok(image);
        };
        let answer = pixels::decode_to_size(&image, padded.width, padded.height)?;
        let region = pixels::crop(&answer, padded.width, padded.x, padded.y, w, h);
        pixels::encode_rgb_png(&region, w, h)
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
