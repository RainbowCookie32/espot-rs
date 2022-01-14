use tokio::runtime::Runtime;
use tokio::sync::{broadcast, mpsc};

use zbus::fdo::Result;
use zbus::{Connection, dbus_interface};

use crate::spotify::{PlayerControl, PlayerStateUpdate, TrackInfo};

#[derive(Clone)]
enum PlaybackStatus {
    Playing,
    Paused,
    Stopped
}

impl ToString for PlaybackStatus {
    fn to_string(&self) -> String {
        match self {
            PlaybackStatus::Playing => "Playing".to_string(),
            PlaybackStatus::Paused => "Paused".to_string(),
            PlaybackStatus::Stopped => "Stopped".to_string()
        }
    }
}

struct Mpris;

#[dbus_interface(name = "org.mpris.MediaPlayer2")]
impl Mpris {
    async fn raise(&self) {}

    async fn quit(&self) {}

    #[dbus_interface(property)]
    async fn can_quit(&self) -> bool {
        false
    }

    #[dbus_interface(property)]
    async fn fullscreen(&self) -> bool {
        false
    }

    #[dbus_interface(property)]
    async fn can_set_fullscreen(&self) -> bool {
        false
    }

    #[dbus_interface(property)]
    async fn can_raise(&self) -> bool {
        false
    }

    #[dbus_interface(property)]
    async fn has_track_list(&self) -> bool {
        false
    }

    #[dbus_interface(property)]
    async fn identity(&self) -> &str {
        "espot-rs"
    }
}

struct MprisPlayer {
    pub track: Option<TrackInfo>,
    pub status: PlaybackStatus,

    control_tx: mpsc::UnboundedSender<PlayerControl>
}

#[dbus_interface(name = "org.mpris.MediaPlayer2.Player")]
impl MprisPlayer {
    async fn next(&self) {
        self.control_tx.send(PlayerControl::NextTrack).unwrap();
    }

    async fn previous(&self) {
        self.control_tx.send(PlayerControl::PreviousTrack).unwrap();
    }

    async fn pause(&self) {
        self.control_tx.send(PlayerControl::Pause).unwrap();
    }

    async fn play_pause(&self) {
        self.control_tx.send(PlayerControl::PlayPause).unwrap();
    }

    async fn stop(&self) {
        self.control_tx.send(PlayerControl::Stop).unwrap();
    }

    async fn play(&self) {
        self.control_tx.send(PlayerControl::Play).unwrap();
    }

    #[dbus_interface(property)]
    async fn playback_status(&self) -> String {
        self.status.to_string()
    }

    #[dbus_interface(property)]
    async fn loop_status(&self) -> &str {
        "Playlist"
    }

    #[dbus_interface(property)]
    async fn shuffle(&self) -> bool {
        true
    }

    #[dbus_interface(property)]
    async fn can_go_next(&self) -> bool {
        true
    }

    #[dbus_interface(property)]
    async fn can_go_previous(&self) -> bool {
        true
    }

    #[dbus_interface(property)]
    async fn can_play(&self) -> bool {
        self.track.is_some()
    }

    #[dbus_interface(property)]
    async fn can_pause(&self) -> bool {
        true
    }

    #[dbus_interface(property)]
    async fn can_control(&self) -> bool {
        true
    }
}


pub fn start_dbus_server(state_rx: broadcast::Receiver<PlayerStateUpdate>, control_tx: mpsc::UnboundedSender<PlayerControl>) {
    std::thread::spawn(move || {
        let rt = Runtime::new().unwrap();

        if let Err(e) = rt.block_on(dbus_loop(state_rx, control_tx)) {
            println!("Error in dbus server: {}", e);
        }
    });
}

async fn dbus_loop(state_rx: broadcast::Receiver<PlayerStateUpdate>, control_tx: mpsc::UnboundedSender<PlayerControl>) -> Result<()> {
    let connection = Connection::session().await?;
    let mut state_rx = state_rx;

    let handler = MprisPlayer {
        track: None,
        status: PlaybackStatus::Stopped,

        control_tx
    };

    connection.object_server()
        .at("/org/mpris/MediaPlayer2", Mpris)
        .await?
    ;

    connection.object_server()
        .at("/org/mpris/MediaPlayer2", handler)
        .await?
    ;

    connection
        .request_name("org.mpris.MediaPlayer2.espot")
        .await?
    ;

    let iface_ref = connection.object_server().interface::<_, MprisPlayer>("/org/mpris/MediaPlayer2").await?;

    loop {
        if let Ok(status) = state_rx.recv().await {
            match status {
                PlayerStateUpdate::Paused => {
                    let mut iface_mut = iface_ref.get_mut().await;

                    iface_mut.status = PlaybackStatus::Paused;
                    iface_mut.playback_status_changed(iface_ref.signal_context()).await?;
                }
                PlayerStateUpdate::Resumed => {
                    let mut iface_mut = iface_ref.get_mut().await;

                    iface_mut.status = PlaybackStatus::Playing;
                    iface_mut.playback_status_changed(iface_ref.signal_context()).await?;
                }
                PlayerStateUpdate::Stopped => {
                    let mut iface_mut = iface_ref.get_mut().await;

                    iface_mut.track = None;
                    iface_mut.status = PlaybackStatus::Stopped;
                    iface_mut.can_play_changed(iface_ref.signal_context()).await?;
                    iface_mut.playback_status_changed(iface_ref.signal_context()).await?;
                }
                PlayerStateUpdate::EndOfTrack(track) => {
                    let mut iface_mut = iface_ref.get_mut().await;

                    iface_mut.track = Some(track);
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
