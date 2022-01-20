mod cache;
mod error;

use nanorand::{Rng, WyRand};
use futures_lite::StreamExt;

use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc};

use librespot::core::session::Session;
use librespot::core::config::SessionConfig;
use librespot::core::spotify_id::SpotifyId;
use librespot::core::authentication::Credentials;

use librespot::playback::config;
use librespot::playback::player::{Player, PlayerEvent};

use rspotify::auth_code::AuthCodeSpotify;
use librespot::metadata::{Playlist, Metadata};
use rspotify::clients::{OAuthClient, BaseClient};
use rspotify::model::{Id, TrackId, PlaylistId, PlayableId, ArtistId, SimplifiedPlaylist};

use cache::CacheHandler;
pub use cache::TrackInfo;


type TaskTx = mpsc::UnboundedSender<WorkerTask>;
type TaskRx = mpsc::UnboundedReceiver<WorkerTask>;

type TaskResultTx = mpsc::UnboundedSender<WorkerResult>;
type TaskResultRx = mpsc::UnboundedReceiver<WorkerResult>;

type StateTx = broadcast::Sender<PlayerStateUpdate>;
type StateRx = broadcast::Receiver<PlayerStateUpdate>;

type ControlTx = mpsc::UnboundedSender<PlayerControl>;
type ControlRx = mpsc::UnboundedReceiver<PlayerControl>;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;


#[derive(Debug)]
pub enum WorkerTask {
    Login(String, String),
    
    GetUserPlaylists,
    GetFeaturedPlaylists,
    GetPlaylistTracksInfo(Playlist),
    GetRecommendationsForPlaylist(Playlist),

    AddTrackToPlaylist(String, String),
    RemoveTrackFromPlaylist(String, String)
}

#[derive(Debug)]
pub enum WorkerResult {
    Login(bool),

    UserPlaylists(Vec<(String, Playlist)>),
    FeaturedPlaylists(Vec<(String, Playlist)>),

    PlaylistTrackInfo(Vec<TrackInfo>),
    PlaylistRecommendations(Vec<TrackInfo>)
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
    api_client: Option<AuthCodeSpotify>,
    api_cache_handler: CacheHandler,

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
        let cache_dir = dirs::cache_dir().unwrap().join("espot-rs");

        let (state_tx, state_rx) = broadcast::channel(5);
        let (control_tx, control_rx) = mpsc::unbounded_channel();

        let (worker_task_tx, worker_task_rx) = mpsc::unbounded_channel();
        let (worker_result_tx, worker_result_rx) = mpsc::unbounded_channel();

        let state_rx_2 = state_tx.subscribe();

        if let Err(err) = std::fs::create_dir_all(&cache_dir.join("audio")) {
            match err.kind() {
                std::io::ErrorKind::AlreadyExists => {},
                _ => panic!("failed to create cache directory")
            }
        }

        let api_cache_handler = CacheHandler::init(cache_dir);

