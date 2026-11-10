//! Finding `Overwatch.exe`, its build and its installed languages.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

pub const GAME_EXE: &str = "Overwatch.exe";
pub const OFFICIAL_LAUNCHER: &str = "Overwatch Launcher.exe";
/// The only client build the server supports.
pub const TARGET_VERSION: &str = "1.74.0.0.104319";

/// The game for a path: the game itself, its installation folder, or the
/// official launcher next to it. Nothing is started.
pub fn resolve_game(path: &Path) -> Option<PathBuf> {
    if is_game(path) {
        return plain_canonical(path);
    }
    let folder = if path.is_dir() {
        path
    } else if file_name_is(path, OFFICIAL_LAUNCHER) && path.is_file() {
        path.parent()?
    } else {
        return None;
    };
    [
        folder.join("_retail_").join(GAME_EXE),
        folder.join(GAME_EXE),
    ]
    .into_iter()
    .find(|candidate| is_game(candidate))
    .and_then(|game| plain_canonical(&game))
}

/// The canonical path, without the `\\?\` prefix Windows adds to local paths.
/// The game finds its data next to its own path, so it gets the plain form.
fn plain_canonical(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    Some(
        canonical
            .to_str()
            .map_or(canonical.clone(), |text| PathBuf::from(plain(text))),
    )
}

/// `\\?\D:\x` becomes `D:\x`; UNC and ordinary paths stay as they are.
fn plain(path: &str) -> &str {
    match path.strip_prefix(r"\\?\") {
        Some(local) if !local.starts_with(r"UNC\") => local,
        _ => path,
    }
}

fn is_game(path: &Path) -> bool {
    file_name_is(path, GAME_EXE) && path.is_file()
}

fn file_name_is(path: &Path, name: &str) -> bool {
    path.file_name()
        .and_then(|file| file.to_str())
        .is_some_and(|file| file.eq_ignore_ascii_case(name))
}

/// The installation's `.build.info`: its version and the language codes in its tags.
#[derive(Debug, Default, PartialEq)]
pub struct BuildInfo {
    pub version: Option<String>,
    pub locales: BTreeSet<String>,
}

/// Read the `.build.info` next to the `_retail_` folder, or next to the game.
pub fn build_info(game: &Path) -> Option<BuildInfo> {
    let root = install_root(game)?;
    fs::read_to_string(root.join(".build.info"))
        .ok()
        .and_then(|text| parse_build_info(&text))
}

/// The installation folder: the one holding `.build.info` and `data/casc`,
/// next to the `_retail_` folder or next to the game.
pub fn install_root(game: &Path) -> Option<PathBuf> {
    [game.parent()?.parent(), game.parent()]
        .into_iter()
        .flatten()
        .find(|folder| folder.join(".build.info").is_file())
        .map(Path::to_path_buf)
}

fn parse_build_info(text: &str) -> Option<BuildInfo> {
    let row = orpheus_casc::BuildInfo::parse(text)?;
    Some(BuildInfo {
        version: row.get("Version").map(str::to_owned),
        locales: row
            .get("Tags")
            .map(|tags| {
                tags.split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|tag| is_locale(tag))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A game language code such as `enUS`.
fn is_locale(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    bytes.len() == 4
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2..].iter().all(u8::is_ascii_uppercase)
}

/// The language to pass as `--tank_Locale`. A build without the language
/// the game picks by itself shows a black screen, so pass the system
/// language when the build has it, else one with the same first letters,
/// else `enUS`.
pub fn choose_locale(installed: &BTreeSet<String>, system: Option<&str>) -> Option<String> {
    if installed.is_empty() {
        return None;
    }
    if let Some(system) = system {
        if installed.contains(system) {
            return Some(system.into());
        }
        let language = system.get(..2).unwrap_or(system);
        if let Some(same) = installed.iter().find(|code| code.starts_with(language)) {
            return Some(same.clone());
        }
    }
    installed.get("enUS").cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD_INFO: &str = "Branch!STRING:0|Active!DEC:1|Tags!STRING:0|Version!STRING:0\n\
        us|1|Windows x86_64 US? deDE speech?:enUS text?:zhCN zhTW|1.74.0.0.104319\n";

    #[test]
    fn reads_the_version_and_locale_tags() {
        let info = parse_build_info(BUILD_INFO).unwrap();
        assert_eq!(info.version.as_deref(), Some(TARGET_VERSION));
        assert_eq!(
            info.locales.iter().map(String::as_str).collect::<Vec<_>>(),
            ["deDE", "enUS", "zhCN", "zhTW"]
        );
    }

    #[test]
    fn prefers_the_system_language_then_the_same_language_then_english() {
        let installed: BTreeSet<String> = ["deDE", "enUS", "zhCN", "zhTW"].map(String::from).into();
        assert_eq!(
            choose_locale(&installed, Some("zhTW")).as_deref(),
            Some("zhTW")
        );
        assert_eq!(
            choose_locale(&installed, Some("deAT")).as_deref(),
            Some("deDE")
        );
        assert_eq!(
            choose_locale(&installed, Some("ruRU")).as_deref(),
            Some("enUS")
        );
        assert_eq!(choose_locale(&BTreeSet::new(), Some("enUS")), None);
    }

    #[test]
    fn verbatim_local_paths_lose_their_prefix() {
        assert_eq!(
            plain(r"\\?\D:\Games\Overwatch.exe"),
            r"D:\Games\Overwatch.exe"
        );
        assert_eq!(plain(r"\\?\UNC\server\share\x"), r"\\?\UNC\server\share\x");
        assert_eq!(plain(r"D:\Games"), r"D:\Games");
    }

    #[test]
    fn resolves_the_game_from_its_folder_or_the_official_launcher() {
        let install = tempfile::tempdir().unwrap();
        let retail = install.path().join("_retail_");
        fs::create_dir(&retail).unwrap();
        fs::write(retail.join(GAME_EXE), b"").unwrap();
        fs::write(install.path().join(OFFICIAL_LAUNCHER), b"").unwrap();
        let expected = plain_canonical(&retail.join(GAME_EXE)).unwrap();
        for path in [
            install.path().to_owned(),
            install.path().join(OFFICIAL_LAUNCHER),
            retail.join("overwatch.EXE"),
        ] {
            assert_eq!(
                resolve_game(&path),
                Some(expected.clone()),
                "{}",
                path.display()
            );
        }
        assert_eq!(resolve_game(&install.path().join("missing.exe")), None);
    }
}
