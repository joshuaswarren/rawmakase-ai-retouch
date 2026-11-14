# AI Remove for RAWmakase

**Experimental.** A [RAWmakase](https://github.com/pch/rawmakase) extension
that adds **AI Remove** to the Remove tool: paint over an object and a hosted
AI model fills the area with what could plausibly be behind it. You use your
own account and API key with the service.

RAWmakase does all the colour work. It renders the region around what you
painted, sends it to this extension as 8-bit sRGB with the painted area, and
keeps the pixels it gets back as a patch in your edit, which renders without
the extension or the service from then on. The extension only turns pixels
into pixels through the service.

## Services and models

| Service | Model | Price per removal |
| --- | --- | --- |
| [fal.ai](https://fal.ai) (default) | Bria FIBO Eraser, [`bria/fibo-edit-1.5/eraser`](https://fal.ai/models/bria/fibo-edit-1.5/eraser), in its pixel-perfect mode (`composite`): only the masked area is rebuilt | $0.04 |
| [Replicate](https://replicate.com) | Bria Eraser, [`bria/eraser`](https://replicate.com/bria/eraser), an official Replicate model. Another model that takes `image` and `mask` inputs can be named in the settings | $0.04 for `bria/eraser` |

Both models are Bria's, trained on licensed data, and take an image and a
mask with no prompt. Prices are the services' as of November 2026; check
their pages for the current ones.

## Privacy and cost

- **The region leaves your computer.** Each removal uploads the rendered
  region around what you painted (at most 2048 pixels a side) and a mask to the
  service you chose. RAWmakase asks once before the first upload. The
  extension asks fal.ai not to store the request's payloads
  (`X-Fal-Store-IO: 0`) and deletes the files it uploads to Replicate once the
  prediction ends; what each service keeps beyond that is up to its own
  policy.
- **Each removal is one paid call.** Nothing is retried automatically, not
  even after a timeout or a rate limit: you decide whether to try again.
- Your key is kept in the system's credential store by RAWmakase, never in
  its settings file. The extension never logs it, and removes anything shaped
  like a key from the services' error messages before you see them.

## Setting it up

1. Get a key:
   - fal.ai: [fal.ai > Dashboard > Keys](https://fal.ai/dashboard/keys);
   - Replicate: [replicate.com > Account > API tokens](https://replicate.com/account/api-tokens).
   Both need billing set up on the service.
2. In RAWmakase, open **Preferences > Extensions > AI Remove**, pick the
   **Service** and paste the key into **fal.ai API key** or **Replicate API
   token**.

Until a key is set, AI Remove says so and points you to that page.

**Environment variables.** When the key setting is empty, the extension reads
`FAL_KEY` (fal.ai) or `REPLICATE_API_TOKEN` (Replicate) from its environment.
RAWmakase passes its own environment on, but a GUI app started from a desktop
launcher, the Dock or the Start menu usually doesn't see variables set in a
shell profile (`.zshrc`, `.bashrc`), so the setting is the reliable place.

## Limitations

- **8-bit.** The region travels as 8-bit sRGB both ways, so a removal over
  deep shadows or smooth gradients may band where a raw edit wouldn't.
- **At most 2048 pixels a side.** A larger region is sent at a lower
  resolution and scaled back by RAWmakase.
- **No HDR.** RAWmakase refuses AI Remove on HDR files and HDR edits.
- **Lightroom never sees these spots.** AI Remove patches are RAWmakase's
  own: they aren't written to XMP, so Lightroom and Camera Raw show the photo
  without them.
- The result depends on the service: it can change between calls and between
  model versions, and a service may decline an image on its content.

## Installing

Install it from RAWmakase: **Preferences > Extensions > Available**, or
**Install from GitHub…** with `https://github.com/pch/rawmakase-ai-remove`. It
needs a RAWmakase with AI Remove ([pch/rawmakase#1093](https://github.com/pch/rawmakase/pull/1093)),
which isn't in a release yet: RAWmakase 0.5.0 reads manifests up to `api = 2`
and refuses this one as written for a newer RAWmakase.

## Building and releasing

```sh
cargo test                         # no network, no key needed
# The RAWmakase commit the SDK is pinned to: it reads `api = 3`.
cargo install --locked --git https://github.com/pch/rawmakase \
    --rev 2f03585df062bf6f3f0ea377af0034f2161ab44e cargo-rawmakase
cargo rawmakase dev                # try it in RAWmakase's developer mode
cargo rawmakase build && cargo rawmakase package
```

To release, raise `version` in both `Cargo.toml` and
`rawmakase-extension.toml`, commit, and push a tag `v<version>`.
`.github/workflows/rawmakase-release.yml` builds the extension for Linux,
macOS and Windows (x86-64 and ARM each), packages it with `cargo rawmakase
package` and attaches the `.rmext` files to the tag's GitHub release, where
RAWmakase's Install from GitHub finds them.

## How it works

- The painted area is made binary (anything painted at all counts) and grown
  by 6 pixels, so the model also replaces the soft fringe a brush tends to
  miss. The mask is sent as a lossless PNG the region's size, white where to
  remove.
- fal.ai: one request to `https://fal.run/bria/fibo-edit-1.5/eraser` with the
  region and the mask as base64 `data:` URIs, which the
  [model's API](https://fal.ai/models/bria/fibo-edit-1.5/eraser/api) accepts,
  and `sync_mode`, which returns the image in the answer.
- Replicate: the region and the mask are uploaded with the files API, a
  prediction is started with `Prefer: wait` and polled until it ends, then
  the uploads are deleted.
- A result at another size than the region is resized to it. Cancelling in
  RAWmakase answers at once; a Replicate prediction is also cancelled on
  Replicate. A removal gives up after 3 minutes.

The source has the details and the documentation each fact was checked
against: `src/fal.rs` and `src/replicate.rs`.

## License

MIT
