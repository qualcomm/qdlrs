// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anstream::println;
use anyhow::Result;
use indexmap::IndexMap;
use owo_colors::OwoColorize;
use parsers::firehose_parser_ack_nak;
use serial::setup_serial_device;
use std::cmp::min;
use std::io::{Read, Write};
use std::str::{self, FromStr};
use types::FirehoseResetMode;
use types::FirehoseStatus;
use types::FirehoseStorageType;
use types::QdlBackend;
use types::QdlChan;
use types::QdlReadWrite;
use usb::setup_usb_device;

use anyhow::bail;
use pbr::{ProgressBar, Units};
use xmltree::{self, Element, XMLNode};

pub mod parsers;
pub mod sahara;
#[cfg(feature = "serial")]
pub mod serial;
pub mod types;
#[cfg(feature = "usb")]
pub mod usb;

pub fn setup_target_device(
    backend: QdlBackend,
    serial_no: Option<String>,
    port: Option<String>,
) -> Result<Box<dyn QdlReadWrite>> {
    match backend {
        QdlBackend::Serial => match setup_serial_device(port) {
            Ok(d) => Ok(Box::new(d)),
            Err(e) => Err(e),
        },
        QdlBackend::Usb => match setup_usb_device(serial_no) {
            Ok(d) => Ok(Box::new(d)),
            Err(e) => Err(e),
        },
    }
}

/// Wrapper for easily creating Firehose-y XML packets
fn firehose_xml_setup(op: &str, kvps: &[(&str, &str)]) -> anyhow::Result<Vec<u8>> {
    let mut xml = Element::new("data");
    let mut op_node = Element::new(op);
    for kvp in kvps.iter() {
        op_node
            .attributes
            .insert(kvp.0.to_owned(), kvp.1.to_owned());
    }

    xml.children.push(XMLNode::Element(op_node));

    // TODO: define a more verbose level
    // println!("SEND: {}", format!("{:?}", xml).bright_cyan());

    let mut buf = Vec::<u8>::new();
    xml.write(&mut buf)?;

    Ok(buf)
}

/// Main Firehose XML reading function
pub fn firehose_read<T: QdlChan>(
    channel: &mut T,
    response_parser: fn(&mut T, &IndexMap<String, String>) -> Result<FirehoseStatus, anyhow::Error>,
) -> Result<FirehoseStatus, anyhow::Error> {
    let mut got_any_data = false;
    let mut pending: Vec<u8> = Vec::new();

    loop {
        // Use BufRead to peek at available data
        let available = match channel.fill_buf() {
            Ok(buf) => buf,
            Err(e) => match e.kind() {
                // In some cases (like with welcome messages), there's no acking
                // and a timeout is the "end of data" marker instead..
                std::io::ErrorKind::TimedOut => {
                    if got_any_data {
                        return Ok(FirehoseStatus::Ack);
                    } else {
                        return Err(e.into());
                    }
                }
                _ => return Err(e.into()),
            },
        };

        got_any_data = true;

        // When channel is a non-packetized BufRead (e.g. serial) XML documents
        // are not separated from each other, or from rawmode data. Search for
        // </data> in the BufRead stream to find the end of the current
        // message.
        let data_end_marker = b"</data>";

        let pending_length = pending.len();
        pending.extend_from_slice(available);

        // Search for the end marker in the pending data
        let end_pos = pending
            .windows(data_end_marker.len())
            .position(|window| window == data_end_marker);

        if let Some(pos) = end_pos {
            let xml_end = pos + data_end_marker.len();

            // xml_end is relative "pending", we need to consume only new the tail
            channel.consume(xml_end - pending_length);

            // Only parse the XML portion
            let xml_chunk = &pending[..xml_end];
            let xml = match xmltree::Element::parse(xml_chunk) {
                Ok(x) => x,
                Err(e) => {
                    // Consume the bad data and continue
                    bail!("Failed to parse XML: {}", e);
                }
            };

            // The current message might have started in "pending", so clear it
            // now. No need to do this if we're bailing above, as it's a local
            // resource.
            pending.clear();

            if xml.name != "data" {
                // TODO: define a more verbose level
                if channel.fh_config().verbose_firehose {
                    println!("{:?}", xml);
                }
                bail!("Got a firehose packet without a data tag");
            }

            // The spec expects there's always a single node only
            if let Some(XMLNode::Element(e)) = xml.children.first() {
                // Check for a 'log' node and print out the message
                if e.name == "log" {
                    if channel.fh_config().skip_firehose_log {
                        continue;
                    }

                    println!(
                        "LOG: {}",
                        e.attributes
                            .get("value")
                            .to_owned()
                            .unwrap_or(&String::from("<garbage log data>"))
                            .bright_black()
                    );

                    continue;
                }

                // DEBUG: "print out incoming packets"
                // TODO: define a more verbose level
                if channel.fh_config().verbose_firehose {
                    println!("RECV: {}", format!("{e:?}").magenta());
                }

                // TODO: Use std::intrinsics::unlikely after it exits nightly
                if e.attributes.get("AttemptRetry").is_some() {
                    return firehose_read::<T>(channel, response_parser);
                } else if e.attributes.get("AttemptRestart").is_some() {
                    // TODO: handle this automagically
                    firehose_reset(channel, &FirehoseResetMode::ResetToEdl, 0)?;
                    bail!("Firehose requested a restart. Run the program again.");
                }

                // Pass other nodes to specialized parsers
                return response_parser(channel, &e.attributes);
            }
        } else {
            // Didn't find the tail of the XML document in "pending" +
            // "available", consume the data into "pending" to let fill_buf()
            // read more data from the underlying Read.
            let available_len = available.len();
            channel.consume(available_len);
        }
    }
}

