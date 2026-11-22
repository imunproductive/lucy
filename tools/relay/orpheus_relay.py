#!/usr/bin/env python3
"""ORPHEUS relay without a DLL: pipes the Overwatch 1.74 client's Battle.net
connection to a local ORPHEUS server.

It does what owwfd_relay.dll does, from outside the game:

1. Finds the client's Battle.net byte-streams in the game's memory (objects
   whose first word is the byte-stream vtable of build 104319).
2. Gives each one a copy of its vtable whose "state" slot always says
   "connected", so the client writes plaintext instead of starting TLS.
3. Gives the stream's ssl_stream a copy of its vtable whose send and recv
   slots go to three small functions this script writes into the game. They
   open a TCP connection to the server's RPC port (127.0.0.1:21119) with the
   game's own ws2_32 and pass the bytes through.

The functions are the only machine code written into the game. Their source,
with every instruction, is in THUNKS_SOURCE below (thunks.asm in the ORPHEUS
repository); THUNKS holds the assembled bytes (nasm -f bin). Nothing in the
game's own code is changed.

Usage (64-bit Python 3.8 or newer on Windows, nothing to install):

    python orpheus_relay.py                      # attach to the running Overwatch.exe,
                                                 # or start ./Overwatch.exe
    python orpheus_relay.py D:\\Games\\Overwatch\\Overwatch.exe
    python orpheus_relay.py --pid 1234
    python orpheus_relay.py --start D:\\Games\\Overwatch\\Overwatch.exe -- --tank_Locale=enUS

A game the script starts gets --BNetServer=127.0.0.1:1119, as `orpheus play`
gives it. A game you start yourself needs that argument too. Keep the script
running while you play: it swaps the streams the client makes when it
reconnects. Only the client build 1.74.0.0.104319 is supported.
"""

import argparse
import ctypes
import ctypes.wintypes as wt
import ipaddress
import os
import struct
import subprocess
import sys
import time

# --- Build 104319 -------------------------------------------------------------

BYTE_STREAM_VTABLE = 0x298_E2F8  # offset from the image base
STATE_OFFSET = 0x20
SSL_STREAM_OFFSET = 0x48
HOST_OFFSET = 0x80
HOST_LENGTH_OFFSET = 0x88
PORT_OFFSET = 0xA8
BYTE_STREAM_SIZE = 0xC0
STATE_SLOT = 1  # byte-stream: connection state
RECV_SLOT = 2  # ssl_stream: recv
SEND_SLOT = 3  # ssl_stream: send
VTABLE_SLOTS = 64
MAX_RELAYS = 64
BATTLE_NET_PORT = 1119
RELAY_PORT = 21119
MAX_REGION = 0x800_0000
SCAN_CHUNK = 1 << 20
SCAN_PERIOD = 0.008
POLL_PERIOD = 10.0  # seconds between status lines
RECV_WAIT_MS = 30

# --- The functions written into the game --------------------------------------

