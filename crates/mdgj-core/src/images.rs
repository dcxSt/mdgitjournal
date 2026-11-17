//! Image import: decode, apply EXIF orientation, strip metadata, downscale, encode JPEG.

use std::io::Cursor;

use image::error::{LimitError, LimitErrorKind};
use image::imageops::FilterType;
use image::{
    DynamicImage, ImageDecoder, ImageError, ImageReader, ImageResult, Limits, Rgb, RgbImage,
};
use jpeg_encoder::{ChromaSubsamplingMethod, ColorType, Encoder, SamplingFactor};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Larger images are refused before decoding: a small, highly compressible file can otherwise
/// decode to gigabytes. 64 MP still covers 48 MP phones and 61 MP cameras.
const MAX_PIXELS: u64 = 64_000_000;
/// The most the decoded pixels may take: as much as the largest 8-bit RGBA image, so 16-bit
/// images must be smaller.
const MAX_DECODED_BYTES: u64 = MAX_PIXELS * 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOpts {
    /// Longest edge of the output in pixels; smaller images are not enlarged.
    pub max_edge: u32,
    /// JPEG quality, 1–100.
    pub quality: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedImage {
    /// Relative to the journal root, e.g. `"assets/2026-11-16-1402-morning-run-5f3a09c2.jpg"`.
    pub path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// Re-encodes a JPEG, PNG, WebP or GIF (first frame) as a small JPEG: EXIF orientation applied,
/// alpha flattened onto white, downscaled with Lanczos3 so the longest edge is at most
/// `opts.max_edge`, then encoded progressive 4:2:0 at `opts.quality`. Nothing but pixels is
/// carried over, so EXIF (including GPS), XMP and ICC data are all dropped. Images over 64
/// megapixels are refused.
///
/// Returns the JPEG and its width and height.
pub fn compress_image(bytes: &[u8], opts: ImageOpts) -> Result<(Vec<u8>, u32, u32)> {
    if opts.max_edge == 0 || !(1..=100).contains(&opts.quality) {
        return Err(Error::Invalid(format!(
            "image settings out of range: longest edge {} px (must be at least 1), quality {} (must be 1–100)",
            opts.max_edge, opts.quality
        )));
    }
    let image = DynamicImage::from(flatten_onto_white(decode(bytes).map_err(read_error)?));
    let (width, height) = fit(image.width(), image.height(), opts.max_edge);
    let image = if (width, height) == (image.width(), image.height()) {
        image
    } else {
        // Not the generic `imageops::resize`: that would be compiled as part of this crate,
        // unoptimised in debug builds and ~30x slower.
        image.resize_exact(width, height, FilterType::Lanczos3)
    };
    let jpeg = encode_jpeg(&image.into_rgb8(), opts.quality)?;
    Ok((jpeg, width, height))
}

fn decode(bytes: &[u8]) -> ImageResult<DynamicImage> {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::DimensionError,
        )));
    }
    // `ImageReader::decode` checks this too, but `into_decoder` leaves it to the caller.
    limits.reserve(decoder.total_bytes())?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn read_error(err: ImageError) -> Error {
    Error::Image(match err {
        ImageError::Unsupported(_) => {
            "unsupported image format; use JPEG, PNG, WebP or GIF".to_owned()
        }
        ImageError::Limits(_) => "the image is too large to import".to_owned(),
        err => format!("could not read the image: {err}"),
    })
}

fn flatten_onto_white(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (out, px) in rgb.pixels_mut().zip(rgba.pixels()) {
        let [r, g, b, a] = px.0;
        let over_white = |c: u8| {
            let (c, a) = (u16::from(c), u16::from(a));
            // Rounded (c·a + 255·(255 − a)) / 255, at most 255.
            ((c * a + 255 * (255 - a) + 127) / 255) as u8
        };
        *out = Rgb([over_white(r), over_white(g), over_white(b)]);
    }
    rgb
}

