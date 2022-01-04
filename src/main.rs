mod dbus;
mod spotify;
mod spinner;

use eframe::{egui, epi};
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use librespot::metadata::Playlist;
use rspotify::model::{FullTrack, Id};

use spotify::*;

#[derive(Deserialize, Serialize)]
pub struct EspotApp {
    #[serde(skip)]
    logged_in: bool,

    #[serde(skip)]
    login_username: String,
    #[serde(skip)]
    login_password: String,

    #[serde(skip)]
    playlists: Vec<Playlist>,
    #[serde(skip)]
    fetching_playlists: bool,

    #[serde(skip)]
    paused: bool,
    #[serde(skip)]
    playback_started: bool,

    #[serde(skip)]
    current_track: Option<FullTrack>,

    #[serde(skip)]
    selected_playlist: Option<usize>,
    #[serde(skip)]
    selected_playlist_tracks: Vec<FullTrack>,

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
    texture_album_cover: Option<(egui::Vec2, egui::TextureId)>
}

impl Default for EspotApp {
    fn default() -> EspotApp {
        let (worker_task_tx, worker_result_rx, state_rx, _, control_tx) = SpotifyWorker::start();

        EspotApp {
            logged_in: false,

            login_username: String::new(),
            login_password: String::new(),

            playlists: Vec::new(),
            fetching_playlists: false,

            paused: true,
            playback_started: false,

            current_track: None,

            selected_playlist: None,
            selected_playlist_tracks: Vec::new(),

            state_rx: Some(state_rx),
            control_tx: Some(control_tx),

            worker_task_tx: Some(worker_task_tx),
            worker_result_rx: Some(worker_result_rx),

            texture_no_cover: None,
            texture_album_cover: None
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

        self.paused = true;

        if self.worker_task_tx.is_none() {
            let (worker_task_tx, worker_result_rx, state_rx, state_rx_dbus, control_tx) = SpotifyWorker::start();

            dbus::start_dbus_server(state_rx_dbus, control_tx.clone());

            self.state_rx = Some(state_rx);
            self.control_tx = Some(control_tx);

            self.worker_task_tx = Some(worker_task_tx);
            self.worker_result_rx = Some(worker_result_rx);
        }

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

        if let Some(track) = self.current_track.as_ref() {
            if self.texture_album_cover.is_none() {
                let album_id = track.album.id.clone().unwrap();
                let album_id = album_id.id();
                let cover_path = dirs::cache_dir().unwrap().join(format!("espot-rs/cover-{}", album_id));

                if let Ok(buffer) = std::fs::read(cover_path) {
                    self.texture_album_cover = EspotApp::make_cover_image(&buffer, frame);
                }
            }
        }

        if self.logged_in {
            self.main_screen(ctx);

            if self.playlists.is_empty() && !self.fetching_playlists {
                if let Some(tx) = self.worker_task_tx.as_ref() {
                    tx.send(WorkerTask::GetUserPlaylists).unwrap();
                    self.fetching_playlists = true;
                }
            }
        }
        else {
            self.login_screen(ctx);
        }

        if let Some(rx) = self.state_rx.as_mut() {
            if let Ok(state) = rx.try_recv() {
                match state {
                    PlayerStateUpdate::Paused => {
                        self.paused = true;
                    }
                    PlayerStateUpdate::Resumed => {
                        self.paused = false;
                    }
                    PlayerStateUpdate::Stopped => {
                        self.current_track = None;

                        if let Some((_, id)) = self.texture_album_cover {
                            frame.free_texture(id);
                            self.texture_album_cover = None;
                        }
                    }
                    PlayerStateUpdate::EndOfTrack(track) => {
                        self.paused = false;
                        self.current_track = Some(track);

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
                            self.login_username = String::new();
                        }
    
                        self.login_password = String::new();
                    }
                    WorkerResult::Playlists(playlists) => {
                        self.playlists = playlists;
                        self.fetching_playlists = false;
                    }
                    WorkerResult::PlaylistTrackInfo(track) => {
                        self.selected_playlist_tracks.push(track);
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
    fn login_screen(&mut self, ctx: &egui::CtxRef) {
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
                    if let Some(tx) = self.worker_task_tx.as_ref() {
                        tx.send(WorkerTask::Login(self.login_username.clone(), self.login_password.clone())).unwrap();
                    }
                }
            });
        });
    }