/// Send a Firehose packet
pub fn firehose_write<T: QdlChan>(channel: &mut T, buf: &mut [u8]) -> anyhow::Result<()> {
    let mut b = buf.to_vec();

    // XML can't be n * 512 bytes long by fh spec
    if !buf.is_empty() && buf.len().is_multiple_of(512) {
        println!("{}", "INFO: Appending '\n' to outgoing XML".bright_black());
        b.push(b'\n');
    }

    match channel.write_all(&b) {
        Ok(_) => Ok(()),
        // Assume FH will hang after NAK..
        Err(_) => firehose_reset(channel, &FirehoseResetMode::ResetToEdl, 0),
    }
}

/// Send a Firehose packet and check for ack/nak
pub fn firehose_write_getack<T: QdlChan>(
    channel: &mut T,
    buf: &mut [u8],
    couldnt_what: String,
) -> anyhow::Result<()> {
    firehose_write(channel, buf)?;

    match firehose_read::<T>(channel, firehose_parser_ack_nak) {
        Ok(FirehoseStatus::Ack) => Ok(()),
        Ok(FirehoseStatus::Nak) => {
            // Assume FH will hang after NAK..
            firehose_reset(channel, &FirehoseResetMode::ResetToEdl, 0)?;
            Err(anyhow::Error::msg(format!("Couldn't {couldnt_what}")))
        }
        Err(e) => Err(e),
    }
}

/// Test performance without sample data
pub fn firehose_benchmark<T: QdlChan>(
    channel: &mut T,
    trials: u32,
    test_write_perf: bool,
) -> anyhow::Result<()> {
    let mut xml = firehose_xml_setup(
        "benchmark",
        &[
            ("trials", &trials.to_string()),
            (
                "TestWritePerformance",
                &(test_write_perf as u32).to_string(),
            ),
            (
                "TestReadPerformance",
                &(!test_write_perf as u32).to_string(),
            ),
        ],
    )?;

    firehose_write_getack(channel, &mut xml, "issue a NOP".to_owned())
}

