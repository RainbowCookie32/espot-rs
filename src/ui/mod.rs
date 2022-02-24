mod utils;

use std::path::PathBuf;

use eframe::{egui, epi};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use librespot::metadata::Playlist;
use rspotify::model::{SearchResult, SearchType};

use crate::spotify::*;

enum CurrentPanel {
    Home,
    Search { query: String, search_type: SearchType, result: Option<SearchResult>, tracks_info: Vec<TrackInfo>, waiting_for_info: bool },
    Playlist { id: String, data: Playlist, tracks_info: Vec<TrackInfo>, waiting_for_info: bool },
    Recommendations { tracks_info: Vec<TrackInfo>, waiting_for_info: bool }
}

// PartialEq on CurrentPanel is only used to determine which panel is selected,
// checking their fields' equivalences is irrelevant.
impl PartialEq for CurrentPanel {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (CurrentPanel::Search { .. }, CurrentPanel::Search { .. }) => true,
            (CurrentPanel::Playlist { .. }, CurrentPanel::Playlist { .. }) => true,
            (CurrentPanel::Recommendations { .. }, CurrentPanel::Recommendations { .. }) => true,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
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

    current_track: Option<TrackInfo>
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
    waiting_for_login_result: bool,

    current_panel: CurrentPanel,

    user_playlists: Vec<(String, Playlist)>,
    featured_playlists: Vec<(String, Playlist)>,

    fetching_user_playlists: bool,
    fetching_featured_playlists: bool,

    playback_status: PlaybackStatus,

    state_rx: Option<broadcast::Receiver<PlayerStateUpdate>>,
    control_tx: Option<mpsc::UnboundedSender<PlayerControl>>,

    worker_task_tx: Option<mpsc::UnboundedSender<WorkerTask>>,
    worker_result_rx: Option<mpsc::UnboundedReceiver<WorkerResult>>,

    texture_no_cover: Option<egui::TextureHandle>,
    texture_album_cover: Option<egui::TextureHandle>,
    
    textures_user_playlists_covers: Vec<Option<egui::TextureHandle>>,
    textures_featured_playlists_covers: Vec<Option<egui::TextureHandle>>
}

#[derive(Deserialize, Serialize)]
pub struct EspotApp {
    p: PersistentData,
    #[serde(skip)]
    v: VolatileData
}

impl Default for EspotApp {
    fn default() -> EspotApp {
        let p = PersistentData {
            cache_path: dirs::cache_dir().unwrap().join("espot-rs"),
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

    fn setup(&mut self, ctx: &egui::Context, _frame: &epi::Frame, storage: Option<&dyn epi::Storage>) {
        if let Some(storage) = storage {
            *self = epi::get_value(storage, epi::APP_KEY).unwrap_or_default()
        }

        if self.v.worker_task_tx.is_none() {
            let (
                worker_task_tx,
                worker_result_rx,
                state_rx,
                state_rx_dbus,
                control_tx
            ) = SpotifyWorker::start();

            #[cfg(target_os = "linux")]
            #[cfg(not(debug_assertions))]
            crate::dbus::start_dbus_server(state_rx_dbus, control_tx.clone());

            self.v.state_rx = Some(state_rx);
            self.v.control_tx = Some(control_tx);

            self.v.worker_task_tx = Some(worker_task_tx);
            self.v.worker_result_rx = Some(worker_result_rx);
        }

        self.v.playback_status.paused = true;
        self.p.cache_path = dirs::cache_dir().unwrap().join("espot-rs");

        let mut definitions = egui::FontDefinitions::default();

        if let Ok(font) = std::fs::read("resources/fonts/japanese.otf") {
            let font_data = egui::FontData::from_owned(font);

            definitions.font_data.insert("jp_font".to_owned(), font_data);

            if let Some(f) = definitions.families.get_mut(&egui::FontFamily::Monospace) {
                f.push(String::from("jp_font"));
            }

            if let Some(f) = definitions.families.get_mut(&egui::FontFamily::Proportional) {
                f.push(String::from("jp_font"));
            }
        }

        ctx.set_fonts(definitions)
    }

    fn save(&mut self, storage: &mut dyn epi::Storage) {
        epi::set_value(storage, epi::APP_KEY, self);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &epi::Frame) {
        if self.v.texture_no_cover.is_none() {
            let buffer = include_bytes!("../../resources/no_cover.png");
            self.v.texture_no_cover = utils::create_texture_from_bytes(ctx, buffer);
        }

        if let Some(track) = self.v.playback_status.current_track.as_ref() {
            if self.v.texture_album_cover.is_none() {
                let path = self.p.cache_path.join(format!("cover-{}", &track.album_id));
                self.v.texture_album_cover = utils::create_texture_from_file(ctx, path);
            }
        }

        for (i, target) in self.v.textures_user_playlists_covers.iter_mut().enumerate() {
            if target.is_none() {
                let (playlist_id, _) = &self.v.user_playlists[i];
                let path = self.p.cache_path.join(format!("cover-{}", playlist_id));
            
                *target = utils::create_texture_from_file(ctx, path);
            }
        }

        for (i, target) in self.v.textures_featured_playlists_covers.iter_mut().enumerate() {
            if target.is_none() {
                let (playlist_id, _) = &self.v.featured_playlists[i];
                let path = self.p.cache_path.join(format!("cover-{}", playlist_id));
            
                *target = utils::create_texture_from_file(ctx, path);
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

        self.handle_messages();

        // TODO: Workaround for not being able to figure out how to request a repaint
        //       from the worker thread. Burns more resources than needed.
        ctx.request_repaint();
    }
}

impl EspotApp {
    fn draw_login_screen(&mut self, ctx: &egui::Context) {
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

                if !self.v.waiting_for_login_result {
                    if ui.button("Log in").clicked() || submitted {
                        self.v.waiting_for_login_result = true;
                        self.send_worker_msg(WorkerTask::Login(self.p.login_username.clone(), self.v.login_password.clone()));
                    }
                }
                else {
                    ui.add(egui::Spinner::new());
                }
            });
        });
    }

