//! `owprobe.dll`: an in-process probe for the Overwatch 1.74 client.
//!
//! `orpheus play --probe` (or owdbg's `--probe`) loads it beside the relay.
//! It listens on `127.0.0.1:ORPHEUS_PROBE_PORT` for the Python client
//! `tools/owprobe` and answers framed JSON requests: memory reads and writes,
//! regions, modules, a per-frame sampler hooked into the client's input
//! sampler, function hooks that log registers and reads into a ring buffer,
//! scripted input written into the client's own input samples, and calls of
//! client functions. Usage: `docs/dev/client-tools.md`, research:
//! `D:/Work/Coding/refs/reports/ClientProbe.md`.
//!
//! Code hooks need a writable image: the decrypted debug build
//! (`Overwatch.debug.exe`). The shipped `Overwatch.exe` maps its code
//! `SEC_NO_CHANGE` after its protector's remap, so only the memory commands
//! work there.
//!
//! This is one of the three crates allowed to use `unsafe` (with the launcher
//! and the relay). Every `unsafe` block states why it is sound.

#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

pub mod expr;
pub mod input;
pub mod layout;
pub mod ring;

#[cfg(windows)]
mod dll;

use std::io::{self, Read, Write};

/// The launcher passes the port in this variable.
pub const PORT_VARIABLE: &str = "ORPHEUS_PROBE_PORT";
/// The probe's log file; without it the log goes next to the DLL.
pub const LOG_VARIABLE: &str = "ORPHEUS_PROBE_LOG";
/// The port without the variable.
pub const DEFAULT_PORT: u16 = 21150;

