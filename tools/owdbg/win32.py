"""ctypes declarations for the Win32 debugging API (x64 only)."""
import ctypes
from ctypes import wintypes as wt

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)

PVOID = ctypes.c_void_p
SIZE_T = ctypes.c_size_t
ULONG_PTR = ctypes.c_uint64

# Creation / access
DEBUG_ONLY_THIS_PROCESS = 0x00000002
CREATE_NEW_PROCESS_GROUP = 0x00000200
CREATE_UNICODE_ENVIRONMENT = 0x00000400
CREATE_NO_WINDOW = 0x08000000
PROCESS_ALL_ACCESS = 0x1F0FFF
THREAD_ALL_ACCESS = 0x1F03FF
INFINITE = 0xFFFFFFFF

# Debug events
EXCEPTION_DEBUG_EVENT = 1
CREATE_THREAD_DEBUG_EVENT = 2
CREATE_PROCESS_DEBUG_EVENT = 3
EXIT_THREAD_DEBUG_EVENT = 4
EXIT_PROCESS_DEBUG_EVENT = 5
LOAD_DLL_DEBUG_EVENT = 6
UNLOAD_DLL_DEBUG_EVENT = 7
OUTPUT_DEBUG_STRING_EVENT = 8
RIP_EVENT = 9

DBG_CONTINUE = 0x00010002
DBG_EXCEPTION_NOT_HANDLED = 0x80010001

# Exception codes
STATUS_BREAKPOINT = 0x80000003
STATUS_SINGLE_STEP = 0x80000004
STATUS_WX86_BREAKPOINT = 0x4000001F
STATUS_ACCESS_VIOLATION = 0xC0000005
STATUS_ILLEGAL_INSTRUCTION = 0xC000001D
STATUS_PRIVILEGED_INSTRUCTION = 0xC0000096
MS_VC_EXCEPTION = 0x406D1388  # SetThreadName
DBG_PRINTEXCEPTION_C = 0x40010006
DBG_PRINTEXCEPTION_WIDE_C = 0x4001000A

EXCEPTION_NAMES = {
    STATUS_BREAKPOINT: "BREAKPOINT", STATUS_SINGLE_STEP: "SINGLE_STEP",
    STATUS_ACCESS_VIOLATION: "ACCESS_VIOLATION", STATUS_ILLEGAL_INSTRUCTION: "ILLEGAL_INSTRUCTION",
    STATUS_PRIVILEGED_INSTRUCTION: "PRIVILEGED_INSTRUCTION", MS_VC_EXCEPTION: "SET_THREAD_NAME",
    0xC0000094: "INT_DIVIDE_BY_ZERO", 0xC00000FD: "STACK_OVERFLOW", 0xC0000409: "STACK_BUFFER_OVERRUN",
    0xC0000374: "HEAP_CORRUPTION", 0xE06D7363: "CPP_EXCEPTION", 0x80000001: "GUARD_PAGE",
    0xC0000006: "IN_PAGE_ERROR", 0xC000008C: "ARRAY_BOUNDS", 0xC0000025: "NONCONTINUABLE",
    0xC0000026: "INVALID_DISPOSITION", 0x40010005: "CONTROL_C",
}

# CONTEXT flags (AMD64)
CONTEXT_AMD64 = 0x00100000
CONTEXT_CONTROL = CONTEXT_AMD64 | 0x1
CONTEXT_INTEGER = CONTEXT_AMD64 | 0x2
CONTEXT_SEGMENTS = CONTEXT_AMD64 | 0x4
CONTEXT_FLOATING_POINT = CONTEXT_AMD64 | 0x8
CONTEXT_DEBUG_REGISTERS = CONTEXT_AMD64 | 0x10
CONTEXT_FULL = CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_FLOATING_POINT
CONTEXT_ALL = CONTEXT_FULL | CONTEXT_SEGMENTS | CONTEXT_DEBUG_REGISTERS

EFLAGS_TF = 0x100
EFLAGS_RF = 0x10000

# Memory
MEM_COMMIT = 0x1000
MEM_RESERVE = 0x2000
MEM_RELEASE = 0x8000
PAGE_READWRITE = 0x04
PAGE_EXECUTE_READWRITE = 0x40
PAGE_GUARD = 0x100
PAGE_NOACCESS = 0x01
EXECUTABLE_PROTECT = 0x10 | 0x20 | 0x40 | 0x80

