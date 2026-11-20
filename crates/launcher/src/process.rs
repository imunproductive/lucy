//! Running copies of the game, starting it and loading the relay into it.

use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    ptr, thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_BAD_LENGTH, ERROR_NO_MORE_FILES, ERROR_PARTIAL_COPY, HANDLE, HMODULE,
        INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Globalization::GetUserDefaultLocaleName,
    System::{
        Diagnostics::{
            Debug::WriteProcessMemory,
            ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPMODULE,
                TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
            },
        },
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, GetProcAddress,
        },
        Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
        },
        Threading::{
            CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_CREATE_THREAD,
            PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION,
            PROCESS_VM_READ, PROCESS_VM_WRITE, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};

use crate::game::GAME_EXE;

const POLL: Duration = Duration::from_millis(50);
/// While a process is still loading, its module snapshot can fail
/// (`ERROR_BAD_LENGTH`, retry as documented), come back empty
/// (`ERROR_NO_MORE_FILES`) or be cut short (`ERROR_PARTIAL_COPY`).
const TRANSIENT_SNAPSHOT_ERRORS: [u32; 3] =
    [ERROR_BAD_LENGTH, ERROR_NO_MORE_FILES, ERROR_PARTIAL_COPY];

/// The Windows user language as a game code: `ru-RU` becomes `ruRU`.
pub fn system_locale() -> Option<String> {
    let mut buffer = [0u16; 85];
    // SAFETY: the buffer is valid for its full length, which is passed.
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if length <= 1 {
        return None;
    }
    let name = String::from_utf16_lossy(&buffer[..length as usize - 1]).replace('-', "");
    (!name.is_empty()).then_some(name)
}

/// PIDs of running copies of the game's executable (`Overwatch.exe`, or a
/// decrypted copy such as `Overwatch.debug.exe`): this copy, and any other.
pub fn running_copies(game: &Path) -> io::Result<(Vec<u32>, Vec<u32>)> {
    let wanted = normalized(game);
    let name = game
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(GAME_EXE);
    let (mut ours, mut others) = (Vec::new(), Vec::new());
    for pid in processes_named(name)? {
        if image_path(pid).is_some_and(|path| normalized(&path) == wanted) {
            ours.push(pid);
        } else {
            others.push(pid);
        }
    }
    Ok((ours, others))
}

/// Close a copy of this game left from an earlier run. A normal close comes
/// first, since the game saves its settings only when it exits by itself.
pub fn close_running_copy(game: &Path, patience: Duration) -> io::Result<()> {
    for force in [false, true] {
        let ours = running_copies(game)?.0;
        if ours.is_empty() {
            return Ok(());
        }
        for pid in &ours {
            let mut taskkill = Command::new("taskkill");
            if force {
                taskkill.arg("/F");
            }
            taskkill
                .args(["/PID", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
        }
        let deadline = Instant::now() + patience;
        while !running_copies(game)?.0.is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(300));
        }
    }
    if running_copies(game)?.0.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other("the previous Overwatch did not close"))
    }
}

/// The relay reads its log file and the server's RPC port from these
/// variables in the game's environment.
pub const RELAY_LOG_VARIABLE: &str = "ORPHEUS_RELAY_LOG";
pub const RELAY_PORT_VARIABLE: &str = "ORPHEUS_RELAY_PORT";
/// The probe reads its port and log file from these.
pub const PROBE_PORT_VARIABLE: &str = "ORPHEUS_PROBE_PORT";
pub const PROBE_LOG_VARIABLE: &str = "ORPHEUS_PROBE_LOG";

/// Where the relay in a started game logs and sends the plaintext.
pub struct RelaySetup<'a> {
    pub log: &'a Path,
    pub rpc_port: u16,
    /// The probe to load after the relay, if any.
    pub probe: Option<&'a ProbeSetup>,
}

/// `owprobe.dll`, the client probe `tools/owprobe` talks to on
/// `127.0.0.1:port` (docs/dev/client-tools.md).
#[derive(Clone, Debug)]
pub struct ProbeSetup {
    pub dll: PathBuf,
    pub port: u16,
    pub log: PathBuf,
}

