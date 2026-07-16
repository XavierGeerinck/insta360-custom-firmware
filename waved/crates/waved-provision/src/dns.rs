//! Minimal wildcard DNS responder for captive provisioning.
//! Every supported question resolves to the camera, so a phone's normal
//! captive-network probe reaches the local HTTP redirect.

use std::io;
use std::net::{Ipv4Addr, UdpSocket as StdUdpSocket};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, warn};

const IFACE: &str = "wlan0";
const SERVER_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 2, 1);
const DNS_PORT: u16 = 53;
const HEADER_LEN: usize = 12;
const MAX_PACKET_LEN: usize = 512;
const TYPE_A: u16 = 1;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
const ANSWER_TTL_SECONDS: u32 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Question {
    id: [u8; 2],
    request_flags: u16,
    end: usize,
    query_type: u16,
    query_class: u16,
}

fn parse_question(packet: &[u8]) -> Option<Question> {
    if packet.len() < HEADER_LEN || packet.len() > MAX_PACKET_LEN {
        return None;
    }

    let request_flags = u16::from_be_bytes(packet[2..4].try_into().ok()?);
    let question_count = u16::from_be_bytes(packet[4..6].try_into().ok()?);
    // Only standard, non-response queries with exactly one question are in
    // scope. In particular, question-name compression is deliberately not
    // followed so cyclic or out-of-bounds pointers cannot consume work.
    if request_flags & 0xf800 != 0 || question_count != 1 {
        return None;
    }

    let mut cursor = HEADER_LEN;
    let mut decoded_len = 0usize;
    let mut labels = 0usize;
    loop {
        let label_len = *packet.get(cursor)? as usize;
        cursor += 1;
        if label_len == 0 {
            break;
        }
        if label_len & 0xc0 != 0 || label_len > 63 {
            return None;
        }
        decoded_len = decoded_len.checked_add(label_len + usize::from(labels > 0))?;
        if decoded_len > 253 {
            return None;
        }
        cursor = cursor.checked_add(label_len)?;
        packet.get(..cursor)?;
        labels += 1;
    }

    let query_type = u16::from_be_bytes(packet.get(cursor..cursor + 2)?.try_into().ok()?);
    let query_class = u16::from_be_bytes(packet.get(cursor + 2..cursor + 4)?.try_into().ok()?);
    cursor += 4;

    Some(Question {
        id: packet[0..2].try_into().ok()?,
        request_flags,
        end: cursor,
        query_type,
        query_class,
    })
}

fn build_response(packet: &[u8], question: Question) -> Vec<u8> {
    let has_answer =
        question.query_class == CLASS_IN && matches!(question.query_type, TYPE_A | TYPE_ANY);
    let mut response = Vec::with_capacity(question.end + usize::from(has_answer) * 16);
    response.extend_from_slice(&question.id);
    let response_flags = 0x8000 | 0x0400 | (question.request_flags & 0x0100);
    response.extend_from_slice(&response_flags.to_be_bytes());
    response.extend_from_slice(&1u16.to_be_bytes());
    response.extend_from_slice(&u16::from(has_answer).to_be_bytes());
    response.extend_from_slice(&0u16.to_be_bytes());
    response.extend_from_slice(&0u16.to_be_bytes());
    response.extend_from_slice(&packet[HEADER_LEN..question.end]);

    if has_answer {
        // The response copies the question at offset 12, so this fixed pointer
        // safely refers to the complete question name.
        response.extend_from_slice(&[0xc0, 0x0c]);
        response.extend_from_slice(&TYPE_A.to_be_bytes());
        response.extend_from_slice(&CLASS_IN.to_be_bytes());
        response.extend_from_slice(&ANSWER_TTL_SECONDS.to_be_bytes());
        response.extend_from_slice(&4u16.to_be_bytes());
        response.extend_from_slice(&SERVER_IP.octets());
    }
    response
}

fn response_for(packet: &[u8]) -> Option<Vec<u8>> {
    let question = parse_question(packet)?;
    Some(build_response(packet, question))
}

