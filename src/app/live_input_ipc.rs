use std::io::ErrorKind;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use super::{LiveInputEvent, LiveInputEventSource};

pub const LIVE_INPUT_IPC_SOCKET_ENV: &str = "SONANT_LIVE_INPUT_SOCKET_PATH";

const LIVE_INPUT_IPC_PACKET_SIZE: usize = 9;

pub struct LiveInputIpcSender {
    socket: UnixDatagram,
    target_path: PathBuf,
}

impl LiveInputIpcSender {
    pub fn new(target_path: impl AsRef<Path>) -> std::io::Result<Self> {
        let socket = UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            target_path: target_path.as_ref().to_path_buf(),
        })
    }

    pub fn send_event(&self, event: LiveInputEvent) {
        let payload = encode_live_input_event(event);
        let _ = self.socket.send_to(&payload, &self.target_path);
    }

    pub fn send_events(&self, events: &[LiveInputEvent]) {
        for event in events {
            self.send_event(*event);
        }
    }
}

pub struct LiveInputIpcSource {
    socket: UnixDatagram,
    socket_path: PathBuf,
}

impl LiveInputIpcSource {
    pub fn bind(socket_path: impl AsRef<Path>) -> std::io::Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }
        let socket = UnixDatagram::bind(&socket_path)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            socket_path,
        })
    }
}

impl LiveInputEventSource for LiveInputIpcSource {
    fn try_pop_live_input_event(&self) -> Option<LiveInputEvent> {
        let mut payload = [0u8; LIVE_INPUT_IPC_PACKET_SIZE];
        let size = match self.socket.recv(&mut payload) {
            Ok(size) => size,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return None,
            Err(_) => return None,
        };
        decode_live_input_event(&payload[..size])
    }
}

impl Drop for LiveInputIpcSource {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn encode_live_input_event(event: LiveInputEvent) -> [u8; LIVE_INPUT_IPC_PACKET_SIZE] {
    let mut payload = [0u8; LIVE_INPUT_IPC_PACKET_SIZE];
    payload[..4].copy_from_slice(&event.time.to_le_bytes());
    payload[4..6].copy_from_slice(&event.port_index.to_le_bytes());
    payload[6..9].copy_from_slice(&event.data);
    payload
}

fn decode_live_input_event(payload: &[u8]) -> Option<LiveInputEvent> {
    if payload.len() != LIVE_INPUT_IPC_PACKET_SIZE {
        return None;
    }
    let mut time_bytes = [0u8; 4];
    let mut port_index_bytes = [0u8; 2];
    time_bytes.copy_from_slice(&payload[..4]);
    port_index_bytes.copy_from_slice(&payload[4..6]);
    Some(LiveInputEvent {
        time: u32::from_le_bytes(time_bytes),
        port_index: u16::from_le_bytes(port_index_bytes),
        data: [payload[6], payload[7], payload[8]],
    })
}

#[cfg(test)]
mod tests {
    use super::{LiveInputIpcSender, LiveInputIpcSource};
    use crate::app::{LiveInputEvent, LiveInputEventSource};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sender_to_source_round_trip_delivers_event() {
        let socket_path = unique_test_socket_path();
        let source = LiveInputIpcSource::bind(&socket_path).expect("bind should succeed");
        let sender = LiveInputIpcSender::new(&socket_path).expect("sender should initialize");
        let event = LiveInputEvent {
            time: 42,
            port_index: 7,
            data: [0x91, 64, 127],
        };

        sender.send_event(event);

        let received = source.try_pop_live_input_event();
        assert_eq!(received, Some(event));
        assert_eq!(source.try_pop_live_input_event(), None);
    }

    #[test]
    fn source_ignores_empty_queue_without_blocking() {
        let socket_path = unique_test_socket_path();
        let source = LiveInputIpcSource::bind(&socket_path).expect("bind should succeed");
        assert_eq!(source.try_pop_live_input_event(), None);
    }

    fn unique_test_socket_path() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        PathBuf::from(format!(
            "/tmp/sonant-live-input-ipc-test-{}-{nonce}.sock",
            std::process::id()
        ))
    }
}
