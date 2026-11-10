#![cfg_attr(not(test), windows_subsystem = "windows")]

mod config;
mod media;
mod paths;
mod process;
mod queue;
mod staging;
mod storage;
mod thumbs;
mod tools;
mod tray;
mod ui;
mod win;
mod ytdlp;

use std::sync::Arc;

use gpui::{
    px, size, App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use config::Config;
use queue::Manager;
use ui::{assets::Assets, text_input, Boot, Cadence};

fn main() {
    if !win::claim_single_instance() {
        win::show_running_instance();
        return;
    }

    Application::new().with_assets(Assets).run(|cx: &mut App| {
        text_input::bind_keys(cx);

        let (changed_tx, changed_rx) = async_channel::bounded(1);
        let manager = Manager::new(
            Arc::new(ytdlp::YtDlpRunner),
            storage::data_dir().join("queue.json"),
            changed_tx,
        );

        let (tray_tx, tray_rx) = async_channel::unbounded();
        let tray = match tray::create(tray_tx) {
            Ok(tray) => Some(tray),
            Err(err) => {
                eprintln!("tray icon unavailable: {err}");
                None
            }
        };

        let boot = Boot {
            config: Config::load(),
            tools: tools::locate(),
            manager,
            changed: changed_rx,
            tray,
            tray_actions: tray_rx,
        };

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(480.), px(680.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Cadence".into()),
                appears_transparent: true,
                traffic_light_position: None,
            }),
            window_min_size: Some(size(px(420.), px(520.))),
            app_id: Some("Cadence".to_string()),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            cx.new(|cx| Cadence::new(window, cx, boot))
        })
        .expect("the main window opens");
        cx.activate(true);
    });
}
