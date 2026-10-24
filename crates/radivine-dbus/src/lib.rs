//! `radivine-dbus` — Minimal pure-Rust D-Bus session-bus client over `UnixStream`
//! (`ACTION_SYSTEM.md` §7, §8; `ADR-028`, `ADR-041`).
//!
//! Enforces `#![forbid(unsafe_code)]` with zero Radivine crate dependencies so both
//! `radivine-actions` and `radivine-worker` can call session D-Bus services without coupling
//! the background worker to the radial wheel or action registry.
//!
//! Implements:
//! - SASL `EXTERNAL` authentication and `org.freedesktop.DBus.Hello`
//! - `org.freedesktop.FileManager1.ShowItemProperties(as, s)`
//! - `org.freedesktop.FileManager1.ShowItems(as, s)` (`E6`)
//! - `org.freedesktop.Notifications.Notify(susssasa{sv}i)`

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default D-Bus socket I/O timeout (`3 s`).
const DBUS_IO_TIMEOUT: Duration = Duration::from_secs(3);

/// Error communicating with a session D-Bus service.
#[derive(thiserror::Error, Debug)]
pub enum DbusError {
    /// Session bus socket address could not be determined or connected.
    #[error("session D-Bus unavailable: {0}")]
    Unavailable(String),
    /// SASL authentication or handshake failed.
    #[error("D-Bus authentication failed: {0}")]
    Auth(String),
    /// Remote D-Bus method returned an `ERROR` message.
    #[error("D-Bus error ({name}): {message}")]
    RemoteError {
        /// D-Bus error name (e.g. `org.freedesktop.DBus.Error.ServiceUnknown`).
        name: String,
        /// Optional human-readable error message from the bus/service.
        message: String,
    },
    /// I/O or wire protocol error.
    #[error("D-Bus I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl DbusError {
    /// Returns `true` if this error indicates the target service/interface is not supported or running.
    #[must_use]
    pub fn is_service_unsupported(&self) -> bool {
        match self {
            Self::Unavailable(_) => true,
            Self::RemoteError { name, .. } => {
                name.contains("ServiceUnknown")
                    || name.contains("NameHasNoOwner")
                    || name.contains("UnknownMethod")
                    || name.contains("UnknownInterface")
                    || name.contains("UnknownObject")
            }
            _ => false,
        }
    }
}

/// Resolves the session D-Bus Unix socket path from `DBUS_SESSION_BUS_ADDRESS` or `/run/user/<uid>/bus`.
pub fn resolve_session_bus_path() -> Result<PathBuf, DbusError> {
    if let Ok(addr) = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        && let Some(path) = parse_dbus_unix_path(&addr)
    {
        return Ok(path);
    }
    let uid = current_euid().unwrap_or(1000);
    let fallback = PathBuf::from(format!("/run/user/{uid}/bus"));
    if fallback.exists() {
        return Ok(fallback);
    }
    Err(DbusError::Unavailable(
        "DBUS_SESSION_BUS_ADDRESS not set and /run/user/<uid>/bus not found".to_string(),
    ))
}

