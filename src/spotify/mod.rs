mod cache;

use std::collections::HashMap;

use reqwest::Client;
use nanorand::{Rng, WyRand};
use futures_lite::StreamExt;

use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc};

use librespot::core::session::Session;
use librespot::core::config::SessionConfig;
use librespot::core::spotify_id::SpotifyId;
use librespot::core::authentication::Credentials;

use librespot::playback::player::{Player, PlayerEvent};
use librespot::metadata::{Playlist, Metadata};

use rspotify::auth_code::AuthCodeSpotify;
use rspotify::model::{Id, TrackId, PlaylistId, PlayableId};
use rspotify::clients::{OAuthClient, BaseClient};

pub use cache::TrackInfo;


type TaskTx = mpsc::UnboundedSender<WorkerTask>;
type TaskRx = mpsc::UnboundedReceiver<WorkerTask>;

type TaskResultTx = mpsc::UnboundedSender<WorkerResult>;
type TaskResultRx = mpsc::UnboundedReceiver<WorkerResult>;

type StateTx = broadcast::Sender<PlayerStateUpdate>;
type StateRx = broadcast::Receiver<PlayerStateUpdate>;

type ControlTx = mpsc::UnboundedSender<PlayerControl>;
type ControlRx = mpsc::UnboundedReceiver<PlayerControl>;


#[derive(Debug)]
pub enum WorkerTask {
    Login(String, String),
    
    GetUserPlaylists,
    GetPlaylistTracksInfo(Playlist),

    RemoveTrackFromPlaylist(String, String)
}

#[derive(Debug)]
pub enum WorkerResult {
    Login(bool),
    Playlists(Vec<(String, Playlist)>),
    PlaylistTrackInfo(Vec<TrackInfo>)
}

#[derive(Debug)]
pub enum PlayerControl {
    Play,
    Pause,
    Stop,
    PlayPause,

    StartPlaylist(Vec<TrackInfo>),
    StartPlaylistAtTrack(Vec<TrackInfo>, TrackInfo),

    NextTrack,
    PreviousTrack
}

#[derive(Clone, Debug)]
pub enum PlayerStateUpdate {
    Paused,
    Resumed,
    Stopped,
    EndOfTrack(TrackInfo)
}

pub struct SpotifyWorker {
    http_client: Client,

    api_cache: HashMap<String, TrackInfo>,
    api_client: Option<AuthCodeSpotify>,

    spotify_player: Option<Player>,
    spotify_session: Option<Session>,

    state_tx: StateTx,
    control_rx: ControlRx,

    worker_task_rx: TaskRx,
    worker_result_tx: TaskResultTx,

    player_paused: bool,
    player_current_track: usize,
    player_tracks_queue: Vec<TrackInfo>
}

impl SpotifyWorker {
    pub fn start() -> (TaskTx, TaskResultRx, StateRx, StateRx, ControlTx) {
        let http_client = Client::new();

        let (state_tx, state_rx) = broadcast::channel(5);
        let (control_tx, control_rx) = mpsc::unbounded_channel();

        let (worker_task_tx, worker_task_rx) = mpsc::unbounded_channel();
        let (worker_result_tx, worker_result_rx) = mpsc::unbounded_channel();

        let cache_dir = dirs::cache_dir().unwrap().join("espot-rs");

        let state_rx_2 = state_tx.subscribe();

        if let Err(err) = std::fs::create_dir_all(&cache_dir) {
            match err.kind() {
                std::io::ErrorKind::AlreadyExists => {},
                _ => panic!("failed to create cache directory")
            }
        }

        let api_cache = ron::from_str(&std::fs::read_to_string(&cache_dir.join("tracks.ron")).unwrap_or_default()).unwrap_or_default();

        let worker = SpotifyWorker {
            http_client,

            api_client: None,
            api_cache,

            spotify_player: None,
            spotify_session: None,

            state_tx,
            control_rx,

            worker_task_rx,
            worker_result_tx,

            player_paused: true,
            player_current_track: 0,
            player_tracks_queue: Vec::new()
        };

        std::thread::spawn(move || {
            let rt = Runtime::new().unwrap();
            let mut worker = worker;

            rt.block_on(worker.process_events());
        });

        (worker_task_tx, worker_result_rx, state_rx, state_rx_2, control_tx)
    }