/// The port from the launcher's variable, or the default.
pub fn probe_port(value: Option<&str>) -> u16 {
    value
        .and_then(|text| text.trim().parse().ok())
        .filter(|&port| port != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// The base the research's addresses use (the IDA list, owindex, owdbg).
pub const ORPHEUS_BASE: u64 = 0x7FF7_88F1_0000;

/// Client addresses as ORPHEUS VAs (D:/Work/Coding/refs/reports/ClientInput.md).
pub mod client {
    /// In the input sampler `0x7FF78961B350`: `rdi` = the local player's
    /// component 59, the sample at `+0x103C` complete and not yet merged.
    pub const SAMPLE_SITE: u64 = 0x7FF7_8961_C7FC;
    /// `set_view(component59, float quaternion[4])`.
    pub const SET_VIEW: u64 = 0x7FF7_894E_00A0;
    /// A component's owner: `[comp+8] ^ rol64([POINTER_KEY], 2) ^ POINTER_SALT`.
    pub const POINTER_KEY: u64 = 0x7FF7_8BDF_47FC;
    pub const POINTER_SALT: u64 = 0xF320_E424_72A9_4F14;
    /// Component 4's vtable; its `+0x160` is the position.
    pub const COMP4_VTABLE: u64 = 0x7FF7_8B51_C1E8;
    pub const COMP4_POSITION: u64 = 0x160;
    /// The entity's id.
    pub const ENTITY_ID: u64 = 0xB0;

    /// Component 31 (the mover), 44 (`MECPossessorData`) and 47 (the
    /// statescript component) by their vtables.
    pub const COMP31_VTABLE: u64 = 0x7FF7_8B51_E7C8;
    pub const COMP44_VTABLE: u64 = 0x7FF7_8B52_03D8;
    pub const COMP47_VTABLE: u64 = 0x7FF7_8B52_1F68;
    /// Component 44's mirror field 0 (`m_1AB5E773`): the entity the player
    /// possesses.
    pub const POSSESSED: u64 = 0xD0;

    /// The world: `[entity+0x28] ^ WORLD_KEY`; its systems:
    /// `[world+0x140] ^ (SYSTEMS_KEY - [SYSTEMS_KEY_GLOBAL])`
    /// (`0x7FF789CB7F10`).
    pub const ENTITY_WORLD: u64 = 0x28;
    pub const WORLD_KEY: u64 = 0x98DE_97B5_0B3B_E1E3;
    pub const WORLD_SYSTEMS: u64 = 0x140;
    pub const SYSTEMS_KEY: u64 = 0x4E34_94C6_3319_4824;
    pub const SYSTEMS_KEY_GLOBAL: u64 = 0x7FF7_8BDF_4BD0;
    /// The camera: `CAMERA_GET(systems[4])` returns the camera object; its
    /// world matrix is the 64 bytes at `+0x10`, encrypted with the key
    /// `CAMERA_KEY([camera+0x50])` (`0x7FF78970A120`).
    pub const CAMERA_SYSTEM: u64 = 4;
    pub const CAMERA_GET: u64 = 0x7FF7_894F_2CD0;
    pub const CAMERA_KEY: u64 = 0x7FF7_894C_05F0;
    pub const CAMERA_MATRIX: u64 = 0x10;
    pub const CAMERA_SEED: u64 = 0x50;
    /// `CAMERA_KEY` faults unless its return address is inside
    /// `[DECRYPT_CALLERS, +[DECRYPT_CALLERS+8] as u32)` after a CALL.
    pub const DECRYPT_CALLERS: u64 = 0x7FF7_8B4F_1600;

    /// Component 47's statescript host and its instances
    /// (`tools/owprobe/statescript.py`).
    pub const HOST_IN_COMP47: u64 = 0xD0;
    pub const HOST_INSTANCES: u64 = 0x8;
    pub const HOST_COUNT: u64 = 0x10;
    /// The host's entity variables (`#Ne`): a variable store of the same
    /// layout at `[host+0x1F0]` (`0x7FF78AAB3A00` reads `[[instance
    /// store+8]+0x1F0]`; the store's `+8` is the host, `0x7FF78AAB2620`).
    pub const HOST_ENTITY_VARIABLES: u64 = 0x1F0;
    /// An instance: its id, variable store, state array (indexed by the
    /// graph's state index, made on first use, `0x7FF78AAA1460`) and graph.
    pub const INSTANCE_ID: u64 = 0x18;
    pub const INSTANCE_VARIABLES: u64 = 0x98;
    pub const INSTANCE_STATES: u64 = 0xA0;
    pub const INSTANCE_STATE_COUNT: u64 = 0xA8;
    pub const INSTANCE_GRAPH: u64 = 0x1E8;
    /// A state's flags: bit 0 while it runs.
    pub const STATE_FLAGS: u64 = 0x40;
    /// The variable store: 16 buckets `{entries, u32 count}` of 32 bytes
    /// from `+0x20` by `index & 0xF`, 16-byte entries `{u16 index, variable}`;
    /// a variable's own value at `+0x50` (Grapple.md "DayFirstGrapple2").
    /// A variable with bindings (their count at `+0x48`, a StateStack's or a
    /// ChaseVar's) has its value at `+0x60`, worked out from the bindings
    /// when it is read (`0x7FF78AAB70B0`).
    pub const VARIABLE_BUCKETS: u64 = 0x20;
    pub const VARIABLE_VALUE: u64 = 0x50;
    pub const VARIABLE_BINDINGS: u64 = 0x48;
    pub const VARIABLE_BOUND_VALUE: u64 = 0x60;

    /// Component 59: the sample being built and the stored view.
    pub const SAMPLE_YAW: u64 = 0x1042;
    pub const SAMPLE_RIGHT: u64 = 0x104B;
    pub const SAMPLE_FORWARD: u64 = 0x104C;
    pub const SAMPLE_BUTTONS: u64 = 0x1050;
    pub const SAMPLE_ACTION: u64 = 0x1054;
    pub const VIEW_YAW: u64 = 0x107A;
    /// The command ring: 32 records of 36 bytes `{u32 frame; 28 bytes}`.
    pub const COMMAND_RING: u64 = 0xB60;
    pub const COMMAND_RECORD: u64 = 36;
    pub const COMMAND_SLOTS: u64 = 32;
}

/// Requests and answers are UTF-8 JSON, each behind its length (u32, little
/// endian).
pub const MAX_FRAME: usize = 64 << 20;

/// The next frame, or `None` at the end of the stream.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {length} bytes"),
        ));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let length = u32::try_from(body.len())
        .ok()
        .filter(|&length| length as usize <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

/// An ORPHEUS VA in the client image, at the image's base in this run.
pub fn runtime(base: u64, orpheus_va: u64) -> u64 {
    orpheus_va.wrapping_sub(ORPHEUS_BASE).wrapping_add(base)
}

/// `SizeOfImage` and the RVA range of the code section (`.text`, else the
/// first executable section) from the PE headers at the start of a file.
pub fn pe_layout(headers: &[u8]) -> Option<(u64, (u64, u64))> {
    let u16_at = |offset: usize| {
        headers
            .get(offset..offset + 2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    };
    let u32_at = |offset: usize| {
        headers
            .get(offset..offset + 4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    };
    if headers.get(..2)? != b"MZ" {
        return None;
    }
    let nt = u32_at(0x3C)? as usize;
    if headers.get(nt..nt + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(nt + 6)? as usize;
    let optional = nt + 24;
    let size = u64::from(u32_at(optional + 56)?);
    let table = optional + u16_at(nt + 20)? as usize;
    let mut code = None;
    for index in 0..sections {
        let at = table + index * 40;
        let name = headers.get(at..at + 8)?;
        let range = (
            u64::from(u32_at(at + 12)?),
            u64::from(u32_at(at + 12)?) + u64::from(u32_at(at + 8)?),
        );
        let executable = u32_at(at + 36)? & 0x2000_0000 != 0;
        if name.starts_with(b".text\0") {
            return Some((size, range));
        }
        if executable && code.is_none() {
            code = Some(range);
        }
    }
    Some((size, code?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let mut stream = Vec::new();
        write_frame(&mut stream, br#"{"cmd":"info"}"#).unwrap();
        write_frame(&mut stream, b"").unwrap();
        let mut reader = stream.as_slice();
        assert_eq!(
            read_frame(&mut reader).unwrap().unwrap(),
            br#"{"cmd":"info"}"#
        );
        assert_eq!(read_frame(&mut reader).unwrap().unwrap(), b"");
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn finds_the_image_size_and_the_code_section() {
        let mut headers = vec![0u8; 0x400];
        headers[..2].copy_from_slice(b"MZ");
        headers[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        headers[0x80..0x84].copy_from_slice(b"PE\0\0");
        headers[0x86..0x88].copy_from_slice(&2u16.to_le_bytes());
        headers[0x94..0x96].copy_from_slice(&0xF0u16.to_le_bytes());
        headers[0x98 + 56..0x98 + 60].copy_from_slice(&0x0379_9000u32.to_le_bytes());
        let table = 0x98 + 0xF0;
        for (index, (name, rva, size, flags)) in [
            (&b".rdata\0\0"[..], 0x2000u32, 0x100u32, 0x4000_0040u32),
            (b".text\0\0\0", 0x1000, 0x25B_2808, 0x6000_0020),
        ]
        .into_iter()
        .enumerate()
        {
            let at = table + index * 40;
            headers[at..at + 8].copy_from_slice(name);
            headers[at + 8..at + 12].copy_from_slice(&size.to_le_bytes());
            headers[at + 12..at + 16].copy_from_slice(&rva.to_le_bytes());
            headers[at + 36..at + 40].copy_from_slice(&flags.to_le_bytes());
        }
        assert_eq!(
            pe_layout(&headers),
            Some((0x0379_9000, (0x1000, 0x1000 + 0x25B_2808)))
        );
        assert_eq!(pe_layout(b"MZ"), None);
    }
}