# Job objects
JobObjectExtendedLimitInformation = 9
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x2000


class EXCEPTION_RECORD(ctypes.Structure):
    _fields_ = [("ExceptionCode", wt.DWORD), ("ExceptionFlags", wt.DWORD),
                ("ExceptionRecord", PVOID), ("ExceptionAddress", PVOID),
                ("NumberParameters", wt.DWORD), ("ExceptionInformation", ULONG_PTR * 15)]


class EXCEPTION_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("ExceptionRecord", EXCEPTION_RECORD), ("dwFirstChance", wt.DWORD)]


class CREATE_THREAD_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("hThread", wt.HANDLE), ("lpThreadLocalBase", PVOID), ("lpStartAddress", PVOID)]


class CREATE_PROCESS_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("hFile", wt.HANDLE), ("hProcess", wt.HANDLE), ("hThread", wt.HANDLE),
                ("lpBaseOfImage", PVOID), ("dwDebugInfoFileOffset", wt.DWORD),
                ("nDebugInfoSize", wt.DWORD), ("lpThreadLocalBase", PVOID),
                ("lpStartAddress", PVOID), ("lpImageName", PVOID), ("fUnicode", wt.WORD)]


class EXIT_THREAD_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("dwExitCode", wt.DWORD)]


class EXIT_PROCESS_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("dwExitCode", wt.DWORD)]


class LOAD_DLL_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("hFile", wt.HANDLE), ("lpBaseOfDll", PVOID), ("dwDebugInfoFileOffset", wt.DWORD),
                ("nDebugInfoSize", wt.DWORD), ("lpImageName", PVOID), ("fUnicode", wt.WORD)]


class UNLOAD_DLL_DEBUG_INFO(ctypes.Structure):
    _fields_ = [("lpBaseOfDll", PVOID)]


class OUTPUT_DEBUG_STRING_INFO(ctypes.Structure):
    _fields_ = [("lpDebugStringData", PVOID), ("fUnicode", wt.WORD), ("nDebugStringLength", wt.WORD)]


class RIP_INFO(ctypes.Structure):
    _fields_ = [("dwError", wt.DWORD), ("dwType", wt.DWORD)]


class _DEBUG_EVENT_UNION(ctypes.Union):
    _fields_ = [("Exception", EXCEPTION_DEBUG_INFO), ("CreateThread", CREATE_THREAD_DEBUG_INFO),
                ("CreateProcessInfo", CREATE_PROCESS_DEBUG_INFO), ("ExitThread", EXIT_THREAD_DEBUG_INFO),
                ("ExitProcess", EXIT_PROCESS_DEBUG_INFO), ("LoadDll", LOAD_DLL_DEBUG_INFO),
                ("UnloadDll", UNLOAD_DLL_DEBUG_INFO), ("DebugString", OUTPUT_DEBUG_STRING_INFO),
                ("RipInfo", RIP_INFO)]


class DEBUG_EVENT(ctypes.Structure):
    _fields_ = [("dwDebugEventCode", wt.DWORD), ("dwProcessId", wt.DWORD), ("dwThreadId", wt.DWORD),
                ("u", _DEBUG_EVENT_UNION)]