    fn draw_main_screen(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("playback_status").show(ctx, | ui | {
            self.draw_playback_status(ui);
        });

        egui::SidePanel::left("side_panel").show(ctx, | ui | {
            self.draw_side_panel(ui);
        });

        egui::CentralPanel::default().show(ctx, | ui | {
            match &self.v.current_panel {
                CurrentPanel::Home => self.draw_home_panel(ui),
                CurrentPanel::Search { .. } => self.draw_search_panel(ui),
                CurrentPanel::Playlist { .. } => self.draw_playlist_panel(ui),
                CurrentPanel::Recommendations { .. } => self.draw_recommendations_panel(ui)
            }
        });
    }

    fn draw_playback_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if let Some(handle) = self.v.texture_album_cover.as_ref() {
                ui.image(handle.id(), egui::vec2(96.0, 96.0));
            }
            else if let Some(handle) = self.v.texture_no_cover.as_ref() {
                ui.image(handle.id(), egui::vec2(96.0, 96.0));
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
                                    match &self.v.current_panel {
                                        CurrentPanel::Search { tracks_info, .. } => tracks_info.clone(),
                                        CurrentPanel::Playlist { tracks_info, .. } => tracks_info.clone(),
                                        CurrentPanel::Recommendations { tracks_info, .. } => tracks_info.clone(),
                                        _ => return
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

            {
                let checked = matches!(self.v.current_panel, CurrentPanel::Search { .. });

                if ui.selectable_label(checked, "Search").clicked() {
                    self.v.current_panel = CurrentPanel::Search {
                        query: String::new(),
                        search_type: SearchType::Track,
                        result: None,
                        tracks_info: Vec::new(),
                        waiting_for_info: false
                    };
                }
            }

            ui.separator();

            let playlists = ui.collapsing("Playlists", | ui | {
                if !self.v.user_playlists.is_empty() {
                    let font_id = egui::TextStyle::Body.resolve(ui.style());
                    let glyph_width = ui.fonts().glyph_width(&font_id, 'A');

                    for (_id, p) in self.v.user_playlists.iter() {
                        let checked = {
                            match &self.v.current_panel {
                                CurrentPanel::Playlist { id, .. } => id == _id,
                                _ => false
                            }
                        };

                        let mut label = p.name.clone();
                        let trimmed = utils::trim_string(ui.available_width(), glyph_width, &mut label);

                        let mut playlist_label = ui.selectable_label(checked, label);
                        let label_clicked = playlist_label.clicked();
                        
                        let mut get_recommendations = false;
                        let mut opened_from_ctx_menu = false;

                        if trimmed {
                            playlist_label = playlist_label.on_hover_text(&p.name);
                        }

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
                            self.v.current_panel = CurrentPanel::Playlist {
                                id: _id.clone(),
                                data: p.clone(),
                                tracks_info: Vec::new(),
                                waiting_for_info: true
                            };

                            self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(p.clone()));
                        }
                        else if get_recommendations {
                            self.v.current_panel = CurrentPanel::Recommendations {
                                tracks_info: Vec::new(),
                                waiting_for_info: true
                            };
                            
                            self.send_worker_msg(WorkerTask::GetRecommendationsForPlaylist(p.clone()));
                        }
                    }
                }
                else {
                    ui.label("No playlists found...");
                }
            });