/// Start the game in its folder, appending its output to `log`. A game in
/// tournament mode dials the lobby itself and gets no relay.
pub fn start_game(
    game: &Path,
    arguments: &[String],
    log: &Path,
    relay: Option<RelaySetup<'_>>,
) -> io::Result<Child> {
    if let Some(folder) = log.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let output: File = OpenOptions::new().create(true).append(true).open(log)?;
    let mut command = Command::new(game);
    command.args(arguments);
    if let Some(relay) = relay {
        command
            .env(RELAY_LOG_VARIABLE, relay.log)
            .env(RELAY_PORT_VARIABLE, relay.rpc_port.to_string());
        if let Some(probe) = relay.probe {
            command
                .env(PROBE_LOG_VARIABLE, &probe.log)
                .env(PROBE_PORT_VARIABLE, probe.port.to_string());
        }
    }
    command
        .current_dir(game.parent().unwrap_or(Path::new(".")))
        .stdout(output.try_clone()?)
        .stderr(output)
        .spawn()
}

/// Load the DLL into the game as soon as its loader is ready, and return the
/// module's base address there.
pub fn inject_when_ready(child: &mut Child, dll: &Path, timeout: Duration) -> io::Result<usize> {
    let pid = child.id();
    inject_when_ready_with(pid, || child.try_wait(), dll, timeout)
}

/// [`inject_when_ready`] for a game whose exit `exited` reports.
pub fn inject_when_ready_with(
    pid: u32,
    mut exited: impl FnMut() -> io::Result<Option<ExitStatus>>,
    dll: &Path,
    timeout: Duration,
) -> io::Result<usize> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = exited()? {
            return Err(io::Error::other(format!(
                "Overwatch exited ({status}) before the relay was loaded"
            )));
        }
        match remote_load_library(pid) {
            Ok(_) => break,
            Err(error) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Overwatch did not become ready for the relay: {error}"),
                ));
            }
            Err(_) => thread::sleep(POLL),
        }
    }
    inject(dll, pid, deadline.saturating_duration_since(Instant::now()))
}

/// Load the DLL into a process with `LoadLibraryW` on a remote thread.
pub fn inject(dll: &Path, pid: u32, timeout: Duration) -> io::Result<usize> {
    let dll = dll.canonicalize()?;
    if let Some(base) = loaded_module(pid, &dll)? {
        return Ok(base);
    }
    let loader = remote_load_library(pid)?;
    let wide: Vec<u16> = dll.as_os_str().encode_wide().chain([0]).collect();
    let bytes = wide.len() * size_of::<u16>();
    let access = PROCESS_CREATE_THREAD
        | PROCESS_QUERY_INFORMATION
        | PROCESS_VM_OPERATION
        | PROCESS_VM_WRITE
        | PROCESS_VM_READ;
    // SAFETY: plain handle request; the handle is closed by `Handle`.
    let process = Handle::open(unsafe { OpenProcess(access, 0, pid) })?;
    // SAFETY: allocates fresh memory in the target; freed below unless the
    // remote thread may still read it.
    let remote = unsafe {
        VirtualAllocEx(
            process.0,
            ptr::null(),
            bytes,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if remote.is_null() {
        return Err(io::Error::last_os_error());
    }
    // The path may be freed once the remote thread is done with it, or when
    // no thread was started.
    let (mut started, mut finished) = (false, false);
    let result = (|| {
        let mut written = 0;
        // SAFETY: `wide` is valid for `bytes` bytes and `remote` was allocated with that size.
        if unsafe {
            WriteProcessMemory(process.0, remote, wide.as_ptr().cast(), bytes, &mut written)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if written != bytes {
            return Err(io::Error::other("incomplete remote write"));
        }
        // SAFETY: `loader` is LoadLibraryW in the target, which takes one
        // pointer argument: the path just written there.
        let thread = Handle::open(unsafe {
            CreateRemoteThread(
                process.0,
                ptr::null(),
                0,
                Some(std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
                >(loader)),
                remote,
                0,
                ptr::null_mut(),
            )
        })?;
        started = true;
        let milliseconds = u32::try_from(timeout.as_millis())
            .unwrap_or(u32::MAX - 1)
            .max(1);
        // SAFETY: waits on a valid thread handle.
        match unsafe { WaitForSingleObject(thread.0, milliseconds) } {
            WAIT_OBJECT_0 => finished = true,
            WAIT_TIMEOUT => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the DLL loader in PID {pid} timed out"),
                ));
            }
            _ => return Err(io::Error::last_os_error()),
        }
        let mut code = 0;
        // SAFETY: reads the exit code of the finished thread.
        unsafe { GetExitCodeThread(thread.0, &mut code) };
        loaded_module(pid, &dll)?.ok_or_else(|| {
            io::Error::other(format!(
                "the DLL did not load in PID {pid} (thread exit {code:#x}): {}",
                dll.display()
            ))
        })
    })();
    if !started || finished {
        // SAFETY: the remote thread is done with the path, or never started.
        unsafe { VirtualFreeEx(process.0, remote, 0, MEM_RELEASE) };
    }
    result
}

