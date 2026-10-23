use alloc::vec::Vec;
use alloyos_mcp::tile_images::{self, ImageAsset, MAX_IMAGE_DIMENSION, MAX_IMAGES};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};

#[derive(Default)]
pub struct ImageCache(Vec<(ImageAsset, Image)>);
impl ImageCache {
    pub fn update(&mut self, assets: &[ImageAsset]) -> bool {
        let assets = &assets[..assets.len().min(MAX_IMAGES)];
        if self.0.iter().map(|a| &a.0).eq(assets.iter()) { return false; }
        let old = core::mem::take(&mut self.0);
        self.0 = assets.iter().map(|asset| {
            let image = old.iter().find(|a| &a.0 == asset).map(|a| a.1.clone())
                .unwrap_or_else(|| decode_image(&asset.data).unwrap_or_default());
            (asset.clone(), image)
        }).collect();
        true
    }
    pub fn get(&self, src: &str) -> Image {
        self.0.iter().find(|a| a.0.src == src).map(|a| a.1.clone()).unwrap_or_default()
    }
}

fn decode_image(hex: &str) -> Option<Image> {
    let bytes = tile_images::decode(hex)?;
    let options = DecoderOptions::new_safe().set_max_width(MAX_IMAGE_DIMENSION)
        .set_max_height(MAX_IMAGE_DIMENSION).png_set_strip_to_8bit(true)
        .png_set_add_alpha_channel(true).jpeg_set_out_colorspace(ColorSpace::RGBA);
    let (width, height, rgba) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut decoder = zune_png::PngDecoder::new_with_options(ZCursor::new(&bytes), options);
        decoder.decode_headers().ok()?;
        let (w, h) = decoder.dimensions()?;
        let components = decoder.colorspace()?.num_components();
        let raw = decoder.decode_raw().ok()?;
        let rgba = match components {
            4 => raw,
            2 => raw.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            _ => return None,
        };
        (w, h, rgba)
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&bytes), options);
        decoder.decode_headers().ok()?;
        let (w, h) = decoder.dimensions()?;
        (w, h, decoder.decode().ok()?)
    } else { return None; };
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION
        || rgba.len() != width.checked_mul(height)?.checked_mul(4)? { return None; }
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width as u32, height as u32);
    pixels.make_mut_bytes().copy_from_slice(&rgba);
    Some(Image::from_rgba8(pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_alpha_bad_input_and_dimension_limit() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&[255,0,0,255,0,255,0,128]).unwrap();
        }
        let size = decode_image(&tile_images::encode(&bytes)).unwrap().size();
        assert_eq!((size.width, size.height), (2,1));
        assert!(decode_image("000102").is_none());
        bytes[16..20].copy_from_slice(&513u32.to_be_bytes());
        assert!(decode_image(&tile_images::encode(&bytes)).is_none());
    }
}
