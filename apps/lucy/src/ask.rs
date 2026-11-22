//! The questions `lucy` asks in the console, and the answers it
//! remembers in the state folder for next time (`ask_*` in `ow174/cli.py`).

use std::{
    fs,
    io::{self, BufRead, Write},
    path::Path,
};

use lucy_lobby::account_name;

use crate::settings::Mode;

/// The server joined last time (`SERVER_ADDRESS_FILE` in `ow174/paths.py`).
pub const SERVER_ADDRESS_FILE: &str = "server_address.txt";
/// The name used to join last time.
pub const PLAYER_NAME_FILE: &str = "player_name.txt";
/// The host's public address, asked when hosting.
pub const PUBLIC_ADDRESS_FILE: &str = "public_address.txt";
const MODE_MENU: &str = "
Which mode?
  1  Play in retail mode (default Overwatch mode)
  2  Play in tournament mode (used on LANs by pros)
  3  Host a server for others and play on it
  4  Host a server for others without playing here
  5  Join a server in retail mode (default Overwatch mode)
  6  Join a server in tournament mode (used on LANs by pros)
";
const NO_ANSWER: &str = "No answer to the question. Give it as an option instead, see lucy --help.";

/// Ask for the mode. Enter picks retail.
pub fn mode(input: &mut impl BufRead, output: &mut impl Write) -> io::Result<Mode> {
    writeln!(output, "{MODE_MENU}")?;
    let count = Mode::ALL.len();
    loop {
        let answer = answer(
            input,
            output,
            &format!("Press 1 to {count}, then Enter (just Enter for 1): "),
        )?;
        if answer.is_empty() {
            return Ok(Mode::Retail);
        }
        if let Some(mode) = answer
            .parse::<usize>()
            .ok()
            .and_then(|number| Mode::ALL.get(number.checked_sub(1)?))
        {
            return Ok(*mode);
        }
    }
}

/// Ask the host for the public address players over the internet join with;
/// their games are sent there for a match too. Enter reuses the one from
/// last time, `-` is none (only players on this network or a VPN).
pub fn public_address(
    input: &mut impl BufRead,
    output: &mut impl Write,
    saved: &Path,
) -> io::Result<String> {
    let last = remembered(saved);
    writeln!(
        output,
        "Players over the internet join with your public address (whatismyip.com shows it)."
    )?;
    let hint = if last.is_empty() {
        "just Enter if only your network plays".to_owned()
    } else {
        format!("just Enter for {last}, - for none")
    };
    loop {
        let answer = answer(input, output, &format!("Your public address ({hint}): "))?;
        if answer == "-" || (answer.is_empty() && last.is_empty()) {
            match fs::remove_file(saved) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => return Ok(String::new()),
            }
        }
        let answer = if answer.is_empty() {
            last.clone()
        } else {
            answer
        };
        if is_host_name(&answer) {
            remember(saved, &answer)?;
            return Ok(answer);
        }
        writeln!(
            output,
            "Type only the address, for example 1.2.3.4, without a port."
        )?;
    }
}

/// Ask for the server address. Enter reuses the one from last time.
pub fn server_address(
    input: &mut impl BufRead,
    output: &mut impl Write,
    saved: &Path,
) -> io::Result<String> {
    let last = remembered(saved);
    let prompt = format!("Server address, like 1.2.3.4:3724{}: ", reuse_hint(&last));
    loop {
        let answer = answer_or(input, output, &prompt, &last)?;
        if is_server_address(&answer) {
            remember(saved, &answer)?;
            return Ok(answer);
        }
        writeln!(
            output,
            "Type the address as host:port, for example 1.2.3.4:3724."
        )?;
    }
}

/// Ask for the player's name on the server. Enter reuses the one from last time.
pub fn player_name(
    input: &mut impl BufRead,
    output: &mut impl Write,
    saved: &Path,
) -> io::Result<String> {
    let last = remembered(saved);
    let prompt = format!("Your name on the server{}: ", reuse_hint(&last));
    loop {
        let answer = answer_or(input, output, &prompt, &last)?;
        match account_name::check(&answer) {
            Ok(()) => {
                remember(saved, &answer)?;
                return Ok(answer);
            }
            Err(error) => writeln!(output, "That name can't be used: {error}.")?,
        }
    }
}

/// A host name or IPv4 address without a port.
pub fn is_host_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|char| char.is_alphanumeric() || matches!(char, '.' | '-'))
}

/// `host:port` with a port from 1 to 65535.
pub fn is_server_address(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.contains(' ')
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port
            .parse::<u32>()
            .is_ok_and(|port| (1..65536).contains(&port))
}

pub fn parse_server_address(text: &str) -> Result<String, String> {
    if is_server_address(text) {
        Ok(text.to_owned())
    } else {
        Err(format!(
            "{text} is not a server address. Use host:port, like 1.2.3.4:3724."
        ))
    }
}