THUNKS_SOURCE = r"""
; nasm -f bin: one allocation in the game, this code at +0, the data page at +0x1000.
BITS 64
DEFAULT REL
%define DATA            0x1000
%define D_SOCKET        DATA + 0x00     ; ws2_32!socket
%define D_CONNECT       DATA + 0x08     ; ws2_32!connect
%define D_SEND          DATA + 0x10     ; ws2_32!send
%define D_RECV          DATA + 0x18     ; ws2_32!recv
%define D_SETSOCKOPT    DATA + 0x20     ; ws2_32!setsockopt
%define D_CLOSESOCKET   DATA + 0x28     ; ws2_32!closesocket
%define D_ADDRESS       DATA + 0x30     ; sockaddr_in of the RPC port (16 bytes)
%define D_TIMEOUT       DATA + 0x40     ; DWORD receive timeout in ms
%define D_STATE_CALLS   DATA + 0x48     ; counters, read by the script's log
%define D_SENDS         DATA + 0x50
%define D_RECVS         DATA + 0x58
%define D_ERRORS        DATA + 0x60
%define D_CONNECTS      DATA + 0x68
%define D_FAILURES      DATA + 0x70
%define D_ENTRIES       DATA + 0x100    ; 64 x {stream, socket}
%define ENTRIES         64
%define NOT_CONNECTED   -1              ; socket slot: connect on next use
%define CONNECTING      -2              ; socket slot: another thread connects

base:
state:                                  ; +0x000 int state(byte_stream) -> 1
    lock inc qword [base + D_STATE_CALLS]
    mov eax, 1
    ret

    align 16
socket_of:                              ; +0x010 rax = socket of ssl_stream rcx, or -1
    push rbx
    push rsi
    sub rsp, 0x38
    lea rbx, [base + D_ENTRIES]
    mov esi, ENTRIES
.find:
    cmp [rbx], rcx
    je .found
    add rbx, 16
    dec esi
    jnz .find
    mov rax, -1                         ; not a registered stream
    jmp .out
.found:
    mov rax, [rbx + 8]
    cmp rax, CONNECTING
    jne .claim
    pause                               ; another thread is connecting it
    jmp .found
.claim:
    cmp rax, NOT_CONNECTED
    jne .out                            ; connected already
    mov rdx, CONNECTING
    lock cmpxchg [rbx + 8], rdx
    jne .found                          ; another thread got there first
    mov ecx, 2                          ; AF_INET
    mov edx, 1                          ; SOCK_STREAM
    mov r8d, 6                          ; IPPROTO_TCP
    call [base + D_SOCKET]
    cmp rax, -1                         ; INVALID_SOCKET
    je .failed
    mov rsi, rax
    mov rcx, rsi
    mov edx, 0xffff                     ; SOL_SOCKET
    mov r8d, 0x1006                     ; SO_RCVTIMEO
    lea r9, [base + D_TIMEOUT]
    mov qword [rsp + 0x20], 4
    call [base + D_SETSOCKOPT]
    mov rcx, rsi
    lea rdx, [base + D_ADDRESS]
    mov r8d, 16
    call [base + D_CONNECT]
    test eax, eax
    jnz .refused
    lock inc qword [base + D_CONNECTS]
    mov rax, rsi
    mov [rbx + 8], rax
    jmp .out
.refused:
    mov rcx, rsi
    call [base + D_CLOSESOCKET]
.failed:
    lock inc qword [base + D_FAILURES]
    mov rax, NOT_CONNECTED
    mov [rbx + 8], rax                  ; the next call tries again
.out:
    add rsp, 0x38
    pop rsi
    pop rbx
    ret

    align 16
send:                                   ; +0x100 int send(ssl_stream, buffer, length, *sent) -> 0
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    sub rsp, 0x20
    mov rsi, rdx
    mov rdi, r8
    mov r12, r9
    mov r13, r8
    lock inc qword [base + D_SENDS]
    call socket_of
    cmp rax, -1
    je .done
    mov rbx, rax
.next:
    test rdi, rdi
    jz .done
    mov r8, rdi
    mov eax, 0x7fffffff
    cmp r8, rax
    cmova r8, rax
    mov rcx, rbx
    mov rdx, rsi
    xor r9d, r9d
    call [base + D_SEND]
    test eax, eax
    jle .error
    movsxd rax, eax
    add rsi, rax
    sub rdi, rax
    jmp .next
.error:
    lock inc qword [base + D_ERRORS]
.done:
    test r12, r12                       ; always "sent the whole buffer", like the DLL
    jz .ret
    mov [r12], r13
.ret:
    xor eax, eax
    add rsp, 0x20
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret

    align 16
recv:                                   ; +0x180 int recv(ssl_stream, destination, capacity, *got) -> 0
    push rbx
    push rsi
    push rdi
    push r12
    push r13
    sub rsp, 0x20
    mov rsi, rdx
    mov rdi, r8
    mov r12, r9
    xor r13d, r13d
    lock inc qword [base + D_RECVS]
    call socket_of
    cmp rax, -1
    je .done
    test rsi, rsi
    jz .done
    test rdi, rdi
    jz .done
    mov rcx, rax
    mov rdx, rsi
    mov r8, rdi
    mov eax, 0x7fffffff
    cmp r8, rax
    cmova r8, rax
    xor r9d, r9d
    call [base + D_RECV]                ; waits up to the receive timeout
    test eax, eax
    jle .done                           ; closed, timed out or failed: 0 bytes
    mov r13d, eax
.done:
    test r12, r12
    jz .ret
    mov [r12], r13
.ret:
    xor eax, eax
    add rsp, 0x20
    pop r13
    pop r12
    pop rdi
    pop rsi
    pop rbx
    ret
"""

THUNKS = bytes.fromhex(
    "f048ff0540100000b801000000c3909053564883ec38488d1de3100000be4000"
    "000048390b74144883c310ffce75f348c7c0ffffffffe9b1000000488b430848"
    "83f8fe7504f390ebf24883f8ff0f859900000048c7c2fefffffff0480fb15308"
    "75d9b902000000ba0100000041b806000000ff15880f00004883f8ff745b4889"
    "c64889f1baffff000041b8061000004c8d0daa0f000048c744242004000000ff"
    "157b0f00004889f1488d15810f000041b810000000ff154d0f000085c07511f0"
    "48ff05a10f00004889f048894308eb1c4889f1ff154f0f0000f048ff058f0f00"
    "0048c7c0ffffffff488943084883c4385e5bc390909090909090909090909090"
    "535657415441554883ec204889d64c89c74d89cc4d89c5f048ff05310f0000e8"
    "ecfeffff4883f8ff743d4889c34885ff74354989f8b8ffffff7f4939c04c0f47"
    "c04889d94889f24531c9ff15c00e000085c07e0b4863c04801c64829c7ebcef0"
    "48ff05f90e00004d85e474044d892c2431c04883c420415d415c5f5e5bc39090"
    "535657415441554883ec204889d64c89c74d89cc4531edf048ff05b90e0000e8"
    "6cfeffff4883f8ff742f4885f6742a4885ff74254889c14889f24989f8b8ffff"
    "ff7f4939c04c0f47c04531c9ff15460e000085c07e034189c54d85e474044d89"
    "2c2431c04883c420415d415c5f5e5bc3"
)
STATE_ENTRY = 0x000
SEND_ENTRY = 0x100
RECV_ENTRY = 0x180

