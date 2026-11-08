//! Bounded serial reads keep link commands responsive when USB stops sending.
use mavlink::dialects::ardupilotmega::MavMessage;
use mavlink::error::{MessageReadError, MessageWriteError};
use mavlink::{write_versioned_msg, MavConnection, MavHeader, MavlinkReader, MavlinkVersion};
use std::io::{self, Read};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const READ_TIMEOUT: Duration = Duration::from_millis(100);

fn receive<R: Read>(
    reader: &mut MavlinkReader<R>,
    version: MavlinkVersion,
) -> Result<(MavHeader, MavMessage), MessageReadError> {
    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        match reader.read_message::<MavMessage>(version) {
            Ok(message) => return Ok(message),
            Err(error @ MessageReadError::Io(_)) => return Err(error),
            Err(_) if Instant::now() >= deadline => {
                return Err(
                    io::Error::new(io::ErrorKind::TimedOut, "Serial decode deadline").into(),
                )
            }
            Err(_) => {}
        }
    }
}

pub fn connect(url: &str) -> io::Result<Box<dyn MavConnection<MavMessage> + Send + Sync>> {
    let (name, baud) = url[7..]
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Expected serial:port:baud"))?;
    let baud = baud
        .parse::<u32>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let port = serialport::new(name, baud).timeout(READ_TIMEOUT).open()?;
    let writer = port.try_clone()?;
    Ok(Box::new(Connection {
        port: Mutex::new(MavlinkReader::new(port)),
        writer: Mutex::new(writer),
        sequence: Mutex::new(0),
        version: MavlinkVersion::V2,
    }))
}

struct Connection {
    port: Mutex<MavlinkReader<Box<dyn serialport::SerialPort>>>,
    writer: Mutex<Box<dyn serialport::SerialPort>>,
    sequence: Mutex<u8>,
    version: MavlinkVersion,
}
impl MavConnection<MavMessage> for Connection {
    fn recv(&self) -> Result<(MavHeader, MavMessage), MessageReadError> {
        receive(&mut *self.port.lock().unwrap(), self.version)
    }
    fn send(&self, header: &MavHeader, message: &MavMessage) -> Result<usize, MessageWriteError> {
        let mut port = self.writer.lock().unwrap();
        let mut sequence = self.sequence.lock().unwrap();
        let header = MavHeader {
            sequence: *sequence,
            ..*header
        };
        *sequence = sequence.wrapping_add(1);
        write_versioned_msg(&mut *port, self.version, header, message)
    }
    fn set_protocol_version(&mut self, version: MavlinkVersion) {
        self.version = version;
    }
    fn protocol_version(&self) -> MavlinkVersion {
        self.version
    }
    fn recv_raw(&self) -> Result<mavlink::MAVLinkMessageRaw, MessageReadError> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "raw frames are not kept").into())
    }
    fn try_recv(&self) -> Result<(MavHeader, MavMessage), MessageReadError> {
        self.recv()
    }
    fn send_raw(&self, _: &mavlink::MAVLinkMessageRaw) -> Result<usize, MessageWriteError> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "raw frames are not sent").into())
    }
    /// Both versions are always decoded (MAVLink 2 first, then 1).
    fn set_allow_recv_any_version(&mut self, _: bool) {}
    fn allow_recv_any_version(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Failing(io::ErrorKind);
    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "test"))
        }
    }
    #[test]
    fn idle_and_removed_ports_return_control() {
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::BrokenPipe] {
            let mut reader = MavlinkReader::new(Failing(kind));
            assert!(
                matches!(receive(&mut reader, MavlinkVersion::V2), Err(MessageReadError::Io(e)) if e.kind() == kind)
            );
        }
    }
}
