//! The rule for account names (`docs/user/hosting.md`, "Player names").
//!
//! A remote player's name arrives over the network (the lobby login 21800,
//! the Battle.net session key) and becomes the file `profiles/<name>.json`
//! on the host. A name is accepted only when it already is a safe file stem
//! as typed, so each name has its own file: nothing is stripped or replaced.
//! Letter case is the one difference names may have and still share a file:
//! the account key ignores it, and so does the file lookup.

use std::fmt;

use lucy_bnet::session_key::MAX_NAME_BYTES;

/// At most this many characters (the `join` prompt's limit).
pub const MAX_CHARS: usize = 24;
/// At most this many UTF-8 bytes: the name must fit the Battle.net session
/// key the retail frontend passes on to the lobby.
pub const MAX_BYTES: usize = MAX_NAME_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameError {
    Empty,
    TooLong,
    /// The first character outside letters, digits, `-` and `_`.
    Character(char),
    /// A name Windows keeps for a device (`CON`, `NUL`, `COM1`...): its file
    /// opens the device, not a file.
    Reserved,
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(formatter, "the name is empty"),
            Self::TooLong => write!(
                formatter,
                "the name is longer than {MAX_CHARS} characters or {MAX_BYTES} bytes"
            ),
            Self::Character(character) => write!(
                formatter,
                "the name contains {character:?}; use only letters, digits, - and _"
            ),
            Self::Reserved => write!(
                formatter,
                "Windows keeps the name for a device (CON, PRN, AUX, NUL, COM0-9, LPT0-9)"
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Whether `name` may name an account: 1 to [`MAX_CHARS`] characters and at
/// most [`MAX_BYTES`] bytes, each a letter or digit of any script
/// ([`char::is_alphanumeric`]), `-` or `_`, and not a Windows device name.
/// Spaces, dots, slashes, control and formatting characters are refused, so
/// a name can't start or end with a space or dot or name another folder.
pub fn check(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.len() > MAX_BYTES || name.chars().count() > MAX_CHARS {
        return Err(NameError::TooLong);
    }
    if let Some(character) = name
        .chars()
        .find(|&character| !(character.is_alphanumeric() || matches!(character, '-' | '_')))
    {
        return Err(NameError::Character(character));
    }
    if is_reserved_device_name(name) {
        return Err(NameError::Reserved);
    }
    Ok(())
}

/// Whether Windows opens a device for a file of this name, with any
/// extension: `CON`, `PRN`, `AUX`, `NUL`, `COM0`-`COM9`, `LPT0`-`LPT9` and
/// the superscript `COM¹`-`COM³`, `LPT¹`-`LPT³`, in any letter case.
pub fn is_reserved_device_name(file_name: &str) -> bool {
    let stem = file_name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let Some(number) = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
    else {
        return false;
    };
    let mut characters = number.chars();
    matches!(
        (characters.next(), characters.next()),
        (Some('0'..='9' | '¹' | '²' | '³'), None)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_letters_of_any_script_digits_dash_and_underscore() {
        for name in ["rust", "Alpha_2", "a-b", "玩家", "Жора", "Ånnа", "컴1"] {
            assert_eq!(check(name), Ok(()), "{name}");
        }
        assert_eq!(check(&"x".repeat(MAX_CHARS)), Ok(()));
        // 19 three-byte characters fit the session key's 58 bytes, 20 don't.
        assert_eq!(check(&"测".repeat(19)), Ok(()));
        assert_eq!(check(&"测".repeat(20)), Err(NameError::TooLong));
        assert_eq!(check(&"x".repeat(MAX_CHARS + 1)), Err(NameError::TooLong));
    }

    #[test]
    fn refuses_paths_device_names_and_invisible_characters() {
        for (name, error) in [
            ("", NameError::Empty),
            ("../evil", NameError::Character('.')),
            ("..\\evil", NameError::Character('.')),
            ("a/b", NameError::Character('/')),
            ("C:\\Windows", NameError::Character(':')),
            ("/etc/passwd", NameError::Character('/')),
            ("\\\\server\\share", NameError::Character('\\')),
            (" padded", NameError::Character(' ')),
            ("trailing.", NameError::Character('.')),
            ("two words", NameError::Character(' ')),
            ("con.json", NameError::Character('.')),
            ("tab\there", NameError::Character('\t')),
            ("nul\0byte", NameError::Character('\0')),
            ("right\u{202e}left", NameError::Character('\u{202e}')),
            ("zero\u{200b}width", NameError::Character('\u{200b}')),
            ("CON", NameError::Reserved),
            ("con", NameError::Reserved),
            ("nul", NameError::Reserved),
            ("Aux", NameError::Reserved),
            ("COM1", NameError::Reserved),
            ("lpt9", NameError::Reserved),
            ("COM¹", NameError::Reserved),
        ] {
            assert_eq!(check(name), Err(error), "{name:?}");
        }
        for name in ["CONSOLE", "COM10", "LPT", "NULL", "icon"] {
            assert_eq!(check(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn device_names_keep_their_device_with_any_extension() {
        for file in [
            "CON.json",
            "con.png",
            "nul",
            "NUL .txt",
            "com3.tar.gz",
            "LPT².json",
        ] {
            assert!(is_reserved_device_name(file), "{file}");
        }
        for file in ["console.json", "com.png", "com12.json", "golden.png"] {
            assert!(!is_reserved_device_name(file), "{file}");
        }
    }
}