class CONTEXT(ctypes.Structure):
    """AMD64 CONTEXT (1232 bytes). Must live at a 16-byte aligned address: use new_context()."""
    _fields_ = [("P1Home", ctypes.c_uint64), ("P2Home", ctypes.c_uint64), ("P3Home", ctypes.c_uint64),
                ("P4Home", ctypes.c_uint64), ("P5Home", ctypes.c_uint64), ("P6Home", ctypes.c_uint64),
                ("ContextFlags", wt.DWORD), ("MxCsr", wt.DWORD),
                ("SegCs", wt.WORD), ("SegDs", wt.WORD), ("SegEs", wt.WORD), ("SegFs", wt.WORD),
                ("SegGs", wt.WORD), ("SegSs", wt.WORD), ("EFlags", wt.DWORD),
                ("Dr0", ctypes.c_uint64), ("Dr1", ctypes.c_uint64), ("Dr2", ctypes.c_uint64),
                ("Dr3", ctypes.c_uint64), ("Dr6", ctypes.c_uint64), ("Dr7", ctypes.c_uint64),
                ("Rax", ctypes.c_uint64), ("Rcx", ctypes.c_uint64), ("Rdx", ctypes.c_uint64),
                ("Rbx", ctypes.c_uint64), ("Rsp", ctypes.c_uint64), ("Rbp", ctypes.c_uint64),
                ("Rsi", ctypes.c_uint64), ("Rdi", ctypes.c_uint64), ("R8", ctypes.c_uint64),
                ("R9", ctypes.c_uint64), ("R10", ctypes.c_uint64), ("R11", ctypes.c_uint64),
                ("R12", ctypes.c_uint64), ("R13", ctypes.c_uint64), ("R14", ctypes.c_uint64),
                ("R15", ctypes.c_uint64), ("Rip", ctypes.c_uint64),
                ("FltSave", ctypes.c_ubyte * 512), ("VectorRegister", ctypes.c_ubyte * 416),
                ("VectorControl", ctypes.c_uint64), ("DebugControl", ctypes.c_uint64),
                ("LastBranchToRip", ctypes.c_uint64), ("LastBranchFromRip", ctypes.c_uint64),
                ("LastExceptionToRip", ctypes.c_uint64), ("LastExceptionFromRip", ctypes.c_uint64)]


assert ctypes.sizeof(CONTEXT) == 1232 and ctypes.sizeof(DEBUG_EVENT) == 176

GPR_NAMES = ("rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
             "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15")
CONTEXT_FIELD = {n: n.capitalize() for n in GPR_NAMES}
CONTEXT_FIELD.update(rip="Rip", eflags="EFlags", dr0="Dr0", dr1="Dr1", dr2="Dr2", dr3="Dr3",
                     dr6="Dr6", dr7="Dr7")


def new_context(flags):
    """A zeroed CONTEXT at a 16-byte aligned address (the buffer is kept alive by the struct)."""
    buf = ctypes.create_string_buffer(ctypes.sizeof(CONTEXT) + 16)
    off = (-ctypes.addressof(buf)) & 15
    ctx = CONTEXT.from_buffer(buf, off)
    ctx.ContextFlags = flags
    return ctx


class STARTUPINFOW(ctypes.Structure):
    _fields_ = [("cb", wt.DWORD), ("lpReserved", wt.LPWSTR), ("lpDesktop", wt.LPWSTR),
                ("lpTitle", wt.LPWSTR), ("dwX", wt.DWORD), ("dwY", wt.DWORD), ("dwXSize", wt.DWORD),
                ("dwYSize", wt.DWORD), ("dwXCountChars", wt.DWORD), ("dwYCountChars", wt.DWORD),
                ("dwFillAttribute", wt.DWORD), ("dwFlags", wt.DWORD), ("wShowWindow", wt.WORD),
                ("cbReserved2", wt.WORD), ("lpReserved2", PVOID), ("hStdInput", wt.HANDLE),
                ("hStdOutput", wt.HANDLE), ("hStdError", wt.HANDLE)]


class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [("hProcess", wt.HANDLE), ("hThread", wt.HANDLE), ("dwProcessId", wt.DWORD),
                ("dwThreadId", wt.DWORD)]


class MEMORY_BASIC_INFORMATION(ctypes.Structure):
    _fields_ = [("BaseAddress", ctypes.c_uint64), ("AllocationBase", ctypes.c_uint64),
                ("AllocationProtect", wt.DWORD), ("PartitionId", wt.WORD), ("RegionSize", ctypes.c_uint64),
                ("State", wt.DWORD), ("Protect", wt.DWORD), ("Type", wt.DWORD)]


class IO_COUNTERS(ctypes.Structure):
    _fields_ = [(n, ctypes.c_uint64) for n in ("ReadOperationCount", "WriteOperationCount",
                                                "OtherOperationCount", "ReadTransferCount",
                                                "WriteTransferCount", "OtherTransferCount")]


