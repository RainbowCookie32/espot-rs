use std::path::PathBuf;

use eframe::egui::TextureId;
use eframe::epi::{Frame, Image};

use image::GenericImageView;


pub fn create_texture_from_file(frame: &Frame, path: PathBuf) -> Option<TextureId> {
    // let cover_path = cache_path.join(format!("cover-{}", id));

    let buffer = std::fs::read(path).ok()?;
    create_texture_from_bytes(frame, &buffer)
}

pub fn create_texture_from_bytes(frame: &Frame, buffer: &[u8]) -> Option<TextureId> {
    let image = image::load_from_memory(buffer).ok()?;

    let image_buf = image.to_rgba8();
    let image_size = [image.width() as usize, image.height() as usize];
    let image_pixels = image_buf.into_vec();

    let image = Image::from_rgba_unmultiplied(image_size, &image_pixels);
    Some(frame.alloc_texture(image))
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
    let mut text_chars: Vec<char> = text.chars().collect();

    let mut trimmed = false;

    let max_chars_in_space = (available_width / glyph_width) as usize;

    if text_chars.len() >= max_chars_in_space {
        while text_chars.len() >= max_chars_in_space {
            text_chars.pop();
        }
        
        *text = text_chars.into_iter().collect();
        text.push_str("...");

        trimmed = true;
    }

    trimmed
}