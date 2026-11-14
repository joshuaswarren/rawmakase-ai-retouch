//! The region's pixels and the mask: from the app's buffers to PNG, and
//! from the provider's image back into the result.
use crate::provider::ProviderError;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, ImageReader, Limits, RgbImage};
use std::io::Cursor;

/// How far the mask grows past the painted area, in the region's pixels,
/// so the model also replaces the fringe a brush stroke tends to miss (an
/// object's soft edge, its shadow line).
const GROW: usize = 6;

/// A buffer's rows, `stride` bytes apart, packed tightly.
pub(crate) fn packed(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    channels: usize,
) -> Vec<u8> {
    let row = width * channels;
    let mut out = Vec::with_capacity(row * height);
    for y in 0..height {
        out.extend_from_slice(&data[y * stride..][..row]);
    }
    out
}

/// The grow radius for a `width` x `height` region: [`GROW`], but at most an
/// eighth of the shorter side so a small region isn't swallowed whole.
pub(crate) fn grow_radius(width: usize, height: usize) -> usize {
    GROW.min(width.min(height) / 8).max(1)
}

/// The binary mask the providers take: 255 where any coverage was painted
/// (above 0), grown by `radius` pixels (a disc), 0 elsewhere.
pub(crate) fn binary_mask(coverage: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    // For each pixel, the horizontal distance to the nearest painted pixel
    // in its row, when within `radius`.
    let far = usize::MAX;
    let mut across = vec![far; width * height];
    for y in 0..height {
        let row = &coverage[y * width..][..width];
        let out = &mut across[y * width..][..width];
        let mut last = None;
        for x in 0..width {
            if row[x] > 0 {
                last = Some(x);
            }
            if let Some(l) = last {
                out[x] = x - l;
            }
        }
        let mut next = None;
        for x in (0..width).rev() {
            if row[x] > 0 {
                next = Some(x);
            }
            if let Some(n) = next {
                out[x] = out[x].min(n - x);
            }
        }
    }
    // Then a pixel is in the disc of a painted one when some row within
    // `radius` has one close enough across.
    let r2 = radius * radius;
    let mut mask = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(height - 1);
            let inside = (lo..=hi).any(|yy| {
                let dx = across[yy * width + x];
                let dy = yy.abs_diff(y);
                dx != far && dx * dx + dy * dy <= r2
            });
            if inside {
                mask[y * width + x] = 255;
            }
        }
    }
    mask
}

/// Packed RGB as a PNG.
pub(crate) fn encode_rgb_png(
    rgb: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, ProviderError> {
    encode(rgb, width, height, ExtendedColorType::Rgb8)
}

/// A one-channel mask as a greyscale PNG: lossless, so black stays black.
pub(crate) fn encode_mask_png(
    mask: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, ProviderError> {
    encode(mask, width, height, ExtendedColorType::L8)
}

fn encode(
    data: &[u8],
    width: usize,
    height: usize,
    colour: ExtendedColorType,
) -> Result<Vec<u8>, ProviderError> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(data, width as u32, height as u32, colour)
        .map_err(|e| ProviderError::Invalid(format!("couldn't encode the region: {e}")))?;
    Ok(out)
}

/// Decodes the provider's image (PNG, JPEG or WebP) to packed 8-bit RGB at
/// `width` x `height`, resizing it when the provider returned another size.
/// Alpha, which the eraser keeps from the input (none here), is dropped.
pub(crate) fn decode_to_size(
    encoded: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, ProviderError> {
    let bad = |e: &dyn std::fmt::Display| {
        ProviderError::Invalid(format!("the provider's image doesn't decode: {e}"))
    };
    let mut reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .map_err(|e| bad(&e))?;
    // A service answer is untrusted: no larger than 8K a side, 1 GiB decoded.
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| bad(&e))?.into_rgb8();
    let decoded = if decoded.dimensions() == (width as u32, height as u32) {
        decoded
    } else {
        image::imageops::resize(&decoded, width as u32, height as u32, FilterType::Lanczos3)
    };
    Ok(RgbImage::into_raw(decoded))
}

/// Writes packed RGB into the result, rows `stride` bytes apart.
pub(crate) fn write_rgb(rgb: &[u8], width: usize, height: usize, result: &mut [u8], stride: usize) {
    let row = width * 3;
    for y in 0..height {
        result[y * stride..][..row].copy_from_slice(&rgb[y * row..][..row]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_is_binary_and_grows_as_a_disc() {
        let (w, h) = (31, 31);
        let mut coverage = vec![0u8; w * h];
        coverage[15 * w + 15] = 1; // The faintest paint counts.
        let mask = binary_mask(&coverage, w, h, 6);
        assert!(mask.iter().all(|&v| v == 0 || v == 255));
        let at = |x: usize, y: usize| mask[y * w + x];
        assert_eq!(at(15, 15), 255);
        assert_eq!(at(21, 15), 255);
        assert_eq!(at(15, 9), 255);
        assert_eq!(at(22, 15), 0);
        // (4, 4) away is within 6; (5, 5) is not.
        assert_eq!(at(19, 19), 255);
        assert_eq!(at(20, 20), 0);
        assert_eq!(mask.iter().filter(|&&v| v == 255).count(), 113);
    }

    #[test]
    fn small_regions_grow_less() {
        assert_eq!(grow_radius(2048, 2048), 6);
        assert_eq!(grow_radius(24, 400), 3);
        assert_eq!(grow_radius(3, 3), 1);
    }

    #[test]
    fn strides_are_honoured_both_ways() {
        let data = [1, 2, 3, 9, 4, 5, 6, 9];
        assert_eq!(packed(&data, 1, 2, 4, 3), [1, 2, 3, 4, 5, 6]);
        let mut out = [0u8; 8];
        write_rgb(&[1, 2, 3, 4, 5, 6], 1, 2, &mut out, 4);
        assert_eq!(out, [1, 2, 3, 0, 4, 5, 6, 0]);
    }
}
