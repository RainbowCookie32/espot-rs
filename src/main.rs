mod dbus;
mod spotify;
mod spinner;

use std::path::PathBuf;

use eframe::{egui, epi};
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use librespot::metadata::Playlist;

use spotify::*;

enum CurrentPanel {
    Home,
    Playlist,
    Recommendations
}

impl Default for CurrentPanel {
    fn default() -> CurrentPanel {
        CurrentPanel::Home
    }
}

#[derive(Default)]
struct PlaybackStatus {
    paused: bool,
    started: bool,

    current_track: Option<TrackInfo>,
    
    current_playlist: Option<usize>,
    current_playlist_tracks: Vec<TrackInfo>,

    recommendations: Vec<TrackInfo>
}

#[derive(Deserialize, Serialize)]
pub struct EspotApp {
    #[serde(skip)]
    logged_in: bool,

    login_username: String,
    #[serde(skip)]
    login_password: String,

    #[serde(skip)]
    cache_path: PathBuf,

    #[serde(skip)]
    current_panel: CurrentPanel,

    #[serde(skip)]
    user_playlists: Vec<(String, Playlist)>,
    #[serde(skip)]
    featured_playlists: Vec<(String, Playlist)>,

    #[serde(skip)]
    fetching_user_playlists: bool,
    #[serde(skip)]
    fetching_featured_playlists: bool,
    #[serde(skip)]
    fetching_playlist_recommendations: bool,

    #[serde(skip)]
    playback_status: PlaybackStatus,

    #[serde(skip)]
    state_rx: Option<broadcast::Receiver<PlayerStateUpdate>>,
    #[serde(skip)]
    control_tx: Option<mpsc::UnboundedSender<PlayerControl>>,

    #[serde(skip)]
    worker_task_tx: Option<mpsc::UnboundedSender<WorkerTask>>,
    #[serde(skip)]
    worker_result_rx: Option<mpsc::UnboundedReceiver<WorkerResult>>,

    #[serde(skip)]
    texture_no_cover: Option<(egui::Vec2, egui::TextureId)>,
    #[serde(skip)]
    texture_album_cover: Option<(egui::Vec2, egui::TextureId)>,
    
    #[serde(skip)]
    textures_user_playlists_covers: Vec<Option<(egui::Vec2, egui::TextureId)>>,
    #[serde(skip)]
    textures_featured_playlists_covers: Vec<Option<(egui::Vec2, egui::TextureId)>>
}

impl Default for EspotApp {
    fn default() -> EspotApp {
        let cache_path = dirs::cache_dir().unwrap().join("espot-rs");
        let (worker_task_tx, worker_result_rx, state_rx, _, control_tx) = SpotifyWorker::start();

        EspotApp {
            logged_in: false,

            login_username: String::new(),
            login_password: String::new(),

            cache_path,

            current_panel: CurrentPanel::Home,

            user_playlists: Vec::new(),
            featured_playlists: Vec::new(),

            fetching_user_playlists: false,
            fetching_featured_playlists: false,
            fetching_playlist_recommendations: false,

            playback_status: PlaybackStatus::default(),

            state_rx: Some(state_rx),
            control_tx: Some(control_tx),

            worker_task_tx: Some(worker_task_tx),
            worker_result_rx: Some(worker_result_rx),

            texture_no_cover: None,
            texture_album_cover: None,

            textures_user_playlists_covers: Vec::new(),
            textures_featured_playlists_covers: Vec::new()
        }
    }
}

impl epi::App for EspotApp {
    fn name(&self) -> &str {
        "espot-rs"
    }

    fn setup(&mut self, ctx: &egui::CtxRef, _frame: &epi::Frame, storage: Option<&dyn epi::Storage>) {
        if let Some(storage) = storage {
            *self = epi::get_value(storage, epi::APP_KEY).unwrap_or_default()
        }

        self.playback_status.paused = true;

        if self.worker_task_tx.is_none() {
            let (worker_task_tx, worker_result_rx, state_rx, state_rx_dbus, control_tx) = SpotifyWorker::start();

            #[cfg(not(debug_assertions))]
            dbus::start_dbus_server(state_rx_dbus, control_tx.clone());

            self.state_rx = Some(state_rx);
            self.control_tx = Some(control_tx);

            self.worker_task_tx = Some(worker_task_tx);
            self.worker_result_rx = Some(worker_result_rx);
        }

        self.cache_path = dirs::cache_dir().unwrap().join("espot-rs");

        let mut definitions = egui::FontDefinitions::default();

        if let Ok(font) = std::fs::read("resources/fonts/japanese.otf") {
            let font_data = egui::FontData::from_owned(font);

            definitions.font_data.insert("jp_font".to_owned(), font_data);

            if let Some(f) = definitions.fonts_for_family.get_mut(&egui::FontFamily::Monospace) {
                f.push(String::from("jp_font"));
            }

            if let Some(f) = definitions.fonts_for_family.get_mut(&egui::FontFamily::Proportional) {
                f.push(String::from("jp_font"));
            }
        }

        ctx.set_fonts(definitions)
    }