/// Send a "Hello"-type packet to the Device
pub fn firehose_configure<T: QdlChan>(
    channel: &mut T,
    skip_storage_init: bool,
) -> anyhow::Result<()> {
    let config = channel.fh_config();
    // Spec requirement
    assert!(
        config
            .send_buffer_size
            .is_multiple_of(config.storage_sector_size)
    );
    // Sanity requirement
    assert!(
        config
            .send_buffer_size
            .is_multiple_of(config.storage_sector_size)
    );
    let mut xml = firehose_xml_setup(
        "configure",
        &[
            ("AckRawDataEveryNumPackets", "0"), // TODO: (low prio)
            (
                "SkipWrite",
                &(channel.fh_config().bypass_storage as u32).to_string(),
            ),
            ("SkipStorageInit", &(skip_storage_init as u32).to_string()),
            ("MemoryName", &config.storage_type.to_string()),
            ("AlwaysValidate", &(config.hash_packets as u32).to_string()),
            ("Verbose", &(config.verbose_firehose as u32).to_string()),
            ("MaxDigestTableSizeInBytes", "8192"), // TODO: (low prio)
            (
                "MaxPayloadSizeToTargetInBytes",
                &config.send_buffer_size.to_string(),
            ),
            // Zero-length-packet aware host
            ("ZLPAwareHost", "1"),
        ],
    )?;

    firehose_write(channel, &mut xml)
}

/// Do nothing, hopefully succesfully
pub fn firehose_nop<T: QdlChan>(channel: &mut T) -> anyhow::Result<()> {
    let mut xml = firehose_xml_setup("nop", &[("value", "ping")])?;

    firehose_write_getack(channel, &mut xml, "issue a NOP".to_owned())
}

/// Get information about the physical partition of a storage medium (e.g. LUN)
/// Prints to \<log\> only
pub fn firehose_get_storage_info<T: QdlChan>(
    channel: &mut T,
    phys_part_idx: u8,
) -> anyhow::Result<()> {
    let mut xml = firehose_xml_setup(
        "getstorageinfo",
        &[("physical_partition_number", &phys_part_idx.to_string())],
    )?;

    firehose_write(channel, &mut xml)?;

    firehose_read::<T>(channel, firehose_parser_ack_nak).and(Ok(()))
}

/// Alter Device (TODO: or Host) storage
pub fn firehose_patch<T: QdlChan>(
    channel: &mut T,
    byte_off: u64,
    slot: u8,
    phys_part_idx: u8,
    size: u64,
    start_sector: &str,
    val: &str,
) -> anyhow::Result<()> {
    let mut xml: Vec<u8> = firehose_xml_setup(
        "patch",
        &[
            (
                "SECTOR_SIZE_IN_BYTES",
                &channel.fh_config().storage_sector_size.to_string(),
            ),
            ("byte_offset", &byte_off.to_string()),
            ("filename", "DISK"), // DISK means "patch device's storage"
            ("slot", &slot.to_string()),
            ("physical_partition_number", &phys_part_idx.to_string()),
            ("size_in_bytes", &size.to_string()),
            ("start_sector", start_sector),
            ("value", val),
        ],
    )?;

    firehose_write_getack(channel, &mut xml, "patch".to_string())
}

/// Peek at memory
/// Prints to \<log\> only
pub fn firehose_peek<T: QdlChan>(
    channel: &mut T,
    addr: u64,
    byte_count: u64,
) -> anyhow::Result<()> {
    if channel.fh_config().skip_firehose_log {
        println!(
            "{}",
            "Warning: firehose <peek> only prints to <log>, remove --skip-firehose-log"
                .bright_red()
        );
    }

    let mut xml: Vec<u8> = firehose_xml_setup(
        "peek",
        &[
            ("address64", &addr.to_string()),
            ("size_in_bytes", &byte_count.to_string()),
        ],
    )?;

    firehose_write_getack(channel, &mut xml, format!("peek @ {addr:#x}"))
}