            playlists.header_response.context_menu(| ui | {
                if ui.selectable_label(false, "Refresh").clicked() {
                    self.v.current_panel = CurrentPanel::Home;

                    self.v.user_playlists = Vec::new();
                    self.v.fetching_user_playlists = true;

                    self.send_worker_msg(WorkerTask::GetUserPlaylists);

                    ui.close_menu();
                }
            });

            ui.separator();

            let (selected, empty,  waiting) = match &self.v.current_panel {
                CurrentPanel::Recommendations { tracks_info, waiting_for_info } => (true, tracks_info.is_empty(), *waiting_for_info),
                _ => (false, false, true)
            };

            ui.add_enabled(!waiting && !empty, egui::SelectableLabel::new(selected, "Recommendations"));
    }

    fn draw_home_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            ui.heading("Your playlists");

            if self.v.fetching_user_playlists {
                ui.add(egui::Spinner::new());
            }
        });

        ui.separator();

        egui::ScrollArea::horizontal().id_source("user_playlists_scroll").show(ui, | ui | {
            ui.horizontal(| ui | {
                for (i, (id, playlist)) in self.v.user_playlists.iter().enumerate() {
                    let tint = egui::Color32::from_rgba_unmultiplied(96 , 96, 96, 160);

                    let texture_handle = {
                        if let Some(Some(handle)) = self.v.textures_user_playlists_covers.get(i)  {
                            handle
                        }
                        else if let Some(handle) = self.v.texture_no_cover.as_ref() {
                            handle
                        }
                        else {
                            return;
                        }
                    };

                    let button = ui.add(egui::ImageButton::new(texture_handle.id(), egui::vec2(96.0, 96.0)).tint(tint));
                    let text = egui::RichText::new(&playlist.name).strong();
                    let label = ui.put(button.rect, egui::Label::new(text));

                    if button.clicked() || label.clicked() {
                        self.v.current_panel = CurrentPanel::Playlist {
                            id: id.clone(),
                            data: playlist.clone(),
                            tracks_info: Vec::new(),
                            waiting_for_info: true
                        };

                        self.send_worker_msg(WorkerTask::GetPlaylistTracksInfo(playlist.clone()));
                    }
                }
            });
        });

        ui.add_space(20.0);

        ui.horizontal(| ui | {
            ui.heading("Featured by Spotify");

            if self.v.fetching_featured_playlists {
                ui.add(egui::Spinner::new());
            }
        });

        ui.separator();

        egui::ScrollArea::horizontal().id_source("spotify_featured_scroll").show(ui, | ui | {
            ui.horizontal(| ui | {
                for (i, (_, playlist)) in self.v.featured_playlists.iter().enumerate() {
                    let tint = egui::Color32::from_rgba_unmultiplied(96 , 96, 96, 160);

                    let texture_handle = {
                        if let Some(Some(handle)) = self.v.textures_featured_playlists_covers.get(i)  {
                            handle
                        }
                        else if let Some(handle) = self.v.texture_no_cover.as_ref() {
                            handle
                        }
                        else {
                            return;
                        }
                    };

                    let button = ui.add(egui::ImageButton::new(texture_handle.id(), egui::vec2(96.0, 96.0)).tint(tint));
                    let text = egui::RichText::new(&playlist.name).strong();
                    let label = ui.put(button.rect, egui::Label::new(text));

                    if button.clicked() || label.clicked() {
                                
                    }
                }
            });
        });
    }

    fn draw_search_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            ui.label("Search query");

            let submitted = {
                if let CurrentPanel::Search { query, search_type, result, waiting_for_info, .. } = &mut self.v.current_panel {
                    let lost_focus = ui.text_edit_singleline(query).lost_focus();

                    ui.separator();

                    egui::ComboBox::from_id_source("search_kind")
                        .selected_text(format!("{:?}", search_type))
                        .show_ui(ui, | ui | {
                            ui.selectable_value(search_type, SearchType::Track, "Track");
        
                            ui.add_enabled_ui(false, | ui | {
                                ui.selectable_value(search_type, SearchType::Album, "Album");
                                ui.selectable_value(search_type, SearchType::Artist, "Artist");
                                ui.selectable_value(search_type, SearchType::Playlist, "Playlist");
                                ui.selectable_value(search_type, SearchType::Show, "Show");
                            });
                        })
                    ;
        
                    ui.separator();

                    let enabled = !query.is_empty() && !*waiting_for_info;
            
                    let button = ui.add_enabled(enabled, egui::Button::new("Search"));
                    let submitted = button.clicked() || (lost_focus & ui.input().key_pressed(egui::Key::Enter));
    
                    if submitted {
                        *result = None;
                        *waiting_for_info = true;
                    }

                    submitted
                }
                else {
                    false
                }
            };

            if submitted {
                if let CurrentPanel::Search { query, search_type, .. } = &self.v.current_panel {
                    self.send_worker_msg(WorkerTask::Search(query.clone(), *search_type));
                }
            }
        });

        ui.separator();
        ui.style_mut().wrap = Some(false);

        egui::ScrollArea::vertical().show(ui, | ui | {
            if let CurrentPanel::Search { result, waiting_for_info, .. } = &self.v.current_panel {
                if !*waiting_for_info {
                    if let Some(results) = result.as_ref() {
                        match results {
                            SearchResult::Tracks(_) => {
                                self.draw_songs_list(ui);
                            }
                            SearchResult::Artists(_) => {},
                            _ => {},
                        }
                    }
                }
            }
        });
    }

    fn draw_playlist_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if let CurrentPanel::Playlist { data, tracks_info, .. } = &self.v.current_panel {
                let label = {
                    if data.tracks.len() == 1 {
                        format!("{} (1 track)", &data.name)
                    }
                    else {
                        format!("{} ({} tracks)", &data.name, data.tracks.len())
                    }
                };

                ui.strong(label);

                if !self.is_playlist_ready() {
                    ui.add(egui::Spinner::new());
                }
                else if ui.button("Play").clicked() {
                    self.v.playback_status.started = true;
                    self.send_player_msg(PlayerControl::StartPlaylist(tracks_info.clone()));
                }
            }
            else {
                ui.strong("Select a playlist on the sidebar...");
            }
        });

        ui.separator();
        self.draw_songs_list(ui);
    }

    fn draw_recommendations_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(| ui | {
            if let CurrentPanel::Recommendations { tracks_info, waiting_for_info } = &self.v.current_panel {
                if !waiting_for_info {
                    let tracks = tracks_info.len();
    
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
                    ui.add(egui::Spinner::new());
                }
    
                if ui.button("Play").clicked() {
                    self.v.playback_status.started = true;
                    self.send_player_msg(PlayerControl::StartPlaylist(tracks_info.clone()));
                }
            }
        });

        ui.separator();
        self.draw_songs_list(ui);
    }

    fn draw_songs_list(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, | ui | {
            ui.style_mut().wrap = Some(false);

            let mut remove_track = None;
            let mut start_playlist = None;

            ui.columns(4, | cols | {
                let tracks_iter = {
                    match &self.v.current_panel {
                        CurrentPanel::Playlist { tracks_info, .. } => {
                            tracks_info.iter()
                        }
                        CurrentPanel::Recommendations { tracks_info, .. } => {
                            tracks_info.iter()
                        }
                        CurrentPanel::Search { tracks_info, .. } => {
                            tracks_info.iter()
                        }
                        _ => return
                    }
                };

                cols[0].label("Title");
                cols[1].label("Artists");
                cols[2].label("Album");
                cols[3].label("Duration");

                for (track_idx, track) in tracks_iter.enumerate() {
                    let track_name_label = {
                        let mut track_name = track.name.clone();

                        let glyph_width = {
                            let chars = track_name.chars().collect::<Vec<char>>();
                            let font_id = egui::TextStyle::Body.resolve(cols[0].style());
                            
                            cols[0].fonts().glyph_width(&font_id, chars[0])
                        };

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

                        let glyph_width = {
                            let chars = artists_string.chars().collect::<Vec<char>>();
                            let font_id = egui::TextStyle::Body.resolve(cols[1].style());
                            
                            cols[1].fonts().glyph_width(&font_id, chars[0])
                        };

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

                        let glyph_width = {
                            let chars = album_name.chars().collect::<Vec<char>>();
                            let font_id = egui::TextStyle::Body.resolve(cols[2].style());
                            
                            cols[2].fonts().glyph_width(&font_id, chars[0])
                        };

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
                        let glyph_width = cols[3].fonts().glyph_width(&egui::TextStyle::Body.resolve(cols[3].style()), '0');
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
                            match &self.v.current_panel {
                                CurrentPanel::Search { tracks_info, .. } => {
                                    tracks_info.clone()
                                }
                                CurrentPanel::Playlist { tracks_info, .. } => {
                                    tracks_info.clone()
                                }
                                CurrentPanel::Recommendations { tracks_info, .. } => {
                                    tracks_info.clone()
                                }
                                _ => {
                                    return;
                                }
                            }
                        };

                        self.v.playback_status.paused = false;
                        self.v.playback_status.started = true;

                        self.send_player_msg(PlayerControl::StartPlaylistAtTrack(tracks, track.clone()));
                    }

                    track_name_label.context_menu(| ui | {
                        if ui.selectable_label(false, "Play from here").clicked() {
                            let tracks = {
                                match &self.v.current_panel {
                                    CurrentPanel::Search { tracks_info, .. } => {
                                        tracks_info.clone()
                                    }
                                    CurrentPanel::Playlist { tracks_info, .. } => {
                                        tracks_info.clone()
                                    }
                                    CurrentPanel::Recommendations { tracks_info, .. } => {
                                        tracks_info.clone()
                                    }
                                    _ => {
                                        return;
                                    }
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

                        if let CurrentPanel::Playlist { id, .. } = &self.v.current_panel {
                            if ui.selectable_label(false, "Remove").clicked() {
                                let id = id.clone();

                                remove_track = Some((id, track.id.clone(), track_idx));
                                ui.close_menu();
                            }
                        }
                    });
                }
            });

            if let Some((playlist, track_id, track_idx)) = remove_track {
                match &mut self.v.current_panel {
                    CurrentPanel::Playlist { tracks_info, waiting_for_info, .. } => {
                        *waiting_for_info = true;
                        tracks_info.remove(track_idx);
    
                        self.send_worker_msg(WorkerTask::RemoveTrackFromPlaylist(track_id, playlist));
                        self.send_worker_msg(WorkerTask::GetUserPlaylists);
                    }
                    CurrentPanel::Recommendations { tracks_info, .. } => {
                        tracks_info.remove(track_idx);
                    }
                    _ => {}
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

    fn handle_messages(&mut self) {
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
                        self.v.texture_album_cover = None;
                    }
                    PlayerStateUpdate::EndOfTrack(track) => {
                        self.v.playback_status.paused = false;
                        self.v.playback_status.current_track = Some(track);
                        self.v.texture_album_cover = None;
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
                        self.v.waiting_for_login_result = false;
                    }
                    WorkerResult::UserPlaylists(playlists) => {
                        self.v.user_playlists = playlists;
                        self.v.fetching_user_playlists = false;
                        self.v.textures_user_playlists_covers = vec![None; self.v.user_playlists.len()];
                    }
                    WorkerResult::FeaturedPlaylists(playlists) => {
                        self.v.featured_playlists = playlists;
                        self.v.fetching_featured_playlists = false;
                        self.v.textures_featured_playlists_covers = vec![None; self.v.featured_playlists.len()];
                    }
                    WorkerResult::SearchResult(s_result) => {
                        if let CurrentPanel::Search { result, tracks_info, waiting_for_info, .. } = &mut self.v.current_panel {
                            if let SearchResult::Tracks(tracks) = &s_result {
                                *tracks_info = tracks.items
                                    .iter()
                                    .filter_map(| t | TrackInfo::new(t.clone()))
                                    .collect()
                                ;
                            }
    
                            *result = Some(s_result);
                            *waiting_for_info = false;
                        }
                        
                    }
                    WorkerResult::PlaylistTrackInfo(tracks) => {
                        if let CurrentPanel::Playlist { tracks_info, waiting_for_info, .. } = &mut self.v.current_panel {
                            *tracks_info = tracks;
                            *waiting_for_info = false;
                        }
                    }
                    WorkerResult::PlaylistRecommendations(tracks) => {
                        if let CurrentPanel::Recommendations { tracks_info, waiting_for_info } = &mut self.v.current_panel {
                            *tracks_info = tracks;
                            *waiting_for_info = false;
                        }
                    }
                }
            }
        }
    }

    fn is_playlist_ready(&self) -> bool {
        match &self.v.current_panel {
            CurrentPanel::Home => self.v.playback_status.started,
            CurrentPanel::Search { result, tracks_info, waiting_for_info, .. } => {
                result.is_some() && !tracks_info.is_empty() && !waiting_for_info
            }
            CurrentPanel::Playlist { data, tracks_info, waiting_for_info, .. } => {
                data.tracks.len() == tracks_info.len() && !waiting_for_info
            }
            CurrentPanel::Recommendations { tracks_info, waiting_for_info } => {
                !tracks_info.is_empty() && !waiting_for_info
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
