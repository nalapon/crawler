//! Crawler, ported to Rune: a native macOS app for finding and reading files.
//!
//! `cargo run` opens the window. Headless checks use Rune's tooling together with
//! Crawler's own launch flags, for example:
//! `crawler --section folders --folder /tmp --snap out.png --size 1200x760`.

mod activity;
mod app;
mod args;
mod documents;
mod funnel;
mod icons;
mod index;
mod opening;
mod pdf;
mod preview;
mod settings;
mod shell;
mod swift_settings;
mod theme;
mod transfers;

use rune::{AppSpec, Chrome};

fn main() {
    documents::install();
    let spec = AppSpec::<app::App> {
        min_size: Some((760.0, 480.0)),
        fonts: preview::fonts(),
        families: ("System Font", "ui-monospace"),
        setup: Some(app::setup),
        poll: Some(app::poll),
        on_key: Some(app::on_key),
        scenes: app::SCENES,
        ..AppSpec::new("Crawler", (1180.0, 760.0), app::App::launch, app::view)
    };
    preview::with_space_window(
        rune::App::new(spec)
            .chrome(Chrome {
                traffic_lights: Some((14.0, 14.0)),
                drag_regions: true,
                ..Chrome::DEFAULT
            })
            .keep_running()
            .release_gpu_when_closed()
            .menu_item(
                rune::MenuItem::new("Quit Crawler", app::request_quit)
                    .id("crawler-quit")
                    .key_equivalent("cmd+q".parse().expect("valid quit shortcut")),
            )
            .on_window(|_| app::install_quit_menu())
            .on_quit(preview::main_closed)
            .on_close(app::window_closed),
    )
    .run();
}