/// Poke at memory
/// This can lead to lock-ups and resets
// TODO:x
pub fn firehose_poke<T: QdlChan>(
    channel: &mut T,
    addr: u64,
    // TODO: byte count is 1..=8
    byte_count: u8,
    val: u64,
) -> anyhow::Result<()> {
    let mut xml: Vec<u8> = firehose_xml_setup(
        "poke",
        &[
            ("address64", &addr.to_string()),
            ("size_in_bytes", &byte_count.to_string()),
            ("value", &val.to_string()),
        ],
    )?;

    firehose_write_getack(channel, &mut xml, format!("peek @ {addr:#x}"))
}

/// Write to Device storage
pub fn firehose_program_storage<T: QdlChan>(
    channel: &mut T,
    data: &mut impl Read,
    label: &str,
    num_sectors: usize,
    slot: u8,
    phys_part_idx: u8,
    start_sector: &str,
) -> anyhow::Result<()> {
    let mut sectors_left = num_sectors;
    let mut xml = firehose_xml_setup(
        "program",
        &[
            (
                "SECTOR_SIZE_IN_BYTES",
                &channel.fh_config().storage_sector_size.to_string(),
            ),
            ("num_partition_sectors", &num_sectors.to_string()),
            ("slot", &slot.to_string()),
            ("physical_partition_number", &phys_part_idx.to_string()),
            ("start_sector", start_sector),
            (
                "read_back_verify",
                &(channel.fh_config().read_back_verify as u32).to_string(),
            ),
        ],
    )?;

    firehose_write(channel, &mut xml)?;

    if firehose_read::<T>(channel, firehose_parser_ack_nak)? != FirehoseStatus::Ack {
        bail!("<program> was NAKed. Did you set sector-size correctly?");
    }

    let mut pb = ProgressBar::new((sectors_left * channel.fh_config().storage_sector_size) as u64);
    pb.show_time_left = true;
    pb.message(&format!("Sending partition {label}: "));
    pb.set_units(Units::Bytes);

    while sectors_left > 0 {
        let chunk_size_sectors = min(
            sectors_left,
            channel.fh_config().send_buffer_size / channel.fh_config().storage_sector_size,
        );
        let mut buf = vec![
            0u8;
            min(
                channel.fh_config().send_buffer_size,
                chunk_size_sectors * channel.fh_config().storage_sector_size,
            )
        ];
        let _ = data.read(&mut buf).unwrap();

        let n = channel.write(&buf).expect("Error sending data");
        if n != chunk_size_sectors * channel.fh_config().storage_sector_size {
            bail!("Wrote an unexpected number of bytes ({})", n);
        }

        sectors_left -= chunk_size_sectors;
        pb.add((chunk_size_sectors * channel.fh_config().storage_sector_size) as u64);
    }

    // Send a Zero-Length Packet to indicate end of stream
    if channel.fh_config().backend == QdlBackend::Usb && !channel.fh_config().skip_usb_zlp {
        let _ = channel.write(&[]).expect("Error sending ZLP");
    }

    if firehose_read::<T>(channel, firehose_parser_ack_nak)? != FirehoseStatus::Ack {
        bail!("Failed to complete 'write' op");
    }

    Ok(())
}

/// Get a SHA256 digest of a portion of Device storage
pub fn firehose_checksum_storage<T: QdlChan>(
    channel: &mut T,
    num_sectors: usize,
    phys_part_idx: u8,
    start_sector: u32,
) -> anyhow::Result<()> {
    let mut xml = firehose_xml_setup(
        "getsha256digest",
        &[
            (
                "SECTOR_SIZE_IN_BYTES",
                &channel.fh_config().storage_sector_size.to_string(),
            ),
            ("num_partition_sectors", &num_sectors.to_string()),
            ("physical_partition_number", &phys_part_idx.to_string()),
            ("start_sector", &start_sector.to_string()),
        ],
    )?;

    firehose_write(channel, &mut xml)?;

    // TODO: figure out some sane way to figure out the timeout
    if firehose_read::<T>(channel, firehose_parser_ack_nak)? != FirehoseStatus::Ack {
        bail!("Checksum request was NAKed");
    }

    Ok(())
}

