//! Crawler, ported to Rune: a native macOS app for finding and reading files.
//!
//! `cargo run` opens the window. Headless checks use Rune's tooling together with
//! Crawler's own launch flags, for example:
//! `crawler --section folders --folder /tmp --snap out.png --size 1200x760`.

// Stubs fill in module by module during the port.
#![allow(dead_code)]

mod activity;
mod app;
mod args;
mod funnel;
mod index;
mod opening;
mod pdf;
mod preview;
mod settings;
mod shell;
mod theme;
mod transfers;

use rune::{AppSpec, Chrome};

fn main() {
    let spec = AppSpec::<app::App> {
        min_size: Some((760.0, 480.0)),
        fonts: theme::FONTS,
        setup: Some(app::setup),
        poll: Some(app::poll),
        on_key: Some(app::on_key),
        scenes: app::SCENES,
        ..AppSpec::new("Crawler", (1180.0, 760.0), app::App::launch, app::view)
    };
    preview::with_space_window(rune::App::new(spec)
        .chrome(Chrome { traffic_lights: Some((14.0, 14.0)), drag_regions: true, ..Chrome::DEFAULT })
        .on_close(preview::main_closed))
        .run();
}
