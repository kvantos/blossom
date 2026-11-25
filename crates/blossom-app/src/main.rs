//! Blossom: a desktop org-mode notes app.
//!
//! `blossom [VAULT]` opens `VAULT`, or the `vault` from
//! `~/.config/blossom/config.toml`, or a welcome screen to choose one.

mod agenda;
mod app;
mod diagram;
mod draw;
mod editor;
mod excalidraw;
mod file;
mod fonts;
mod meaning;
mod outline;
mod panels;
mod read;
mod search;
mod settings;
mod shot;
mod style;
mod tree;
mod worker;

use std::path::PathBuf;

use blossom_vault::config::{self, Config};

use crate::app::App;
use crate::worker::OpenVault;

/// The app id: the name of the desktop entry and of the `AppStream` file,
/// and the Wayland app id, so the desktop matches the window to its icon.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const APP_ID: &str = "io.github.kvantos.Blossom";

/// The window: size, icon (for X11 and taskbars that read it) and app id.
fn window_settings() -> iced::window::Settings {
    let icon =
        iced::window::icon::from_file_data(include_bytes!("../../../assets/icons/256.png"), None);
    #[cfg(target_os = "linux")]
    let platform_specific = iced::window::settings::PlatformSpecific {
        application_id: APP_ID.to_string(),
        ..Default::default()
    };
    #[cfg(not(target_os = "linux"))]
    let platform_specific = iced::window::settings::PlatformSpecific::default();
    iced::window::Settings {
        size: iced::Size::new(1200.0, 760.0),
        icon: icon.ok(),
        platform_specific,
        ..Default::default()
    }
}

fn main() -> iced::Result {
    let arg = std::env::args_os().nth(1).map(PathBuf::from);
    if arg.as_deref().is_some_and(|a| a == "--help" || a == "-h") {
        println!(
            "usage: blossom [VAULT]\n\nOpens the folder VAULT, or `vault` from ~/.config/blossom/config.toml."
        );
        return Ok(());
    }
    let config = match config::default_path().map(|p| Config::load(&p)) {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            eprintln!("blossom: {e}");
            Config::default()
        }
        None => Config::default(),
    };
    let template = OpenVault::new(PathBuf::new(), &config);
    let root = arg.or_else(|| config.vault.as_deref().map(config::expand));
    let open = root.map(|root| OpenVault {
        root,
        ..template.clone()
    });
    let today = chrono::Local::now().date_naive();
    let shot = shot::Plan::from_env();
    let settings_path = settings::default_path();
    let saved = match settings_path.as_deref().map(settings::Settings::load) {
        Some((s, err)) => {
            if let Some(e) = err {
                eprintln!("blossom: {e}");
            }
            s
        }
        None => settings::Settings::default(),
    };
    iced::application(
        move || {
            App::new(
                open.clone(),
                template.clone(),
                today,
                shot.clone(),
                (saved.clone(), settings_path.clone()),
            )
        },
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .window(window_settings())
    // Closing first writes pending saves (see `App::close`).
    .exit_on_close_request(false)
    .run()
}