/// `LoadLibraryW` in the target, found through the module that exports it
/// here (kernel32 may forward it).
fn remote_load_library(pid: u32) -> io::Result<usize> {
    let kernel32: Vec<u16> = "kernel32.dll".encode_utf16().chain([0]).collect();
    // SAFETY: valid, NUL-terminated names; the module stays loaded in this process.
    let address = unsafe {
        let module = GetModuleHandleW(kernel32.as_ptr());
        GetProcAddress(module, c"LoadLibraryW".as_ptr().cast())
    }
    .ok_or_else(io::Error::last_os_error)? as usize;
    let mut owner: HMODULE = ptr::null_mut();
    // SAFETY: looks up the module containing a code address in this process.
    if unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut owner,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut name = vec![0u16; 32_768];
    // SAFETY: the buffer is valid for its full length, which is passed.
    let length = unsafe { GetModuleFileNameW(owner, name.as_mut_ptr(), name.len() as u32) };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    let owner_name = file_name(&PathBuf::from(OsString::from_wide(
        &name[..length as usize],
    )));
    modules(pid)?
        .into_iter()
        .find(|(path, _)| file_name(path) == owner_name)
        .map(|(_, base)| base + address - owner as usize)
        .ok_or_else(|| io::Error::other(format!("{owner_name} is not loaded in PID {pid} yet")))
}

fn loaded_module(pid: u32, dll: &Path) -> io::Result<Option<usize>> {
    let wanted = normalized(dll);
    Ok(modules(pid)?
        .into_iter()
        .find(|(path, _)| normalized(path) == wanted)
        .map(|(_, base)| base))
}

/// The module paths and base addresses of a process, retrying briefly while it loads.
fn modules(pid: u32) -> io::Result<Vec<(PathBuf, usize)>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match snapshot_modules(pid) {
            Err(error)
                if error
                    .raw_os_error()
                    .is_some_and(|code| TRANSIENT_SNAPSHOT_ERRORS.contains(&(code as u32)))
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

fn snapshot_modules(pid: u32) -> io::Result<Vec<(PathBuf, usize)>> {
    // SAFETY: plain snapshot request; the handle is closed by `Handle`.
    let snapshot = Handle::snapshot(unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
    })?;
    // SAFETY: an all-zero MODULEENTRY32W is valid; dwSize is set before use.
    let mut entry: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
    let mut result = Vec::new();
    // SAFETY: `entry` is a properly sized MODULEENTRY32W.
    if unsafe { Module32FirstW(snapshot.0, &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    loop {
        result.push((wide_path(&entry.szExePath), entry.modBaseAddr as usize));
        // SAFETY: as above.
        if unsafe { Module32NextW(snapshot.0, &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(result)
            } else {
                Err(error)
            };
        }
    }
}

fn processes_named(name: &str) -> io::Result<Vec<u32>> {
    // SAFETY: plain snapshot request; the handle is closed by `Handle`.
    let snapshot = Handle::snapshot(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    // SAFETY: an all-zero PROCESSENTRY32W is valid; dwSize is set before use.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut pids = Vec::new();
    // SAFETY: `entry` is a properly sized PROCESSENTRY32W.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        if wide_path(&entry.szExeFile)
            .to_str()
            .is_some_and(|exe| exe.eq_ignore_ascii_case(name))
        {
            pids.push(entry.th32ProcessID);
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    Ok(pids)
}

/// The full path of a process's executable, when it can be read.
fn image_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: plain handle request; the handle is closed by `Handle`.
    let process =
        Handle::open(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }).ok()?;
    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    // SAFETY: the buffer is valid for `size` characters.
    if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut size) } == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
}

fn wide_path(text: &[u16]) -> PathBuf {
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    PathBuf::from(OsString::from_wide(&text[..end]))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// A path compared the way Windows does: absolute and case-insensitive.
fn normalized(path: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    absolute
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_lowercase()
}

struct Handle(HANDLE);

impl Handle {
    fn open(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn snapshot(handle: HANDLE) -> io::Result<Self> {
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and owned by this value.
        unsafe { CloseHandle(self.0) };
    }
}
