//! Memory of this process: reads and writes that fail instead of faulting,
//! regions, modules, code patches, executable memory near the image, scans.

use std::{ffi::c_void, mem::size_of, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::{FlushInstructionCache, ReadProcessMemory, WriteProcessMemory},
            ToolHelp::{
                CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW,
                TH32CS_SNAPMODULE,
            },
        },
        Memory::{
            MEM_COMMIT, MEM_FREE, MEM_IMAGE, MEM_MAPPED, MEM_PRIVATE, MEM_RESERVE,
            MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS,
            PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualAlloc, VirtualProtect, VirtualQuery,
        },
        Threading::{GetCurrentProcess, GetCurrentProcessId},
    },
};

pub fn read(address: u64, buffer: &mut [u8]) -> bool {
    if buffer.is_empty() {
        return true;
    }
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

/// As much of `length` bytes from `address` as is readable, page by page.
pub fn read_partial(address: u64, length: usize) -> Vec<u8> {
    let mut out = vec![0u8; length];
    if read(address, &mut out) {
        return out;
    }
    let mut done = 0;
    while done < length {
        let at = address + done as u64;
        let page_end = (at | 0xFFF) + 1;
        let chunk = ((page_end - at) as usize).min(length - done);
        if !read(at, &mut out[done..done + chunk]) {
            out.truncate(done);
            break;
        }
        done += chunk;
    }
    out
}

pub fn read_u64(address: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    read(address, &mut bytes).then(|| u64::from_le_bytes(bytes))
}

pub fn read_u32(address: u64) -> Option<u32> {
    let mut bytes = [0u8; 4];
    read(address, &mut bytes).then(|| u32::from_le_bytes(bytes))
}

pub fn read_i16_pair(address: u64) -> Option<[i16; 2]> {
    let mut bytes = [0u8; 4];
    read(address, &mut bytes).then(|| {
        [
            i16::from_le_bytes([bytes[0], bytes[1]]),
            i16::from_le_bytes([bytes[2], bytes[3]]),
        ]
    })
}

pub fn read_f32x3(address: u64) -> Option<[f32; 3]> {
    let mut bytes = [0u8; 12];
    read(address, &mut bytes).then(|| {
        [0, 4, 8].map(|at| f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes")))
    })
}

/// Writes data memory; code goes through [`patch_code`].
pub fn write(address: u64, data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    let mut copied = 0;
    // SAFETY: writes this process's memory; the call fails instead of
    // faulting when the target is not writable.
    let ok = unsafe {
        WriteProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            data.as_ptr().cast(),
            data.len(),
            &mut copied,
        )
    };
    ok != 0 && copied == data.len()
}

pub fn query(address: u64) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: queries this process's address space into a valid struct.
    let filled = unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    (filled != 0).then_some(info)
}

/// One region of the address space.
pub struct Region {
    pub base: u64,
    pub size: u64,
    pub allocation: u64,
    pub state: u32,
    pub protect: u32,
    pub kind: u32,
}

impl Region {
    pub fn committed(&self) -> bool {
        self.state == MEM_COMMIT
    }

    /// Committed and readable without a guard page.
    pub fn readable(&self) -> bool {
        self.committed() && self.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && self.protect != 0
    }

    pub fn state_name(&self) -> &'static str {
        match self.state {
            MEM_COMMIT => "commit",
            MEM_RESERVE => "reserve",
            MEM_FREE => "free",
            _ => "?",
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            MEM_IMAGE => "image",
            MEM_MAPPED => "mapped",
            MEM_PRIVATE => "private",
            _ => "",
        }
    }
}

/// The regions overlapping `[start, end)`.
pub fn regions(start: u64, end: u64) -> Vec<Region> {
    let mut out = Vec::new();
    let mut address = start;
    while address < end {
        let Some(info) = query(address) else {
            break;
        };
        let base = info.BaseAddress as u64;
        let size = info.RegionSize as u64;
        if size == 0 {
            break;
        }
        out.push(Region {
            base,
            size,
            allocation: info.AllocationBase as u64,
            state: info.State,
            protect: info.Protect,
            kind: info.Type,
        });
        address = base + size;
    }
    out
}

/// The private read-write regions: where the heap's objects live.
pub fn heap_regions() -> Vec<Region> {
    regions(0x10000, 0x7FFF_FFFF_0000)
        .into_iter()
        .filter(|region| {
            region.committed()
                && region.kind == MEM_PRIVATE
                && region.protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0
                && region.protect & PAGE_GUARD == 0
        })
        .collect()
}

/// The 8-byte aligned addresses in `regions` holding `value`, at most `max`.
pub fn scan_u64(regions: &[Region], value: u64, max: usize) -> Vec<u64> {
    scan_values(regions, &[value], max)
        .into_iter()
        .map(|(address, _)| address)
        .collect()
}

/// The 8-byte aligned addresses in `regions` holding any of `values` (a
/// few), with the value found, at most `max`.
pub fn scan_values(regions: &[Region], values: &[u64], max: usize) -> Vec<(u64, u64)> {
    let mut found = Vec::new();
    let mut chunk = vec![0u8; 1 << 20];
    for region in regions {
        let mut at = region.base;
        let end = region.base + region.size;
        while at < end {
            let length = ((end - at) as usize).min(chunk.len());
            if read(at, &mut chunk[..length]) {
                for (index, word) in chunk[..length].chunks_exact(8).enumerate() {
                    let word = u64::from_le_bytes(word.try_into().expect("8 bytes"));
                    if values.contains(&word) {
                        found.push((at + index as u64 * 8, word));
                        if found.len() >= max {
                            return found;
                        }
                    }
                }
            }
            at += length as u64;
        }
    }
    found
}

