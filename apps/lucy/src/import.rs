//! `lucy state import`: copy saved player state from an older checkout.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use tracing::info;

/// Files the older checkout kept in its root that the server still reads.
const FILES: &[&str] = &["profile.json", "default_account.txt"];
const PROFILES: &str = "profiles";

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub copied: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
}

/// Copy the profiles and loose state files. Existing files in the state
/// folder are kept unless `overwrite` is set.
pub fn run(checkout: &Path, state: &Path, overwrite: bool) -> io::Result<Report> {
    if !checkout.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is not a folder", checkout.display()),
        ));
    }
    let mut sources: Vec<PathBuf> = FILES.iter().map(PathBuf::from).collect();
    let profiles = checkout.join(PROFILES);
    if profiles.is_dir() {
        for entry in fs::read_dir(&profiles)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                sources.push(Path::new(PROFILES).join(path.file_name().expect("listed file")));
            }
        }
    }
    sources.sort();
    let mut report = Report::default();
    for relative in sources {
        let from = checkout.join(&relative);
        if !from.is_file() {
            continue;
        }
        let to = state.join(&relative);
        if to.exists() && !overwrite {
            report.kept.push(relative);
            continue;
        }
        fs::create_dir_all(to.parent().expect("inside the state folder"))?;
        fs::copy(&from, &to)?;
        info!(file = %relative.display(), "imported");
        report.copied.push(relative);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_profiles_and_keeps_existing_state_unless_asked() {
        let checkout = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("profiles")).unwrap();
        fs::write(checkout.path().join("profiles/Alice.json"), "old alice").unwrap();
        fs::write(checkout.path().join("profiles/notes.txt"), "not a profile").unwrap();
        fs::write(checkout.path().join("default_account.txt"), "Alice").unwrap();
        fs::write(checkout.path().join("game_path.txt"), "not read").unwrap();
        fs::write(state.path().join("default_account.txt"), "Bob").unwrap();

        let report = run(checkout.path(), state.path(), false).unwrap();
        assert_eq!(report.copied, [Path::new("profiles").join("Alice.json")]);
        assert_eq!(report.kept, [PathBuf::from("default_account.txt")]);
        assert_eq!(
            fs::read_to_string(state.path().join("default_account.txt")).unwrap(),
            "Bob"
        );
        assert!(!state.path().join("profiles/notes.txt").exists());

        let report = run(checkout.path(), state.path(), true).unwrap();
        assert_eq!(report.copied.len(), 2);
        assert_eq!(
            fs::read_to_string(state.path().join("default_account.txt")).unwrap(),
            "Alice"
        );
    }
}
