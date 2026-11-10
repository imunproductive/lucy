//! Start a stand-in process the way `orpheus play` starts the game, load the
//! relay into it, and read the relay's banner. Needs `owwfd_relay.dll` next to
//! the test binaries: run `just test-windows`.
#![cfg(windows)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Child,
    thread,
    time::{Duration, Instant},
};

use orpheus_launcher::process::{RelaySetup, inject, inject_when_ready, start_game};

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn relay_dll() -> PathBuf {
    // target/<profile>/deps/inject-<hash>.exe -> target/<profile>/owwfd_relay.dll
    let exe = std::env::current_exe().unwrap();
    let dll = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("owwfd_relay.dll");
    assert!(
        dll.is_file(),
        "{} is missing: run just test-windows",
        dll.display()
    );
    dll
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
    let dll = relay_dll();
    let folder = tempfile::tempdir().unwrap();
    let relay_log = folder.path().join("relay.log");
    let ping = Path::new(r"C:\Windows\System32\PING.EXE");
    let arguments = ["-n", "30", "127.0.0.1"].map(String::from);
    let mut child = Kill(
        start_game(
            ping,
            &arguments,
            &folder.path().join("game.log"),
            RelaySetup {
                log: &relay_log,
                rpc_port: 4321,
            },
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
