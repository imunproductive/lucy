//! The hook itself: the scan for byte-streams, the replacement vtables and
//! the send, recv and state thunks the client calls on its own threads.
//!
//! Memory the client owns is read and written with `ReadProcessMemory` and
//! `WriteProcessMemory` on this process, which fail instead of faulting when
//! a page goes away. No panic may unwind into the client: the thunks and the
//! worker catch them.

use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpStream},
    os::windows::ffi::OsStringExt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicI64, AtomicPtr, Ordering},
    },
    thread,
    time::Duration,
};

use windows_sys::{
    Win32::{
        Foundation::{CloseHandle, HMODULE, TRUE},
        System::{
            Diagnostics::Debug::{OutputDebugStringA, ReadProcessMemory, WriteProcessMemory},
            LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW, GetModuleHandleW},
            Memory::{
                MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE,
                PAGE_GUARD, PAGE_READWRITE, VirtualQuery,
            },
            SystemInformation::{GetSystemInfo, SYSTEM_INFO},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::{CreateThread, GetCurrentProcess},
        },
    },
    core::BOOL,
};

use crate::{
    BATTLE_NET_PORT, BYTE_STREAM_VTABLE, HOST_LENGTH_OFFSET, HOST_OFFSET, MAX_REGION, MAX_RELAYS,
    PORT_OFFSET, RECV_SLOT, RELAY_PORT, SEND_SLOT, SSL_STREAM_OFFSET, STATE_OFFSET, STATE_SLOT,
    VTABLE_SLOTS, hex_preview, image_size, in_heap, matching_words, printable,
};

const SCAN_PERIOD: Duration = Duration::from_millis(8);
const RECV_WAIT: Duration = Duration::from_millis(30);
const SCAN_CHUNK: usize = 1 << 20;
/// The launcher names the log file; without it the log goes next to the DLL.
const LOG_VARIABLE: &str = "ORPHEUS_RELAY_LOG";

static MODULE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static RELAY: OnceLock<Relay> = OnceLock::new();

struct Relay {
    image_start: u64,
    image_end: u64,
    byte_stream_vtable: u64,
    log: Mutex<Option<File>>,
    sockets: Mutex<Vec<Socket>>,
    tables: Mutex<Option<Tables>>,
    swaps: AtomicI64,
    sends: AtomicI64,
    recvs: AtomicI64,
    state_calls: AtomicI64,
    errors: AtomicI64,
}

/// A client `ssl_stream` and its lazily connected pipe to the server.
struct Socket {
    stream: u64,
    pipe: Option<Arc<TcpStream>>,
}

/// The replacement vtables. They live in the `RELAY` static, so they are
/// never freed while the client uses them.
struct Tables {
    original_socket: u64,
    socket: Box<[u64; VTABLE_SLOTS]>,
    byte_stream: Box<[u64; VTABLE_SLOTS]>,
}

impl Relay {
    fn in_image(&self, address: u64) -> bool {
        (self.image_start..self.image_end).contains(&address)
    }

