//! `lucy assets diff`: derive the game data and compare it with a pack.

use std::{fs, io, path::Path};

use lucy_assets::{Difference, Install, Output};
use lucy_launcher::game::install_root;

use crate::{play, settings::Settings};

/// Print each derived file's comparison with `pack`; true when all match.
pub fn diff(settings: &Settings, pack: &Path) -> io::Result<bool> {
    let game = play::configured_game(settings)?;
    let install = install_root(&game).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "no .build.info next to the game")
    })?;
    let locale = play::locale(settings, &game).unwrap_or_else(|| "enUS".into());
    let opened = Install::open(&install, &locale).map_err(io::Error::other)?;
    let scratch = tempfile::tempdir()?;
    let written = lucy_assets::build(&opened, scratch.path()).map_err(io::Error::other)?;
    let mut same = true;
    for path in written {
        let relative = path
            .strip_prefix(scratch.path())
            .map_err(io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        let output = Output {
            path: relative.clone(),
            bytes: fs::read(&path)?,
        };
        let verdict = lucy_assets::diff(&output, pack).map_err(io::Error::other)?;
        match &verdict {
            Difference::Same => println!("same     {relative}"),
            Difference::Missing => println!("missing  {relative} (not in the pack)"),
            Difference::Differs(at) => println!("DIFFERS  {relative} at {at}"),
        }
        // A file the pack lacks (a map it has no collision for) is no mismatch.
        same &= !matches!(verdict, Difference::Differs(_));
    }
    Ok(same)
}