class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64),
                ("LimitFlags", wt.DWORD), ("MinimumWorkingSetSize", SIZE_T),
                ("MaximumWorkingSetSize", SIZE_T), ("ActiveProcessLimit", wt.DWORD),
                ("Affinity", ULONG_PTR), ("PriorityClass", wt.DWORD), ("SchedulingClass", wt.DWORD)]


class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION), ("IoInfo", IO_COUNTERS),
                ("ProcessMemoryLimit", SIZE_T), ("JobMemoryLimit", SIZE_T),
                ("PeakProcessMemoryUsed", SIZE_T), ("PeakJobMemoryUsed", SIZE_T)]


def _decl(dll, name, restype, *argtypes):
    f = getattr(dll, name)
    f.restype = restype
    f.argtypes = list(argtypes)
    return f


CreateProcessW = _decl(k32, "CreateProcessW", wt.BOOL, wt.LPCWSTR, wt.LPWSTR, PVOID, PVOID, wt.BOOL,
                       wt.DWORD, PVOID, wt.LPCWSTR, ctypes.POINTER(STARTUPINFOW),
                       ctypes.POINTER(PROCESS_INFORMATION))
WaitForDebugEvent = _decl(k32, "WaitForDebugEvent", wt.BOOL, ctypes.POINTER(DEBUG_EVENT), wt.DWORD)
ContinueDebugEvent = _decl(k32, "ContinueDebugEvent", wt.BOOL, wt.DWORD, wt.DWORD, wt.DWORD)
DebugSetProcessKillOnExit = _decl(k32, "DebugSetProcessKillOnExit", wt.BOOL, wt.BOOL)
GetThreadContext = _decl(k32, "GetThreadContext", wt.BOOL, wt.HANDLE, PVOID)
SetThreadContext = _decl(k32, "SetThreadContext", wt.BOOL, wt.HANDLE, PVOID)
SuspendThread = _decl(k32, "SuspendThread", wt.DWORD, wt.HANDLE)
ResumeThread = _decl(k32, "ResumeThread", wt.DWORD, wt.HANDLE)
ReadProcessMemory = _decl(k32, "ReadProcessMemory", wt.BOOL, wt.HANDLE, PVOID, PVOID, SIZE_T,
                          ctypes.POINTER(SIZE_T))
WriteProcessMemory = _decl(k32, "WriteProcessMemory", wt.BOOL, wt.HANDLE, PVOID, PVOID, SIZE_T,
                           ctypes.POINTER(SIZE_T))
FlushInstructionCache = _decl(k32, "FlushInstructionCache", wt.BOOL, wt.HANDLE, PVOID, SIZE_T)
VirtualQueryEx = _decl(k32, "VirtualQueryEx", SIZE_T, wt.HANDLE, PVOID,
                       ctypes.POINTER(MEMORY_BASIC_INFORMATION), SIZE_T)
VirtualProtectEx = _decl(k32, "VirtualProtectEx", wt.BOOL, wt.HANDLE, PVOID, SIZE_T, wt.DWORD,
                         ctypes.POINTER(wt.DWORD))
VirtualAllocEx = _decl(k32, "VirtualAllocEx", PVOID, wt.HANDLE, PVOID, SIZE_T, wt.DWORD, wt.DWORD)
CreateRemoteThread = _decl(k32, "CreateRemoteThread", wt.HANDLE, wt.HANDLE, PVOID, SIZE_T, PVOID, PVOID,
                           wt.DWORD, ctypes.POINTER(wt.DWORD))
GetModuleHandleW = _decl(k32, "GetModuleHandleW", wt.HMODULE, wt.LPCWSTR)
GetProcAddress = _decl(k32, "GetProcAddress", PVOID, wt.HMODULE, ctypes.c_char_p)
TerminateProcess = _decl(k32, "TerminateProcess", wt.BOOL, wt.HANDLE, wt.UINT)
GetExitCodeProcess = _decl(k32, "GetExitCodeProcess", wt.BOOL, wt.HANDLE, ctypes.POINTER(wt.DWORD))
CloseHandle = _decl(k32, "CloseHandle", wt.BOOL, wt.HANDLE)
GetFinalPathNameByHandleW = _decl(k32, "GetFinalPathNameByHandleW", wt.DWORD, wt.HANDLE, wt.LPWSTR,
                                  wt.DWORD, wt.DWORD)