    pub async fn process_events(&mut self) {
        let mut rng = WyRand::new();
        let mut player_events = None;

        loop {
            if let Ok(task) = self.worker_task_rx.try_recv() {
                match task {
                    WorkerTask::Login(username, password) => {
                        let (result, rx) = self.login_task(username, password).await;
                        
                        player_events = rx;
                        self.worker_result_tx.send(WorkerResult::Login(result)).unwrap();
                    }
                    WorkerTask::GetUserPlaylists => {
                        let result = self.fetch_playlists_task().await;
                        self.worker_result_tx.send(WorkerResult::Playlists(result)).unwrap();
                    }
                    WorkerTask::GetPlaylistTracksInfo(playlist) => {
                        self.fetch_playlist_tracks_info_task(playlist).await;
                    }
                    WorkerTask::RemoveTrackFromPlaylist(playlist, track) => {
                        self.remove_track_from_playlist_task(playlist, track).await;
                    }
                }
            }

            if let Ok(control) = self.control_rx.try_recv() {
                match control {
                    PlayerControl::Play => {
                        if let Some(player) = self.spotify_player.as_ref() {
                            player.play();
                            self.state_tx.send(PlayerStateUpdate::Resumed).unwrap();
                        }
                    }
                    PlayerControl::Pause => {
                        if let Some(player) = self.spotify_player.as_ref() {
                            player.pause();
                            self.state_tx.send(PlayerStateUpdate::Paused).unwrap();
                        }
                    }
                    PlayerControl::Stop => {
                        if let Some(player) = self.spotify_player.as_ref() {
                            player.stop();
                            self.state_tx.send(PlayerStateUpdate::Stopped).unwrap();
                        }
                    }
                    PlayerControl::PlayPause => {
                        if let Some(player) = self.spotify_player.as_ref() {
                            if self.player_paused {
                                player.play();
                                self.state_tx.send(PlayerStateUpdate::Resumed).unwrap();
                            }
                            else {
                                player.pause();
                                self.state_tx.send(PlayerStateUpdate::Paused).unwrap();
                            }
                        }
                    }
                    PlayerControl::StartPlaylist(mut tracks) => {
                        rng.shuffle(&mut tracks);
                        self.start_playlist_task(tracks);
                        
                    }
                    PlayerControl::StartPlaylistAtTrack(mut tracks, start) => {
                        let mut idx = 0;
                        rng.shuffle(&mut tracks);

                        for (i, track) in tracks.iter().enumerate() {
                            if track.id == start.id {
                                idx = i;
                                break;
                            }
                        }

                        self.start_playlist_at_idx_task(tracks, idx);
                    }
                    PlayerControl::NextTrack => {
                        if let Some(player) = self.spotify_player.as_mut() {
                            self.player_current_track += 1;

                            if self.player_current_track >= self.player_tracks_queue.len() {
                                self.player_current_track = 0;
                            }

                            let track = &self.player_tracks_queue[self.player_current_track];
                            let track_id = SpotifyId::from_uri(&track.id).unwrap();

                            self.state_tx.send(PlayerStateUpdate::EndOfTrack(track.clone())).unwrap();

                            player.load(track_id, true, 0);
                        }
                    }
                    PlayerControl::PreviousTrack => {
                        if let Some(player) = self.spotify_player.as_mut() {
                            if self.player_current_track == 0 {
                                self.player_current_track = self.player_tracks_queue.len() - 1;
                            }
                            else {
                                self.player_current_track -= 1;
                            }

                            let track = &self.player_tracks_queue[self.player_current_track];
                            let track_id = SpotifyId::from_uri(&track.id).unwrap();

                            self.state_tx.send(PlayerStateUpdate::EndOfTrack(track.clone())).unwrap();

                            player.load(track_id, true, 0);
                        }
                    }
                }
            }

            if let Some(events_rx) = player_events.as_mut() {
                if let Ok(event) = events_rx.try_recv() {
                    match event {
                        PlayerEvent::Paused { .. } => {
                            self.player_paused = true;
                        }
                        PlayerEvent::Playing { .. } | PlayerEvent::Started { .. } => {
                            self.player_paused = false;
                        }
                        PlayerEvent::TimeToPreloadNextTrack { .. } => {
                            if !self.player_tracks_queue.is_empty() {
                                let target = {
                                    if self.player_current_track + 1 >= self.player_tracks_queue.len() {
                                        0
                                    }
                                    else {
                                        self.player_current_track + 1
                                    }
                                };

                                let track = &self.player_tracks_queue[target];
                                let track_id = SpotifyId::from_uri(&track.id).unwrap();
    
                                if let Some(player) = self.spotify_player.as_ref() {
                                    player.preload(track_id)
                                }
                            }
                        }
                        PlayerEvent::EndOfTrack { .. } => {
                            self.player_current_track += 1;

                            if self.player_current_track >= self.player_tracks_queue.len() {
                                self.player_current_track = 0;
                            }

                            if let Some(player) = self.spotify_player.as_mut() {
                                let track = &self.player_tracks_queue[self.player_current_track];
                                let track_id = SpotifyId::from_uri(&track.id).unwrap();

                                self.state_tx.send(PlayerStateUpdate::EndOfTrack(track.clone())).unwrap();

                                player.load(track_id, true, 0);
                                player.play();
                            }
                        }
                        _ => {}
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub async fn login_task(&mut self, username: String, password: String) -> (bool, Option<mpsc::UnboundedReceiver<PlayerEvent>>) {
        let session_cfg = SessionConfig::default();
        let session_creds = Credentials::with_password(username, password);

        if let Some(api_creds) = rspotify::Credentials::from_env() {
            let api_cfg = rspotify::Config {
                token_cached: true,
                token_refreshing: true,
                ..Default::default()
            };

            if let Some(api_oauth) = rspotify::OAuth::from_env(rspotify::scopes!("playlist-read-private")) {
                let mut api_client = AuthCodeSpotify::with_config(api_creds, api_oauth, api_cfg);
                let url = api_client.get_authorize_url(false).unwrap_or_default();

                if api_client.prompt_for_token(&url).await.is_ok() {
                    self.api_client = Some(api_client);

                    if let Ok(session) = Session::connect(session_cfg, session_creds, None).await {
                        let player_cfg = librespot::playback::config::PlayerConfig::default();
                        let (player, rx) = Player::new(player_cfg, session.clone(), None, move || {
                            librespot::playback::audio_backend::find(None).unwrap()(None, librespot::playback::config::AudioFormat::default())
                        });

                        self.spotify_player = Some(player);
                        self.spotify_session = Some(session);

                        return (true, Some(rx));
                    }
                }
            }
        }

        (false, None)
    }

    pub async fn fetch_playlists_task(&mut self) -> Vec<(String, Playlist)> {
        let mut result = Vec::new();

        if let Some(client) = self.api_client.as_ref() {
            let mut playlists = client.current_user_playlists();

            if let Some(session) = self.spotify_session.as_ref() {
                while let Ok(item) = playlists.try_next().await {
                    if let Some(playlist) = item {
                        let id = SpotifyId::from_uri(&playlist.id.to_string()).unwrap();
    
                        if let Ok(p) = Playlist::get(session, id).await {
                            result.push((playlist.id.to_string(), p));
                        }
                    }
                    else {
                        break;
                    }
                }
            }
        }

        result
    }

    pub async fn fetch_playlist_tracks_info_task(&mut self, playlist: Playlist) {
        if let Some(client) = self.api_client.as_ref() {
            let cache_dir = dirs::cache_dir().unwrap().join("espot-rs");

            let mut tracks = Vec::with_capacity(playlist.tracks.len());
            let mut cache_modified = false;

            for track_id in playlist.tracks {
                if let Some(track) = self.api_cache.get(&track_id.to_uri()) {
                    tracks.push(track.clone());
                }
                else {
                    let track_id = TrackId::from_id(&track_id.to_base62()).unwrap();

                    if let Ok(track) = client.track(&track_id).await {
                        if let Some(track) = TrackInfo::new(track) {
                            let album_id = &track.album_id;
                            let cover_path = cache_dir.join(format!("cover-{}", album_id));

                            if std::fs::File::open(&cover_path).is_err() {
                                for (size, url) in track.album_images.iter() {
                                    if *size == 300 {
                                        if let Ok(res) = self.http_client.get(url).send().await {
                                            let bytes = res.bytes().await.unwrap_or_default().to_vec();
        
                                            if !bytes.is_empty() {
                                                if let Err(e) = std::fs::write(&cover_path, bytes) {
                                                    println!("error writing cover file: {}", e.to_string());
                                                }
                                            }
                                        }
        
                                        break;
                                    }
                                }
                            }
                            
                            cache_modified = true;
                            tracks.push(track.clone());

                            self.api_cache.insert(track_id.uri(), track);
                        }
                    }
                }
            }

            if cache_modified {
                if let Ok(data) = ron::ser::to_string_pretty(&self.api_cache, ron::ser::PrettyConfig::default()) {
                    if let Err(e) = tokio::fs::write(cache_dir.join("tracks.ron"), data).await {
                        println!("Error saving api cache: {}", e.to_string());
                    }
                }
            }

            self.worker_result_tx.send(WorkerResult::PlaylistTrackInfo(tracks)).unwrap();
        }
    }

    pub fn start_playlist_task(&mut self, tracks: Vec<TrackInfo>) {
        self.player_current_track = 0;
        self.player_tracks_queue = tracks;

        if let Some(player) = self.spotify_player.as_mut() {
            let track = &self.player_tracks_queue[self.player_current_track];
            let track_id = SpotifyId::from_uri(&track.id).unwrap();

            player.load(track_id, true, 0);
            self.state_tx.send(PlayerStateUpdate::EndOfTrack(track.clone())).unwrap();
        }
    }

    pub fn start_playlist_at_idx_task(&mut self, tracks: Vec<TrackInfo>, idx: usize) {
        self.player_current_track = idx;
        self.player_tracks_queue = tracks;

        if let Some(player) = self.spotify_player.as_mut() {
            let track = &self.player_tracks_queue[idx];
            let track_id = SpotifyId::from_uri(&track.id).unwrap();

            player.load(track_id, true, 0);
            self.state_tx.send(PlayerStateUpdate::EndOfTrack(track.clone())).unwrap();
        }
    }

    pub async fn remove_track_from_playlist_task(&mut self, playlist: String, track: String) {
        if let Some(api_client) = self.api_client.as_mut() {
            let playlist_id = PlaylistId::from_uri(&playlist).unwrap();
            let track_id = TrackId::from_uri(&track).unwrap();
            let track_ids: Vec<&dyn PlayableId> = vec![&track_id];

            if let Err(e) = api_client.playlist_remove_all_occurrences_of_items(&playlist_id, track_ids, None).await {
                println!("error removing track from playlist: {}", e.to_string());
            }
        }
    }
}