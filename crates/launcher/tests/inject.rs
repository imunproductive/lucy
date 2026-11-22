//! Start a stand-in process the way `lucy play` starts the game, load the
//! relay and the probe into it, and check they came up. Needs
//! `owwfd_relay.dll` and `owprobe.dll` next to the test binaries: run
//! `just test-windows`.
#![cfg(windows)]

use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Child,
    thread,
    time::{Duration, Instant},
};

use lucy_launcher::process::{ProbeSetup, RelaySetup, inject, inject_when_ready, start_game};

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn built_dll(name: &str) -> PathBuf {
    // The test binary sits below the profile folder (target/<profile>/deps/,
    // or target/<profile>/build/<package>/<hash>/out/ in newer cargo
    // layouts); the DLLs are target/<profile>/<name>.
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .map(|folder| folder.join(name))
        .find(|dll| dll.is_file())
        .unwrap_or_else(|| {
            panic!(
                "no {name} in the folders above {}: run just test-windows",
                exe.display()
            )
        })
}

fn wait_for(path: &Path, text: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let log = fs::read_to_string(path).unwrap_or_default();
        if log.contains(text) || Instant::now() > deadline {
            return log;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "needs owwfd_relay.dll: just test-windows"]
fn the_relay_loads_once_and_uses_the_given_port() {
    let dll = built_dll("owwfd_relay.dll");
    let folder = tempfile::tempdir().unwrap();
    let relay_log = folder.path().join("relay.log");
    let ping = Path::new(r"C:\Windows\System32\PING.EXE");
    let arguments = ["-n", "30", "127.0.0.1"].map(String::from);
    let mut child = Kill(
        start_game(
            ping,
            &arguments,
            &folder.path().join("game.log"),
            Some(RelaySetup {
                log: &relay_log,
                rpc_port: 4321,
                probe: None,
            }),
        )
        .unwrap(),
    );

    let base = inject_when_ready(&mut child.0, &dll, Duration::from_secs(10)).unwrap();
    assert_ne!(base, 0);
    assert_eq!(
        inject(&dll, child.0.id(), Duration::from_secs(5)).unwrap(),
        base,
        "a second load reuses the loaded module"
    );
    let log = wait_for(&relay_log, "[poll] loop=0");
    assert!(
        log.contains("plaintext pipe -> 127.0.0.1:4321"),
        "relay log:\n{log}"
    );
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "the process survives the scan"
    );
}

/// One framed JSON request to the probe and its answer.
fn ask(stream: &mut TcpStream, request: &str) -> String {
    stream
        .write_all(&(request.len() as u32).to_le_bytes())
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut answer = vec![0u8; u32::from_le_bytes(length) as usize];
    stream.read_exact(&mut answer).unwrap();
    String::from_utf8(answer).unwrap()
}

#[test]
#[ignore = "needs owwfd_relay.dll and owprobe.dll: just test-windows"]
fn the_probe_answers_on_the_given_port() {
    let relay = built_dll("owwfd_relay.dll");
    let folder = tempfile::tempdir().unwrap();
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let probe = ProbeSetup {
        dll: built_dll("owprobe.dll"),
        port,
        log: folder.path().join("probe.log"),
    };
    let ping = Path::new(r"C:\Windows\System32\PING.EXE");
    let arguments = ["-n", "30", "127.0.0.1"].map(String::from);
    let mut child = Kill(
        start_game(
            ping,
            &arguments,
            &folder.path().join("game.log"),
            Some(RelaySetup {
                log: &folder.path().join("relay.log"),
                rpc_port: 4321,
                probe: Some(&probe),
            }),
        )
        .unwrap(),
    );
    inject_when_ready(&mut child.0, &relay, Duration::from_secs(10)).unwrap();
    inject(&probe.dll, child.0.id(), Duration::from_secs(5)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match TcpStream::connect((Ipv4Addr::LOCALHOST, port)) {
            Ok(stream) => break stream,
            Err(error) if Instant::now() > deadline => panic!("the probe does not listen: {error}"),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    };
    let info = ask(&mut stream, r#"{"id": 1, "cmd": "info"}"#);
    assert!(
        info.contains(&format!(r#""pid":{}"#, child.0.id())),
        "{info}"
    );
    let base = info
        .split(r#""base":"#)
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .unwrap();
    let header = ask(
        &mut stream,
        &format!(r#"{{"id": 2, "cmd": "read", "addr": {base}, "len": 2}}"#),
    );
    assert!(header.contains(r#""hex":"4d5a""#), "{header}");
    // A call that faults (a jump to an unmapped page) comes back as an error
    // and the process goes on answering.
    let call = ask(
        &mut stream,
        r#"{"id": 3, "cmd": "call", "addr": 16, "thread": "probe", "force": true}"#,
    );
    assert!(call.contains("faulted (0xC0000005"), "{call}");
    let again = ask(&mut stream, r#"{"id": 4, "cmd": "info"}"#);
    assert!(again.contains(r#""ok":true"#), "{again}");
}