#[cfg(target_os = "linux")]
fn bind_to_device(socket: &StdUdpSocket, interface: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    let interface = CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface"))?;
    // SAFETY: the fd is owned by `socket`, and the option points to a live,
    // NUL-terminated CString whose reported length includes that terminator.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            interface.as_ptr().cast(),
            interface.as_bytes_with_nul().len() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_to_device(_socket: &StdUdpSocket, _interface: &str) -> io::Result<()> {
    Ok(())
}

pub struct Server {
    socket: UdpSocket,
}

impl Server {
    /// Bind synchronously so startup failures occur before `ApActive` is sent.
    pub fn bind() -> io::Result<Self> {
        let socket = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, DNS_PORT))?;
        socket.set_nonblocking(true)?;
        bind_to_device(&socket, IFACE)?;
        Ok(Self {
            socket: UdpSocket::from_std(socket)?,
        })
    }

    pub async fn serve(self, mut shutdown: watch::Receiver<bool>) -> io::Result<()> {
        // One extra byte distinguishes a maximum-size query from a truncated
        // oversized datagram; anything over the classic DNS UDP cap is ignored.
        let mut packet = [0u8; MAX_PACKET_LEN + 1];
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                received = self.socket.recv_from(&mut packet) => {
                    let (length, peer) = received?;
                    let Some(response) = response_for(&packet[..length]) else {
                        debug!("ignored malformed DNS packet from {peer}");
                        continue;
                    };
                    if let Err(error) = self.socket.send_to(&response, peer).await {
                        warn!("DNS reply to {peer} failed: {error}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str, query_type: u16) -> Vec<u8> {
        let mut packet = Vec::new();
        packet.extend_from_slice(&[0x12, 0x34]);
        packet.extend_from_slice(&0x0100u16.to_be_bytes());
        packet.extend_from_slice(&1u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        packet.extend_from_slice(&0u16.to_be_bytes());
        for label in name.split('.') {
            packet.push(label.len() as u8);
            packet.extend_from_slice(label.as_bytes());
        }
        packet.push(0);
        packet.extend_from_slice(&query_type.to_be_bytes());
        packet.extend_from_slice(&CLASS_IN.to_be_bytes());
        packet
    }

    fn answer_count(response: &[u8]) -> u16 {
        u16::from_be_bytes(response[6..8].try_into().unwrap())
    }

    #[test]
    fn a_question_resolves_to_camera() {
        let query = query("connectivitycheck.gstatic.com", TYPE_A);
        let question = parse_question(&query).unwrap();
        let response = response_for(&query).unwrap();
        assert_eq!(&response[..2], &[0x12, 0x34]);
        assert_eq!(
            u16::from_be_bytes(response[2..4].try_into().unwrap()),
            0x8500
        );
        assert_eq!(answer_count(&response), 1);
        assert_eq!(&response[question.end..question.end + 2], &[0xc0, 0x0c]);
        assert_eq!(&response[response.len() - 4..], &SERVER_IP.octets());
    }

    #[test]
    fn any_question_gets_an_a_answer() {
        let query = query("captive.apple.com", TYPE_ANY);
        let question = parse_question(&query).unwrap();
        let response = response_for(&query).unwrap();
        assert_eq!(answer_count(&response), 1);
        assert_eq!(
            u16::from_be_bytes(
                response[question.end + 2..question.end + 4]
                    .try_into()
                    .unwrap()
            ),
            TYPE_A
        );
    }

    #[test]
    fn unsupported_type_gets_valid_empty_response() {
        let query = query("example.com", 28);
        let question = parse_question(&query).unwrap();
        let response = response_for(&query).unwrap();
        assert_eq!(answer_count(&response), 0);
        assert_eq!(response.len(), question.end);
        assert_ne!(
            u16::from_be_bytes(response[2..4].try_into().unwrap()) & 0x8000,
            0
        );
    }

    #[test]
    fn rejects_truncated_and_oversized_packets() {
        assert!(response_for(&[0u8; 10]).is_none());
        let mut truncated = query("example.com", TYPE_A);
        truncated.pop();
        assert!(response_for(&truncated).is_none());
        assert!(response_for(&[0u8; MAX_PACKET_LEN + 1]).is_none());
    }

    #[test]
    fn rejects_compressed_question_name() {
        let mut packet = query("example.com", TYPE_A);
        packet.truncate(HEADER_LEN);
        packet.extend_from_slice(&[0xc0, 0x0c, 0, TYPE_A as u8, 0, CLASS_IN as u8]);
        assert!(response_for(&packet).is_none());
    }

    #[test]
    fn rejects_multiple_questions() {
        let mut packet = query("example.com", TYPE_A);
        packet[4..6].copy_from_slice(&2u16.to_be_bytes());
        assert!(response_for(&packet).is_none());
    }
}
