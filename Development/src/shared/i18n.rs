//! Gettext-style localisation.
//!
//! English source strings are the lookup keys, so untranslated text simply falls back to
//! English. A locale is a flat TOML table of `"English source" = "Translation"` pairs.
//! Bundled locales are compiled in from `locales/`; a file at `<app root>/locales/<code>.toml`
//! overrides or adds a locale without rebuilding. Placeholders use `{name}` and are filled
//! by [`tr_args`] / the [`tr!`](crate::tr) macro after translation, so translators can reorder them.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const DEFAULT_LANGUAGE: &str = "en";
const MAX_LOCALE_BYTES: u64 = 2 * 1024 * 1024;

pub struct LocaleInfo {
    pub code: &'static str,
    pub name: &'static str,
    bundled: &'static str,
}

/// Locales shipped inside the binary. English is the source language and needs no catalog.
pub const BUNDLED: &[LocaleInfo] = &[LocaleInfo {
    code: DEFAULT_LANGUAGE,
    name: "English",
    bundled: "",
}];

static CATALOG: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

pub fn is_valid_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.chars().all(|ch| ch.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=4).contains(&part.len()) && part.chars().all(|ch| ch.is_ascii_alphanumeric())
        })
        && code.len() <= 16
}

fn external_dir(root: &Path) -> PathBuf {
    root.join("locales")
}

/// Bundled locales plus `<root>/locales/*.toml`, as `(code, display name)`.
pub fn available(root: &Path) -> Vec<(String, String)> {
    let mut locales: Vec<(String, String)> = BUNDLED
        .iter()
        .map(|locale| (locale.code.to_owned(), locale.name.to_owned()))
        .collect();
    if let Ok(entries) = std::fs::read_dir(external_dir(root)) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(code) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|_| path.extension().is_some_and(|ext| ext == "toml"))
                .filter(|code| is_valid_code(code))
            else {
                continue;
            };
            if locales.iter().all(|(existing, _)| existing != code) {
                let name = read_catalog(&path)
                    .ok()
                    .and_then(|catalog| catalog.get("@language-name").cloned())
                    .unwrap_or_else(|| code.to_owned());
                locales.push((code.to_owned(), name));
            }
        }
    }
    locales
}

fn read_catalog(path: &Path) -> Result<HashMap<String, String>, String> {
    let metadata = std::fs::metadata(path).map_err(|_| "Locale file is unavailable")?;
    if metadata.len() > MAX_LOCALE_BYTES {
        return Err("Locale file is too large".into());
    }
    let text = std::fs::read_to_string(path).map_err(|_| "Locale file could not be read")?;
    parse_catalog(&text)
}

fn parse_catalog(text: &str) -> Result<HashMap<String, String>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|_| "Locale file is not valid TOML")?;
    Ok(table
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key, value.to_owned())))
        .collect())
}

/// Activates a locale. English (or an unknown code) clears the catalog.
pub fn set_language(code: &str, root: &Path) -> Result<(), String> {
    let catalog = if code == DEFAULT_LANGUAGE || !is_valid_code(code) {
        None
    } else {
        let external = external_dir(root).join(format!("{code}.toml"));
        let mut catalog = BUNDLED
            .iter()
            .find(|locale| locale.code == code && !locale.bundled.is_empty())
            .map(|locale| parse_catalog(locale.bundled))
            .transpose()?
            .unwrap_or_default();
        if external.is_file() {
            catalog.extend(read_catalog(&external)?);
        } else if catalog.is_empty() {
            return Err(format!("Locale {code} is not installed"));
        }
        Some(catalog)
    };
    if let Ok(mut active) = CATALOG.write() {
        *active = catalog;
    }
    Ok(())
}

/// Marks a literal for the translation template without translating it (gettext's `N_`), for
/// labels kept in arrays and translated later with [`tr`].
pub const fn n_(source: &'static str) -> &'static str {
    source
}

/// Translates an English source string; returns it unchanged when no translation exists.
pub fn tr(source: &str) -> Cow<'_, str> {
    let Ok(catalog) = CATALOG.read() else {
        return Cow::Borrowed(source);
    };
    match catalog.as_ref().and_then(|catalog| catalog.get(source)) {
        Some(translation) if !translation.is_empty() => Cow::Owned(translation.clone()),
        _ => Cow::Borrowed(source),
    }
}

/// Translates `source`, then replaces each `{name}` placeholder.
pub fn tr_args(source: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut text = tr(source).into_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

/// `tr!("Copied {count} items", count = n)` — translate, then fill named placeholders.
#[macro_export]
macro_rules! tr {
    ($source:expr) => {
        $crate::shared::i18n::tr($source)
    };
    ($source:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::shared::i18n::tr_args(
            $source,
            &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_and_fills_placeholders() {
        assert_eq!(tr("Unknown"), "Unknown");
        assert_eq!(tr_args("{count} items", &[("count", &3)]), "3 items");
        assert!(is_valid_code("id"));
        assert!(is_valid_code("pt-BR"));
        assert!(!is_valid_code("../x"));
    }
}
