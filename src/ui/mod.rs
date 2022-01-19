mod utils;

use std::path::PathBuf;

use eframe::{egui, epi};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use librespot::metadata::Playlist;

use crate::spotify::*;
use crate::spinner::Spinner;

#[derive(PartialEq)]
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
struct PersistentData {
    cache_path: PathBuf,
    login_username: String
}

#[derive(Default)]
struct VolatileData {
    logged_in: bool,
    login_password: String,

    current_panel: CurrentPanel,

    user_playlists: Vec<(String, Playlist)>,
    featured_playlists: Vec<(String, Playlist)>,

    fetching_user_playlists: bool,
    fetching_featured_playlists: bool,
    fetching_playlist_recommendations: bool,

    playback_status: PlaybackStatus,

    state_rx: Option<broadcast::Receiver<PlayerStateUpdate>>,
    control_tx: Option<mpsc::UnboundedSender<PlayerControl>>,

    worker_task_tx: Option<mpsc::UnboundedSender<WorkerTask>>,
    worker_result_rx: Option<mpsc::UnboundedReceiver<WorkerResult>>,

    texture_no_cover: Option<egui::TextureId>,
    texture_album_cover: Option<egui::TextureId>,
    
    textures_user_playlists_covers: Vec<Option<egui::TextureId>>,
    textures_featured_playlists_covers: Vec<Option<egui::TextureId>>
}

#[derive(Deserialize, Serialize)]
pub struct EspotApp {
    p: PersistentData,
    #[serde(skip)]
    v: VolatileData
}

impl Default for EspotApp {
    fn default() -> EspotApp {
        let cache_path = dirs::cache_dir().unwrap().join("espot-rs");

        let p = PersistentData {
            cache_path,
            login_username: String::new()
        };

        let v = VolatileData::default();

        EspotApp {
            p,
            v
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

        self.v.playback_status.paused = true;

        if self.v.worker_task_tx.is_none() {
            let (worker_task_tx, worker_result_rx, state_rx, state_rx_dbus, control_tx) = SpotifyWorker::start();

            #[cfg(target_os = "linux")]
            #[cfg(not(debug_assertions))]
            crate::dbus::start_dbus_server(state_rx_dbus, control_tx.clone());

            self.v.state_rx = Some(state_rx);
            self.v.control_tx = Some(control_tx);

            self.v.worker_task_tx = Some(worker_task_tx);
            self.v.worker_result_rx = Some(worker_result_rx);
        }

        self.p.cache_path = dirs::cache_dir().unwrap().join("espot-rs");

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
        if self.v.texture_no_cover.is_none() {
            let buffer = include_bytes!("../../resources/no_cover.png");
            self.v.texture_no_cover = utils::create_texture_from_bytes(frame, buffer);
        }

        if let Some(track) = self.v.playback_status.current_track.as_ref() {
            if self.v.texture_album_cover.is_none() {
                let path = self.p.cache_path.join(format!("cover-{}", &track.album_id));
                self.v.texture_album_cover = utils::create_texture_from_file(frame, path);
            }
        }

        for (i, target) in self.v.textures_user_playlists_covers.iter_mut().enumerate() {
            if target.is_none() {
                let (playlist_id, _) = &self.v.user_playlists[i];
                let path = self.p.cache_path.join(format!("cover-{}", playlist_id));
            
                *target = utils::create_texture_from_file(frame, path);
            }
        }

        for (i, target) in self.v.textures_featured_playlists_covers.iter_mut().enumerate() {
            if target.is_none() {
                let (playlist_id, _) = &self.v.featured_playlists[i];
                let path = self.p.cache_path.join(format!("cover-{}", playlist_id));
            
                *target = utils::create_texture_from_file(frame, path);
            }
        }

        if self.v.logged_in {
            self.draw_main_screen(ctx);

            if self.v.user_playlists.is_empty() && !self.v.fetching_user_playlists {
                self.v.fetching_user_playlists = true;
                self.send_worker_msg(WorkerTask::GetUserPlaylists);
            }

            if self.v.featured_playlists.is_empty() && !self.v.fetching_featured_playlists {
                self.v.fetching_featured_playlists = true;
                self.send_worker_msg(WorkerTask::GetFeaturedPlaylists);
            }
        }
        else {
            self.draw_login_screen(ctx);
        }

        self.handle_messages(frame);

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
                let usr_field = ui.text_edit_singleline(&mut self.p.login_username);

                ui.label("Password");
                let pwd_field = ui.add(egui::TextEdit::singleline(&mut self.v.login_password).password(true));

                let submitted = (usr_field.lost_focus() || pwd_field.lost_focus()) && ui.input().key_pressed(egui::Key::Enter);

                if ui.button("Log in").clicked() || submitted {
                    self.send_worker_msg(WorkerTask::Login(self.p.login_username.clone(), self.v.login_password.clone()));
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
            match &self.v.current_panel {
                CurrentPanel::Home => self.draw_home_panel(ui),
                CurrentPanel::Playlist => self.draw_playlist_panel(ui),
                CurrentPanel::Recommendations => self.draw_recommendations_panel(ui)
            }
        });
    }

