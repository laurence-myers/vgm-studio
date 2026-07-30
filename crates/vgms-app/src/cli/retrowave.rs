//! `vgmstudio retrowave-probe`: find RetroWave hardware and prove it makes sound.
//!
//! The first thing to run when hardware output misbehaves. Listing needs no
//! device; the chord needs one, and tells you whether the fault is in the
//! framing, the port, or the speakers.

use std::{thread::sleep, time::Duration};

use anyhow::{Context, Result};
use vgms_retrowave::{Bank, Device, PortInfo, device, test_tone};

/// How long each chord rings before the next step.
const CHORD_HOLD: Duration = Duration::from_millis(1500);

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The serial port to test, such as COM3 or /dev/ttyACM0. Defaults to the
    /// first port that looks like RetroWave hardware.
    #[arg(short = 'p', long = "port")]
    pub port: Option<String>,
    /// List the serial ports and stop, without opening anything.
    #[arg(short = 'l', long = "list")]
    pub list_only: bool,
}

/// Lists ports, then (unless `--list`) plays a test chord on one.
///
/// # Errors
/// If the ports cannot be listed, no device is found, or the chosen port cannot
/// be opened or written to.
pub fn run(args: &Args) -> Result<()> {
    let ports = device::enumerate().context("listing serial ports")?;
    print_ports(&ports);

    if args.list_only {
        return Ok(());
    }

    let port_name = match &args.port {
        Some(name) => name.clone(),
        None => {
            let found = device::default_port().context(
                "no RetroWave device found. Pass --port to name one, and check that an \
                 original board's mode switch is set to USB",
            )?;
            println!("\nUsing {} (auto-detected).", found.label);
            found.port_name
        }
    };

    println!("Opening {port_name}...");
    let mut device = Device::open(&port_name).with_context(|| format!("opening {port_name}"))?;
    println!("Open. Initialised, reset and muted.");

    test_tone::enable_opl3(&mut device).context("enabling OPL3 mode")?;

    for (bank, description) in [
        (Bank::Zero, "bank 0 (channels 1-9)"),
        (Bank::One, "bank 1 (channels 10-18, the OPL3 extension)"),
    ] {
        println!("Playing a chord on {description}...");
        test_tone::key_on_chord(&mut device, bank).context("starting the chord")?;
        sleep(CHORD_HOLD);
        test_tone::key_off_chord(&mut device, bank).context("stopping the chord")?;
        sleep(Duration::from_millis(250));
    }

    device.mute().context("silencing the chip")?;
    println!("Done. Both banks played, chip silenced.");
    println!("Heard nothing? Check the 3.5mm output and the board's volume.");
    Ok(())
}

fn print_ports(ports: &[PortInfo]) {
    if ports.is_empty() {
        println!("No serial ports found.");
        return;
    }

    println!("Serial ports:");
    for port in ports {
        let marker = if port.looks_like_retrowave {
            " <- looks like RetroWave hardware"
        } else {
            ""
        };
        println!("  {}{marker}", port.port_name);

        if let Some(usb) = &port.usb {
            println!("      USB {:04x}:{:04x}", usb.vid, usb.pid);
            for (field, value) in [
                ("manufacturer", &usb.manufacturer),
                ("product", &usb.product),
                ("serial", &usb.serial_number),
            ] {
                if let Some(value) = value {
                    println!("      {field}: {value}");
                }
            }
        } else {
            println!("      (not a USB device)");
        }
    }
}
