use std::path::PathBuf;

use eframe::egui::{Context, ColorImage, TextureHandle};


pub fn create_texture_from_file(ctx: &Context, path: PathBuf) -> Option<TextureHandle> {
    let buffer = std::fs::read(path).ok()?;
    create_texture_from_bytes(ctx, &buffer)
}

pub fn create_texture_from_bytes(ctx: &Context, buffer: &[u8]) -> Option<TextureHandle> {
    let image = image::load_from_memory(buffer).ok()?;

    let image_buf = image.to_rgba8();
    let image_size = [image.width() as usize, image.height() as usize];
    let image_pixels = image_buf.into_vec();

    let image = ColorImage::from_rgba_unmultiplied(image_size, &image_pixels);

    Some(ctx.load_texture("texture", image))
}

pub fn make_artists_string(artists: &[String]) -> String {
    let mut result = String::new();

    for (i, artist) in artists.iter().enumerate() {
        result.push_str(artist);

        if i != artists.len() - 1 {
            result.push_str(", ");
        }
    }

    result
}

pub fn trim_string(available_width: f32, glyph_width: f32, text: &mut String) -> bool {
    let char_count = text.chars().count();

    let max_chars = (available_width / glyph_width) as usize;
    let should_trim = char_count >= max_chars;

    if should_trim {
        let mut t = text.chars().collect::<Vec<char>>();
        t.truncate(max_chars - 3);

        *text = t.into_iter().collect::<String>();
        text.push_str("...");
    }

    should_trim
}
