//! Crawler's own launch flags, read from what Rune leaves after its tooling flags.
//! They drive the checks described in AGENTS.md.

use std::path::PathBuf;

use crate::app::Section;

#[derive(Debug, Default, Clone)]
pub struct Launch {
    /// `--section search|folders|activity|transfers|settings` (`--activity`, `--settings`).
    pub section: Option<Section>,
    /// `--folder <absolute-path>`: open Folders at that directory.
    pub folder: Option<PathBuf>,
    /// `--find <query>`: open Search with an initial query.
    pub find: Option<String>,
    /// `--open <absolute-path>`: open a file through the normal Enter policy.
    pub open: Option<PathBuf>,
    /// `--search-root <absolute-path>`: initial browsing/local-search context.
    pub search_root: Option<PathBuf>,
    /// `--search-workers N`: bounded matching worker count.
    pub search_workers: Option<usize>,
    /// `--crawler-theme <name>`: theme for this run only, not persisted (Rune owns `--theme`).
    pub theme: Option<String>,
    /// `--state-report <path>`: write a JSON state report while running.
    pub state_report: Option<PathBuf>,
    pub event_log: Option<PathBuf>,
    pub launch_report: Option<PathBuf>,
    /// `--frame-timing`: report native frames carrying a result, folder or reader.
    pub frame_timing: bool,
    /// Positional paths, as `scripts/crawler [path]` and Finder hand them over.
    pub paths: Vec<PathBuf>,
}

impl Launch {
    pub fn parse() -> Self {
        Self::from_args(rune::tooling::extra_args())
    }

    pub fn from_args(args: &[String]) -> Self {
        let mut out = Launch::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = || it.next().cloned();
            match arg.as_str() {
                "--section" => out.section = value().as_deref().and_then(Section::parse),
                "--activity" => out.section = Some(Section::Activity),
                "--settings" => out.section = Some(Section::Settings),
                "--folder" => out.folder = value().map(PathBuf::from),
                "--find" => out.find = value(),
                "--open" => out.open = value().map(PathBuf::from),
                "--search-root" => out.search_root = value().map(PathBuf::from),
                "--search-workers" => out.search_workers = value().and_then(|v| v.parse().ok()),
                "--crawler-theme" => out.theme = value(),
                "--state-report" => out.state_report = value().map(PathBuf::from),
                "--event-log" => out.event_log = value().map(PathBuf::from),
                "--launch-report" => out.launch_report = value().map(PathBuf::from),
                "--frame-timing" => out.frame_timing = true,
                other if !other.starts_with('-') => out.paths.push(PathBuf::from(other)),
                _ => {}
            }
        }
        out
    }
}