    fn save(&mut self, storage: &mut dyn epi::Storage) {
        epi::set_value(storage, epi::APP_KEY, self);
    }

    fn update(&mut self, ctx: &egui::CtxRef, frame: &epi::Frame) {
        if self.texture_no_cover.is_none() {
            self.texture_no_cover = EspotApp::make_cover_image(include_bytes!("../resources/no_cover.png"), frame);
        }

        if let Some(track) = self.playback_status.current_track.as_ref() {
            EspotApp::load_texture(frame, &mut self.texture_album_cover, &self.cache_path, &track.album_id);
        }

        for (i, target) in self.textures_user_playlists_covers.iter_mut().enumerate() {
            let (playlist_id, _) = &self.user_playlists[i];
            EspotApp::load_texture(frame, target, &self.cache_path, playlist_id);
        }

        for (i, target) in self.textures_featured_playlists_covers.iter_mut().enumerate() {
            let (playlist_id, _) = &self.featured_playlists[i];
            EspotApp::load_texture(frame, target, &self.cache_path, playlist_id);
        }

        if self.logged_in {
            self.draw_main_screen(ctx);

            if self.user_playlists.is_empty() && !self.fetching_user_playlists {
                self.fetching_user_playlists = true;
                self.send_worker_msg(WorkerTask::GetUserPlaylists);
            }

            if self.featured_playlists.is_empty() && !self.fetching_featured_playlists {
                self.fetching_featured_playlists = true;
                self.send_worker_msg(WorkerTask::GetFeaturedPlaylists);
            }
        }
        else {
            self.draw_login_screen(ctx);
        }

        if let Some(rx) = self.state_rx.as_mut() {
            if let Ok(state) = rx.try_recv() {
                match state {
                    PlayerStateUpdate::Paused => {
                        self.playback_status.paused = true;
                    }
                    PlayerStateUpdate::Resumed => {
                        self.playback_status.paused = false;
                    }
                    PlayerStateUpdate::Stopped => {
                        self.playback_status.current_track = None;

                        if let Some((_, id)) = self.texture_album_cover {
                            frame.free_texture(id);
                            self.texture_album_cover = None;
                        }
                    }
                    PlayerStateUpdate::EndOfTrack(track) => {
                        self.playback_status.paused = false;
                        self.playback_status.current_track = Some(track);

                        if let Some((_, id)) = self.texture_album_cover {
                            frame.free_texture(id);
                            self.texture_album_cover = None;
                        }
                    }
                }
            }
        }

        if let Some(rx) = self.worker_result_rx.as_mut() {
            if let Ok(worker_res) = rx.try_recv() {
                match worker_res {
                    WorkerResult::Login(result) => {
                        if result {
                            self.logged_in = true;
                        }
    
                        self.login_password = String::new();
                    }
                    WorkerResult::UserPlaylists(playlists) => {
                        self.user_playlists = playlists;
                        self.fetching_user_playlists = false;
                        
                        for (_, id) in self.textures_user_playlists_covers.iter().flatten() {
                            frame.free_texture(*id);
                        }
                        
                        self.textures_user_playlists_covers = vec![None; self.user_playlists.len()];
                    }
                    WorkerResult::FeaturedPlaylists(playlists) => {
                        self.featured_playlists = playlists;
                        self.fetching_featured_playlists = false;
                        
                        for (_, id) in self.textures_featured_playlists_covers.iter().flatten() {
                            frame.free_texture(*id);
                        }

                        self.textures_featured_playlists_covers = vec![None; self.featured_playlists.len()];
                    }
                    WorkerResult::PlaylistTrackInfo(tracks) => {
                        self.playback_status.current_playlist_tracks = tracks;
                    }
                    WorkerResult::PlaylistRecommendations(tracks) => {
                        self.playback_status.recommendations = tracks;
                        self.fetching_playlist_recommendations = false;
                    }
                }
            }
        }

        // TODO: Workaround for not being able to figure out how to request a repaint
        //       from the worker thread. Burns more resources than needed.
        ctx.request_repaint();
    }
}

