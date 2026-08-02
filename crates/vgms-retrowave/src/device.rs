//! Finding and talking to a RetroWave board over its USB CDC serial port.

use std::{fmt, io, time::Duration};

use crate::{
    commands,
    protocol::{Bank, CmdBuffer},
};

/// USB IDs known to be RetroWave hardware.
///
/// Both board generations are built around Microchip MCUs. This list is a
/// convenience for picking a sensible default port, never a gate: unknown
/// devices are still listed, because a board revision we have not seen must
/// remain selectable by hand.
const KNOWN_USB_IDS: &[(u16, u16)] = &[
    // Original RetroWave OPL3 (verified against real hardware).
    (0x04D8, 0xE966),
];

/// Serial parameters. The baud rate is a formality — this is a USB CDC device,
/// where the host-side rate has no effect on the wire — but a rate must be named.
const BAUD_RATE: u32 = 115_200;

/// Write timeout. Generous: only a wedged device should ever hit it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// What went wrong talking to the hardware.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not open serial port {port}: {source}")]
    Open {
        port: String,
        #[source]
        source: serialport::Error,
    },
    #[error("could not list serial ports: {0}")]
    Enumerate(#[source] serialport::Error),
    #[error("lost contact with the RetroWave device: {0}")]
    Write(#[source] io::Error),
    #[error("no RetroWave device found; connect one, or name a port explicitly")]
    NotFound,
}

/// A serial port a [`Device`] can drive, behind a trait so tests need no hardware.
pub trait SerialIo: Send + fmt::Debug {
    /// Writes every byte, or fails.
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), io::Error>;

    /// Pushes buffered bytes out to the device.
    fn flush(&mut self) -> Result<(), io::Error>;
}

/// The real thing: a `serialport` handle.
struct SerialPortIo(Box<dyn serialport::SerialPort>);

impl fmt::Debug for SerialPortIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialPortIo")
            .field("port", &self.0.name())
            .finish_non_exhaustive()
    }
}

impl SerialIo for SerialPortIo {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        io::Write::write_all(&mut self.0, bytes)
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        io::Write::flush(&mut self.0)
    }
}

/// What the USB layer says about a port, when it is a USB device at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbInfo {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

/// A serial port that might be a RetroWave board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// What to pass to [`Device::open`] — `COM3`, `/dev/ttyACM0`.
    pub port_name: String,
    /// Display text: the port name, plus a USB product string when the platform
    /// offers a useful one.
    pub label: String,
    /// Whether this looks like RetroWave hardware, by USB ID or product string.
    pub looks_like_retrowave: bool,
    /// The raw USB descriptors, for diagnostics.
    pub usb: Option<UsbInfo>,
}

/// Lists the serial ports on this machine, RetroWave-looking ones included.
///
/// Opens nothing, so the settings UI can populate a picker while another audio
/// backend is live.
///
/// Identification is by USB vendor/product ID first. Product strings are a weak
/// signal on Windows, where a CDC device bound to the in-box `usbser.sys` driver
/// reports a generic "USB Serial Device" rather than anything the board chose.
pub fn enumerate() -> Result<Vec<PortInfo>, Error> {
    let ports = serialport::available_ports().map_err(Error::Enumerate)?;

    Ok(ports
        .into_iter()
        .map(|port| {
            let usb = match &port.port_type {
                serialport::SerialPortType::UsbPort(usb) => Some(UsbInfo {
                    vid: usb.vid,
                    pid: usb.pid,
                    manufacturer: usb.manufacturer.clone(),
                    product: usb.product.clone(),
                    serial_number: usb.serial_number.clone(),
                }),
                _ => None,
            };

            let descriptor = usb.as_ref().and_then(|usb| {
                usb.product
                    .as_deref()
                    .filter(|product| !is_generic_description(product))
                    .map(str::to_owned)
            });
            let known_id = usb
                .as_ref()
                .is_some_and(|usb| KNOWN_USB_IDS.contains(&(usb.vid, usb.pid)));

            let named_retrowave = descriptor
                .as_deref()
                .is_some_and(|text| text.to_ascii_lowercase().contains("retrowave"));

            let label = match &descriptor {
                Some(text) => format!("{} — {text}", port.port_name),
                None => port.port_name.clone(),
            };

            PortInfo {
                port_name: port.port_name,
                label,
                looks_like_retrowave: known_id || named_retrowave,
                usb,
            }
        })
        .collect())
}

/// Whether a USB product string is the platform's placeholder rather than the
/// device's own name.
fn is_generic_description(product: &str) -> bool {
    let lowered = product.to_ascii_lowercase();
    lowered.starts_with("usb serial device") || lowered.starts_with("usb-serial")
}

/// The port [`enumerate`] would pick on its own: the first that looks like
/// RetroWave hardware.
pub fn default_port() -> Result<PortInfo, Error> {
    enumerate()?
        .into_iter()
        .find(|port| port.looks_like_retrowave)
        .ok_or(Error::NotFound)
}

/// An opened RetroWave board, initialised and silent.
///
/// Owns no chip state: what the registers hold is the caller's model to keep
/// (see `SerialOpl3Chip`). This layer only moves bytes and runs the fixed
/// sequences.
#[derive(Debug)]
pub struct Device {
    io: Box<dyn SerialIo>,
    /// Scratch for this layer's own command sequences, reused across calls.
    buf: CmdBuffer,
}