# The allocation: code page, data page, the two replacement vtables.
BLOCK_SIZE = 0x3000
DATA = 0x1000
DATA_FUNCTIONS = DATA + 0x00  # socket, connect, send, recv, setsockopt, closesocket
WS2_FUNCTIONS = ("socket", "connect", "send", "recv", "setsockopt", "closesocket")
DATA_ADDRESS = DATA + 0x30
DATA_TIMEOUT = DATA + 0x40
DATA_COUNTERS = DATA + 0x48
COUNTERS = ("stateCalls", "send", "recv", "err", "connects", "connectFailures")
DATA_ENTRIES = DATA + 0x100
NOT_CONNECTED = 0xFFFF_FFFF_FFFF_FFFF
SOCKET_TABLE = 0x2000
BYTE_STREAM_TABLE = 0x2000 + VTABLE_SLOTS * 8

# --- Windows ------------------------------------------------------------------

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
ntdll = ctypes.WinDLL("ntdll")

PROCESS_ACCESS = 0x0008 | 0x0010 | 0x0020 | 0x0400 | 0x1000 | 0x00100000
# VM_OPERATION | VM_READ | VM_WRITE | QUERY_INFORMATION | QUERY_LIMITED | SYNCHRONIZE
MEM_COMMIT, MEM_RESERVE, MEM_PRIVATE = 0x1000, 0x2000, 0x20000
PAGE_READWRITE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_GUARD = 0x04, 0x20, 0x40, 0x100
CREATE_SUSPENDED = 0x4
TH32CS_SNAPPROCESS, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32 = 0x2, 0x8, 0x10
INVALID_HANDLE = ctypes.c_void_p(-1).value
WAIT_OBJECT_0 = 0
LOWEST, HIGHEST = 0x10000, 0x7FFF_FFFE_FFFF


class MEMORY_BASIC_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("BaseAddress", ctypes.c_void_p),
        ("AllocationBase", ctypes.c_void_p),
        ("AllocationProtect", wt.DWORD),
        ("PartitionId", wt.WORD),
        ("RegionSize", ctypes.c_size_t),
        ("State", wt.DWORD),
        ("Protect", wt.DWORD),
        ("Type", wt.DWORD),
    ]


class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wt.DWORD),
        ("cntUsage", wt.DWORD),
        ("th32ProcessID", wt.DWORD),
        ("th32DefaultHeapID", ctypes.c_size_t),
        ("th32ModuleID", wt.DWORD),
        ("cntThreads", wt.DWORD),
        ("th32ParentProcessID", wt.DWORD),
        ("pcPriClassBase", wt.LONG),
        ("dwFlags", wt.DWORD),
        ("szExeFile", wt.WCHAR * 260),
    ]


class MODULEENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wt.DWORD),
        ("th32ModuleID", wt.DWORD),
        ("th32ProcessID", wt.DWORD),
        ("GlblcntUsage", wt.DWORD),
        ("ProccntUsage", wt.DWORD),
        ("modBaseAddr", ctypes.c_void_p),
        ("modBaseSize", wt.DWORD),
        ("hModule", wt.HMODULE),
        ("szModule", wt.WCHAR * 256),
        ("szExePath", wt.WCHAR * 260),
    ]


class PROCESS_BASIC_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("ExitStatus", ctypes.c_void_p),
        ("PebBaseAddress", ctypes.c_void_p),
        ("AffinityMask", ctypes.c_void_p),
        ("BasePriority", ctypes.c_void_p),
        ("UniqueProcessId", ctypes.c_void_p),
        ("InheritedFromUniqueProcessId", ctypes.c_void_p),
    ]


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wt.DWORD),
        ("lpReserved", wt.LPWSTR),
        ("lpDesktop", wt.LPWSTR),
        ("lpTitle", wt.LPWSTR),
        ("dwX", wt.DWORD),
        ("dwY", wt.DWORD),
        ("dwXSize", wt.DWORD),
        ("dwYSize", wt.DWORD),
        ("dwXCountChars", wt.DWORD),
        ("dwYCountChars", wt.DWORD),
        ("dwFillAttribute", wt.DWORD),
        ("dwFlags", wt.DWORD),
        ("wShowWindow", wt.WORD),
        ("cbReserved2", wt.WORD),
        ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wt.HANDLE),
        ("hStdOutput", wt.HANDLE),
        ("hStdError", wt.HANDLE),
    ]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("hProcess", wt.HANDLE),
        ("hThread", wt.HANDLE),
        ("dwProcessId", wt.DWORD),
        ("dwThreadId", wt.DWORD),
    ]


def _bind(library, name, result, *arguments):
    function = getattr(library, name)
    function.restype = result
    function.argtypes = arguments
    return function


