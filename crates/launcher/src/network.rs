//! Which process owns a local TCP connection, and this PC's host name.
//!
//! Ports `ow174/launcher/connections.py`: the Battle.net emulator tells two
//! games on one PC apart by the process that owns each game's connection.

use std::{ffi::OsString, io, mem::size_of, os::windows::ffi::OsStringExt, ptr};

use windows_sys::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    },
    System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW},
};

const AF_INET: u32 = 2;
/// The table can grow between the size query and the read; ask again a few times.
const TABLE_ATTEMPTS: usize = 4;

/// The PID of the process whose IPv4 TCP connection goes from `local_port`
/// to `remote_port` (`connection_owner` in connections.py).
pub fn connection_owner(local_port: u16, remote_port: u16) -> Option<u32> {
    tcp_rows()
        .ok()?
        .into_iter()
        .find(|row| port(row.dwLocalPort) == local_port && port(row.dwRemotePort) == remote_port)
        .map(|row| row.dwOwningPid)
}

/// Ports sit in network order in the low 16 bits.
fn port(value: u32) -> u16 {
    u16::from_be(value as u16)
}

fn tcp_rows() -> io::Result<Vec<MIB_TCPROW_OWNER_PID>> {
    let mut size = 0u32;
    // SAFETY: a null table with a zero size only asks for the size needed.
    unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    for _ in 0..TABLE_ATTEMPTS {
        // u32 elements keep the buffer aligned for the table's u32 fields.
        let mut buffer = vec![0u32; (size as usize).div_ceil(size_of::<u32>()) + 1];
        let mut capacity = (buffer.len() * size_of::<u32>()) as u32;
        // SAFETY: the buffer is valid and writable for `capacity` bytes.
        let status = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut capacity,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            size = capacity;
            continue;
        }
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        // MIB_TCPTABLE_OWNER_PID: a u32 count, then the rows.
        let count = buffer[0] as usize;
        let row_words = size_of::<MIB_TCPROW_OWNER_PID>() / size_of::<u32>();
        let rows = &buffer[1..];
        if rows.len() < count * row_words {
            return Err(io::Error::other("truncated TCP table"));
        }
        return Ok(rows
            .chunks_exact(row_words)
            .take(count)
            .map(|row| MIB_TCPROW_OWNER_PID {
                dwState: row[0],
                dwLocalAddr: row[1],
                dwLocalPort: row[2],
                dwRemoteAddr: row[3],
                dwRemotePort: row[4],
                dwOwningPid: row[5],
            })
            .collect());
    }
    Err(io::Error::other("the TCP table kept growing"))
}

/// This PC's DNS host name, which resolves to its network addresses
/// (Python's `socket.gethostname()`).
pub fn host_name() -> Option<String> {
    let mut size = 0u32;
    // SAFETY: a null buffer with a zero size only asks for the size needed.
    unsafe { GetComputerNameExW(ComputerNameDnsHostname, ptr::null_mut(), &mut size) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: the buffer is valid for `size` characters, which is passed.
    if unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut size) } == 0 {
        return None;
    }
    let name = OsString::from_wide(&buffer[..size as usize]);
    name.into_string().ok().filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener, TcpStream};

    #[test]
    fn finds_the_process_that_owns_a_loopback_connection() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server = listener.local_addr().unwrap().port();
        let client = TcpStream::connect((Ipv4Addr::LOCALHOST, server)).unwrap();
        let _accepted = listener.accept().unwrap();
        let local = client.local_addr().unwrap().port();
        assert_eq!(connection_owner(local, server), Some(std::process::id()));
        assert_eq!(connection_owner(server, server), None);
    }
}