impl EspotApp {
    fn draw_login_screen(&mut self, ctx: &egui::CtxRef) {
        egui::CentralPanel::default().show(ctx, | ui | {
            ui.vertical_centered(| ui | {
                ui.heading("espot-rs");
                ui.separator();
            });

            ui.vertical_centered(| ui | {
                ui.label("Username");
                let usr_field = ui.text_edit_singleline(&mut self.login_username);

                ui.label("Password");
                let pwd_field = ui.add(egui::TextEdit::singleline(&mut self.login_password).password(true));

                let submitted = (usr_field.lost_focus() || pwd_field.lost_focus()) && ui.input().key_pressed(egui::Key::Enter);

                if ui.button("Log in").clicked() || submitted {
                    self.send_worker_msg(WorkerTask::Login(self.login_username.clone(), self.login_password.clone()));
                }
            });
        });
    }

    fn draw_main_screen(&mut self, ctx: &egui::CtxRef) {
        egui::TopBottomPanel::bottom("playback_status").show(ctx, | ui | {
            self.draw_playback_status(ui);
        });

        egui::SidePanel::left("side_panel").show(ctx, | ui | {
            self.draw_side_panel(ui);
        });

        egui::CentralPanel::default().show(ctx, | ui | {
            match &self.current_panel {
                CurrentPanel::Home => self.draw_home_panel(ui),
                CurrentPanel::Playlist => self.draw_playlist_panel(ui),
                CurrentPanel::Recommendations => self.draw_recommendations_panel(ui)
            }
        });
    }

    fn draw_playback_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if let Some((size, id)) = self.texture_album_cover.as_ref() {
                ui.image(*id, *size);
            }
            else if let Some((size, id)) = self.texture_no_cover.as_ref() {
                ui.image(*id, *size);
            }