SIZE_P = ctypes.POINTER(ctypes.c_size_t)
OpenProcess = _bind(kernel32, "OpenProcess", wt.HANDLE, wt.DWORD, wt.BOOL, wt.DWORD)
CloseHandle = _bind(kernel32, "CloseHandle", wt.BOOL, wt.HANDLE)
ReadProcessMemory = _bind(
    kernel32, "ReadProcessMemory", wt.BOOL, wt.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, SIZE_P
)
WriteProcessMemory = _bind(
    kernel32, "WriteProcessMemory", wt.BOOL, wt.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, SIZE_P
)
VirtualAllocEx = _bind(
    kernel32, "VirtualAllocEx", ctypes.c_void_p, wt.HANDLE, ctypes.c_void_p, ctypes.c_size_t, wt.DWORD, wt.DWORD
)
VirtualProtectEx = _bind(
    kernel32,
    "VirtualProtectEx",
    wt.BOOL,
    wt.HANDLE,
    ctypes.c_void_p,
    ctypes.c_size_t,
    wt.DWORD,
    ctypes.POINTER(wt.DWORD),
)
VirtualQueryEx = _bind(
    kernel32,
    "VirtualQueryEx",
    ctypes.c_size_t,
    wt.HANDLE,
    ctypes.c_void_p,
    ctypes.POINTER(MEMORY_BASIC_INFORMATION),
    ctypes.c_size_t,
)
FlushInstructionCache = _bind(kernel32, "FlushInstructionCache", wt.BOOL, wt.HANDLE, ctypes.c_void_p, ctypes.c_size_t)
WaitForSingleObject = _bind(kernel32, "WaitForSingleObject", wt.DWORD, wt.HANDLE, wt.DWORD)
GetExitCodeProcess = _bind(kernel32, "GetExitCodeProcess", wt.BOOL, wt.HANDLE, ctypes.POINTER(wt.DWORD))
ResumeThread = _bind(kernel32, "ResumeThread", wt.DWORD, wt.HANDLE)
TerminateProcess = _bind(kernel32, "TerminateProcess", wt.BOOL, wt.HANDLE, wt.UINT)
CreateToolhelp32Snapshot = _bind(kernel32, "CreateToolhelp32Snapshot", wt.HANDLE, wt.DWORD, wt.DWORD)
Process32FirstW = _bind(kernel32, "Process32FirstW", wt.BOOL, wt.HANDLE, ctypes.POINTER(PROCESSENTRY32W))
Process32NextW = _bind(kernel32, "Process32NextW", wt.BOOL, wt.HANDLE, ctypes.POINTER(PROCESSENTRY32W))
Module32FirstW = _bind(kernel32, "Module32FirstW", wt.BOOL, wt.HANDLE, ctypes.POINTER(MODULEENTRY32W))
Module32NextW = _bind(kernel32, "Module32NextW", wt.BOOL, wt.HANDLE, ctypes.POINTER(MODULEENTRY32W))
QueryFullProcessImageNameW = _bind(
    kernel32, "QueryFullProcessImageNameW", wt.BOOL, wt.HANDLE, wt.DWORD, wt.LPWSTR, ctypes.POINTER(wt.DWORD)
)
CreateProcessW = _bind(
    kernel32,
    "CreateProcessW",
    wt.BOOL,
    wt.LPCWSTR,
    wt.LPWSTR,
    ctypes.c_void_p,
    ctypes.c_void_p,
    wt.BOOL,
    wt.DWORD,
    ctypes.c_void_p,
    wt.LPCWSTR,
    ctypes.POINTER(STARTUPINFOW),
    ctypes.POINTER(PROCESS_INFORMATION),
)
NtQueryInformationProcess = _bind(
    ntdll,
    "NtQueryInformationProcess",
    wt.LONG,
    wt.HANDLE,
    ctypes.c_int,
    ctypes.c_void_p,
    wt.ULONG,
    ctypes.POINTER(wt.ULONG),
)


class RelayError(Exception):
    pass


def windows_error(what):
    code = ctypes.get_last_error()
    return RelayError(f"{what} failed: {ctypes.FormatError(code).strip()} (error {code})")


def in_heap(pointer):
    """A heap pointer as the client allocates it: 8-byte aligned, above the low 1 TiB."""
    return 0x0000_0100_0000_0000 <= pointer < 0x0000_7FF0_0000_0000 and pointer & 7 == 0


def printable(data):
    return "".join(chr(byte) if 32 <= byte < 127 else "." for byte in data)


def hex_preview(data):
    return " ".join(f"{byte:02X}" for byte in data[:48])


# --- Processes ----------------------------------------------------------------


def processes_named(name):
    """PIDs of the processes whose executable is `name` (case-insensitive)."""
    snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    if snapshot == INVALID_HANDLE:
        raise windows_error("CreateToolhelp32Snapshot")
    found = []
    try:
        entry = PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(entry)
        more = Process32FirstW(snapshot, ctypes.byref(entry))
        while more:
            if entry.szExeFile.lower() == name.lower():
                found.append(entry.th32ProcessID)
            more = Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        CloseHandle(snapshot)
    return found


