//! What a hosted eraser is to this program: a PNG image and a PNG mask in,
//! an encoded image out.

/// A hosted model that removes what a mask covers.
pub(crate) trait Provider: Send {
    /// What the user calls the service, for messages (`fal.ai`).
    fn name(&self) -> &'static str;
    /// The model id saved with the patch (`bria/fibo-edit-1.5/eraser`).
    fn model(&self) -> String;
    /// Everything besides the pixels the result depends on: the service,
    /// the model, its options and this program's mask policy.
    fn model_key(&self) -> String;
    /// Sends `image_png` (RGB) and `mask_png` (the image's size, white where
    /// to remove) and returns the image the model made, encoded as the
    /// service returned it (PNG, JPEG or WebP). Checks `cancel` between
    /// steps and returns [`ProviderError::Cancelled`] once it is true.
    fn erase(
        &self,
        image_png: &[u8],
        mask_png: &[u8],
        cancel: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, ProviderError>;
}

/// Why a call failed, sorted by what the user can do about it. Messages may
/// hold what the service said; [`crate::sanitize`] cleans them before the
/// app sees them.
#[derive(Debug)]
pub(crate) enum ProviderError {
    /// The service rejected the key (HTTP 401 or 403).
    Auth { status: u16, message: String },
    /// The service can't be reached or won't work now: network failures,
    /// timeouts, rate limits, server errors.
    Unavailable(String),
    /// The service declined the image's content (a safety filter).
    Refused(String),
    /// The request can't work as made: a bad setting, an input the model
    /// rejects, an answer this program can't read.
    Invalid(String),
    /// The app cancelled the request.
    Cancelled,
}

/// Whether a message from a service reads as a content or safety refusal.
/// Neither Replicate nor Bria's own API documents one fixed code for it on
/// this model, so this is a match on the words they use.
pub(crate) fn reads_as_refusal(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "content_policy",
        "content policy",
        "content moderation",
        "content_moderation",
        "moderation",
        "nsfw",
        "flagged as sensitive",
        "safety",
        "inappropriate",
    ]
    .iter()
    .any(|word| message.contains(word))
}