    fn log(&self, message: &str) {
        let line = format!("[wfd] {message}\n");
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

    /// The pipe for a client stream, connecting it on first use.
    fn pipe(&self, stream: u64) -> Option<Arc<TcpStream>> {
        let mut sockets = self.sockets.lock().ok()?;
        let socket = sockets.iter_mut().find(|socket| socket.stream == stream)?;
        if socket.pipe.is_none() {
            match TcpStream::connect((Ipv4Addr::LOCALHOST, RELAY_PORT)) {
                Ok(pipe) => {
                    let _ = pipe.set_read_timeout(Some(RECV_WAIT));
                    self.log(&format!(
                        "*** relay CONNECTED to 127.0.0.1:{RELAY_PORT} for clientSock={stream:016X} ***"
                    ));
                    socket.pipe = Some(Arc::new(pipe));
                }
                Err(error) => {
                    self.log(&format!(
                        "relay connect() 127.0.0.1:{RELAY_PORT} failed: {error}"
                    ));
                }
            }
        }
        socket.pipe.clone()
    }

    fn register(&self, stream: u64) {
        if let Ok(mut sockets) = self.sockets.lock() {
            if !sockets.iter().any(|socket| socket.stream == stream) && sockets.len() < MAX_RELAYS {
                sockets.push(Socket { stream, pipe: None });
            }
        }
    }

    /// Walk the committed private read-write regions for byte-streams that
    /// still use the client's vtable, and swap their vtables.
    fn scan(&self) {
        let mut info = SYSTEM_INFO::default();
        // SAFETY: fills a SYSTEM_INFO.
        unsafe { GetSystemInfo(&mut info) };
        let mut address = info.lpMinimumApplicationAddress as u64;
        let highest = info.lpMaximumApplicationAddress as u64;
        let mut chunk = vec![0u8; SCAN_CHUNK];
        while address < highest {
            let mut region = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: queries this process's address space into a valid struct.
            let filled = unsafe {
                VirtualQuery(
                    address as *const c_void,
                    &mut region,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if filled == 0 {
                break;
            }
            let base = region.BaseAddress as u64;
            let size = region.RegionSize as u64;
            if region.State == MEM_COMMIT
                && region.Type == MEM_PRIVATE
                && in_heap(base)
                && region.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0
                && region.Protect & PAGE_GUARD == 0
                && size <= MAX_REGION
            {
                self.scan_region(base, base + size, &mut chunk);
            }
            address = base + size;
        }
    }

    fn scan_region(&self, start: u64, end: u64, chunk: &mut [u8]) {
        let mut at = start;
        while at < end {
            let length = (end - at).min(chunk.len() as u64) as usize;
            if read(at, &mut chunk[..length]) {
                let found: Vec<u64> =
                    matching_words(&chunk[..length], at, end, self.byte_stream_vtable).collect();
                for byte_stream in found {
                    self.swap(byte_stream);
                }
            }
            at += length as u64;
        }
    }

    fn swap(&self, byte_stream: u64) {
        let Some(stream) = read_u64(byte_stream + SSL_STREAM_OFFSET).filter(|&s| in_heap(s)) else {
            return;
        };
        let Some(vtable) = read_u64(stream).filter(|&v| self.in_image(v)) else {
            return;
        };
        let Ok(mut tables) = self.tables.lock() else {
            return;
        };
        if let Some(tables) = tables.as_ref() {
            let replaced = tables.socket.as_ptr() as u64;
            if vtable == replaced || vtable != tables.original_socket {
                return; // already swapped, or a different stream class
            }
        }
        let swaps = self.swaps.fetch_add(1, Ordering::Relaxed) + 1;
        let port = read_u32(byte_stream + PORT_OFFSET).unwrap_or(0) as u16;
        if swaps <= 20 || port != BATTLE_NET_PORT {
            let length = read_u64(byte_stream + HOST_LENGTH_OFFSET)
                .unwrap_or(0)
                .min(79) as usize;
            let pointer = read_u64(byte_stream + HOST_OFFSET).unwrap_or(0);
            let host_at = if in_heap(pointer) {
                pointer
            } else {
                byte_stream + HOST_OFFSET
            };
            let mut host = vec![0u8; length];
            if !read(host_at, &mut host) {
                host.clear();
            }
            let state = read_u32(byte_stream + STATE_OFFSET).unwrap_or(0);
            self.log(&format!(
                "BYTE-STREAM bs={byte_stream:016X} sock={stream:016X} sockVt(rva {:X}) state={state} host='{}' port={port} swaps={swaps}",
                vtable - self.image_start,
                printable(&host)
            ));
        }
        self.register(stream);
        let tables = tables.get_or_insert_with(|| self.build_tables(vtable));
        write_u64(stream, tables.socket.as_ptr() as u64);
        write_u64(byte_stream, tables.byte_stream.as_ptr() as u64);
    }

    fn build_tables(&self, socket_vtable: u64) -> Tables {
        let copy = |from: u64| {
            let mut slots = [0u64; VTABLE_SLOTS];
            for (index, slot) in slots.iter_mut().enumerate() {
                *slot = read_u64(from + index as u64 * 8).unwrap_or(0);
            }
            slots
        };
        let mut socket = copy(socket_vtable);
        socket[RECV_SLOT] = recv_thunk as *const () as u64;
        socket[SEND_SLOT] = send_thunk as *const () as u64;
        let mut byte_stream = copy(self.byte_stream_vtable);
        byte_stream[STATE_SLOT] = state_thunk as *const () as u64;
        let tables = Tables {
            original_socket: socket_vtable,
            socket: Box::new(socket),
            byte_stream: Box::new(byte_stream),
        };
        self.log(&format!(
            "built vts: sockHeap={:p} (from {socket_vtable:X} rva {:X}) bsHeap={:p}",
            tables.socket.as_ptr(),
            socket_vtable - self.image_start,
            tables.byte_stream.as_ptr()
        ));
        tables
    }

    fn send(&self, stream: u64, buffer: *const u8, length: u64) {
        let count = self.sends.fetch_add(1, Ordering::Relaxed) + 1;
        let pipe = self.pipe(stream);
        let address = buffer as u64;
        let readable = in_heap(address) || self.in_image(address);
        if count <= 8 || count % 500 == 0 {
            self.log(&format!(
                "SEND #{count} self={stream:016X} len={length} relay={}",
                pipe.is_some()
            ));
            if readable {
                let mut preview = vec![0u8; length.min(48) as usize];
                if read(address, &mut preview) {
                    self.log(&format!(
                        "  send[{}]: {}",
                        preview.len(),
                        hex_preview(&preview)
                    ));
                }
            }
        }
        let (Some(pipe), true) = (pipe, readable) else {
            return;
        };
        // SAFETY: the client passes its own buffer of `length` bytes, already
        // checked to lie in its heap or image.
        let data = unsafe { std::slice::from_raw_parts(buffer, length as usize) };
        if let Err(error) = (&*pipe).write_all(data) {
            self.log(&format!("  send() err {error}"));
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn recv(&self, stream: u64, destination: *mut u8, capacity: u64) -> u64 {
        let count = self.recvs.fetch_add(1, Ordering::Relaxed) + 1;
        let Some(pipe) = self.pipe(stream) else {
            return 0;
        };
        if destination.is_null() || capacity == 0 {
            return 0;
        }
        let capacity = capacity.min(i32::MAX as u64) as usize;
        // SAFETY: the client passes its own receive buffer of `capacity` bytes.
        let buffer = unsafe { std::slice::from_raw_parts_mut(destination, capacity) };
        match (&*pipe).read(buffer) {
            Ok(0) => {
                if count <= 8 {
                    self.log(&format!("RECV #{count} server closed"));
                }
                0
            }
            Ok(got) => {
                if count <= 8 || count % 500 == 0 {
                    self.log(&format!("RECV #{count} self={stream:016X} got={got}"));
                    self.log(&format!(
                        "  recv[{}]: {}",
                        got.min(48),
                        hex_preview(&buffer[..got])
                    ));
                }
                got as u64
            }
            Err(error) => {
                if count <= 8
                    && !matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    )
                {
                    self.log(&format!("RECV #{count} recv err {error}"));
                }
                0
            }
        }
    }
}

/// Byte-stream slot 1: the connection state; 1 means connected.
extern "C" fn state_thunk(_byte_stream: *mut c_void) -> i32 {
    let _ = catch_unwind(|| {
        if let Some(relay) = RELAY.get() {
            relay.state_calls.fetch_add(1, Ordering::Relaxed);
        }
    });
    1
}

/// `ssl_stream` slot 3: `send(this, buffer, length, *sent) -> 0`. Always
/// reports the whole buffer as sent.
extern "C" fn send_thunk(
    stream: *mut c_void,
    buffer: *mut c_void,
    length: u64,
    sent: *mut u64,
) -> i32 {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(relay) = RELAY.get() {
            relay.send(stream as u64, buffer.cast(), length);
        }
    }));
    if !sent.is_null() {
        // SAFETY: the client passes a pointer to its result variable.
        unsafe { *sent = length };
    }
    0
}

/// `ssl_stream` slot 2: `recv(this, destination, capacity, *got) -> 0`.
/// Waits briefly for data and reports 0 bytes when there is none.
extern "C" fn recv_thunk(
    stream: *mut c_void,
    destination: *mut c_void,
    capacity: u64,
    got: *mut u64,
) -> i32 {
    let received = catch_unwind(AssertUnwindSafe(|| {
        RELAY.get().map_or(0, |relay| {
            relay.recv(stream as u64, destination.cast(), capacity)
        })
    }))
    .unwrap_or(0);
    if !got.is_null() {
        // SAFETY: the client passes a pointer to its result variable.
        unsafe { *got = received };
    }
    0
}

fn read(address: u64, buffer: &mut [u8]) -> bool {
    let mut copied = 0;
    // SAFETY: reads this process's memory into a valid buffer; the call fails
    // instead of faulting when the source is not readable.
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut copied,
        )
    };
    ok != 0 && copied == buffer.len()
}