def image_path(handle):
    size = wt.DWORD(32768)
    buffer = ctypes.create_unicode_buffer(size.value)
    if not QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)):
        return None
    return buffer.value


def open_process(pid):
    handle = OpenProcess(PROCESS_ACCESS, False, pid)
    if not handle:
        raise windows_error(f"OpenProcess({pid}) (run this as the same user as the game, or as administrator)")
    return handle


def start_suspended(game, arguments):
    """Start the game in its folder with its main thread frozen."""
    command = subprocess.list2cmdline([game, *arguments])
    startup = STARTUPINFOW()
    startup.cb = ctypes.sizeof(startup)
    info = PROCESS_INFORMATION()
    line = ctypes.create_unicode_buffer(command)
    if not CreateProcessW(
        game,
        line,
        None,
        None,
        False,
        CREATE_SUSPENDED,
        None,
        os.path.dirname(game),
        ctypes.byref(startup),
        ctypes.byref(info),
    ):
        raise windows_error(f"starting {game}")
    return info


# --- The relay ----------------------------------------------------------------


class Game:
    """Memory of the game process, read and written from outside."""

    def __init__(self, handle, pid):
        self.handle = handle
        self.pid = pid

    def read(self, address, length):
        buffer = ctypes.create_string_buffer(length)
        copied = ctypes.c_size_t()
        if ReadProcessMemory(self.handle, address, buffer, length, ctypes.byref(copied)) and copied.value == length:
            return buffer.raw
        return None

    def read_into(self, address, buffer, length):
        copied = ctypes.c_size_t()
        return (
            bool(ReadProcessMemory(self.handle, address, buffer, length, ctypes.byref(copied)))
            and copied.value == length
        )

    def u64(self, address):
        data = self.read(address, 8)
        return None if data is None else struct.unpack("<Q", data)[0]

    def u32(self, address):
        data = self.read(address, 4)
        return None if data is None else struct.unpack("<I", data)[0]

    def write(self, address, data):
        copied = ctypes.c_size_t()
        if not WriteProcessMemory(self.handle, address, data, len(data), ctypes.byref(copied)) or copied.value != len(
            data
        ):
            raise windows_error(f"writing {len(data)} bytes at {address:016X}")

    def write_u64(self, address, value):
        self.write(address, struct.pack("<Q", value))

    def exited(self):
        return WaitForSingleObject(self.handle, 0) == WAIT_OBJECT_0

    def image_base(self):
        """The executable's base, from the PEB: readable even before the loader ran."""
        info = PROCESS_BASIC_INFORMATION()
        status = NtQueryInformationProcess(self.handle, 0, ctypes.byref(info), ctypes.sizeof(info), None)
        if status != 0:
            raise RelayError(f"NtQueryInformationProcess failed: status {status & 0xFFFFFFFF:08X}")
        base = self.u64(info.PebBaseAddress + 0x10)  # PEB.ImageBaseAddress
        if not base:
            raise RelayError("could not read the game's image base")
        return base

    def image_size(self, base):
        headers = self.read(base, 0x1000)
        if headers is None or headers[:2] != b"MZ":
            raise RelayError("the game's PE headers are not readable")
        nt = struct.unpack_from("<I", headers, 0x3C)[0]
        if headers[nt : nt + 4] != b"PE\0\0":
            raise RelayError("the game's PE headers are not readable")
        return struct.unpack_from("<I", headers, nt + 24 + 56)[0]  # SizeOfImage

    def module(self, name):
        """Base of a loaded module, or None while it isn't loaded (or the loader is busy)."""
        snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, self.pid)
        if snapshot == INVALID_HANDLE:
            return None
        try:
            entry = MODULEENTRY32W()
            entry.dwSize = ctypes.sizeof(entry)
            more = Module32FirstW(snapshot, ctypes.byref(entry))
            while more:
                if entry.szModule.lower() == name.lower():
                    return entry.modBaseAddr
                more = Module32NextW(snapshot, ctypes.byref(entry))
        finally:
            CloseHandle(snapshot)
        return None

    def exports(self, base, names):
        """Addresses of the named exports of the module at `base`."""
        headers = self.read(base, 0x1000)
        nt = struct.unpack_from("<I", headers, 0x3C)[0]
        directory_rva, directory_size = struct.unpack_from("<II", headers, nt + 24 + 112)
        directory = self.read(base + directory_rva, 40)
        count_names, functions_rva, names_rva, ordinals_rva = struct.unpack_from("<IIII", directory, 24)
        count_functions = struct.unpack_from("<I", directory, 20)[0]
        functions = struct.unpack(f"<{count_functions}I", self.read(base + functions_rva, 4 * count_functions))
        name_rvas = struct.unpack(f"<{count_names}I", self.read(base + names_rva, 4 * count_names))
        ordinals = struct.unpack(f"<{count_names}H", self.read(base + ordinals_rva, 2 * count_names))
        wanted = set(names)
        found = {}
        for name_rva, ordinal in zip(name_rvas, ordinals):
            raw = self.read(base + name_rva, 64) or b""
            name = raw.split(b"\0", 1)[0].decode("ascii", "replace")
            if name in wanted:
                rva = functions[ordinal]
                if directory_rva <= rva < directory_rva + directory_size:
                    raise RelayError(f"ws2_32!{name} is forwarded; this Windows is not supported")
                found[name] = base + rva
        missing = wanted - found.keys()
        if missing:
            raise RelayError(f"ws2_32 has no {', '.join(sorted(missing))}")
        return found


