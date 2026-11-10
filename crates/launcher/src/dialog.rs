//! The file dialog that asks the player for the game (`pick_game` in
//! `ow174/launcher/game.py`).

use std::{ffi::OsString, io, mem::size_of, os::windows::ffi::OsStringExt, path::PathBuf};

use windows_sys::Win32::UI::Controls::Dialogs::{
    CommDlgExtendedError, GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR,
    OFN_PATHMUSTEXIST, OPENFILENAMEW,
};

use crate::game::{GAME_EXE, OFFICIAL_LAUNCHER};

const MAX_PATH_CHARS: usize = 32_768;

/// Ask for `Overwatch.exe` (or the official launcher next to it) in a file
/// dialog. `Ok(None)` when the player closes it.
pub fn open_game_dialog() -> io::Result<Option<PathBuf>> {
    let filter = wide(&format!(
        "Overwatch game or launcher\0{GAME_EXE};{OFFICIAL_LAUNCHER}\0"
    ));
    let title = wide(&format!("Select _retail_\\{GAME_EXE}"));
    let mut file = vec![0u16; MAX_PATH_CHARS];
    // SAFETY: an all-zero OPENFILENAMEW is valid; the used fields are set below.
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = file.as_mut_ptr();
    dialog.nMaxFile = file.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_NOCHANGEDIR;
    // SAFETY: every pointer in `dialog` points at a live, NUL-terminated
    // buffer of the length given next to it.
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        // SAFETY: plain query of the dialog's last error.
        return match unsafe { CommDlgExtendedError() } {
            0 => Ok(None),
            code => Err(io::Error::other(format!(
                "the file dialog failed (error {code:#x})"
            ))),
        };
    }
    let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Ok(Some(PathBuf::from(OsString::from_wide(&file[..end]))))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
