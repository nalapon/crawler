//! Native-only input and content frame timings. Rune's --native --trace also records Metal presents.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use rune::{FrameOutcome, Runtime};

use crate::app::{App, Reader};
use crate::{funnel, pdf, preview};

#[derive(Default)]
pub struct Inputs {
    pub search: Option<(u64, Instant)>,
    pub folder: Option<(u64, Instant)>,
    pub open: Option<(PathBuf, Instant)>,
}

pub fn attach(host: rune::App<App>) -> rune::App<App> {
    if !rune::tooling::extra_args()
        .iter()
        .any(|arg| arg == "--frame-timing")
    {
        return host;
    }
    let encoded = Rc::new(Cell::new(None));
    let stamp = encoded.clone();
    let mut search = None;
    let mut folder = None;
    let mut reader = None;
    let mut first_frame = true;
    host.on_frame_encoded(move |frame| {
        if frame.outcome() == FrameOutcome::Drawn {
            stamp.set(Some((frame.seq(), frame.at())));
        }
    })
    .on_frame_drawn(move |rt| {
        let Some((seq, at)) = encoded.take() else {
            return;
        };
        let app = &rt.state;
        let input = &app.frame_inputs;
        let started = app.state_started();
        let log = |kind, from: Option<Instant>| {
            let ms = |origin| at.saturating_duration_since(origin).as_secs_f64() * 1_000.0;
            eprintln!(
                "crawler-frame {}",
                serde_json::json!({
                    "kind": kind,
                    "sequence": seq,
                    "state_to_encode_ms": ms(started),
                    "input_to_encode_ms": from.map(ms),
                })
            );
        };
        if first_frame {
            first_frame = false;
            log("window", None);
        }
        if let Some(id) = funnel::frame_search_result(app)
            && search != Some(id)
            && visible(rt, "search-main-list")
        {
            search = Some(id);
            log(
                "search",
                input
                    .search
                    .filter(|(query, _)| *query == id)
                    .map(|(_, at)| at),
            );
        }
        if let Some((_, visit)) = funnel::frame_folder(app)
            && folder != Some(visit)
        {
            folder = Some(visit);
            log(
                "folder",
                input
                    .folder
                    .filter(|(id, _)| *id == visit)
                    .map(|(_, at)| at),
            );
        }
        let active_reader = match app.reader {
            Reader::Document => preview::frame_reader(app).map(std::path::Path::to_owned),
            Reader::Pdf => pdf::active_path(app).filter(|path| pdf::is_ready(app, path)),
            Reader::None => None,
        };
        if active_reader != reader && (active_reader.is_none() || visible(rt, "reader-sheet")) {
            reader = active_reader;
            if let Some(path) = reader.as_deref() {
                log(
                    "reader",
                    input
                        .open
                        .as_ref()
                        .filter(|(opened, _)| opened == path)
                        .map(|(_, at)| *at),
                );
            }
        }
    })
}

fn visible(rt: &Runtime<App>, key: &str) -> bool {
    let Some((rect, opacity)) = rt.presented(key) else {
        return false;
    };
    let Some((viewport, _)) = rt.presented("workspace") else {
        return false;
    };
    opacity > 0.0
        && rect[2] > 0.0
        && rect[3] > 0.0
        && rect[0] < viewport[0] + viewport[2]
        && rect[0] + rect[2] > viewport[0]
        && rect[1] < viewport[1] + viewport[3]
        && rect[1] + rect[3] > viewport[1]
}