class Relay:
    def __init__(self, game, host, port, log):
        self.game = game
        self.host = host
        self.port = port
        self.log_file = log
        self.image_start = game.image_base()
        self.image_end = None  # read at the first scan: a frozen game may still be mapping
        self.byte_stream_vtable = self.image_start + BYTE_STREAM_VTABLE
        self.block = self.allocate()
        self.ws2_ready = False
        self.original_socket = None
        self.streams = []  # ssl_streams with an entry, in entry order
        self.connected = set()
        self.swaps = 0
        self.chunk = bytearray(SCAN_CHUNK)
        self.chunk_view = (ctypes.c_char * SCAN_CHUNK).from_buffer(self.chunk)
        self.log(
            f"==== orpheus_relay (plaintext pipe -> {host}:{port}) base={self.image_start:016X} "
            f"bsVt={self.byte_stream_vtable:016X} block={self.block:016X} ===="
        )

    def log(self, message):
        line = f"[wfd] {message}"
        print(line, flush=True)
        if self.log_file:
            self.log_file.write(time.strftime("%H:%M:%S ") + line + "\n")
            self.log_file.flush()

    def allocate(self):
        """The code, data and vtable pages in the game. They are never freed: the
        game may call them until it exits, even after this script stops."""
        block = VirtualAllocEx(self.game.handle, None, BLOCK_SIZE, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
        if not block:
            raise windows_error("VirtualAllocEx")
        data = bytearray(BLOCK_SIZE - DATA)
        # sockaddr_in: AF_INET, then the port and the address in network order.
        address = struct.pack("<H", 2) + struct.pack(">H", self.port) + ipaddress.IPv4Address(self.host).packed
        data[DATA_ADDRESS - DATA : DATA_ADDRESS - DATA + 16] = address.ljust(16, b"\0")
        struct.pack_into("<I", data, DATA_TIMEOUT - DATA, RECV_WAIT_MS)
        for entry in range(MAX_RELAYS):
            struct.pack_into("<QQ", data, DATA_ENTRIES - DATA + 16 * entry, 0, NOT_CONNECTED)
        self.game.write(block + DATA, bytes(data))
        self.game.write(block, THUNKS)
        old = wt.DWORD()
        if not VirtualProtectEx(self.game.handle, block, DATA, PAGE_EXECUTE_READ, ctypes.byref(old)):
            raise windows_error("VirtualProtectEx")
        FlushInstructionCache(self.game.handle, block, DATA)
        return block

    def resolve_ws2(self):
        """Point the data page at the game's ws2_32, once the game has loaded it."""
        if self.ws2_ready:
            return True
        base = self.game.module("ws2_32.dll")
        if base is None:
            return False
        exports = self.game.exports(base, WS2_FUNCTIONS)
        self.game.write(
            self.block + DATA_FUNCTIONS, b"".join(struct.pack("<Q", exports[name]) for name in WS2_FUNCTIONS)
        )
        self.ws2_ready = True
        self.log(f"ws2_32 at {base:016X}")
        return True

    def in_image(self, address):
        return self.image_start <= address < self.image_end

    def scan(self):
        """Walk the committed private read-write regions for byte-streams that still
        use the client's vtable, and swap their vtables."""
        if self.image_end is None:
            try:
                self.image_end = self.image_start + self.game.image_size(self.image_start)
            except RelayError:
                return
        pattern = struct.pack("<Q", self.byte_stream_vtable)
        region = MEMORY_BASIC_INFORMATION()
        address = LOWEST
        while address < HIGHEST:
            if not VirtualQueryEx(self.game.handle, address, ctypes.byref(region), ctypes.sizeof(region)):
                break
            base = region.BaseAddress or 0
            size = region.RegionSize
            if (
                region.State == MEM_COMMIT
                and region.Type == MEM_PRIVATE
                and in_heap(base)
                and region.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE)
                and not region.Protect & PAGE_GUARD
                and size <= MAX_REGION
                and base != self.block
            ):
                self.scan_region(base, base + size, pattern)
            address = base + size

    def scan_region(self, start, end, pattern):
        at = start
        while at < end:
            length = min(end - at, SCAN_CHUNK)
            if self.game.read_into(at, self.chunk_view, length):
                found = []
                # Searching for the vtable's low 5 bytes is about 8 times faster than for
                # all 8: the search skips by its last byte, and the full value ends in zeros,
                # the commonest byte in the heap. The rest is compared at each hit.
                prefix = pattern[:5]
                index = self.chunk.find(prefix, 0, length)
                while index >= 0:
                    if (
                        index & 7 == 0
                        and self.chunk[index : index + 8] == pattern
                        and at + index + BYTE_STREAM_SIZE <= end
                    ):
                        found.append(at + index)
                    index = self.chunk.find(prefix, index + 1, length)
                for byte_stream in found:
                    self.swap(byte_stream)
            at += length

    def swap(self, byte_stream):
        stream = self.game.u64(byte_stream + SSL_STREAM_OFFSET)
        if stream is None or not in_heap(stream):
            return
        vtable = self.game.u64(stream)
        if vtable is None or not self.in_image(vtable):
            return
        if self.original_socket is not None and vtable != self.original_socket:
            return  # a different stream class
        if not self.resolve_ws2():
            return  # swap it on a later scan, once the game has loaded ws2_32
        self.swaps += 1
        port = (self.game.u32(byte_stream + PORT_OFFSET) or 0) & 0xFFFF
        if self.swaps <= 20 or port != BATTLE_NET_PORT:
            length = min(self.game.u64(byte_stream + HOST_LENGTH_OFFSET) or 0, 79)
            pointer = self.game.u64(byte_stream + HOST_OFFSET) or 0
            host = self.game.read(pointer if in_heap(pointer) else byte_stream + HOST_OFFSET, length) or b""
            state = self.game.u32(byte_stream + STATE_OFFSET) or 0
            self.log(
                f"BYTE-STREAM bs={byte_stream:016X} sock={stream:016X} sockVt(rva {vtable - self.image_start:X}) "
                f"state={state} host='{printable(host)}' port={port} swaps={self.swaps}"
            )
        if not self.register(stream):
            return
        if self.original_socket is None:
            self.build_tables(vtable)
        self.game.write_u64(stream, self.block + SOCKET_TABLE)
        self.game.write_u64(byte_stream, self.block + BYTE_STREAM_TABLE)

    def register(self, stream):
        if stream in self.streams:
            return True
        if len(self.streams) >= MAX_RELAYS:
            return False
        entry = self.block + DATA_ENTRIES + 16 * len(self.streams)
        self.game.write_u64(entry + 8, NOT_CONNECTED)
        self.game.write_u64(entry, stream)
        self.streams.append(stream)
        return True

    def build_tables(self, socket_vtable):
        def copy(source):
            data = self.game.read(source, VTABLE_SLOTS * 8)
            if data is None:  # the end of the table runs into an unreadable page
                return [self.game.u64(source + 8 * slot) or 0 for slot in range(VTABLE_SLOTS)]
            return list(struct.unpack(f"<{VTABLE_SLOTS}Q", data))

        socket = copy(socket_vtable)
        socket[RECV_SLOT] = self.block + RECV_ENTRY
        socket[SEND_SLOT] = self.block + SEND_ENTRY
        byte_stream = copy(self.byte_stream_vtable)
        byte_stream[STATE_SLOT] = self.block + STATE_ENTRY
        self.game.write(self.block + SOCKET_TABLE, struct.pack(f"<{VTABLE_SLOTS}Q", *socket))
        self.game.write(self.block + BYTE_STREAM_TABLE, struct.pack(f"<{VTABLE_SLOTS}Q", *byte_stream))
        self.original_socket = socket_vtable
        self.log(
            f"built vts: sockHeap={self.block + SOCKET_TABLE:016X} (from {socket_vtable:X} "
            f"rva {socket_vtable - self.image_start:X}) bsHeap={self.block + BYTE_STREAM_TABLE:016X}"
        )

    def counters(self):
        data = self.game.read(self.block + DATA_COUNTERS, 8 * len(COUNTERS))
        return dict(zip(COUNTERS, struct.unpack(f"<{len(COUNTERS)}Q", data))) if data else {}

    def report_connections(self):
        """Log each stream's connection when the game makes it."""
        if not self.streams:
            return
        data = self.game.read(self.block + DATA_ENTRIES, 16 * len(self.streams))
        if data is None:
            return
        for index, stream in enumerate(self.streams):
            socket = struct.unpack_from("<Q", data, 16 * index + 8)[0]
            connected = socket < NOT_CONNECTED - 1
            if connected and stream not in self.connected:
                self.connected.add(stream)
                self.log(f"*** relay CONNECTED to {self.host}:{self.port} for clientSock={stream:016X} ***")
            elif not connected:
                self.connected.discard(stream)

    def run(self):
        turn = 0
        next_poll = 0.0
        while not self.game.exited():
            started = time.perf_counter()
            try:
                self.scan()
                self.report_connections()
            except RelayError:
                if self.game.exited():
                    break  # a write raced the game's exit
                raise
            if started >= next_poll:
                next_poll = started + POLL_PERIOD
                counts = self.counters()
                self.log(
                    f"[poll] loop={turn} scan={(time.perf_counter() - started) * 1000:.0f}ms swaps={self.swaps} "
                    + " ".join(f"{name}={counts.get(name, 0)}" for name in COUNTERS)
                    + f" relays={len(self.streams)}"
                )
            turn += 1
            time.sleep(SCAN_PERIOD)
        code = wt.DWORD()
        GetExitCodeProcess(self.game.handle, ctypes.byref(code))
        self.log(f"the game exited (code {code.value:#x})")


