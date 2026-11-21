//! The DLL side: started by `DllMain`, it serves the Python client on a
//! worker thread and runs the hooks on the client's threads.

mod call;
mod game;
mod hook;
mod mem;
mod server;

use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::Write,
    os::windows::ffi::OsStringExt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    sync::{
        Arc, LazyLock, Mutex, OnceLock,
        atomic::{AtomicPtr, AtomicU32, Ordering},
    },
    time::Instant,
};

use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, HMODULE, TRUE},
        System::{
            Diagnostics::Debug::OutputDebugStringA,
            LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::{CreateThread, GetCurrentThreadId},
        },
    },
    core::BOOL,
};

use crate::{
    LOG_VARIABLE, PORT_VARIABLE, probe_port,
    ring::{Event, Ring},
};

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static PROBE: LazyLock<Probe> = LazyLock::new(Probe::new);

/// Records the ring keeps.
const RING_CAPACITY: usize = 1 << 18;

pub struct Probe {
    pub base: u64,
    pub image_size: u64,
    /// The code section, from the executable's file (the protector scrambles
    /// the headers in memory).
    pub text: (u64, u64),
    pub exe: String,
    pub port: u16,
    pub started: Instant,
    log: Mutex<Option<File>>,
    pub ring: Mutex<Ring>,
    /// Every hook ever installed, for the rest of the process.
    pub hooks: Mutex<Vec<Arc<hook::Hook>>>,
    pub arena: Mutex<mem::Arena>,
    pub shared: OnceLock<Result<hook::Shared, String>>,
    pub game: game::Game,
    next_hook: AtomicU32,
}

/// The probe; it exists before any hook does.
pub fn probe() -> &'static Probe {
    &PROBE
}

impl Probe {
    pub fn log(&self, message: &str) {
        let line = format!("[probe] {message}\n");
        if let Some(file) = self.log.lock().ok().as_mut().and_then(|file| file.as_mut()) {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
        let mut text = line.into_bytes();
        text.retain(|&byte| byte != 0);
        text.push(0);
        // SAFETY: a NUL-terminated byte string.
        unsafe { OutputDebugStringA(text.as_ptr()) };
    }

    pub fn micros(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    pub fn record(&self, event: Event) {
        // SAFETY: a plain thread id query.
        let thread = unsafe { GetCurrentThreadId() };
        let time = self.micros();
        if let Ok(mut ring) = self.ring.lock() {
            ring.push(time, thread, event);
        }
    }

    pub fn next_hook_id(&self) -> u32 {
        self.next_hook.fetch_add(1, Ordering::Relaxed)
    }

    /// The shared return stub and gadgets, built on first use.
    pub fn shared(&self) -> Result<&hook::Shared, String> {
        self.shared
            .get_or_init(|| {
                let mut arena = self.arena.lock().map_err(|_| "arena lock")?;
                hook::shared(&mut arena, self.text)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// An ORPHEUS VA as this run's address.
    pub fn runtime(&self, orpheus_va: u64) -> u64 {
        crate::runtime(self.base, orpheus_va)
    }
}

/// The executable's path, `SizeOfImage` and code section, from its file.
fn image_layout(base: u64) -> (String, u64, (u64, u64)) {
    let mut name = vec![0u16; 32_768];
    // SAFETY: the buffer is valid for its full length, which is passed; a
    // null module means the executable.
    let length =
        unsafe { GetModuleFileNameW(ptr::null_mut(), name.as_mut_ptr(), name.len() as u32) }
            as usize;
    let exe = PathBuf::from(std::ffi::OsString::from_wide(&name[..length]));
    let headers = fs::read(&exe)
        .ok()
        .map(|mut bytes| {
            bytes.truncate(0x1000);
            bytes
        })
        .unwrap_or_default();
    let (size, text) = crate::pe_layout(&headers).unwrap_or((0x0400_0000, (0x1000, 0x0200_0000)));
    (
        exe.display().to_string(),
        size,
        (base + text.0, base + text.1),
    )
}

fn open_log() -> Option<File> {
    let path = std::env::var_os(LOG_VARIABLE)
        .map(PathBuf::from)
        .or_else(|| {
            let mut name = vec![0u16; 32_768];
            // SAFETY: the buffer is valid for its full length, which is passed.
            let length = unsafe {
                GetModuleFileNameW(
                    MODULE.load(Ordering::Acquire),
                    name.as_mut_ptr(),
                    name.len() as u32,
                )
            } as usize;
            let module = PathBuf::from(std::ffi::OsString::from_wide(&name[..length]));
            Some(module.parent()?.join("log").join("probe.log"))
        })?;
    if let Some(folder) = path.parent() {
        let _ = fs::create_dir_all(folder);
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

impl Probe {
    fn new() -> Self {
        // SAFETY: a null name means this executable.
        let base = unsafe { GetModuleHandleW(ptr::null()) } as u64;
        let (exe, image_size, text) = image_layout(base);
        Probe {
            base,
            image_size,
            text,
            exe,
            port: probe_port(std::env::var(PORT_VARIABLE).ok().as_deref()),
            started: Instant::now(),
            log: Mutex::new(open_log()),
            ring: Mutex::new(Ring::new(RING_CAPACITY)),
            hooks: Mutex::new(Vec::new()),
            arena: Mutex::new(mem::Arena::new(base)),
            shared: OnceLock::new(),
            game: game::Game::default(),
            next_hook: AtomicU32::new(1),
        }
    }
}

fn run() {
    let probe = probe();
    probe.log(&format!(
        "==== owprobe on 127.0.0.1:{}, image {:#X}+{:#X}, code {:#X}..{:#X}, {} ====",
        probe.port, probe.base, probe.image_size, probe.text.0, probe.text.1, probe.exe
    ));
    server::serve(probe);
}

extern "system" fn worker(_: *mut c_void) -> u32 {
    let _ = catch_unwind(AssertUnwindSafe(run));
    0
}

/// Start the worker thread when the DLL is loaded; nothing else runs under
/// the loader lock.
#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HMODULE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module, Ordering::Release);
        // SAFETY: plain loader calls with this module's handle; the new
        // thread starts once the loader lock is released.
        unsafe {
            DisableThreadLibraryCalls(module);
            let thread = CreateThread(
                ptr::null(),
                0,
                Some(worker),
                ptr::null(),
                0,
                ptr::null_mut(),
            );
            if !thread.is_null() {
                CloseHandle(thread);
            }
        }
    }
    TRUE
}