/// `width`×`height` scaled down, keeping the aspect ratio, so the longest edge is `max_edge`.
fn fit(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = |edge: u32| {
        let scaled =
            (u64::from(edge) * u64::from(max_edge) + u64::from(longest) / 2) / u64::from(longest);
        // At most max_edge, so it fits.
        scaled.max(1) as u32
    };
    (scale(width), scale(height))
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let too_large = |_| {
        Error::Image(
            "the image is too large to save as JPEG (65535 px per side at most)".to_owned(),
        )
    };
    let width = u16::try_from(image.width()).map_err(too_large)?;
    let height = u16::try_from(image.height()).map_err(too_large)?;
    let mut jpeg = Vec::new();
    let mut encoder = Encoder::new(&mut jpeg, quality);
    encoder.set_sampling_factor(SamplingFactor::R_4_2_0);
    encoder.set_chroma_subsampling_method(ChromaSubsamplingMethod::Average);
    encoder.set_progressive(true);
    encoder.set_optimized_huffman_tables(true);
    encoder
        .encode(image.as_raw(), width, height, ColorType::Rgb)
        .map_err(|err| Error::Image(format!("could not encode the image: {err}")))?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::metadata::Orientation;
    use image::{ImageFormat, Rgba, RgbaImage};

    const OPTS: ImageOpts = ImageOpts {
        max_edge: 1024,
        quality: 60,
    };

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    /// The marker and payload of every segment before the first scan.
    fn jpeg_segments(jpeg: &[u8]) -> Vec<(u8, &[u8])> {
        assert_eq!(&jpeg[..2], [0xFF, 0xD8], "missing SOI");
        let mut segments = Vec::new();
        let mut at = 2;
        loop {
            assert_eq!(jpeg[at], 0xFF, "expected a marker at {at}");
            let marker = jpeg[at + 1];
            let len = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
            segments.push((marker, &jpeg[at + 4..at + 2 + len]));
            if marker == 0xDA {
                return segments;
            }
            at += 2 + len;
        }
    }

    /// Asserts a progressive 4:2:0 JPEG whose only APPn segment is the JFIF header.
    fn assert_clean_progressive_420(jpeg: &[u8]) {
        let segments = jpeg_segments(jpeg);
        let app: Vec<_> = segments
            .iter()
            .filter(|(m, _)| (0xE0..=0xEF).contains(m))
            .collect();
        assert_eq!(
            app.len(),
            1,
            "unexpected APPn segments: {:?}",
            app.iter().map(|(m, _)| m).collect::<Vec<_>>()
        );
        assert_eq!(app[0].0, 0xE0);
        assert!(app[0].1.starts_with(b"JFIF\0"));
        assert!(
            !jpeg.windows(6).any(|w| w == b"Exif\0\0"),
            "EXIF data in output"
        );
        assert!(
            !segments.iter().any(|(m, _)| *m == 0xFE),
            "comment segment in output"
        );

        let (_, sof) = segments
            .iter()
            .find(|(m, _)| *m == 0xC2)
            .expect("not progressive (no SOF2)");
        assert_eq!(sof[5], 3, "expected 3 components");
        assert_eq!(sof[7], 0x22, "luma should be sampled 2x2 (4:2:0)");
        assert_eq!((sof[10], sof[13]), (0x11, 0x11));
    }

    /// A gradient with noise in 4×4 px blocks, which compresses about like a photo: like a
    /// photo's detail, it is still there at 1024 px (per-pixel noise would be averaged away).
    fn noisy_photo(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, y| {
            let mut seed = (x / 4).wrapping_mul(0x9E37_79B1) ^ (y / 4).wrapping_mul(0x85EB_CA77);
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed % 65) as i32 - 32;
            let channel = |v: u32| (v as i32 + noise).clamp(0, 255) as u8;
            Rgb([
                channel(x * 255 / width),
                channel(y * 255 / height),
                channel((x + y) * 128 / (width + height)),
            ])
        })
    }

    #[test]
    fn twelve_megapixel_photo_becomes_small_clean_jpeg() {
        let png = encode(
            &DynamicImage::ImageRgb8(noisy_photo(4000, 3000)),
            ImageFormat::Png,
        );
        let (jpeg, width, height) = compress_image(&png, OPTS).unwrap();
        assert_eq!((width, height), (1024, 768));
        assert!(jpeg.len() <= 200_000, "{} bytes", jpeg.len());
        assert!(
            jpeg.len() >= 60_000,
            "{} bytes: too easy to stand in for a photo",
            jpeg.len()
        );
        let decoded = image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 768));
        assert_clean_progressive_420(&jpeg);
    }

    /// A minimal big-endian TIFF block holding one IFD entry: Orientation = `value`.
    fn exif_orientation(value: u16) -> Vec<u8> {
        let mut tiff = b"MM\0\x2a".to_vec();
        tiff.extend(8_u32.to_be_bytes()); // offset of IFD0
        tiff.extend(1_u16.to_be_bytes()); // one entry
        tiff.extend(0x0112_u16.to_be_bytes()); // tag: Orientation
        tiff.extend(3_u16.to_be_bytes()); // type: SHORT
        tiff.extend(1_u32.to_be_bytes()); // count
        tiff.extend(value.to_be_bytes());
        tiff.extend([0, 0]); // padding of the 4-byte value field
        tiff.extend(0_u32.to_be_bytes()); // no next IFD
        tiff
    }

    /// 64×32, left half red, right half blue.
    fn red_blue() -> RgbImage {
        RgbImage::from_fn(64, 32, |x, _| {
            if x < 32 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        })
    }

    fn jpeg_with_orientation(image: &RgbImage, orientation: u16) -> Vec<u8> {
        let mut jpeg = Vec::new();
        let mut encoder = Encoder::new(&mut jpeg, 95);
        encoder
            .add_exif_metadata(&exif_orientation(orientation))
            .unwrap();
        let (w, h) = (image.width() as u16, image.height() as u16);
        encoder
            .encode(image.as_raw(), w, h, ColorType::Rgb)
            .unwrap();
        jpeg
    }

    fn is_red(px: &Rgb<u8>) -> bool {
        px[0] > 200 && px[2] < 60
    }

    fn is_blue(px: &Rgb<u8>) -> bool {
        px[2] > 200 && px[0] < 60
    }

    #[test]
    fn exif_orientation_is_applied_then_stripped() {
        let input = jpeg_with_orientation(&red_blue(), 6); // 6: rotate 90° clockwise to display
        let mut decoder = ImageReader::new(Cursor::new(&input))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(
            decoder.orientation().unwrap(),
            Orientation::Rotate90,
            "test input is wrong"
        );

        let (jpeg, width, height) = compress_image(&input, OPTS).unwrap();
        assert_eq!((width, height), (32, 64));
        let out = image::load_from_memory(&jpeg).unwrap().into_rgb8();
        assert_eq!(out.dimensions(), (32, 64));
        // Clockwise: the left (red) half ends up on top.
        assert!(is_red(out.get_pixel(16, 8)), "{:?}", out.get_pixel(16, 8));
        assert!(
            is_blue(out.get_pixel(16, 56)),
            "{:?}",
            out.get_pixel(16, 56)
        );
        assert_clean_progressive_420(&jpeg);
    }

    #[test]
    fn exif_orientation_mirror() {
        let input = jpeg_with_orientation(&red_blue(), 2); // 2: flip horizontally
        let (jpeg, width, height) = compress_image(&input, OPTS).unwrap();
        assert_eq!((width, height), (64, 32));
        let out = image::load_from_memory(&jpeg).unwrap().into_rgb8();
        assert!(is_blue(out.get_pixel(8, 16)) && is_red(out.get_pixel(56, 16)));
    }

    #[test]
    fn never_upscales() {
        let png = encode(&DynamicImage::ImageRgb8(red_blue()), ImageFormat::Png);
        let (_, width, height) = compress_image(&png, OPTS).unwrap();
        assert_eq!((width, height), (64, 32));
    }

    #[test]
    fn fit_keeps_aspect_ratio() {
        assert_eq!(fit(4000, 3000, 1024), (1024, 768));
        assert_eq!(fit(3000, 4000, 1024), (768, 1024));
        assert_eq!(fit(1024, 1024, 1024), (1024, 1024));
        assert_eq!(fit(1025, 1, 1024), (1024, 1));
        assert_eq!(fit(100_000, 1, 1024), (1024, 1));
        assert_eq!(fit(3, 2, 2), (2, 1)); // 1.33 rounds down
        assert_eq!(fit(5, 3, 2), (2, 1)); // 1.2 rounds down
        assert_eq!(fit(4, 3, 2), (2, 2)); // 1.5 rounds up
        assert_eq!(fit(10, 20, 1024), (10, 20));
    }

    #[test]
    fn alpha_is_flattened_onto_white() {
        let rgba = RgbaImage::from_fn(4, 1, |x, _| match x {
            0 => Rgba([0, 0, 0, 0]),
            1 => Rgba([0, 0, 0, 255]),
            2 => Rgba([0, 0, 0, 128]),
            _ => Rgba([200, 100, 50, 255]),
        });
        let rgb = flatten_onto_white(DynamicImage::ImageRgba8(rgba));
        let pixels: Vec<_> = rgb.pixels().map(|p| p.0).collect();
        assert_eq!(
            pixels,
            [[255, 255, 255], [0, 0, 0], [127, 127, 127], [200, 100, 50]]
        );
    }

    #[test]
    fn transparent_png_comes_out_white() {
        let clear = DynamicImage::ImageRgba8(RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 0])));
        let (jpeg, ..) = compress_image(&encode(&clear, ImageFormat::Png), OPTS).unwrap();
        let out = image::load_from_memory(&jpeg).unwrap().into_rgb8();
        assert!(out.pixels().all(|p| p.0.iter().all(|&c| c >= 250)));
    }

    #[test]
    fn decodes_every_supported_format() {
        let image = DynamicImage::ImageRgb8(red_blue());
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Gif,
        ] {
            let (jpeg, width, height) = compress_image(&encode(&image, format), OPTS).unwrap();
            assert_eq!((width, height), (64, 32), "{format:?}");
            assert_clean_progressive_420(&jpeg);
        }
    }

    #[test]
    fn grayscale_input() {
        let gray =
            DynamicImage::ImageLuma16(image::ImageBuffer::from_pixel(8, 8, image::Luma([40_000])));
        let (jpeg, ..) = compress_image(&encode(&gray, ImageFormat::Png), OPTS).unwrap();
        assert_clean_progressive_420(&jpeg);
    }

    #[test]
    fn rejects_non_images() {
        let err = compress_image(b"definitely not an image", OPTS).unwrap_err();
        assert!(matches!(err, Error::Image(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "unsupported image format; use JPEG, PNG, WebP or GIF"
        );

        let png = encode(&DynamicImage::ImageRgb8(red_blue()), ImageFormat::Png);
        let err = compress_image(&png[..png.len() / 2], OPTS).unwrap_err();
        assert!(
            err.to_string().starts_with("could not read the image: "),
            "{err}"
        );
    }

    #[test]
    fn refuses_huge_images_before_decoding() {
        // 72 MP of grey: a PNG of about 70 KB that would decode to over 200 MB of RGB.
        let gray = DynamicImage::ImageLuma8(image::GrayImage::new(9000, 8000));
        let err = compress_image(&encode(&gray, ImageFormat::Png), OPTS).unwrap_err();
        assert!(matches!(err, Error::Image(_)), "{err:?}");
        assert_eq!(err.to_string(), "the image is too large to import");
    }

    #[test]
    fn rejects_bad_options() {
        let png = encode(&DynamicImage::ImageRgb8(red_blue()), ImageFormat::Png);
        for opts in [
            ImageOpts {
                max_edge: 0,
                quality: 60,
            },
            ImageOpts {
                max_edge: 1024,
                quality: 0,
            },
            ImageOpts {
                max_edge: 1024,
                quality: 101,
            },
        ] {
            assert!(
                matches!(compress_image(&png, opts), Err(Error::Invalid(_))),
                "{opts:?}"
            );
        }
    }
}
