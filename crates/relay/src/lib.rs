//! `owwfd_relay.dll`: pipes the Overwatch 1.74 client's Battle.net
//! connection to the local server in plaintext.
//!
//! The client's transport is a byte-stream whose `ssl_stream` member does
//! the TLS work. Forcing the byte-stream's state slot to "connected" makes the
//! client write plaintext: a WebSocket upgrade, then Battle.net frames. The
//! relay swaps the stream's `send` and `recv` slots for a pipe to the
//! server's RPC port, so TLS never runs. Addresses are build 104319's.
//!
//! This is one of the two crates allowed to use `unsafe` (the other is the
//! launcher). Every `unsafe` block states why it is sound.

#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

#[cfg(windows)]
mod hook;

/// Byte-stream vtable, as an offset from the client image base.
pub const BYTE_STREAM_VTABLE: u64 = 0x298_E2F8;
/// Byte-stream fields.
pub const STATE_OFFSET: u64 = 0x20;
pub const SSL_STREAM_OFFSET: u64 = 0x48;
pub const HOST_OFFSET: u64 = 0x80;
pub const HOST_LENGTH_OFFSET: u64 = 0x88;
pub const PORT_OFFSET: u64 = 0xa8;
/// A byte-stream candidate needs this many readable bytes.
pub const BYTE_STREAM_SIZE: u64 = 0xc0;
/// Vtable slots: the byte-stream's state, the `ssl_stream`'s recv and send.
pub const STATE_SLOT: usize = 1;
pub const RECV_SLOT: usize = 2;
pub const SEND_SLOT: usize = 3;
/// Vtable slots copied into each replacement table.
pub const VTABLE_SLOTS: usize = 64;
/// At most this many byte-streams get a relay socket; entries are never removed.
pub const MAX_RELAYS: usize = 64;
/// The server's Battle.net RPC port, which takes the plaintext.
pub const RELAY_PORT: u16 = 21119;
/// The port the client dials; only other ports are always logged.
pub const BATTLE_NET_PORT: u16 = 1119;
/// Largest private region the scan reads.
pub const MAX_REGION: u64 = 0x800_0000;

/// A heap pointer as the client allocates it: 8-byte aligned, in the user
/// address range above the low 1 TiB.
pub fn in_heap(pointer: u64) -> bool {
    (0x0000_0100_0000_0000..0x0000_7FF0_0000_0000).contains(&pointer) && pointer & 7 == 0
}

/// `SizeOfImage` from the PE headers at the start of a 64-bit image.
pub fn image_size(headers: &[u8]) -> Option<u64> {
    let read_u32 = |offset: usize| {
        headers
            .get(offset..offset + 4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
    };
    if headers.get(..2)? != b"MZ" {
        return None;
    }
    let nt = read_u32(0x3c)? as usize;
    if headers.get(nt..nt + 4)? != b"PE\0\0" {
        return None;
    }
    // Signature (4) and file header (20), then SizeOfImage at optional header + 56.
    read_u32(nt + 24 + 56).map(u64::from)
}

/// Addresses of the 8-byte aligned words equal to `value` in memory copied
/// from `base`, leaving room for a whole byte-stream before `end`.
pub fn matching_words(memory: &[u8], base: u64, end: u64, value: u64) -> impl Iterator<Item = u64> {
    memory
        .chunks_exact(8)
        .enumerate()
        .filter(move |(_, word)| u64::from_le_bytes((*word).try_into().expect("8 bytes")) == value)
        .map(move |(index, _)| base + index as u64 * 8)
        .filter(move |address| address + BYTE_STREAM_SIZE <= end)
}

/// Up to 48 bytes as hex, for the log.
pub fn hex_preview(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(48)
        .map(|byte| format!("{byte:02X} "))
        .collect()
}

/// Printable ASCII, with a dot for anything else.
pub fn printable(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| {
            if (32..127).contains(&byte) {
                byte as char
            } else {
                '.'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_pointers_are_aligned_and_in_the_user_range() {
        assert!(in_heap(0x0000_0200_0000_0010));
        assert!(!in_heap(0x0000_0200_0000_0014));
        assert!(!in_heap(0x0000_00FF_FFFF_FFF8));
        assert!(!in_heap(0x0000_7FF0_0000_0000));
    }

    #[test]
    fn reads_the_image_size_from_pe_headers() {
        let mut headers = vec![0u8; 0x200];
        headers[..2].copy_from_slice(b"MZ");
        headers[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        headers[0x80..0x84].copy_from_slice(b"PE\0\0");
        headers[0x80 + 80..0x80 + 84].copy_from_slice(&0x0300_0000u32.to_le_bytes());
        assert_eq!(image_size(&headers), Some(0x0300_0000));
        headers[0x80] = b'X';
        assert_eq!(image_size(&headers), None);
        assert_eq!(image_size(b"MZ"), None);
    }

    #[test]
    fn finds_aligned_words_with_room_for_a_byte_stream() {
        let value = 0x1122_3344_5566_7788u64;
        let mut memory = vec![0u8; 0x200];
        for offset in [0x10, 0x150] {
            memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        memory[0x31..0x39].copy_from_slice(&value.to_le_bytes()); // unaligned
        let base = 0x1000;
        let found: Vec<u64> = matching_words(&memory, base, base + 0x200, value).collect();
        assert_eq!(found, [base + 0x10]);
    }
}