WaitForSingleObject = _decl(k32, "WaitForSingleObject", wt.DWORD, wt.HANDLE, wt.DWORD)
CreateJobObjectW = _decl(k32, "CreateJobObjectW", wt.HANDLE, PVOID, wt.LPCWSTR)
SetInformationJobObject = _decl(k32, "SetInformationJobObject", wt.BOOL, wt.HANDLE, ctypes.c_int, PVOID,
                                wt.DWORD)
AssignProcessToJobObject = _decl(k32, "AssignProcessToJobObject", wt.BOOL, wt.HANDLE, wt.HANDLE)
OpenProcess = _decl(k32, "OpenProcess", wt.HANDLE, wt.DWORD, wt.BOOL, wt.DWORD)

WNDENUMPROC = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
EnumWindows = _decl(user32, "EnumWindows", wt.BOOL, WNDENUMPROC, wt.LPARAM)
GetWindowThreadProcessId = _decl(user32, "GetWindowThreadProcessId", wt.DWORD, wt.HWND,
                                 ctypes.POINTER(wt.DWORD))
IsWindowVisible = _decl(user32, "IsWindowVisible", wt.BOOL, wt.HWND)
GetWindowTextW = _decl(user32, "GetWindowTextW", ctypes.c_int, wt.HWND, wt.LPWSTR, ctypes.c_int)
GetClassNameW = _decl(user32, "GetClassNameW", ctypes.c_int, wt.HWND, wt.LPWSTR, ctypes.c_int)
PostMessageW = _decl(user32, "PostMessageW", wt.BOOL, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM)
GetClientRect = _decl(user32, "GetClientRect", wt.BOOL, wt.HWND, ctypes.POINTER(wt.RECT))
GetWindowRect = _decl(user32, "GetWindowRect", wt.BOOL, wt.HWND, ctypes.POINTER(wt.RECT))
PrintWindow = _decl(user32, "PrintWindow", wt.BOOL, wt.HWND, wt.HDC, wt.UINT)
GetDC = _decl(user32, "GetDC", wt.HDC, wt.HWND)
ReleaseDC = _decl(user32, "ReleaseDC", ctypes.c_int, wt.HWND, wt.HDC)
MapVirtualKeyW = _decl(user32, "MapVirtualKeyW", wt.UINT, wt.UINT, wt.UINT)
CreateCompatibleDC = _decl(gdi32, "CreateCompatibleDC", wt.HDC, wt.HDC)
CreateCompatibleBitmap = _decl(gdi32, "CreateCompatibleBitmap", wt.HBITMAP, wt.HDC, ctypes.c_int, ctypes.c_int)
SelectObject = _decl(gdi32, "SelectObject", wt.HGDIOBJ, wt.HDC, wt.HGDIOBJ)
GetDIBits = _decl(gdi32, "GetDIBits", ctypes.c_int, wt.HDC, wt.HBITMAP, wt.UINT, wt.UINT, PVOID, PVOID, wt.UINT)
DeleteObject = _decl(gdi32, "DeleteObject", wt.BOOL, wt.HGDIOBJ)
DeleteDC = _decl(gdi32, "DeleteDC", wt.BOOL, wt.HDC)


def last_error():
    return ctypes.get_last_error()


def handle_path(handle):
    """Path of an open file handle (debug events hand out image file handles)."""
    buf = ctypes.create_unicode_buffer(1024)
    n = GetFinalPathNameByHandleW(handle, buf, 1024, 0)
    if not n or n >= 1024:
        return None
    path = buf.value
    return path[4:] if path.startswith("\\\\?\\") else path


def exception_name(code):
    return EXCEPTION_NAMES.get(code, f"{code:08X}")


def kill_on_close_job(process):
    """A job object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE holding `process`: the process dies
    when the last handle to the job closes (also when Python is killed). Returns the job handle
    (keep it open) or None."""
    job = CreateJobObjectW(None, None)
    if not job:
        return None
    info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    SetInformationJobObject(job, JobObjectExtendedLimitInformation, ctypes.byref(info), ctypes.sizeof(info))
    if not AssignProcessToJobObject(job, process):
        CloseHandle(job)
        return None
    return job


