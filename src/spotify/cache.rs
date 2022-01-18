use std::path::PathBuf;
use std::collections::HashMap;

use tokio::fs;
use reqwest::Client;
use rspotify::model::FullTrack;
use serde::{Deserialize, Serialize};

pub struct CacheHandler {
    http_client: Client,

    cache_dir: PathBuf,
    cached_tracks: HashMap<String, TrackInfo>,
}

impl CacheHandler {
    pub fn init(cache_dir: PathBuf) -> CacheHandler {
        let http_client = Client::new();

        let cache_data_path = cache_dir.join("tracks.ron");
        let cache_data = std::fs::read_to_string(&cache_data_path).unwrap_or_default();
        let cached_tracks = ron::from_str(&cache_data).unwrap_or_default();

        CacheHandler {
            http_client,

            cache_dir,
            cached_tracks
        }
    }

    pub fn get_track_info(&self, id: &str) -> Option<TrackInfo> {
        if let Some(track) = self.cached_tracks.get(id) {
            // This should already be there, but making sure never killed anyone.
            futures_lite::future::block_on(self.cache_cover_image(&track.album_id, &track.album_images));
            
            Some(track.clone())
        }
        else {
            None
        }
    }

    pub fn cache_track_info(&mut self, track: FullTrack) -> Option<TrackInfo> {
        if let Some(track) = TrackInfo::new(track) {
            let id = track.id.clone();

            futures_lite::future::block_on(self.cache_cover_image(&track.album_id, &track.album_images));
            self.cached_tracks.insert(id, track.clone());

            Some(track)
        }
        else {
            None
        }
    }

    pub async fn cache_cover_image(&self, id: &str, images: &[(u32, String)]) {
        let path = self.cache_dir.join(format!("cover-{}", id));
    
        if !path.exists() {
            for (size, url) in images.iter() {
                // Spotify doesn't include size data for some images for some reason,
                // so because of uwrap_or_default(), a properly sized image might be 0 here.
                if *size == 0 || *size == 300 {
                    if let Ok(res) = self.http_client.get(url).send().await {
                        let bytes = res.bytes().await.unwrap_or_default().to_vec();
    
                        if !bytes.is_empty() {
                            if let Err(e) = fs::write(&path, bytes).await {
                                println!("error writing cover file: {}", e);
                            }
                        }
                    }
    
                    break;
                }
            }
        }
    }

    pub async fn save_cache(&self) {
        if let Ok(data) = ron::ser::to_string_pretty(&self.cached_tracks, ron::ser::PrettyConfig::default()) {
            if let Err(e) = fs::write(self.cache_dir.join("tracks.ron"), data).await {
                println!("Error saving api cache: {}", e);
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TrackInfo {
    pub id: String,

    pub name: String,
    pub duration_ms: u128,

    pub artists: Vec<String>,

    pub album_id: String,
    pub album_name: String,
    // Size, url.
    pub album_images: Vec<(u32, String)>
}

impl TrackInfo {
    pub fn new(track: FullTrack) -> Option<TrackInfo> {
        let track_id = track.id?;
        let id = track_id.to_string();
        
        let name = track.name;
        let duration_ms = track.duration.as_millis();

        let artists = track.artists.into_iter().map(| a | a.name).collect();

        let album_id = track.album.id?.to_string();
        let album_name = track.album.name;
        let album_images = track.album.images.into_iter().map(| i | (i.height.unwrap_or_default(), i.url)).collect();

        let track_info = TrackInfo {
            id,

            name,
            duration_ms,

            artists,

            album_id,
            album_name,
            album_images
        };

        Some(track_info)
    }
}