/// `--name`: a name the account rule accepts ([`account_name::check`]).
pub fn parse_player_name(text: &str) -> Result<String, String> {
    account_name::check(text)
        .map(|()| text.to_owned())
        .map_err(|error| format!("{text:?} can't be a name: {error}."))
}

fn reuse_hint(last: &str) -> String {
    if last.is_empty() {
        String::new()
    } else {
        format!(" (just Enter for {last})")
    }
}

/// The answer, or the remembered one for an empty answer.
fn answer_or(
    input: &mut impl BufRead,
    output: &mut impl Write,
    prompt: &str,
    last: &str,
) -> io::Result<String> {
    let answer = answer(input, output, prompt)?;
    Ok(if answer.is_empty() {
        last.to_owned()
    } else {
        answer
    })
}

/// One trimmed line. Without a console to answer, the option is the way.
fn answer(input: &mut impl BufRead, output: &mut impl Write, prompt: &str) -> io::Result<String> {
    write!(output, "{prompt}")?;
    output.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, NO_ANSWER));
    }
    Ok(line.trim().to_owned())
}

fn remembered(saved: &Path) -> String {
    fs::read_to_string(saved)
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}

fn remember(saved: &Path, answer: &str) -> io::Result<()> {
    if let Some(folder) = saved.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(saved, answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn asked<T>(
        answers: &str,
        question: impl FnOnce(&mut Cursor<Vec<u8>>, &mut Vec<u8>) -> io::Result<T>,
    ) -> (io::Result<T>, String) {
        let mut output = Vec::new();
        let result = question(&mut Cursor::new(answers.as_bytes().to_vec()), &mut output);
        (result, String::from_utf8(output).unwrap())
    }

    #[test]
    fn the_mode_menu_takes_a_number_and_enter_is_retail() {
        assert_eq!(asked("\n", mode).0.unwrap(), Mode::Retail);
        let (chosen, shown) = asked("7\nx\n5\n", mode);
        assert_eq!(chosen.unwrap(), Mode::Join);
        assert_eq!(shown.matches("Press 1 to 6").count(), 3);
        assert_eq!(asked("6\n", mode).0.unwrap(), Mode::JoinTournament);
        let error = asked("", mode).0.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn answers_are_remembered_and_enter_reuses_them() {
        let state = tempfile::tempdir().unwrap();
        let saved = state.path().join("new").join(SERVER_ADDRESS_FILE);
        let ask = |answers| {
            asked(answers, |input, output| {
                server_address(input, output, &saved)
            })
        };
        let (address, shown) = ask("1.2.3.4\n 1.2.3.4:3724 \n");
        assert_eq!(address.unwrap(), "1.2.3.4:3724");
        assert!(shown.contains("Type the address as host:port"));
        assert_eq!(fs::read_to_string(&saved).unwrap(), "1.2.3.4:3724");
        let (address, shown) = ask("\n");
        assert_eq!(address.unwrap(), "1.2.3.4:3724");
        assert!(shown.contains("(just Enter for 1.2.3.4:3724)"));

        let saved = state.path().join(PLAYER_NAME_FILE);
        let ask = |answers| asked(answers, |input, output| player_name(input, output, &saved));
        assert_eq!(ask("\nbad name\nAna-1\n").0.unwrap(), "Ana-1");
        assert_eq!(ask("\n").0.unwrap(), "Ana-1");
    }

    #[test]
    fn the_public_address_can_be_kept_changed_or_cleared() {
        let state = tempfile::tempdir().unwrap();
        let saved = state.path().join(PUBLIC_ADDRESS_FILE);
        let ask = |answers| {
            asked(answers, |input, output| {
                public_address(input, output, &saved)
            })
        };
        assert_eq!(ask("\n").0.unwrap(), "");
        assert!(!saved.exists());
        let (address, shown) = ask("1.2.3.4:3724\ngames.example\n");
        assert_eq!(address.unwrap(), "games.example");
        assert!(shown.contains("without a port"));
        let (address, shown) = ask("\n");
        assert_eq!(address.unwrap(), "games.example");
        assert!(shown.contains("just Enter for games.example, - for none"));
        assert_eq!(ask("-\n").0.unwrap(), "");
        assert!(!saved.exists());
    }

    #[test]
    fn addresses_are_checked_like_the_reference() {
        assert!(is_server_address("1.2.3.4:3724"));
        assert!(is_server_address("host.example:1"));
        for bad in [
            "1.2.3.4",
            ":3724",
            "a b:3724",
            "host:0",
            "host:65536",
            "host:",
            "host:+1",
        ] {
            assert!(!is_server_address(bad), "{bad}");
        }
        assert!(is_host_name("1.2.3.4") && is_host_name("my-host.example"));
        assert!(!is_host_name("") && !is_host_name("1.2.3.4:3724"));
    }
}