/// The addresses in `[start, end)` where `pattern` matches (`None` bytes
/// match anything), at most `max`.
pub fn scan_pattern(start: u64, end: u64, pattern: &[Option<u8>], max: usize) -> Vec<u64> {
    let mut found = Vec::new();
    let Some(first) = pattern.first() else {
        return found;
    };
    let overlap = pattern.len() - 1;
    let mut chunk = vec![0u8; (1 << 20) + overlap];
    for region in regions(start, end) {
        if !region.readable() {
            continue;
        }
        let mut at = region.base.max(start);
        let stop = (region.base + region.size).min(end);
        while at < stop {
            let length = ((stop - at) as usize + overlap).min(chunk.len());
            let data = &mut chunk[..length];
            let got = if read(at, data) {
                length
            } else {
                let partial = read_partial(at, length);
                data[..partial.len()].copy_from_slice(&partial);
                partial.len()
            };
            let data = &chunk[..got];
            for index in 0..got.saturating_sub(overlap) {
                if first.is_some_and(|byte| data[index] != byte) {
                    continue;
                }
                if pattern
                    .iter()
                    .zip(&data[index..])
                    .all(|(want, &have)| want.is_none_or(|want| want == have))
                {
                    found.push(at + index as u64);
                    if found.len() >= max {
                        return found;
                    }
                }
            }
            at += (length - overlap).max(1) as u64;
        }
    }
    found
}

/// The modules of this process: (path, base, size).
pub fn modules() -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    // SAFETY: plain snapshot request; the handle is closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, GetCurrentProcessId()) };
    if snapshot == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: an all-zero MODULEENTRY32W is valid; dwSize is set before use.
    let mut entry: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<MODULEENTRY32W>() as u32;
    // SAFETY: `entry` is a properly sized MODULEENTRY32W.
    let mut more = unsafe { Module32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let end = entry
            .szExePath
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExePath.len());
        out.push((
            String::from_utf16_lossy(&entry.szExePath[..end]),
            entry.modBaseAddr as u64,
            u64::from(entry.modBaseSize),
        ));
        // SAFETY: as above.
        more = unsafe { Module32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot handle is valid and owned here.
    unsafe { CloseHandle(snapshot) };
    out
}

/// Write code bytes: the page is made writable for the write and set back.
/// Fails on a `SEC_NO_CHANGE` image (the shipped `Overwatch.exe`).
pub fn patch_code(address: u64, data: &[u8]) -> std::io::Result<()> {
    let mut old: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: changes the protection of committed pages of this process; the
    // call fails instead of faulting.
    if unsafe {
        VirtualProtect(
            address as *const c_void,
            data.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the pages were just made writable and `data` is a valid slice.
    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len()) };
    let mut ignored = 0;
    // SAFETY: as above, restoring the previous protection.
    unsafe {
        VirtualProtect(address as *const c_void, data.len(), old, &mut ignored);
        FlushInstructionCache(GetCurrentProcess(), address as *const c_void, data.len());
    }
    Ok(())
}

/// Read-write-execute memory within 2 GB of `near`, so code there reaches
/// the image with 32-bit displacements.
pub fn alloc_near(near: u64, size: usize) -> Option<u64> {
    const GRANULE: u64 = 0x1_0000;
    const REACH: u64 = 0x7000_0000;
    let size = (size as u64).next_multiple_of(GRANULE);
    let low = near.saturating_sub(REACH).max(0x1_0000);
    let mut candidate = (near & !(GRANULE - 1)).saturating_sub(GRANULE);
    while candidate > low {
        let info = query(candidate)?;
        if info.State == MEM_FREE {
            let free_end = info.BaseAddress as u64 + info.RegionSize as u64;
            let start = candidate.min(free_end.saturating_sub(size)) & !(GRANULE - 1);
            if start >= info.BaseAddress as u64 && start + size <= free_end {
                // SAFETY: reserves and commits fresh memory at a free address.
                let got = unsafe {
                    VirtualAlloc(
                        start as *const c_void,
                        size as usize,
                        MEM_COMMIT | MEM_RESERVE,
                        PAGE_EXECUTE_READWRITE,
                    )
                };
                if !got.is_null() {
                    return Some(got as u64);
                }
            }
            candidate = (info.BaseAddress as u64 & !(GRANULE - 1)).saturating_sub(GRANULE);
        } else {
            candidate = (info.AllocationBase as u64 & !(GRANULE - 1)).saturating_sub(GRANULE);
        }
    }
    None
}

/// A bump allocator over executable memory near the image. Nothing is
/// freed: a removed hook's stub may still be running on another thread.
pub struct Arena {
    near: u64,
    next: u64,
    end: u64,
}

impl Arena {
    pub const fn new(near: u64) -> Self {
        Self {
            near,
            next: 0,
            end: 0,
        }
    }

    pub fn alloc(&mut self, size: usize) -> Result<u64, String> {
        let size = (size as u64).next_multiple_of(16);
        if self.next + size > self.end {
            let block = 1 << 20;
            let start = alloc_near(self.near, block)
                .ok_or("no free memory within 2 GB of the image for code")?;
            self.next = start;
            self.end = start + block as u64;
        }
        let at = self.next;
        self.next += size;
        Ok(at)
    }
}

/// Fill `address` with `code`, which the arena allocated (read-write-execute).
pub fn put_code(address: u64, code: &[u8]) {
    // SAFETY: the arena's memory is committed read-write-execute and the
    // slot was allocated for at least `code.len()` bytes.
    unsafe {
        ptr::copy_nonoverlapping(code.as_ptr(), address as *mut u8, code.len());
        FlushInstructionCache(GetCurrentProcess(), address as *const c_void, code.len());
    }
}