    fn draw_playback_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if let Some(id) = self.v.texture_album_cover.as_ref() {
                ui.image(*id, egui::vec2(96.0, 96.0));
            }
            else if let Some(id) = self.v.texture_no_cover.as_ref() {
                ui.image(*id, egui::vec2(96.0, 96.0));
            }

            ui.vertical(| ui | {
                ui.add_space(5.0);

                if let Some(track) = self.v.playback_status.current_track.as_ref() {
                    let artists_label = utils::make_artists_string(&track.artists);

                    ui.heading(&track.name);
                    ui.label(artists_label);
                }
                else {
                    ui.heading("...");
                    ui.label("...");
                }

                ui.add_space(5.0);
                
                ui.horizontal(| ui | {
                    let can_move = self.v.playback_status.started;
                    let can_start = self.is_playlist_ready();
    
                    ui.add_enabled_ui(can_move, | ui | {
                        if ui.button("⏮").clicked() {
                            self.send_player_msg(PlayerControl::PreviousTrack);
                        }
                    });
    
                    ui.add_enabled_ui(can_start, | ui | {
                        let button_label = if self.v.playback_status.paused {"▶"} else {"⏸"};

                        if ui.button(button_label).clicked() {
                            if !self.v.playback_status.started {
                                let tracks = {
                                    match self.v.current_panel {
                                        CurrentPanel::Home | CurrentPanel::Playlist => self.v.playback_status.current_playlist_tracks.clone(),
                                        CurrentPanel::Recommendations => self.v.playback_status.recommendations.clone(),
                                    }
                                };

                                self.v.playback_status.started = true;
                                self.send_player_msg(PlayerControl::StartPlaylist(tracks));
                            }
                            else if self.v.playback_status.paused {
                                self.send_player_msg(PlayerControl::Play);
                            }
                            else {
                                self.send_player_msg(PlayerControl::Pause);
                            }
    
                            self.v.playback_status.paused = !self.v.playback_status.paused;
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

            if ui.selectable_label(self.v.current_panel == CurrentPanel::Home, "Home").clicked() {
                self.v.current_panel = CurrentPanel::Home;
            }

            ui.separator();

            ui.collapsing("Playlists", | ui | {
                if !self.v.user_playlists.is_empty() {
                    for (i, (_, p)) in self.v.user_playlists.iter().enumerate() {
                        let checked = {
                            if let Some(selected) = self.v.playback_status.current_playlist.as_ref() {
                                self.v.current_panel == CurrentPanel::Playlist && i == *selected
                            }
                            else {
                                false
                            }
                        };

                        let playlist_label = ui.selectable_label(checked, &p.name);
                        let label_clicked = playlist_label.clicked();
                        
                        let mut get_recommendations = false;
                        let mut opened_from_ctx_menu = false;

                        playlist_label.context_menu(| ui | {
                            if ui.selectable_label(false, "View").clicked() {
                                opened_from_ctx_menu = true;
                                ui.close_menu();
                            }

                            if ui.selectable_label(false, "Get recommendations").clicked() {
                                get_recommendations = true;
                                ui.close_menu();
                            }
                        });

                        if label_clicked || opened_from_ctx_menu {
                            if let Some(currently_selected) = self.v.playback_status.current_playlist.as_ref() {
                                if i != *currently_selected {
                                    self.v.playback_status.current_playlist = Some(i);
                                    self.v.playback_status.current_playlist_tracks = Vec::with_capacity(p.tracks.len());
                                }
                            }
                            else {
                                self.v.playback_status.current_playlist = Some(i);
                                self.v.playback_status.current_playlist_tracks = Vec::with_capacity(p.tracks.len());
                            }

                            self.v.current_panel = CurrentPanel::Playlist;
                            self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(p.clone()));
                        }
                        else if get_recommendations {
                            self.v.current_panel = CurrentPanel::Recommendations;
                            self.v.playback_status.recommendations = Vec::new();
                            self.v.fetching_playlist_recommendations = true;
                            
                            self.send_worker_msg(WorkerTask::GetRecommendationsForPlaylist(p.clone()));
                        }
                    }
                }
                else {
                    ui.label("No playlists found...");
                }
            });

            ui.separator();

            let selected = self.v.current_panel == CurrentPanel::Recommendations;
            let enabled = !self.v.fetching_playlist_recommendations && self.v.playback_status.recommendations.is_empty();

            if ui.add_enabled(!enabled, egui::SelectableLabel::new(selected, "Recommendations")).clicked() {
                self.v.current_panel = CurrentPanel::Recommendations;
            }
    }

    fn draw_home_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            ui.heading("Your playlists");

            if self.v.fetching_user_playlists {
                ui.add(Spinner::new());
            }
        });

        ui.separator();

        egui::ScrollArea::horizontal().id_source("user_playlists_scroll").show(ui, | ui | {
            ui.horizontal(| ui | {
                for (i, (_, playlist)) in self.v.user_playlists.iter().enumerate() {
                    let tint = egui::Color32::from_rgba_unmultiplied(96 , 96, 96, 160);

                    if let Some(t) = self.v.textures_user_playlists_covers.get(i) {
                        if let Some(id) = t {
                            let button = egui::ImageButton::new(*id, egui::vec2(96.0, 96.0)).tint(tint);
                            let button = ui.add(button);

                            let text = egui::RichText::new(&playlist.name).strong();
                            let label = ui.put(button.rect, egui::Label::new(text));

                            if button.clicked() || label.clicked() {
                                self.v.current_panel = CurrentPanel::Playlist;

                                self.v.playback_status.current_playlist = Some(i);
                                self.v.playback_status.current_playlist_tracks = Vec::with_capacity(playlist.tracks.len());
                                self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(playlist.clone()));
                            }
                        }
                    }
                    else if let Some(id) = self.v.texture_no_cover.as_ref() {
                        let button = egui::ImageButton::new(*id, egui::vec2(96.0, 96.0)).tint(tint);
                        let button = ui.add(button);

                        let text = egui::RichText::new(&playlist.name).strong();
                        let label = ui.put(button.rect, egui::Label::new(text));

                        if button.clicked() || label.clicked() {
                            self.v.current_panel = CurrentPanel::Playlist;

                            self.v.playback_status.current_playlist = Some(i);
                            self.v.playback_status.current_playlist_tracks = Vec::with_capacity(playlist.tracks.len());
                            self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(playlist.clone()));
                        }
                    }
                }
            });
        });

        ui.add_space(20.0);

        ui.horizontal(| ui | {
            ui.heading("Featured by Spotify");

            if self.v.fetching_featured_playlists {
                ui.add(Spinner::new());
            }
        });

        ui.separator();

        egui::ScrollArea::horizontal().id_source("spotify_featured_scroll").show(ui, | ui | {
            ui.horizontal(| ui | {
                for (i, (_, playlist)) in self.v.featured_playlists.iter().enumerate() {
                    let tint = egui::Color32::from_rgba_unmultiplied(96 , 96, 96, 160);

                    if let Some(t) = self.v.textures_featured_playlists_covers.get(i) {
                        if let Some(id) = t {
                            let button = egui::ImageButton::new(*id, egui::vec2(96.0, 96.0)).tint(tint);
                            let button = ui.add(button);

                            let text = egui::RichText::new(&playlist.name).strong();
                            let label = ui.put(button.rect, egui::Label::new(text));

                            if button.clicked() || label.clicked() {
                                
                            }
                        }
                    }
                    else if let Some(id) = self.v.texture_no_cover.as_ref() {
                        let button = egui::ImageButton::new(*id, egui::vec2(96.0, 96.0)).tint(tint);
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
            if let Some(idx) = self.v.playback_status.current_playlist.as_ref() {
                let (_, playlist) = &self.v.user_playlists[*idx];

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

            if self.v.playback_status.current_playlist.is_some() && !self.is_playlist_ready() {
                ui.add(Spinner::new());
            }
            else if ui.button("Play").clicked() {
                self.v.playback_status.started = true;
                self.send_player_msg(PlayerControl::StartPlaylist(self.v.playback_status.current_playlist_tracks.clone()));
            }
        });

        ui.separator();
        self.draw_songs_list(ui);
    }

    fn draw_recommendations_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if self.is_playlist_ready() {
                let tracks = self.v.playback_status.recommendations.len();

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
                ui.add(Spinner::new());
            }

            if ui.button("Play").clicked() {
                self.v.playback_status.started = true;
                self.send_player_msg(PlayerControl::StartPlaylist(self.v.playback_status.recommendations.clone()));
            }
        });

        ui.separator();
        self.draw_songs_list(ui);
    }

    fn draw_songs_list(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, | ui | {
            let is_user_playlist = {
                match self.v.current_panel {
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
                        self.v.playback_status.current_playlist_tracks.iter()
                    }
                    else {
                        self.v.playback_status.recommendations.iter()
                    }
                };

                cols[0].label("Title");
                cols[1].label("Artists");
                cols[2].label("Album");
                cols[3].label("Duration");

                for (track_idx, track) in tracks_iter.enumerate() {
                    let glyph_width = cols[0].fonts().glyph_width(egui::TextStyle::Body, 'A');

                    let track_name_label = {
                        let mut track_name = track.name.clone();

                        let available_width = cols[0].available_width();
                        let trimmed = utils::trim_string(available_width, glyph_width, &mut track_name);

                        let checked = {
                            if let Some(t) = self.v.playback_status.current_track.as_ref() {
                                t.id == track.id
                            }
                            else {
                                false
                            }
                        };

                        if trimmed {
                            cols[0].selectable_label(checked, track_name).on_hover_text(&track.name)
                        }
                        else {
                            cols[0].selectable_label(checked, track_name)
                        }
                    };

                    let _track_artist_label = {
                        let artists = utils::make_artists_string(&track.artists);
                        let mut artists_string = artists.clone();

                        let available_width = cols[1].available_width();
                        let trimmed = utils::trim_string(available_width, glyph_width, &mut artists_string);

                        if trimmed {
                            cols[1].selectable_label(false, artists_string).on_hover_text(artists)
                        }
                        else {
                            cols[1].selectable_label(false, artists_string)
                        }
                    };

                    let _track_album_label = {
                        let mut album_name = track.album_name.clone();

                        let available_width = cols[2].available_width();
                        let trimmed = utils::trim_string(available_width, glyph_width, &mut album_name);

                        if trimmed {
                            cols[2].selectable_label(false, album_name).on_hover_text(track.album_name.clone())
                        }
                        else {
                            cols[2].selectable_label(false, album_name)
                        }
                    };

                    let _track_duration_label = {
                        let duration = format!("{}:{:02}", (track.duration_ms / 1000) / 60, (track.duration_ms / 1000) % 60);
                        let mut duration_string = duration.clone();

                        let available_width = cols[3].available_width();
                        let trimmed = utils::trim_string(available_width, glyph_width, &mut duration_string);

                        if trimmed {
                            cols[3].selectable_label(false, duration_string).on_hover_text(duration)
                        }
                        else {
                            cols[3].selectable_label(false, duration_string)
                        }
                    };
                    
                    if track_name_label.clicked() && self.is_playlist_ready() {
                        let tracks = {
                            if is_user_playlist {
                                self.v.playback_status.current_playlist_tracks.clone()
                            }
                            else {
                                self.v.playback_status.recommendations.clone()
                            }
                        };

                        self.v.playback_status.paused = false;
                        self.v.playback_status.started = true;
                        self.send_player_msg(PlayerControl::StartPlaylistAtTrack(tracks.clone(), track.clone()));
                    }

                    track_name_label.context_menu(| ui | {
                        if ui.selectable_label(false, "Play from here").clicked() {
                            let tracks = {
                                if is_user_playlist {
                                    self.v.playback_status.current_playlist_tracks.clone()
                                }
                                else {
                                    self.v.playback_status.recommendations.clone()
                                }
                            };

                            start_playlist = Some((tracks, track.clone()));
                            ui.close_menu();
                        }

                        ui.menu_button("Add to playlist", | ui | {
                            for (id, playlist) in self.v.user_playlists.iter() {
                                if ui.selectable_label(false, playlist.name.as_str()).clicked() {
                                    self.send_worker_msg(WorkerTask::AddTrackToPlaylist(track.id.clone(), id.clone()));
                                    self.send_worker_msg(WorkerTask::GetUserPlaylists);

                                    ui.close_menu();
                                }
                            }
                        });

                        if ui.selectable_label(false, "Remove").clicked() {
                            let id = {
                                if is_user_playlist {
                                    if let Some(i) = self.v.playback_status.current_playlist.as_ref() {
                                        self.v.user_playlists[*i].0.clone()
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
                }
            });

            if let Some((playlist, track_id, track_idx)) = remove_track {
                if is_user_playlist {
                    self.v.fetching_user_playlists = true;
                    self.v.playback_status.current_playlist_tracks.remove(track_idx);
                    self.send_worker_msg(WorkerTask::RemoveTrackFromPlaylist(track_id, playlist));
                    self.send_worker_msg(WorkerTask::GetUserPlaylists);
                }
                else {
                    self.v.playback_status.recommendations.remove(track_idx);
                }
            }

            if let Some((playlist, track)) = start_playlist {
                self.v.playback_status.paused = false;
                self.v.playback_status.started = true;
                self.send_player_msg(PlayerControl::StartPlaylistAtTrack(playlist, track));
            }

            ui.style_mut().wrap = None;
        });
    }

    fn handle_messages(&mut self, frame: &epi::Frame) {
        if let Some(rx) = self.v.state_rx.as_mut() {
            if let Ok(state) = rx.try_recv() {
                match state {
                    PlayerStateUpdate::Paused => {
                        self.v.playback_status.paused = true;
                    }
                    PlayerStateUpdate::Resumed => {
                        self.v.playback_status.paused = false;
                    }
                    PlayerStateUpdate::Stopped => {
                        self.v.playback_status.current_track = None;

                        if let Some(id) = self.v.texture_album_cover {
                            frame.free_texture(id);
                            self.v.texture_album_cover = None;
                        }
                    }
                    PlayerStateUpdate::EndOfTrack(track) => {
                        self.v.playback_status.paused = false;
                        self.v.playback_status.current_track = Some(track);

                        if let Some(id) = self.v.texture_album_cover {
                            frame.free_texture(id);
                            self.v.texture_album_cover = None;
                        }
                    }
                }
            }
        }

        if let Some(rx) = self.v.worker_result_rx.as_mut() {
            if let Ok(worker_res) = rx.try_recv() {
                match worker_res {
                    WorkerResult::Login(result) => {
                        if result {
                            self.v.logged_in = true;
                        }
    
                        self.v.login_password = String::new();
                    }
                    WorkerResult::UserPlaylists(playlists) => {
                        self.v.user_playlists = playlists;
                        self.v.fetching_user_playlists = false;
                        
                        for id in self.v.textures_user_playlists_covers.iter().flatten() {
                            frame.free_texture(*id);
                        }
                        
                        self.v.textures_user_playlists_covers = vec![None; self.v.user_playlists.len()];
                    }
                    WorkerResult::FeaturedPlaylists(playlists) => {
                        self.v.featured_playlists = playlists;
                        self.v.fetching_featured_playlists = false;
                        
                        for id in self.v.textures_featured_playlists_covers.iter().flatten() {
                            frame.free_texture(*id);
                        }

                        self.v.textures_featured_playlists_covers = vec![None; self.v.featured_playlists.len()];
                    }
                    WorkerResult::PlaylistTrackInfo(tracks) => {
                        self.v.playback_status.current_playlist_tracks = tracks;
                    }
                    WorkerResult::PlaylistRecommendations(tracks) => {
                        self.v.playback_status.recommendations = tracks;
                        self.v.fetching_playlist_recommendations = false;
                    }
                }
            }
        }
    }

    fn is_playlist_ready(&self) -> bool {
        match self.v.current_panel {
            CurrentPanel::Home => false,
            CurrentPanel::Playlist => {
                if let Some(i) = self.v.playback_status.current_playlist.as_ref() {
                    if let Some((_, p)) = self.v.user_playlists.get(*i) {
                        self.v.playback_status.current_playlist_tracks.len() == p.tracks.len()
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
                !self.v.fetching_playlist_recommendations && !self.v.playback_status.recommendations.is_empty()
            }
        }
    }

    fn send_worker_msg(&self, message: WorkerTask) {
        if let Some(tx) = self.v.worker_task_tx.as_ref() {
            tx.send(message).unwrap();
        }
    }

    fn send_player_msg(&self, message: PlayerControl) {
        if let Some(tx) = self.v.control_tx.as_ref() {
            tx.send(message).unwrap();
        }
    }
}