fn read_u64(address: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    read(address, &mut bytes).then(|| u64::from_le_bytes(bytes))
}

fn read_u32(address: u64) -> Option<u32> {
    let mut bytes = [0u8; 4];
    read(address, &mut bytes).then(|| u32::from_le_bytes(bytes))
}

fn write_u64(address: u64, value: u64) -> bool {
    let bytes = value.to_le_bytes();
    let mut copied = 0;
    // SAFETY: writes this process's memory; the call fails instead of
    // faulting when the target is not writable.
    let ok = unsafe {
        WriteProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            bytes.as_ptr().cast(),
            bytes.len(),
            &mut copied,
        )
    };
    ok != 0 && copied == bytes.len()
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
            Some(module.parent()?.join("log").join("wfd.log"))
        })?;
    if let Some(folder) = path.parent() {
        let _ = fs::create_dir_all(folder);
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn start() -> Option<&'static Relay> {
    let game: Vec<u16> = "Overwatch.exe".encode_utf16().chain([0]).collect();
    // SAFETY: valid NUL-terminated name; a null name means this executable.
    let base = unsafe {
        let module = GetModuleHandleW(game.as_ptr());
        if module.is_null() {
            GetModuleHandleW(ptr::null())
        } else {
            module
        }
    } as u64;
    let mut headers = vec![0u8; 0x1000];
    let size = if read(base, &mut headers) {
        image_size(&headers)?
    } else {
        return None;
    };
    let relay = RELAY.get_or_init(|| Relay {
        image_start: base,
        image_end: base + size,
        byte_stream_vtable: base + BYTE_STREAM_VTABLE,
        log: Mutex::new(open_log()),
        sockets: Mutex::new(Vec::new()),
        tables: Mutex::new(None),
        swaps: AtomicI64::new(0),
        sends: AtomicI64::new(0),
        recvs: AtomicI64::new(0),
        state_calls: AtomicI64::new(0),
        errors: AtomicI64::new(0),
    });
    relay.log(&format!(
        "==== owwfd_relay (plaintext pipe -> 127.0.0.1:{RELAY_PORT}) base={base:016X} bsVt={:016X} ====",
        relay.byte_stream_vtable
    ));
    Some(relay)
}

fn run() {
    let Some(relay) = catch_unwind(start).ok().flatten() else {
        return;
    };
    for turn in 0u64.. {
        if catch_unwind(AssertUnwindSafe(|| relay.scan())).is_err() {
            relay.log("scan failed; the relay stops scanning");
            return;
        }
        if turn % 80 == 0 {
            relay.log(&format!(
                "[poll] loop={turn} swaps={} send={} recv={} stateCalls={} relays={} err={}",
                relay.swaps.load(Ordering::Relaxed),
                relay.sends.load(Ordering::Relaxed),
                relay.recvs.load(Ordering::Relaxed),
                relay.state_calls.load(Ordering::Relaxed),
                relay.sockets.lock().map_or(0, |sockets| sockets.len()),
                relay.errors.load(Ordering::Relaxed),
            ));
        }
        thread::sleep(SCAN_PERIOD);
    }
}

extern "system" fn worker(_: *mut c_void) -> u32 {
    let _ = catch_unwind(run);
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
