use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

const NTP_PACKET_BYTES: usize = 48;
const NTP_UNIX_EPOCH_DELTA: u64 = 2_208_988_800;

pub async fn synchronize(server: &str) -> Result<(), NtpError> {
    let mut request = [0_u8; NTP_PACKET_BYTES];
    request[0] = 0x23; // no leap warning, NTP v4, client mode
    let timestamp = ntp_timestamp(SystemTime::now())?;
    request[40..48].copy_from_slice(&timestamp);

    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let mut addresses = tokio::net::lookup_host((server, 123)).await?;
    let address = addresses
        .next()
        .ok_or_else(|| NtpError::Protocol("server resolved to no addresses".into()))?;
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(4), socket.send_to(&request, address))
        .await
        .map_err(|_| NtpError::Timeout)??;

    let mut response = [0_u8; 512];
    let (length, source) =
        tokio::time::timeout(Duration::from_secs(4), socket.recv_from(&mut response))
            .await
            .map_err(|_| NtpError::Timeout)??;
    if source.ip() != address.ip() {
        return Err(NtpError::Protocol(
            "response did not originate from the selected server".into(),
        ));
    }
    let mut target = parse_response(&response[..length], &timestamp)?;
    target = target
        .checked_add(started.elapsed() / 2)
        .ok_or_else(|| NtpError::Protocol("server timestamp overflow".into()))?;
    set_system_time(target)
}

fn parse_response(packet: &[u8], request_transmit: &[u8; 8]) -> Result<Duration, NtpError> {
    if packet.len() < NTP_PACKET_BYTES {
        return Err(NtpError::Protocol(
            "response is shorter than 48 bytes".into(),
        ));
    }
    let leap = packet[0] >> 6;
    let version = (packet[0] >> 3) & 0x07;
    let mode = packet[0] & 0x07;
    if leap == 3 || version < 3 || mode != 4 {
        return Err(NtpError::Protocol(
            "response has invalid leap, version, or mode fields".into(),
        ));
    }
    if !(1..=15).contains(&packet[1]) {
        return Err(NtpError::Protocol("response has invalid stratum".into()));
    }
    if packet[24..32] != request_transmit[..] {
        return Err(NtpError::Protocol(
            "response origin timestamp does not match request".into(),
        ));
    }
    unix_duration(&packet[40..48])
}

fn ntp_timestamp(time: SystemTime) -> Result<[u8; 8], NtpError> {
    let unix = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| NtpError::Protocol("system clock predates Unix epoch".into()))?;
    let seconds = unix
        .as_secs()
        .checked_add(NTP_UNIX_EPOCH_DELTA)
        .ok_or_else(|| NtpError::Protocol("timestamp overflow".into()))?;
    let fraction = ((u64::from(unix.subsec_nanos())) << 32) / 1_000_000_000;
    let mut timestamp = [0_u8; 8];
    timestamp[..4].copy_from_slice(&(seconds as u32).to_be_bytes());
    timestamp[4..].copy_from_slice(&(fraction as u32).to_be_bytes());
    Ok(timestamp)
}

fn unix_duration(timestamp: &[u8]) -> Result<Duration, NtpError> {
    let seconds = u64::from(u32::from_be_bytes(timestamp[..4].try_into().unwrap()));
    if seconds < NTP_UNIX_EPOCH_DELTA {
        return Err(NtpError::Protocol(
            "server timestamp predates Unix epoch".into(),
        ));
    }
    let fraction = u64::from(u32::from_be_bytes(timestamp[4..8].try_into().unwrap()));
    let nanos = fraction.saturating_mul(1_000_000_000) >> 32;
    Ok(Duration::new(seconds - NTP_UNIX_EPOCH_DELTA, nanos as u32))
}

fn set_system_time(duration: Duration) -> Result<(), NtpError> {
    let value = libc::timespec {
        tv_sec: duration
            .as_secs()
            .try_into()
            .map_err(|_| NtpError::Protocol("server timestamp does not fit system clock".into()))?,
        tv_nsec: duration.subsec_nanos().into(),
    };
    // SAFETY: `value` is fully initialized and points to a valid timespec.
    let result = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &value) };
    if result == 0 {
        Ok(())
    } else {
        Err(NtpError::Clock(std::io::Error::last_os_error()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NtpError {
    #[error("NTP IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("NTP request timed out")]
    Timeout,
    #[error("invalid NTP response: {0}")]
    Protocol(String),
    #[error("cannot set system clock: {0}")]
    Clock(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_server_response_and_rejects_wrong_origin() {
        let origin = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut packet = [0_u8; 48];
        packet[0] = 0x24;
        packet[1] = 2;
        packet[24..32].copy_from_slice(&origin);
        let timestamp = ntp_timestamp(UNIX_EPOCH + Duration::from_secs(1_700_000_000)).unwrap();
        packet[40..48].copy_from_slice(&timestamp);
        assert_eq!(
            parse_response(&packet, &origin).unwrap().as_secs(),
            1_700_000_000
        );
        assert!(parse_response(&packet, &[0; 8]).is_err());
    }

    #[test]
    fn rejects_unsynchronized_or_client_mode_response() {
        let mut packet = [0_u8; 48];
        packet[0] = 0xe3;
        packet[1] = 0;
        assert!(parse_response(&packet, &[0; 8]).is_err());
    }
}
