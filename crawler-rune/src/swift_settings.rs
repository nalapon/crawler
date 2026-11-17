//! One-time import of the Swift app's settings, so replacing it keeps the theme, pinned folder,
//! recent files, default editor and Markdown reader zoom. The Swift preferences are only read.

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUserDefaults};
use rune::prefs::Prefs;

use crate::theme::Theme;

const SWIFT_DOMAIN: &str = "studio.crawler.native-prototype";
const IMPORTED: &str = "importedSwiftSettings";

/// Copies settings the port hasn't saved yet from the Swift app's defaults, once.
pub fn import(prefs: &mut Prefs) {
    if prefs.get::<bool>(IMPORTED).is_some() {
        return;
    }
    let defaults = NSUserDefaults::standardUserDefaults();
    if let Some(swift) = defaults.persistentDomainForName(&NSString::from_str(SWIFT_DOMAIN)) {
        copy(prefs, &swift);
    }
    let _ = prefs.set(IMPORTED, &true);
}

fn copy(prefs: &mut Prefs, swift: &NSDictionary<NSString, AnyObject>) {
    let value = |key: &str| swift.objectForKey(&NSString::from_str(key));
    let string = |key: &str| {
        value(key)?
            .downcast::<NSString>()
            .ok()
            .map(|s| s.to_string())
    };
    if prefs.get::<Theme>("theme").is_none()
        && let Some(theme) = string("theme").as_deref().and_then(Theme::parse)
    {
        let _ = prefs.set("theme", &theme);
    }
    if prefs.get::<PathBuf>("pinnedFolder").is_none()
        && let Some(path) = string("pinnedFolder")
    {
        let _ = prefs.set("pinnedFolder", &PathBuf::from(path));
    }
    if prefs.get::<String>("defaultTextEditorBundleID").is_none()
        && let Some(id) = string("defaultTextEditorBundleID")
    {
        let _ = prefs.set("defaultTextEditorBundleID", &id);
    }
    if prefs.get::<Vec<PathBuf>>("recentOpenedFiles").is_none()
        && let Some(list) = value("recentOpenedFiles").and_then(|v| v.downcast::<NSArray>().ok())
    {
        let recent: Vec<PathBuf> = strings(&list).map(PathBuf::from).take(32).collect();
        let _ = prefs.set("recentOpenedFiles", &recent);
    }
    if prefs.get::<f64>("markdownReaderScale").is_none()
        && let Some(scale) = value("markdownReaderScale")
            .and_then(|v| v.downcast::<NSNumber>().ok())
            .map(|n| n.doubleValue())
    {
        let _ = prefs.set("markdownReaderScale", &scale);
    }
}

fn strings(list: &Retained<NSArray>) -> impl Iterator<Item = String> + '_ {
    list.iter()
        .filter_map(|item| item.downcast::<NSString>().ok().map(|s| s.to_string()))
}