    fn main_screen(&mut self, ctx: &egui::CtxRef) {
        egui::TopBottomPanel::bottom("playback_status").show(ctx, | ui | {
            ui.horizontal(| ui | {
                if let Some((size, id)) = self.texture_album_cover.as_ref() {
                    ui.image(id.clone(), size.clone());
                }
                else if let Some((size, id)) = self.texture_no_cover.as_ref() {
                    ui.image(id.clone(), size.clone());
                }

                ui.vertical(| ui | {
                    ui.add_space(5.0);

                    if let Some(track) = self.current_track.as_ref() {
                        let artists_label = {
                            let mut artists_label = String::new();

                            for (i, artist) in track.artists.iter().enumerate() {
                                artists_label.push_str(&artist.name);

                                if i != track.artists.len() - 1 {
                                    artists_label.push_str(", ");
                                }
                            }

                            artists_label
                        };

                        ui.heading(&track.name);
                        ui.label(artists_label);
                    }
                    else {
                        ui.heading("...");
                        ui.label("...");
                    }

                    ui.add_space(5.0);
                    
                    ui.horizontal(| ui | {
                        let can_move = self.playback_started;
                        let can_start = self.is_playlist_ready();
        
                        ui.add_enabled_ui(can_move, | ui | {
                            if ui.button("⏮").clicked() {
                                if let Some(tx) = self.control_tx.as_ref() {
                                    tx.send(PlayerControl::PreviousTrack).unwrap();
                                }
                            }
                        });
        
                        ui.add_enabled_ui(can_start, | ui | {
                            let button_label = if self.paused {"▶"} else {"⏸"};

                            if ui.button(button_label).clicked() {
                                if let Some(tx) = self.control_tx.as_ref() {
                                    if !self.playback_started {
                                        self.playback_started = true;
                                        tx.send(PlayerControl::StartPlaylist(self.selected_playlist_tracks.clone())).unwrap();
                                    }
                                    else if self.paused {
                                        tx.send(PlayerControl::Play).unwrap();
                                    }
                                    else {
                                        tx.send(PlayerControl::Pause).unwrap();
                                    }
            
                                    self.paused = !self.paused;
                                }
                            }
                        });
        
                        ui.add_enabled_ui(can_move, | ui | {
                            if ui.button("⏭").clicked() {
                                if let Some(tx) = self.control_tx.as_ref() {
                                    tx.send(PlayerControl::NextTrack).unwrap();
                                }
                            }
                        });
                    })
                });
            });
        });

        egui::SidePanel::left("side_panel").show(ctx, | ui | {
            ui.separator();
            ui.label("espot-rs");
            ui.separator();

            ui.collapsing("Playlists", | ui | {
                if !self.playlists.is_empty() {
                    for (i, p) in self.playlists.iter().enumerate() {
                        if ui.selectable_label(false, &p.name).clicked() {
                            if let Some(currently_selected) = self.selected_playlist.as_ref() {
                                if i != *currently_selected {
                                    self.selected_playlist = Some(i);
                                    self.selected_playlist_tracks = Vec::with_capacity(p.tracks.len());

                                    if let Some(tx) = self.worker_task_tx.as_ref() {
                                        tx.send(WorkerTask::GetPlaylistTracksInfo(p.clone())).unwrap();
                                    }
                                }
                            }
                            else {
                                self.selected_playlist = Some(i);
                                self.selected_playlist_tracks = Vec::with_capacity(p.tracks.len());

                                if let Some(tx) = self.worker_task_tx.as_ref() {
                                    tx.send(WorkerTask::GetPlaylistTracksInfo(p.clone())).unwrap();
                                }
                            }
                        }
                    }
                }
                else {
                    ui.label("No playlists found...");
                }
            });
        });

        egui::CentralPanel::default().show(ctx, | ui | {
            egui::ScrollArea::vertical().show(ui, | ui | {
                ui.horizontal(| ui | {
                    if let Some(idx) = self.selected_playlist.as_ref() {
                        let track_count = &self.playlists[*idx].tracks.len();
                        let playlist_title = &self.playlists[*idx].name;

                        let label = {
                            if *track_count == 1 {
                                format!("{} (1 track)", playlist_title)
                            }
                            else {
                                format!("{} ({} tracks)", playlist_title, track_count)
                            }
                        };

                        ui.strong(label);
                    }
                    else {
                        ui.strong("Select a playlist on the sidebar...");
                    }

                    if self.selected_playlist.is_some() && !self.is_playlist_ready() {
                        ui.add(spinner::Spinner::new());
                    }
                });

                ui.separator();
                ui.style_mut().wrap = Some(false);

                ui.columns(4, | cols | {
                    cols[0].label("Title");
                    cols[1].label("Artists");
                    cols[2].label("Album");
                    cols[3].label("Duration");

                    for track in self.selected_playlist_tracks.iter() {
                        let glyph_width = cols[0].fonts().glyph_width(egui::TextStyle::Body, 'A');

                        let title_label = EspotApp::trim_string(cols[0].available_width(), glyph_width, track.name.clone());
                        let artists_label = {
                            let mut artists_label = String::new();

                            for (i, artist) in track.artists.iter().enumerate() {
                                artists_label.push_str(&artist.name);

                                if i != track.artists.len() - 1 {
                                    artists_label.push_str(", ");
                                }
                            }

                            EspotApp::trim_string(cols[1].available_width(), glyph_width, artists_label)
                        };

                        let album_label = EspotApp::trim_string(cols[2].available_width(), glyph_width, track.album.name.clone());
                        let duration_label = EspotApp::trim_string(cols[3].available_width(), glyph_width, format!("{}:{:02}", track.duration.as_secs() / 60, track.duration.as_secs() % 60));
                        
                        if cols[0].selectable_label(false, title_label).clicked() {
                            if self.is_playlist_ready() {
                                if let Some(tx) = self.control_tx.as_ref() {
                                    self.paused = false;
                                    self.playback_started = true;
    
                                    tx.send(PlayerControl::StartPlaylistAtTrack(self.selected_playlist_tracks.clone(), track.clone())).unwrap();
                                }
                            }
                        }

                        let _ = cols[1].selectable_label(false, artists_label);
                        let _ = cols[2].selectable_label(false, album_label);
                        let _ = cols[3].selectable_label(false, duration_label);
                    }
                });

                ui.style_mut().wrap = None;
            });
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
        if let Some(i) = self.selected_playlist.as_ref() {
            if let Some(p) = self.playlists.get(*i) {
                self.selected_playlist_tracks.len() == p.tracks.len()
            }
            else {
                false
            }
        }
        else {
            false
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
}

fn main() {
    let app = EspotApp::default();
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(Box::new(app), native_options);
}