/// Parses a `unix:path=/path/to/bus` address string.
#[must_use]
pub fn parse_dbus_unix_path(address: &str) -> Option<PathBuf> {
    for entry in address.split(';') {
        let Some(rest) = entry.trim().strip_prefix("unix:") else {
            continue;
        };
        for kv in rest.split(',') {
            if let Some(p) = kv.strip_prefix("path=")
                && !p.is_empty()
            {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

fn current_euid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Calls `org.freedesktop.FileManager1.ShowItemProperties(uris, startup_id)` on the session bus.
pub fn call_show_item_properties(
    socket_override: Option<&Path>,
    uris: &[String],
    startup_id: &str,
) -> Result<(), DbusError> {
    call_file_manager_uris_method("ShowItemProperties", socket_override, uris, startup_id)
}

/// Calls `org.freedesktop.FileManager1.ShowItems(uris, startup_id)` on the session bus (`E6`).
pub fn call_show_items(
    socket_override: Option<&Path>,
    uris: &[String],
    startup_id: &str,
) -> Result<(), DbusError> {
    call_file_manager_uris_method("ShowItems", socket_override, uris, startup_id)
}

fn call_file_manager_uris_method(
    member: &str,
    socket_override: Option<&Path>,
    uris: &[String],
    startup_id: &str,
) -> Result<(), DbusError> {
    let sock_path = match socket_override {
        Some(p) => p.to_path_buf(),
        None => resolve_session_bus_path()?,
    };
    let mut client = SessionBusClient::connect(&sock_path)?;

    let mut body = Vec::new();
    // Array of strings `as`: 4-byte byte length followed by 4-byte aligned strings
    body.extend_from_slice(&0u32.to_le_bytes());
    let array_start = body.len();
    for uri in uris {
        align_to(&mut body, 4);
        write_dbus_string(&mut body, uri);
    }
    let array_byte_len = (body.len() - array_start) as u32;
    body[0..4].copy_from_slice(&array_byte_len.to_le_bytes());

    // `startup_id: s`
    align_to(&mut body, 4);
    write_dbus_string(&mut body, startup_id);

    client.call_method(
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
        member,
        Some("ass"),
        &body,
    )?;
    Ok(())
}

/// Calls `org.freedesktop.Notifications.Notify(...)` on the session bus.
pub fn call_desktop_notify(
    socket_override: Option<&Path>,
    app_name: &str,
    app_icon: &str,
    summary: &str,
    body_text: &str,
    expire_timeout_ms: i32,
) -> Result<u32, DbusError> {
    let sock_path = match socket_override {
        Some(p) => p.to_path_buf(),
        None => resolve_session_bus_path()?,
    };
    let mut client = SessionBusClient::connect(&sock_path)?;

    // Signature: "susssasa{sv}i"
    let mut body = Vec::new();
    // 1. app_name (s)
    write_dbus_string(&mut body, app_name);
    // 2. replaces_id (u)
    align_to(&mut body, 4);
    body.extend_from_slice(&0u32.to_le_bytes());
    // 3. app_icon (s)
    align_to(&mut body, 4);
    write_dbus_string(&mut body, app_icon);
    // 4. summary (s)
    align_to(&mut body, 4);
    write_dbus_string(&mut body, summary);
    // 5. body (s)
    align_to(&mut body, 4);
    write_dbus_string(&mut body, body_text);
    // 6. actions (as) - empty array
    align_to(&mut body, 4);
    body.extend_from_slice(&0u32.to_le_bytes());
    // 7. hints (a{sv}) - empty dict; note dict entries are 8-byte aligned structs so the start of
    //    array elements after the 4-byte length is padded to an 8-byte boundary.
    align_to(&mut body, 4);
    body.extend_from_slice(&0u32.to_le_bytes());
    align_to(&mut body, 8);
    // 8. expire_timeout (i)
    align_to(&mut body, 4);
    body.extend_from_slice(&expire_timeout_ms.to_le_bytes());

    let reply_body = client.call_method(
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        Some("susssasa{sv}i"),
        &body,
    )?;

    if reply_body.len() >= 4 {
        Ok(u32::from_le_bytes([
            reply_body[0],
            reply_body[1],
            reply_body[2],
            reply_body[3],
        ]))
    } else {
        Ok(0)
    }
}

struct SessionBusClient {
    reader: BufReader<UnixStream>,
    next_serial: u32,
}

impl SessionBusClient {
    fn connect(path: &Path) -> Result<Self, DbusError> {
        let stream =
            UnixStream::connect(path).map_err(|e| DbusError::Unavailable(e.to_string()))?;
        let _ = stream.set_read_timeout(Some(DBUS_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(DBUS_IO_TIMEOUT));

        let mut reader = BufReader::new(stream);
        let uid_str = current_euid().unwrap_or(1000).to_string();
        let mut hex_uid = String::with_capacity(uid_str.len() * 2);
        for b in uid_str.bytes() {
            hex_uid.push_str(&format!("{b:02x}"));
        }

        let auth_cmd = format!("\0AUTH EXTERNAL {hex_uid}\r\n");
        reader.get_mut().write_all(auth_cmd.as_bytes())?;
        reader.get_mut().flush()?;

        let mut line = String::new();
        reader.read_line(&mut line)?;
        if !line.starts_with("OK ") {
            return Err(DbusError::Auth(line.trim().to_string()));
        }

        reader.get_mut().write_all(b"BEGIN\r\n")?;
        reader.get_mut().flush()?;

        let mut client = Self {
            reader,
            next_serial: 1,
        };

        // Mandatory org.freedesktop.DBus.Hello() handshake
        client.call_method(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "Hello",
            None,
            &[],
        )?;

        Ok(client)
    }

    fn call_method(
        &mut self,
        destination: &str,
        object_path: &str,
        interface: &str,
        member: &str,
        signature: Option<&str>,
        body: &[u8],
    ) -> Result<Vec<u8>, DbusError> {
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);

        let msg = encode_method_call(
            serial,
            destination,
            object_path,
            interface,
            member,
            signature,
            body,
        );
        self.reader.get_mut().write_all(&msg)?;
        self.reader.get_mut().flush()?;

        // Read messages until we receive the METHOD_RETURN (2) or ERROR (3) matching `serial`.
        loop {
            let reply = read_dbus_message(&mut self.reader)?;
            if reply.reply_serial != Some(serial) {
                continue;
            }
            match reply.msg_type {
                2 => return Ok(reply.body),
                3 => {
                    let err_name = reply
                        .error_name
                        .unwrap_or_else(|| "org.freedesktop.DBus.Error.Failed".to_string());
                    let err_msg = extract_first_string(&reply.body).unwrap_or_default();
                    return Err(DbusError::RemoteError {
                        name: err_name,
                        message: err_msg,
                    });
                }
                _ => continue,
            }
        }
    }
}

struct IncomingMessage {
    msg_type: u8,
    reply_serial: Option<u32>,
    error_name: Option<String>,
    body: Vec<u8>,
}

fn encode_method_call(
    serial: u32,
    destination: &str,
    object_path: &str,
    interface: &str,
    member: &str,
    signature: Option<&str>,
    body: &[u8],
) -> Vec<u8> {
    let mut header = Vec::with_capacity(128 + body.len());
    // Fixed 12-byte header + 4-byte header-fields array length placeholder
    header.push(b'l'); // Little-endian
    header.push(1); // METHOD_CALL
    header.push(0); // Flags: reply expected
    header.push(1); // Protocol version 1
    header.extend_from_slice(&(body.len() as u32).to_le_bytes());
    header.extend_from_slice(&serial.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes()); // placeholder for fields array byte length

    let fields_start = header.len(); // offset 16

    // Field 1: PATH (o)
    append_header_field_string(&mut header, 1, b'o', object_path);
    // Field 2: INTERFACE (s)
    append_header_field_string(&mut header, 2, b's', interface);
    // Field 3: MEMBER (s)
    append_header_field_string(&mut header, 3, b's', member);
    // Field 6: DESTINATION (s)
    append_header_field_string(&mut header, 6, b's', destination);
    // Field 8: SIGNATURE (g), if non-empty
    if let Some(sig) = signature
        && !sig.is_empty()
    {
        append_header_field_signature(&mut header, 8, sig);
    }

    let fields_byte_len = (header.len() - fields_start) as u32;
    header[12..16].copy_from_slice(&fields_byte_len.to_le_bytes());

    // Pad entire header to an 8-byte boundary before appending the body
    align_to(&mut header, 8);
    header.extend_from_slice(body);
    header
}

fn append_header_field_string(buf: &mut Vec<u8>, code: u8, sig_char: u8, value: &str) {
    align_to(buf, 8);
    buf.push(code);
    buf.push(1); // signature length = 1
    buf.push(sig_char);
    buf.push(0); // NUL terminator of signature
    align_to(buf, 4);
    write_dbus_string(buf, value);
}

fn append_header_field_signature(buf: &mut Vec<u8>, code: u8, sig: &str) {
    align_to(buf, 8);
    buf.push(code);
    buf.push(1);
    buf.push(b'g');
    buf.push(0);
    let sig_bytes = sig.as_bytes();
    buf.push(u8::try_from(sig_bytes.len()).unwrap_or(0));
    buf.extend_from_slice(sig_bytes);
    buf.push(0);
}

fn write_dbus_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    buf.push(0);
}

fn align_to(buf: &mut Vec<u8>, alignment: usize) {
    let rem = buf.len() % alignment;
    if rem != 0 {
        buf.resize(buf.len() + (alignment - rem), 0);
    }
}

fn read_dbus_message<R: Read>(reader: &mut R) -> Result<IncomingMessage, DbusError> {
    let mut fixed = [0u8; 16];
    reader.read_exact(&mut fixed)?;

    let le = fixed[0] == b'l';
    let msg_type = fixed[1];
    let body_len = read_u32(&fixed[4..8], le) as usize;
    let fields_len = read_u32(&fixed[12..16], le) as usize;

    let padded_fields_len = (fields_len + 7) & !7;
    let mut fields_buf = vec![0u8; padded_fields_len];
    reader.read_exact(&mut fields_buf)?;

    let mut body = vec![0u8; body_len];
    reader.read_exact(&mut body)?;

    let (reply_serial, error_name) = parse_header_fields(&fields_buf[..fields_len], le);
    Ok(IncomingMessage {
        msg_type,
        reply_serial,
        error_name,
        body,
    })
}

fn parse_header_fields(fields: &[u8], le: bool) -> (Option<u32>, Option<String>) {
    let mut pos = 0usize;
    let mut reply_serial = None;
    let mut error_name = None;

    while pos < fields.len() {
        // Each header field struct `(yv)` starts at an 8-byte boundary (offset 16 in message is 8-byte aligned)
        pos = (pos + 7) & !7;
        if pos + 4 > fields.len() {
            break;
        }
        let code = fields[pos];
        let sig_len = fields[pos + 1] as usize;
        pos += 2;
        if pos + sig_len + 1 > fields.len() {
            break;
        }
        let sig = &fields[pos..pos + sig_len];
        pos += sig_len + 1; // skip signature + NUL

        match sig {
            b"u" => {
                pos = (pos + 3) & !3;
                if pos + 4 > fields.len() {
                    break;
                }
                let val = read_u32(&fields[pos..pos + 4], le);
                pos += 4;
                if code == 5 {
                    reply_serial = Some(val);
                }
            }
            b"s" | b"o" => {
                pos = (pos + 3) & !3;
                if pos + 4 > fields.len() {
                    break;
                }
                let slen = read_u32(&fields[pos..pos + 4], le) as usize;
                pos += 4;
                if pos + slen + 1 > fields.len() {
                    break;
                }
                if code == 4 {
                    error_name =
                        Some(String::from_utf8_lossy(&fields[pos..pos + slen]).into_owned());
                }
                pos += slen + 1;
            }
            b"g" => {
                if pos >= fields.len() {
                    break;
                }
                let glen = fields[pos] as usize;
                pos += 1 + glen + 1;
            }
            b"y" => {
                pos += 1;
            }
            _ => break,
        }
    }

    (reply_serial, error_name)
}

fn extract_first_string(body: &[u8]) -> Option<String> {
    if body.len() < 4 {
        return None;
    }
    let slen = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
    if body.len() < 4 + slen {
        return None;
    }
    Some(String::from_utf8_lossy(&body[4..4 + slen]).into_owned())
}

fn read_u32(bytes: &[u8], le: bool) -> u32 {
    let arr = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if le {
        u32::from_le_bytes(arr)
    } else {
        u32::from_be_bytes(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    fn encode_method_return(reply_serial: u32, body: &[u8]) -> Vec<u8> {
        let mut header = vec![b'l', 2, 1, 1]; // LE, METHOD_RETURN, NO_REPLY_EXPECTED, v1
        header.extend_from_slice(&(body.len() as u32).to_le_bytes());
        header.extend_from_slice(&100u32.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        let fields_start = header.len();

        // Field 5: REPLY_SERIAL (u)
        align_to(&mut header, 8);
        header.push(5);
        header.push(1);
        header.push(b'u');
        header.push(0);
        align_to(&mut header, 4);
        header.extend_from_slice(&reply_serial.to_le_bytes());

        let fields_len = (header.len() - fields_start) as u32;
        header[12..16].copy_from_slice(&fields_len.to_le_bytes());
        align_to(&mut header, 8);
        header.extend_from_slice(body);
        header
    }

    fn spawn_mock_bus(sock_path: PathBuf, reply_body: Vec<u8>) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(&sock_path).unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            // AUTH EXTERNAL
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("AUTH EXTERNAL"));
            reader
                .get_mut()
                .write_all(b"OK 0123456789abcdef0123456789abcdef\r\n")
                .unwrap();
            // BEGIN
            line.clear();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "BEGIN");

            // 1. Hello() call (serial 1)
            let hello_msg = read_dbus_message(&mut reader).unwrap();
            assert_eq!(hello_msg.msg_type, 1);
            let mut hello_reply = Vec::new();
            write_dbus_string(&mut hello_reply, ":1.42");
            let ret1 = encode_method_return(1, &hello_reply);
            reader.get_mut().write_all(&ret1).unwrap();

            // 2. Actual method call (serial 2)
            let call_msg = read_dbus_message(&mut reader).unwrap();
            assert_eq!(call_msg.msg_type, 1);
            let ret2 = encode_method_return(2, &reply_body);
            reader.get_mut().write_all(&ret2).unwrap();
        })
    }

    #[test]
    fn parse_dbus_address_and_error_classification() {
        assert_eq!(
            parse_dbus_unix_path("unix:path=/run/user/1000/bus,guid=0123"),
            Some(PathBuf::from("/run/user/1000/bus"))
        );
        assert_eq!(parse_dbus_unix_path("tcp:host=127.0.0.1"), None);

        let err = DbusError::RemoteError {
            name: "org.freedesktop.DBus.Error.ServiceUnknown".to_string(),
            message: "not running".to_string(),
        };
        assert!(err.is_service_unsupported());
    }

    #[test]
    fn mock_bus_notify_show_properties_and_show_items() {
        let dir = tempfile::tempdir().unwrap();

        // 1. call_desktop_notify
        let sock1 = dir.path().join("bus1.sock");
        let h1 = spawn_mock_bus(sock1.clone(), 77u32.to_le_bytes().to_vec());
        let id = call_desktop_notify(
            Some(&sock1),
            "Radivine",
            "radivine",
            "Converted to WebP",
            "photo.webp saved beside the original.",
            5000,
        )
        .unwrap();
        assert_eq!(id, 77);
        h1.join().unwrap();

        // 2. call_show_item_properties
        let sock2 = dir.path().join("bus2.sock");
        let h2 = spawn_mock_bus(sock2.clone(), Vec::new());
        call_show_item_properties(
            Some(&sock2),
            &["file:///tmp/photo.webp".to_string()],
            "radivine",
        )
        .unwrap();
        h2.join().unwrap();

        // 3. call_show_items (E6)
        let sock3 = dir.path().join("bus3.sock");
        let h3 = spawn_mock_bus(sock3.clone(), Vec::new());
        call_show_items(
            Some(&sock3),
            &["file:///tmp/photo.webp".to_string()],
            "radivine",
        )
        .unwrap();
        h3.join().unwrap();
    }
}
