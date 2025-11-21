// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.

use indexmap::IndexMap;

use owo_colors::OwoColorize;

use crate::{
    FirehoseError, FirehoseResetMode, FirehoseStatus, NakError, QdlChan, firehose_configure,
    firehose_read, firehose_reset,
};

/// The highest protocol version currently supported by the library
pub(crate) const FH_PROTO_VERSION_SUPPORTED: u32 = 1;

// Parsers are kept separate for more flexibility (e.g. log replay analysis)

/// Check "value" for ack/nak (generic)
pub fn firehose_parser_ack_nak<T: QdlChan>(
    _: &mut T,
    attrs: &IndexMap<String, String>,
) -> Result<FirehoseStatus, FirehoseError> {
    let val = attrs.get("value").to_owned();
    match &val.unwrap()[..] {
        "ACK" => Ok(FirehoseStatus::Ack),
        "NAK" => Ok(FirehoseStatus::Nak),
        _ => Err(FirehoseError::MalformedData(attrs.clone())),
    }
}

/// Parse the \<configure\> response
pub fn firehose_parser_configure_response<T: QdlChan>(
    channel: &mut T,
    attrs: &IndexMap<String, String>,
) -> Result<FirehoseStatus, FirehoseError> {
    if let Ok(status) = firehose_parser_ack_nak(channel, attrs) {
        // The device can't handle that big of a buffer and it auto-reconfigures to the max it can
        if status == FirehoseStatus::Nak {
            if let Some(val) = attrs.get("MaxPayloadSizeToTargetInBytes").to_owned() {
                channel.mut_fh_config().send_buffer_size = val.parse::<usize>().unwrap();
            } else {
                firehose_reset(channel, &FirehoseResetMode::ResetToEdl, 0)?;
                return Err(FirehoseError::Nak(NakError::Configure));
            }
        }
    }

    let device_max_write_payload_size = attrs
        .get("MaxPayloadSizeToTargetInBytesSupported")
        .unwrap()
        .parse::<usize>()
        .unwrap();

    // TODO: define version of the spec we support and validate it
    let version = attrs.get("Version").unwrap();
    let min_version_supported = attrs
        .get("MinVersionSupported")
        .unwrap()
        .parse::<u32>()
        .unwrap();

    println!("Found protocol version {}", version.bright_blue());

    if min_version_supported < FH_PROTO_VERSION_SUPPORTED {
        return Err(FirehoseError::ProtocolVersionIncompatibility {
            device_min_version: min_version_supported,
        });
    }

    // TODO: MaxPayloadSizeFromTargetInBytes seems useless when xfers are abstracted through libusb
    // TODO: ^ is usually 1kiB (reaaally small), newer (citation needed) devices don't advertise it

    channel.mut_fh_config().xml_buf_size = attrs
        .get("MaxXMLSizeInBytes")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    channel.mut_fh_config().send_buffer_size = attrs
        .get("MaxPayloadSizeToTargetInBytes")
        .unwrap()
        .parse::<usize>()
        .unwrap();

    // If the device can take a larger buffer, reconfigure it.
    if channel.fh_config().send_buffer_size < device_max_write_payload_size {
        println!(
            "Reconfiguring the device to use a larger ({}kB) send buffer",
            device_max_write_payload_size / 1024
        );

        channel.mut_fh_config().send_buffer_size = device_max_write_payload_size;
        firehose_configure(channel, true)?;
        firehose_read(channel, firehose_parser_ack_nak)?;
    }

    Ok(FirehoseStatus::Ack)
}