        let worker = SpotifyWorker {
            api_client: None,
            api_cache_handler,

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
                        let mut result = false;

                        if let Ok(rx) = self.login_task(username, password).await {
                            result = true;
                            player_events = Some(rx);
                        }
                        
                        // TODO: Pass the error to the UI and show to user.
                        self.worker_result_tx.send(WorkerResult::Login(result)).unwrap();
                    }
                    WorkerTask::GetUserPlaylists => {
                        if let Ok(result) = self.fetch_user_playlists_task().await {
                            // TODO: Pass the error to the UI and show to user.
                            self.worker_result_tx.send(WorkerResult::UserPlaylists(result)).unwrap();
                        }
                    }
                    WorkerTask::GetFeaturedPlaylists => {
                        if let Ok(result) = self.fetch_featured_playlists_task().await {
                            // TODO: Pass the error to the UI and show to user.
                            self.worker_result_tx.send(WorkerResult::FeaturedPlaylists(result)).unwrap();
                        }
                    }
                    WorkerTask::GetPlaylistTracksInfo(playlist) => {
                        if self.fetch_playlist_tracks_info_task(playlist).await.is_err() {
                            // TODO: Pass the error to the UI and show to user.
                        }
                    }
                    WorkerTask::GetRecommendationsForPlaylist(playlist) => {
                        if let Ok(result) = self.get_recommendations_task(playlist, &mut rng).await {
                            // TODO: Pass the error to the UI and show to user.
                            self.worker_result_tx.send(WorkerResult::PlaylistRecommendations(result)).unwrap();
                        }
                    }
                    WorkerTask::AddTrackToPlaylist(track, playlist) => {
                        if self.add_track_to_playlist_task(track, playlist).await.is_err() {
                            // TODO: Pass the error to the UI and show to user.
                        }
                    }
                    WorkerTask::RemoveTrackFromPlaylist(track, playlist) => {
                        if self.remove_track_from_playlist_task(track, playlist).await.is_err() {
                            // TODO: Pass the error to the UI and show to user.
                        }
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
                        
                        if self.start_playlist_task(tracks).is_err() {
                            // TODO: Pass the error to the UI and show to user.
                        }
                    }
                    PlayerControl::StartPlaylistAtTrack(mut tracks, start) => {
                        let mut idx = 0;
                        rng.shuffle(&mut tracks);

                        if let Some((i, _)) = tracks.iter().enumerate().find(| (_, track) | track.id == start.id) {
                            idx = i;
                        } 

                        if self.start_playlist_at_idx_task(tracks, idx).is_err() {
                            // TODO: Pass the error to the UI and show to user.
                        }
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

    pub async fn login_task(&mut self, username: String, password: String) -> Result<mpsc::UnboundedReceiver<PlayerEvent>> {
        let session_cfg = SessionConfig::default();
        let session_creds = Credentials::with_password(username, password);

        let mut api_client = {
            let api_creds = rspotify::Credentials::from_env().ok_or(error::APILoginError::Credentials)?;
            let api_cfg = rspotify::Config {
                token_cached: true,
                token_refreshing: true,
                ..Default::default()
            };

            let api_oauth = rspotify::OAuth::from_env(rspotify::scopes!("playlist-read-private")).ok_or(error::APILoginError::OAuth)?;

            AuthCodeSpotify::with_config(api_creds, api_oauth, api_cfg)
        };

        let url = api_client.get_authorize_url(false).unwrap_or_default();
        
        if api_client.prompt_for_token(&url).await.is_ok() {
            let player_cfg = config::PlayerConfig {
                gapless: true,
                normalisation_type: config::NormalisationType::Auto,
                normalisation_method: config::NormalisationMethod::Dynamic,
                ..Default::default()
            };

            let cache = {
                let cache_dir = dirs::cache_dir().unwrap().join("espot-rs");
                let system_location = Some(cache_dir.join("system"));
                let audio_location = Some(cache_dir.join("audio"));
                
                librespot::core::cache::Cache::new(system_location, audio_location, None).ok()
            };
            
            let session = Session::connect(session_cfg, session_creds, cache).await?;

            let (player, rx) = Player::new(player_cfg, session.clone(), None, move || {
                librespot::playback::audio_backend::find(None).unwrap()(None, config::AudioFormat::default())
            });

            self.api_client = Some(api_client);
            self.spotify_player = Some(player);
            self.spotify_session = Some(session);

            Ok(rx)
        }
        else {
            Err(Box::new(error::APILoginError::Token))
        }
    }

    pub async fn fetch_user_playlists_task(&mut self) -> Result<Vec<(String, Playlist)>> {        
        let client = self.api_client.as_ref().ok_or(error::WorkerError::NoAPIClient)?;
        let playlists = client.current_user_playlists_manual(None, None).await?;

        self.process_playlist_info(playlists.items).await
    }

    pub async fn fetch_featured_playlists_task(&mut self) -> Result<Vec<(String, Playlist)>> {
        let client = self.api_client.as_ref().ok_or(error::WorkerError::NoAPIClient)?;
        let featured = client.featured_playlists(None, None, None, Some(5), None).await?;

        self.process_playlist_info(featured.playlists.items).await
    }

    pub async fn fetch_playlist_tracks_info_task(&mut self, playlist: Playlist) -> Result<()> {
        let track_ids = playlist.tracks.into_iter().map(|t| t.to_uri()).collect();
        let tracks = self.make_track_info_vec(track_ids).await?;

        self.worker_result_tx.send(WorkerResult::PlaylistTrackInfo(tracks)).unwrap();
        Ok(())
    }

    pub async fn get_recommendations_task(&mut self, playlist: Playlist, rng: &mut WyRand) -> Result<Vec<TrackInfo>> {
        let client = self.api_client.as_ref().ok_or(error::WorkerError::NoAPIClient)?;

        let mut playlist_tracks: Vec<TrackId> = playlist.tracks
            .into_iter()
            .filter_map(| id | {
                TrackId::from_uri(&id.to_uri()).ok()
            })
            .collect()
        ;

        // The max amount of tracks for seeding you can use is 5, so shuffle them around
        // and then grab the first 5 elements for our recommendation adventures.
        rng.shuffle(&mut playlist_tracks);
        playlist_tracks.truncate(5);

        let seed_artists: Option<&Vec<ArtistId>> = None;
        let seed_genres: Option<Vec<&str>> = None;

        let results = client.recommendations(
            None,
            seed_artists,
            seed_genres,
            Some(&playlist_tracks),
            None,
            Some(50)
        ).await.unwrap();

        let tracks = results.tracks
            .into_iter()
            .filter_map(| t | t.id)
            .map(| id | id.uri())
            .collect()
        ;

        self.make_track_info_vec(tracks).await
    }

    async fn get_tracks_info(&mut self, tracks: &[TrackId]) -> Result<Vec<TrackInfo>> {
        let client = self.api_client.as_ref().ok_or(error::WorkerError::NoAPIClient)?;

        let mut cache_dirty = false;
        let mut result = Vec::with_capacity(tracks.len());

        // The tracks endpoint accepts a maximum of 50 tracks at a time.
        for tracks_batch in tracks.chunks(50) {
            let api_response = client.tracks(&tracks_batch.to_vec(), None).await?;

            for track in api_response {
                if let Some(track) = self.api_cache_handler.cache_track_info(track) {
                    cache_dirty = true;
                    result.push(track.clone());
                }
            }
        }

        if cache_dirty {
            self.api_cache_handler.save_cache().await;
        }

        Ok(result)
    }

    pub fn start_playlist_task(&mut self, tracks: Vec<TrackInfo>) -> Result<()> {
        let player = self.spotify_player.as_mut().ok_or(error::WorkerError::NoSpotifyPlayer)?;
        let track = tracks[0].clone();
        let track_id = SpotifyId::from_uri(&track.id).map_err(|_| error::WorkerError::BadSpotifyId)?;

        player.load(track_id, true, 0);

        self.player_current_track = 0;
        self.player_tracks_queue = tracks;
        self.state_tx.send(PlayerStateUpdate::EndOfTrack(track)).unwrap();

        Ok(())
    }

    pub fn start_playlist_at_idx_task(&mut self, tracks: Vec<TrackInfo>, idx: usize) -> Result<()> {
        let player = self.spotify_player.as_mut().ok_or(error::WorkerError::NoSpotifyPlayer)?;
        let track = tracks[idx].clone();
        let track_id = SpotifyId::from_uri(&track.id).map_err(|_| error::WorkerError::BadSpotifyId)?;

        player.load(track_id, true, 0);

        self.player_current_track = idx;
        self.player_tracks_queue = tracks;
        self.state_tx.send(PlayerStateUpdate::EndOfTrack(track)).unwrap();

        Ok(())
    }

    pub async fn add_track_to_playlist_task(&mut self, track: String, playlist: String) -> Result<()> {
        let api_client = self.api_client.as_mut().ok_or(error::WorkerError::NoAPIClient)?;
        let track_id = TrackId::from_uri(&track).map_err(|_| error::WorkerError::BadSpotifyId)?;
        let playlist_id = PlaylistId::from_uri(&playlist).map_err(|_| error::WorkerError::BadSpotifyId)?;

        let items: Vec<&dyn PlayableId> = vec![&track_id];

        api_client.playlist_add_items(&playlist_id, items, None).await.map(|_| Ok(()))?
    }

    pub async fn remove_track_from_playlist_task(&mut self, track: String, playlist: String) -> Result<()> {
        let api_client = self.api_client.as_mut().ok_or(error::WorkerError::NoAPIClient)?;
        let playlist_id = PlaylistId::from_uri(&playlist).map_err(|_| error::WorkerError::BadSpotifyId)?;
        let track_id = TrackId::from_uri(&track).map_err(|_| error::WorkerError::BadSpotifyId)?;
        let track_ids: Vec<&dyn PlayableId> = vec![&track_id];
        
        api_client.playlist_remove_all_occurrences_of_items(&playlist_id, track_ids, None).await.map(|_| Ok(()))?
    }

    async fn process_playlist_info(&mut self, playlists: Vec<SimplifiedPlaylist>) -> Result<Vec<(String, Playlist)>> {
        let session = self.spotify_session.as_ref().ok_or(error::WorkerError::NoSpotifySession)?;
        let mut result = Vec::new();

        for playlist in playlists {
            let playlist_uri = playlist.id.uri();
            let playlist_id = SpotifyId::from_uri(&playlist_uri).map_err(|_| error::WorkerError::BadSpotifyId)?;

            let images: Vec<(u32, String)> = playlist.images
                .iter()
                .map(| i | {
                    (i.width.unwrap_or_default(), i.url.clone())
                })
                .collect()
            ;

            self.api_cache_handler.cache_cover_image(&playlist_uri, &images).await;

            if let Ok(p) = Playlist::get(session, playlist_id).await {
                result.push((playlist.id.to_string(), p));
            }
        }

        Ok(result)
    }

    async fn make_track_info_vec(&mut self, tracks: Vec<String>) -> Result<Vec<TrackInfo>> {
        let mut result = Vec::new();

        let tracks_to_fetch: Vec<TrackId> = tracks.into_iter()
            // Only fetch tracks with a valid Spotify ID.
            .filter_map(| uri | {
                TrackId::from_uri(&uri).ok()
            })
            // And filter out the tracks that we already have cached.
            .filter(| track | {
                if let Some(track) = self.api_cache_handler.get_track_info(&track.uri()) {
                    result.push(track);
                    false
                }
                else {
                    true
                }
            })
            .collect()
        ;

        let mut fetched_tracks = self.get_tracks_info(&tracks_to_fetch).await?;
        result.append(&mut fetched_tracks);

        Ok(result)
    }
}