/// Read (sector-aligned) parts of storage.
pub fn firehose_read_storage(
    channel: &mut impl QdlChan,
    out: &mut impl Write,
    num_sectors: usize,
    slot: u8,
    phys_part_idx: u8,
    start_sector: u32,
) -> anyhow::Result<()> {
    let mut bytes_left = num_sectors * channel.fh_config().storage_sector_size;
    let mut xml = firehose_xml_setup(
        "read",
        &[
            (
                "SECTOR_SIZE_IN_BYTES",
                &channel.fh_config().storage_sector_size.to_string(),
            ),
            ("num_partition_sectors", &num_sectors.to_string()),
            ("slot", &slot.to_string()),
            ("physical_partition_number", &phys_part_idx.to_string()),
            ("start_sector", &start_sector.to_string()),
        ],
    )?;

    firehose_write(channel, &mut xml)?;
    if firehose_read(channel, firehose_parser_ack_nak)? != FirehoseStatus::Ack {
        bail!("Read request was NAKed");
    }

    let mut pb = ProgressBar::new(bytes_left as u64);
    pb.set_units(Units::Bytes);

    let mut last_read_was_zero_len = false;
    while bytes_left > 0 {
        let chunk_size_bytes = min(bytes_left, channel.fh_config().recv_buffer_size);
        let mut buf = vec![0; chunk_size_bytes];

        let n = channel.read(&mut buf).expect("Error receiving data");
        if n == 0 {
            // TODO: need more robustness here
            /* Every 2 or 3 packets should be empty? */
            last_read_was_zero_len = true;
            continue;
        }

        last_read_was_zero_len = false;
        let _ = out.write(&buf[..n])?;

        bytes_left -= n;
        pb.add(n as u64);
    }

    if !last_read_was_zero_len && channel.fh_config().backend == QdlBackend::Usb {
        // Issue a dummy read to drain the queue
        let _ = channel.read(&mut [])?;
    }

    if firehose_read(channel, firehose_parser_ack_nak)? != FirehoseStatus::Ack {
        bail!("Failed to complete 'read' op");
    }

    Ok(())
}

/// Reboot or power off the Device
pub fn firehose_reset<T: QdlChan>(
    channel: &mut T,
    mode: &FirehoseResetMode,
    delay_in_sec: u32,
) -> anyhow::Result<()> {
    let mut xml = firehose_xml_setup(
        "power",
        &[
            (
                "value",
                match mode {
                    FirehoseResetMode::ResetToEdl => "reset_to_edl",
                    FirehoseResetMode::Reset => "reset",
                    FirehoseResetMode::Off => "off",
                },
            ),
            ("DelayInSeconds", &delay_in_sec.to_string()),
        ],
    )?;

    firehose_write_getack(channel, &mut xml, "reset the Device".to_owned())
}

/// Mark a physical storage partition as bootable
pub fn firehose_set_bootable<T: QdlChan>(channel: &mut T, drive_idx: u8) -> anyhow::Result<()> {
    let mut xml = firehose_xml_setup(
        "setbootablestoragedrive",
        &[("value", &drive_idx.to_string())],
    )?;

    firehose_write_getack(
        channel,
        &mut xml,
        format!("set partition {drive_idx} as bootable"),
    )
}

pub fn firehose_get_default_sector_size(t: &str) -> Option<usize> {
    match FirehoseStorageType::from_str(t).unwrap() {
        FirehoseStorageType::Emmc => Some(512),
        FirehoseStorageType::Nand => Some(4096),
        FirehoseStorageType::Nvme => Some(512),
        FirehoseStorageType::Ufs => Some(4096),
        FirehoseStorageType::Spinor => Some(4096),
    }
}
