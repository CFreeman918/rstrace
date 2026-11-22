//! Load a photo as RGBA8, applying EXIF orientation and capping its size.

use image::{DynamicImage, ImageDecoder, ImageReader};
use std::path::Path;

/// Photos larger than this (long edge) are downscaled; detection and tracing
/// gain nothing beyond it and memory use stays reasonable.
pub const MAX_EDGE: u32 = 5000;

pub struct Photo {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub fn load_photo(path: &Path) -> Result<Photo, String> {
    let reader = ImageReader::open(path)
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("Unsupported image: {e}"))?;
    let orientation = decoder.orientation().ok();
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| format!("Cannot decode image: {e}"))?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(from_dynamic(img))
}

pub fn from_dynamic(mut img: DynamicImage) -> Photo {
    if img.width().max(img.height()) > MAX_EDGE {
        img = img.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle);
    }
    let rgba = img.into_rgba8();
    Photo {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    }
}
