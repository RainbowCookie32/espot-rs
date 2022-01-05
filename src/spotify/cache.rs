use rspotify::model::FullTrack;
use serde::{Deserialize, Serialize};

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