# --- Command line -------------------------------------------------------------


def parse_arguments(argv):
    parser = argparse.ArgumentParser(
        description="Pipe the Overwatch 1.74 client's Battle.net connection to a local ORPHEUS server, "
        "without injecting a DLL. Attaches to the running game, or starts it.",
        epilog="Arguments after -- go to the game when the script starts it.",
    )
    parser.add_argument(
        "game", nargs="?", default="Overwatch.exe", help="the game's executable (default: ./Overwatch.exe)"
    )
    parser.add_argument("--pid", type=int, help="attach to this process instead of looking for the game by name")
    parser.add_argument("--start", action="store_true", help="always start a new game, even if one runs")
    parser.add_argument("--host", default="127.0.0.1", help="the server's Battle.net RPC address (default 127.0.0.1)")
    parser.add_argument(
        "--port", type=int, default=RELAY_PORT, help=f"the server's Battle.net RPC port (default {RELAY_PORT})"
    )
    parser.add_argument(
        "--bnet",
        default="127.0.0.1:1119",
        help="--BNetServer for a game the script starts (default 127.0.0.1:1119)",
    )
    parser.add_argument("--log", help="also append the log to this file")
    own, extra = argv, []
    if "--" in argv:
        split = argv.index("--")
        own, extra = argv[:split], argv[split + 1 :]
    arguments = parser.parse_args(own)
    arguments.extra = extra
    try:
        ipaddress.IPv4Address(arguments.host)
    except ValueError:
        parser.error(f"--host must be an IPv4 address, not {arguments.host}")
    if not 0 < arguments.port < 65536:
        parser.error("--port must be between 1 and 65535")
    return arguments


