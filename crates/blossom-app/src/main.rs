//! Blossom: a desktop org-mode notes app.
//!
//! `blossom [VAULT]` opens `VAULT`, or the `vault` from
//! `~/.config/blossom/config.toml`, or a welcome screen to choose one.

mod app;
mod outline;
mod read;
mod shot;
mod style;
mod tree;
mod worker;

use std::path::PathBuf;

use blossom_vault::config::{self, Config};

use crate::app::App;
use crate::worker::OpenVault;

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
    let template = OpenVault {
        root: PathBuf::new(),
        keywords: config.keywords(),
        ignore: config.ignore.clone(),
        index_dir: None,
    };
    let root = arg.or_else(|| config.vault.as_deref().map(config::expand));
    let open = root.map(|root| OpenVault {
        root,
        ..template.clone()
    });
    let today = chrono::Local::now().date_naive();
    let shot = shot::Plan::from_env();
    iced::application(
        move || App::new(open.clone(), template.clone(), today, shot.clone()),
        App::update,
        App::view,
    )
    .title(App::title)
    .theme(App::theme)
    .subscription(App::subscription)
    .window_size((1200.0, 760.0))
    .run()
}
