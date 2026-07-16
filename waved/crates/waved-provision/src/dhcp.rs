//! Minimal DHCPv4 server for the temporary provisioning network.
//! Packet parsing/encoding is isolated from socket I/O so untrusted datagrams
//! can be covered without binding privileged ports in tests.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, UdpSocket as StdUdpSocket};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, warn};

const IFACE: &str = "wlan0";
const SERVER_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 2, 1);
const POOL_FIRST: u8 = 50;
const POOL_LAST: u8 = 200;
const BOOTP_FIXED_LEN: usize = 236;
const DHCP_HEADER_LEN: usize = 240;
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];
const LEASE_SECONDS: u32 = 12 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Ack = 5,
    Nak = 6,
}

impl MessageType {
    fn from_byte(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Discover),
            3 => Some(Self::Request),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DhcpRequest {
    xid: [u8; 4],
    flags: [u8; 2],
    ciaddr: Ipv4Addr,
    chaddr: Vec<u8>,
    client_key: Vec<u8>,
    message_type: MessageType,
    requested_ip: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reply {
    Offer(Ipv4Addr),
    Ack(Ipv4Addr),
    Nak,
}

fn option_ipv4(value: &[u8]) -> Option<Ipv4Addr> {
    (value.len() == 4).then(|| Ipv4Addr::new(value[0], value[1], value[2], value[3]))
}

fn parse_request(packet: &[u8]) -> Option<DhcpRequest> {
    if packet.len() < DHCP_HEADER_LEN
        || packet[0] != 1
        || packet[1] != 1
        || packet[2] == 0
        || packet[2] > 16
        || packet[BOOTP_FIXED_LEN..DHCP_HEADER_LEN] != MAGIC_COOKIE
    {
        return None;
    }

    let hlen = packet[2] as usize;
    let chaddr = packet.get(28..28 + hlen)?.to_vec();
    let mut message_type = None;
    let mut requested_ip = None;
    let mut server_id = None;
    let mut client_id = None;
    let mut cursor = DHCP_HEADER_LEN;

    while cursor < packet.len() {
        let code = packet[cursor];
        cursor += 1;
        match code {
            0 => continue,
            255 => break,
            _ => {
                let len = *packet.get(cursor)? as usize;
                cursor += 1;
                let value = packet.get(cursor..cursor.checked_add(len)?)?;
                cursor += len;
                match code {
                    50 => requested_ip = option_ipv4(value),
                    53 if value.len() == 1 => message_type = MessageType::from_byte(value[0]),
                    54 => server_id = option_ipv4(value),
                    61 if !value.is_empty() => client_id = Some(value.to_vec()),
                    _ => {}
                }
            }
        }
    }

    Some(DhcpRequest {
        xid: packet[4..8].try_into().ok()?,
        flags: packet[10..12].try_into().ok()?,
        ciaddr: Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
        chaddr: chaddr.clone(),
        client_key: client_id.unwrap_or(chaddr),
        message_type: message_type?,
        requested_ip,
        server_id,
    })
}

fn pool_ip(last_octet: u8) -> Ipv4Addr {
    Ipv4Addr::new(192, 168, 2, last_octet)
}

fn in_pool(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[..3] == [192, 168, 2] && (POOL_FIRST..=POOL_LAST).contains(&octets[3])
}

#[derive(Default)]
struct LeasePool {
    by_client: HashMap<Vec<u8>, Ipv4Addr>,
    by_ip: HashMap<Ipv4Addr, Vec<u8>>,
}

impl LeasePool {
    fn offer(&mut self, client: &[u8]) -> Option<Ipv4Addr> {
        if let Some(ip) = self.by_client.get(client) {
            return Some(*ip);
        }

        // A stable byte fold spreads common MAC addresses across the pool.
        let pool_len = POOL_LAST - POOL_FIRST + 1;
        let start = client
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_mul(31).wrapping_add(*byte))
            % pool_len;
        for offset in 0..pool_len {
            let octet = POOL_FIRST + (start + offset) % pool_len;
            let ip = pool_ip(octet);
            if !self.by_ip.contains_key(&ip) {
                self.by_client.insert(client.to_vec(), ip);
                self.by_ip.insert(ip, client.to_vec());
                return Some(ip);
            }
        }
        None
    }

    fn respond(&mut self, request: &DhcpRequest) -> Option<Reply> {
        match request.message_type {
            MessageType::Discover => self.offer(&request.client_key).map(Reply::Offer),
            MessageType::Request => {
                if request.server_id.is_some() && request.server_id != Some(SERVER_IP) {
                    return None;
                }
                let requested = request
                    .requested_ip
                    .or((!request.ciaddr.is_unspecified()).then_some(request.ciaddr));
                let Some(ip) = requested else {
                    return Some(Reply::Nak);
                };
                let owner_matches = self
                    .by_ip
                    .get(&ip)
                    .map(|owner| owner.as_slice() == request.client_key.as_slice())
                    .unwrap_or(true);
                if in_pool(ip) && owner_matches {
                    self.by_client.insert(request.client_key.clone(), ip);
                    self.by_ip.insert(ip, request.client_key.clone());
                    Some(Reply::Ack(ip))
                } else {
                    Some(Reply::Nak)
                }
            }
            _ => None,
        }
    }
}

fn push_option(packet: &mut Vec<u8>, code: u8, value: &[u8]) {
    packet.push(code);
    packet.push(value.len() as u8);
    packet.extend_from_slice(value);
}

fn build_reply(request: &DhcpRequest, reply: Reply) -> Vec<u8> {
    let (message_type, offered_ip) = match reply {
        Reply::Offer(ip) => (MessageType::Offer, ip),
        Reply::Ack(ip) => (MessageType::Ack, ip),
        Reply::Nak => (MessageType::Nak, Ipv4Addr::UNSPECIFIED),
    };

    let mut packet = vec![0u8; BOOTP_FIXED_LEN];
    packet[0] = 2;
    packet[1] = 1;
    packet[2] = request.chaddr.len() as u8;
    packet[4..8].copy_from_slice(&request.xid);
    packet[10..12].copy_from_slice(&request.flags);
    packet[16..20].copy_from_slice(&offered_ip.octets());
    packet[20..24].copy_from_slice(&SERVER_IP.octets());
    packet[28..28 + request.chaddr.len()].copy_from_slice(&request.chaddr);
    packet.extend_from_slice(&MAGIC_COOKIE);
    push_option(&mut packet, 53, &[message_type as u8]);
    push_option(&mut packet, 54, &SERVER_IP.octets());

    if reply != Reply::Nak {
        push_option(&mut packet, 51, &LEASE_SECONDS.to_be_bytes());
        push_option(&mut packet, 1, &Ipv4Addr::new(255, 255, 255, 0).octets());
        push_option(&mut packet, 3, &SERVER_IP.octets());
        push_option(&mut packet, 6, &SERVER_IP.octets());
        push_option(&mut packet, 28, &Ipv4Addr::new(192, 168, 2, 255).octets());
    }
    packet.push(255);
    packet
}

#[cfg(target_os = "linux")]
fn bind_to_device(socket: &StdUdpSocket, interface: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;

    let interface = CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface"))?;
    // SAFETY: the fd is owned by `socket`, the option points to a live
    // NUL-terminated CString, and its byte count includes that terminator.
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

pub async fn serve(mut shutdown: watch::Receiver<bool>) -> io::Result<()> {
    let socket = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 67))?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    bind_to_device(&socket, IFACE)?;
    let socket = UdpSocket::from_std(socket)?;
    let mut leases = LeasePool::default();
    let mut packet = vec![0u8; 1500];

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            received = socket.recv_from(&mut packet) => {
                let (length, peer) = received?;
                let Some(request) = parse_request(&packet[..length]) else {
                    debug!("ignored malformed DHCP packet from {peer}");
                    continue;
                };
                let Some(reply) = leases.respond(&request) else {
                    continue;
                };
                let response = build_reply(&request, reply);
                if let Err(error) = socket
                    .send_to(&response, (Ipv4Addr::BROADCAST, 68))
                    .await
                {
                    warn!("DHCP reply to {peer} failed: {error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_packet(message_type: MessageType, requested: Option<Ipv4Addr>) -> Vec<u8> {
        let mut packet = vec![0u8; BOOTP_FIXED_LEN];
        packet[0] = 1;
        packet[1] = 1;
        packet[2] = 6;
        packet[4..8].copy_from_slice(&[1, 2, 3, 4]);
        packet[10..12].copy_from_slice(&[0x80, 0]);
        packet[28..34].copy_from_slice(&[0x02, 0, 0, 0, 0, 1]);
        packet.extend_from_slice(&MAGIC_COOKIE);
        push_option(&mut packet, 53, &[message_type as u8]);
        if let Some(ip) = requested {
            push_option(&mut packet, 50, &ip.octets());
        }
        packet.push(255);
        packet
    }

    fn option_value(packet: &[u8], wanted: u8) -> Option<Vec<u8>> {
        let mut cursor = DHCP_HEADER_LEN;
        while cursor < packet.len() {
            let code = packet[cursor];
            cursor += 1;
            if code == 255 {
                return None;
            }
            if code == 0 {
                continue;
            }
            let len = *packet.get(cursor)? as usize;
            cursor += 1;
            let value = packet.get(cursor..cursor + len)?;
            cursor += len;
            if code == wanted {
                return Some(value.to_vec());
            }
        }
        None
    }

    #[test]
    fn rejects_truncated_and_bad_cookie_packets() {
        assert!(parse_request(&[0u8; 20]).is_none());
        let mut packet = request_packet(MessageType::Discover, None);
        packet[236] = 0;
        assert!(parse_request(&packet).is_none());
    }

    #[test]
    fn discover_gets_stable_offer_inside_pool() {
        let request = parse_request(&request_packet(MessageType::Discover, None)).unwrap();
        let mut pool = LeasePool::default();
        let first = pool.respond(&request).unwrap();
        let second = pool.respond(&request).unwrap();
        assert_eq!(first, second);
        let Reply::Offer(ip) = first else {
            panic!("expected offer")
        };
        assert!(in_pool(ip));

        let packet = build_reply(&request, first);
        assert_eq!(packet[0], 2);
        assert_eq!(
            option_value(&packet, 53),
            Some(vec![MessageType::Offer as u8])
        );
        assert_eq!(option_value(&packet, 54), Some(SERVER_IP.octets().to_vec()));
    }

    #[test]
    fn valid_request_gets_ack() {
        let ip = pool_ip(77);
        let request = parse_request(&request_packet(MessageType::Request, Some(ip))).unwrap();
        let mut pool = LeasePool::default();
        assert_eq!(pool.respond(&request), Some(Reply::Ack(ip)));
        let packet = build_reply(&request, Reply::Ack(ip));
        assert_eq!(packet[16..20], ip.octets());
        assert_eq!(
            option_value(&packet, 53),
            Some(vec![MessageType::Ack as u8])
        );
    }

    #[test]
    fn out_of_pool_request_gets_nak() {
        let request = parse_request(&request_packet(
            MessageType::Request,
            Some(Ipv4Addr::new(192, 168, 2, 10)),
        ))
        .unwrap();
        let mut pool = LeasePool::default();
        assert_eq!(pool.respond(&request), Some(Reply::Nak));
        let packet = build_reply(&request, Reply::Nak);
        assert_eq!(packet[16..20], [0, 0, 0, 0]);
        assert_eq!(
            option_value(&packet, 53),
            Some(vec![MessageType::Nak as u8])
        );
        assert!(option_value(&packet, 51).is_none());
    }

    #[test]
    fn pool_never_allocates_outside_bounds() {
        let mut pool = LeasePool::default();
        for client in 0u16..=(POOL_LAST - POOL_FIRST) as u16 {
            let ip = pool.offer(&client.to_be_bytes()).unwrap();
            assert!(in_pool(ip));
        }
        assert!(pool.offer(b"one-too-many").is_none());
    }
}