impl Device {
    /// Opens `port_name` and brings the board up: expander init, chip reset, and
    /// a mute sweep, so it is silent whatever state it was left in.
    pub fn open(port_name: &str) -> Result<Self, Error> {
        let port = serialport::new(port_name, BAUD_RATE)
            .timeout(WRITE_TIMEOUT)
            .open()
            .map_err(|source| Error::Open {
                port: port_name.to_owned(),
                source,
            })?;

        // Not every CDC stack gates writes on DTR, but some do, and asserting it
        // is harmless on the ones that do not.
        let mut port = port;
        let _ = port.write_data_terminal_ready(true);

        Self::with_io(Box::new(SerialPortIo(port)))
    }

    /// Drives an arbitrary [`SerialIo`], running the same bring-up. For tests.
    pub fn with_io(io: Box<dyn SerialIo>) -> Result<Self, Error> {
        let mut device = Self {
            io,
            buf: CmdBuffer::new(),
        };
        device.initialise()?;
        Ok(device)
    }

    fn initialise(&mut self) -> Result<(), Error> {
        commands::queue_io_init(&mut self.buf);
        self.flush_buf()?;

        self.reset_chip()?;
        self.mute()
    }

    /// Hard-resets the YMF262 and waits for it to settle.
    ///
    /// Costly (see [`commands::RESET_SETTLE`]), so this belongs to opening and
    /// closing a port, not to seeking or loading a song.
    pub fn reset_chip(&mut self) -> Result<(), Error> {
        commands::queue_chip_reset(&mut self.buf);
        self.flush_buf()?;
        std::thread::sleep(commands::RESET_SETTLE);
        Ok(())
    }

    /// Silences the chip by sweeping its registers.
    ///
    /// Leaves the register file clobbered; anyone modelling the hardware should
    /// sweep through their own model instead so the two stay in step.
    pub fn mute(&mut self) -> Result<(), Error> {
        commands::queue_mute_sweep(&mut self.buf, |_, _, _| {});
        self.flush_buf()
    }

    /// Writes one register. Convenience for probing — playback goes through
    /// [`Self::send`] in bulk.
    pub fn write_reg(&mut self, bank: Bank, reg: u8, value: u8) -> Result<(), Error> {
        self.buf.push_write(bank, reg, value);
        self.flush_buf()
    }

    /// Sends already-packed wire bytes, as produced by a chip's command buffer.
    pub fn send(&mut self, wire: &[u8]) -> Result<(), Error> {
        if wire.is_empty() {
            return Ok(());
        }
        self.io.write_all(wire).map_err(Error::Write)?;
        self.io.flush().map_err(Error::Write)
    }

    fn flush_buf(&mut self) -> Result<(), Error> {
        self.buf.seal();
        // Take the bytes out of the buffer first: `send` borrows self mutably.
        let wire = std::mem::take(&mut self.buf);
        let result = self.send(wire.wire());
        self.buf = wire;
        self.buf.clear_wire();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records everything written, so tests can assert on the wire bytes.
    #[derive(Debug, Default, Clone)]
    struct MockIo {
        written: Arc<Mutex<Vec<u8>>>,
    }

    impl SerialIo for MockIo {
        fn write_all(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
            self.written.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), io::Error> {
            Ok(())
        }
    }

    /// Fails every write, standing in for an unplugged device.
    #[derive(Debug)]
    struct DeadIo;

    impl SerialIo for DeadIo {
        fn write_all(&mut self, _bytes: &[u8]) -> Result<(), io::Error> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "device gone"))
        }

        fn flush(&mut self) -> Result<(), io::Error> {
            Ok(())
        }
    }

    fn opened() -> (Device, Arc<Mutex<Vec<u8>>>) {
        let io = MockIo::default();
        let written = Arc::clone(&io.written);
        let device = Device::with_io(Box::new(io)).expect("bring-up succeeds");
        (device, written)
    }

    #[test]
    fn opening_a_device_initialises_resets_and_mutes_it() {
        let (_device, written) = opened();
        let wire = written.lock().unwrap();

        // The resynchronising empty transaction leads.
        assert_eq!(&wire[..4], [0x00, 0x01, 0x01, 0x02]);
        // Init, reset and sweep together run to a few KiB.
        assert!(
            wire.len() > 1000,
            "bring-up looks too short: {}",
            wire.len()
        );
    }

    #[test]
    fn a_dead_port_surfaces_its_error_rather_than_pretending() {
        let error = Device::with_io(Box::new(DeadIo))
            .expect_err("bring-up must fail on a dead port");
        assert!(matches!(error, Error::Write(_)));
    }

    #[test]
    fn sending_nothing_touches_the_port_not_at_all() {
        let (mut device, written) = opened();
        let before = written.lock().unwrap().len();
        device.send(&[]).expect("empty send succeeds");
        assert_eq!(written.lock().unwrap().len(), before);
    }

    #[test]
    fn a_register_write_reaches_the_wire() {
        let (mut device, written) = opened();
        written.lock().unwrap().clear();

        device.write_reg(Bank::Zero, 0x20, 0x01).expect("write");

        let mut expected = Vec::new();
        let mut buf = CmdBuffer::new();
        buf.push_write(Bank::Zero, 0x20, 0x01);
        buf.seal();
        expected.extend_from_slice(buf.wire());

        assert_eq!(*written.lock().unwrap(), expected);
    }

    #[test]
    fn the_command_buffer_is_reused_rather_than_leaking_bytes() {
        let (mut device, _written) = opened();
        device.write_reg(Bank::Zero, 0x20, 0x01).expect("write");
        device.write_reg(Bank::Zero, 0x21, 0x02).expect("write");
        assert!(
            device.buf.is_empty(),
            "the scratch buffer should be empty between commands"
        );
    }

    #[test]
    fn a_generic_windows_description_is_not_treated_as_a_device_name() {
        assert!(is_generic_description("USB Serial Device (COM3)"));
        assert!(is_generic_description("usb-serial CH340"));
        assert!(!is_generic_description("RetroWave OPL3 Express"));
    }
}