            ui.vertical(| ui | {
                ui.add_space(5.0);

                if let Some(track) = self.playback_status.current_track.as_ref() {
                    let artists_label = EspotApp::make_artists_string(&track.artists);

                    ui.heading(&track.name);
                    ui.label(artists_label);
                }
                else {
                    ui.heading("...");
                    ui.label("...");
                }

                ui.add_space(5.0);
                
                ui.horizontal(| ui | {
                    let can_move = self.playback_status.started;
                    let can_start = self.is_playlist_ready();
    
                    ui.add_enabled_ui(can_move, | ui | {
                        if ui.button("⏮").clicked() {
                            self.send_player_msg(PlayerControl::PreviousTrack);
                        }
                    });
    
                    ui.add_enabled_ui(can_start, | ui | {
                        let button_label = if self.playback_status.paused {"▶"} else {"⏸"};

                        if ui.button(button_label).clicked() {
                            if !self.playback_status.started {
                                let tracks = {
                                    match self.current_panel {
                                        CurrentPanel::Home | CurrentPanel::Playlist => self.playback_status.current_playlist_tracks.clone(),
                                        CurrentPanel::Recommendations => self.playback_status.recommendations.clone(),
                                    }
                                };

                                self.playback_status.started = true;
                                self.send_player_msg(PlayerControl::StartPlaylist(tracks));
                            }
                            else if self.playback_status.paused {
                                self.send_player_msg(PlayerControl::Play);
                            }
                            else {
                                self.send_player_msg(PlayerControl::Pause);
                            }
    
                            self.playback_status.paused = !self.playback_status.paused;
                        }
                    });
    
                    ui.add_enabled_ui(can_move, | ui | {
                        if ui.button("⏭").clicked() {
                            self.send_player_msg(PlayerControl::NextTrack);
                        }
                    });
                })
            });
        });
    }

    fn draw_side_panel(&mut self, ui: &mut egui::Ui) {
        ui.separator();
            ui.label("espot-rs");
            ui.separator();

            if ui.selectable_label(false, "Home").clicked() {
                self.current_panel = CurrentPanel::Home;
            }

            ui.collapsing("Playlists", | ui | {
                if !self.user_playlists.is_empty() {
                    for (i, (_, p)) in self.user_playlists.iter().enumerate() {
                        let playlist_label = ui.selectable_label(false, &p.name);

                        let label_clicked = playlist_label.clicked();
                        
                        let mut get_recommendations = false;
                        let mut opened_from_ctx_menu = false;

                        playlist_label.context_menu(| ui | {
                            if ui.selectable_label(false, "Open playlist").clicked() {
                                opened_from_ctx_menu = true;
                                ui.close_menu();
                            }

                            if ui.selectable_label(false, "Get recommendations").clicked() {
                                get_recommendations = true;
                                ui.close_menu();
                            }
                        });

                        if label_clicked || opened_from_ctx_menu {
                            if let Some(currently_selected) = self.playback_status.current_playlist.as_ref() {
                                if i != *currently_selected {
                                    self.playback_status.current_playlist = Some(i);
                                    self.playback_status.current_playlist_tracks = Vec::with_capacity(p.tracks.len());

                                    self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(p.clone()));
                                }
                            }
                            else {
                                self.playback_status.current_playlist = Some(i);
                                self.playback_status.current_playlist_tracks = Vec::with_capacity(p.tracks.len());

                                self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(p.clone()));
                            }

                            self.current_panel = CurrentPanel::Playlist;
                        }
                        else if get_recommendations {
                            self.current_panel = CurrentPanel::Recommendations;
                            self.playback_status.recommendations = Vec::new();
                            self.fetching_playlist_recommendations = true;
                            
                            self.send_worker_msg(WorkerTask::GetRecommendationsForPlaylist(p.clone()));
                        }
                    }
                }
                else {
                    ui.label("No playlists found...");
                }
            });

            let condition = !self.fetching_playlist_recommendations && self.playback_status.recommendations.is_empty();

            if ui.add_enabled(!condition, egui::SelectableLabel::new(false, "Recommendations")).clicked() {
                self.current_panel = CurrentPanel::Recommendations;
            }
    }

    fn draw_home_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            ui.heading("Your playlists");

            if self.fetching_user_playlists {
                ui.add(spinner::Spinner::new());
            }
        });

        ui.separator();

        egui::ScrollArea::horizontal().id_source("user_playlists_scroll").show(ui, | ui | {
            ui.horizontal(| ui | {
                for (i, (_, playlist)) in self.user_playlists.iter().enumerate() {
                    let tint = egui::Color32::from_rgba_unmultiplied(96 , 96, 96, 160);

                    if let Some(t) = self.textures_user_playlists_covers.get(i) {
                        if let Some((size, id)) = t {
                            let button = egui::ImageButton::new(*id, *size).tint(tint);
                            let button = ui.add(button);

                            let text = egui::RichText::new(&playlist.name).strong();
                            let label = ui.put(button.rect, egui::Label::new(text));

                            if button.clicked() || label.clicked() {
                                self.current_panel = CurrentPanel::Playlist;

                                self.playback_status.current_playlist = Some(i);
                                self.playback_status.current_playlist_tracks = Vec::with_capacity(playlist.tracks.len());
                                self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(playlist.clone()));
                            }
                        }
                    }
                    else if let Some((size, id)) = self.texture_no_cover.as_ref() {
                        let button = egui::ImageButton::new(*id, *size).tint(tint);
                        let button = ui.add(button);

                        let text = egui::RichText::new(&playlist.name).strong();
                        let label = ui.put(button.rect, egui::Label::new(text));

                        if button.clicked() || label.clicked() {
                            self.current_panel = CurrentPanel::Playlist;

                            self.playback_status.current_playlist = Some(i);
                            self.playback_status.current_playlist_tracks = Vec::with_capacity(playlist.tracks.len());
                            self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(playlist.clone()));
                        }
                    }
                }
            });
        });

        ui.add_space(20.0);

        ui.horizontal(| ui | {
            ui.heading("Featured by Spotify");

            if self.fetching_featured_playlists {
                ui.add(spinner::Spinner::new());
            }
        });

        ui.separator();

        egui::ScrollArea::horizontal().id_source("spotify_featured_scroll").show(ui, | ui | {
            ui.horizontal(| ui | {
                for (i, (_, playlist)) in self.featured_playlists.iter().enumerate() {
                    let tint = egui::Color32::from_rgba_unmultiplied(96 , 96, 96, 160);

                    if let Some(t) = self.textures_featured_playlists_covers.get(i) {
                        if let Some((size, id)) = t {
                            let button = egui::ImageButton::new(*id, *size).tint(tint);
                            let button = ui.add(button);

                            let text = egui::RichText::new(&playlist.name).strong();
                            let label = ui.put(button.rect, egui::Label::new(text));

                            if button.clicked() || label.clicked() {
                                
                            }
                        }
                    }
                    else if let Some((size, id)) = self.texture_no_cover.as_ref() {
                        let button = egui::ImageButton::new(*id, *size).tint(tint);
                        let button = ui.add(button);

                        let text = egui::RichText::new(&playlist.name).strong();
                        let label = ui.put(button.rect, egui::Label::new(text));

                        if button.clicked() || label.clicked() {
                            
                        }
                    }
                }
            });
        });
    }

    fn draw_playlist_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if let Some(idx) = self.playback_status.current_playlist.as_ref() {
                let (_, playlist) = &self.user_playlists[*idx];

                let label = {
                    if playlist.tracks.len() == 1 {
                        format!("{} (1 track)", &playlist.name)
                    }
                    else {
                        format!("{} ({} tracks)", &playlist.name, playlist.tracks.len())
                    }
                };

                ui.strong(label);
            }
            else {
                ui.strong("Select a playlist on the sidebar...");
            }

            if self.playback_status.current_playlist.is_some() && !self.is_playlist_ready() {
                ui.add(spinner::Spinner::new());
            }
        });

        ui.separator();
        self.draw_songs_list(ui);
    }

    fn draw_recommendations_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if self.is_playlist_ready() {
                let tracks = self.playback_status.recommendations.len();

                let label = {
                    if tracks == 1 {
                        String::from("Recommendations (1 track)")
                    }
                    else {
                        format!("Recommendations ({} tracks)", tracks)
                    }
                };

                ui.strong(label);
            }
            else {
                ui.strong("Fetching recommendations...");
                ui.add(spinner::Spinner::new());
            }
        });

        ui.separator();
        self.draw_songs_list(ui);
    }

    fn draw_songs_list(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, | ui | {
            let is_user_playlist = {
                match self.current_panel {
                    CurrentPanel::Playlist => true,
                    CurrentPanel::Recommendations => false,
                    // If we are not on either of those panels, we shouldn't be rendering this.
                    _ => return
                }
            };

            ui.style_mut().wrap = Some(false);

            let mut remove_track = None;
            let mut start_playlist = None;

            ui.columns(4, | cols | {
                let tracks_iter = {
                    if is_user_playlist {
                        self.playback_status.current_playlist_tracks.iter()
                    }
                    else {
                        self.playback_status.recommendations.iter()
                    }
                };

                cols[0].label("Title");
                cols[1].label("Artists");
                cols[2].label("Album");
                cols[3].label("Duration");

                for (track_idx, track) in tracks_iter.enumerate() {
                    let glyph_width = cols[0].fonts().glyph_width(egui::TextStyle::Body, 'A');

                    let title_label = EspotApp::trim_string(cols[0].available_width(), glyph_width, track.name.clone());
                    let artists_label = EspotApp::trim_string(cols[1].available_width(), glyph_width, EspotApp::make_artists_string(&track.artists));

                    let album_label = EspotApp::trim_string(cols[2].available_width(), glyph_width, track.album_name.clone());
                    let duration_label = EspotApp::trim_string(cols[3].available_width(), glyph_width, format!("{}:{:02}", (track.duration_ms / 1000) / 60, (track.duration_ms / 1000) % 60));

                    let track_name_label = cols[0].selectable_label(false, title_label);
                    
                    if track_name_label.clicked() && self.is_playlist_ready() {
                        let tracks = {
                            if is_user_playlist {
                                self.playback_status.current_playlist_tracks.clone()
                            }
                            else {
                                self.playback_status.recommendations.clone()
                            }
                        };

                        self.playback_status.paused = false;
                        self.playback_status.started = true;
                        self.send_player_msg(PlayerControl::StartPlaylistAtTrack(tracks.clone(), track.clone()));
                    }

                    track_name_label.context_menu(| ui | {
                        if ui.selectable_label(false, "Play from here").clicked() {
                            let tracks = {
                                if is_user_playlist {
                                    self.playback_status.current_playlist_tracks.clone()
                                }
                                else {
                                    self.playback_status.recommendations.clone()
                                }
                            };

                            self.playback_status.paused = false;
                            self.playback_status.started = true;
                            start_playlist = Some((tracks, track.clone()));

                            ui.close_menu();
                        }

                        if ui.selectable_label(false, "Remove").clicked() {
                            let id = {
                                if is_user_playlist {
                                    if let Some(i) = self.playback_status.current_playlist.as_ref() {
                                        self.user_playlists[*i].0.clone()
                                    }
                                    else {
                                        return;
                                    }
                                }
                                else {
                                    String::new()
                                }
                            };

                            remove_track = Some((id, track.id.clone(), track_idx));
                            ui.close_menu();
                        }
                    });

                    let _ = cols[1].selectable_label(false, artists_label);
                    let _ = cols[2].selectable_label(false, album_label);
                    let _ = cols[3].selectable_label(false, duration_label);
                }
            });

            if let Some((playlist, track_id, track_idx)) = remove_track {
                if is_user_playlist {
                    self.fetching_user_playlists = true;
                    self.playback_status.current_playlist_tracks.remove(track_idx);
                    self.send_worker_msg(WorkerTask::RemoveTrackFromPlaylist(playlist, track_id));
                    self.send_worker_msg(WorkerTask::GetUserPlaylists);
                }
                else {
                    self.playback_status.recommendations.remove(track_idx);
                }
            }

            if let Some((playlist, track)) = start_playlist {
                self.send_player_msg(PlayerControl::StartPlaylistAtTrack(playlist, track));
            }

            ui.style_mut().wrap = None;
        });
    }

    fn trim_string(available_width: f32, glyph_width: f32, text: String) -> String {
        let mut text = text;
        let mut text_chars: Vec<char> = text.chars().collect();

        let max_chars_in_space = (available_width / glyph_width) as usize;

        if text_chars.len() >= max_chars_in_space {
            while text_chars.len() >= max_chars_in_space {
                text_chars.pop();
            }
            
            text = text_chars.into_iter().collect();
            text.push_str("...");
        }

        text
    }

    fn is_playlist_ready(&self) -> bool {
        match self.current_panel {
            CurrentPanel::Home => false,
            CurrentPanel::Playlist => {
                if let Some(i) = self.playback_status.current_playlist.as_ref() {
                    if let Some((_, p)) = self.user_playlists.get(*i) {
                        self.playback_status.current_playlist_tracks.len() == p.tracks.len()
                    }
                    else {
                        false
                    }
                }
                else {
                    false
                }
            }
            CurrentPanel::Recommendations => {
                !self.fetching_playlist_recommendations && !self.playback_status.recommendations.is_empty()
            }
        }
    }

    fn send_worker_msg(&self, message: WorkerTask) {
        if let Some(tx) = self.worker_task_tx.as_ref() {
            tx.send(message).unwrap();
        }
    }

    fn send_player_msg(&self, message: PlayerControl) {
        if let Some(tx) = self.control_tx.as_ref() {
            tx.send(message).unwrap();
        }
    }

    fn load_texture(frame: &epi::Frame, target: &mut Option<(egui::Vec2, egui::TextureId)>, cache_path: &PathBuf, id: &str) {
        if target.is_none() {
            let cover_path = cache_path.join(format!("cover-{}", id));

            if let Ok(buffer) = std::fs::read(cover_path) {
                *target = EspotApp::make_cover_image(&buffer, frame);
            }
        }
    }

    fn make_cover_image(buffer: &[u8], frame: &epi::Frame) -> Option<(egui::Vec2, egui::TextureId)> {
        if let Ok(image) = image::load_from_memory(buffer) {
            let image_buf = image.to_rgba8();
            let image_size = [image.width() as usize, image.height() as usize];
            let image_pixels = image_buf.into_vec();

            let image = epi::Image::from_rgba_unmultiplied(image_size, &image_pixels);
            let size = egui::Vec2::new(96.0, 96.0);
            let texture = frame.alloc_texture(image);
            
            Some((size, texture))
        }
        else {
            None
        }
    }

    fn make_artists_string(artists: &[String]) -> String {
        let mut result = String::new();

        for (i, artist) in artists.iter().enumerate() {
            result.push_str(artist);

            if i != artists.len() - 1 {
                result.push_str(", ");
            }
        }

        result
    }
}

fn main() {
    let app = EspotApp::default();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(Box::new(app), native_options);
}