def attach_target(arguments):
    """(handle, pid) of the running game to patch, or None to start one."""
    if arguments.pid is not None:
        return open_process(arguments.pid), arguments.pid
    if arguments.start:
        return None
    name = os.path.basename(arguments.game)
    pids = processes_named(name)
    if not pids:
        return None
    wanted = os.path.normcase(os.path.abspath(arguments.game))
    handles = [(open_process(pid), pid) for pid in pids]
    exact = [(handle, pid) for handle, pid in handles if os.path.normcase(image_path(handle) or "") == wanted]
    chosen = exact if exact else handles
    if len(chosen) > 1:
        for handle, _ in handles:
            CloseHandle(handle)
        raise RelayError(
            f"{len(chosen)} copies of {name} are running ({', '.join(map(str, pids))}); pick one with --pid"
        )
    for handle, pid in handles:
        if pid != chosen[0][1]:
            CloseHandle(handle)
    return chosen[0]


def main(argv):
    if sys.platform != "win32" or struct.calcsize("P") != 8:
        print("orpheus_relay needs 64-bit Python on Windows.", file=sys.stderr)
        return 2
    arguments = parse_arguments(argv)
    log = open(arguments.log, "a", encoding="utf-8") if arguments.log else None
    started = None
    try:
        target = attach_target(arguments)
        if target is not None:
            handle, pid = target
            print(f"Attaching to {image_path(handle) or 'the game'} (PID {pid}).", flush=True)
            print("If the game says it was unable to log you in, click LOGIN.", flush=True)
        else:
            game = os.path.abspath(arguments.game)
            if not os.path.isfile(game):
                raise RelayError(f"{game} doesn't exist and the game isn't running; pass the path to Overwatch.exe")
            game_arguments = [f"--BNetServer={arguments.bnet}", *arguments.extra]
            started = start_suspended(game, game_arguments)
            handle, pid = started.hProcess, started.dwProcessId
            print(f"Started {game} {subprocess.list2cmdline(game_arguments)} frozen (PID {pid}).", flush=True)
        relay = Relay(Game(handle, pid), arguments.host, arguments.port, log)
        if started is not None:
            ResumeThread(started.hThread)
            CloseHandle(started.hThread)
            started = None
            print("Unfroze the game.", flush=True)
        print("Keep this window open while you play. Ctrl+C stops the relay.", flush=True)
        relay.run()
        return 0
    except KeyboardInterrupt:
        if started is not None:
            TerminateProcess(started.hProcess, 1)  # still frozen, without the relay
            print("Stopped before the game was unfrozen; closed it.", flush=True)
            return 1
        print(
            "Stopped. Connections the relay already made keep working; a reconnect needs the relay again.",
            flush=True,
        )
        return 0
    except RelayError as error:
        print(f"error: {error}", file=sys.stderr)
        if started is not None:
            TerminateProcess(started.hProcess, 1)  # a frozen game without the relay is no use
        return 1
    finally:
        if log:
            log.close()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
