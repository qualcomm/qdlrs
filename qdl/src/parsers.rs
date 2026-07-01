// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.

use indexmap::IndexMap;

use anyhow::{Context, bail};
use owo_colors::OwoColorize;
use std::str::FromStr;

use crate::{
    FirehoseResetMode, FirehoseStatus, QdlChan, firehose_configure, firehose_read, firehose_reset,
};

/// The highest protocol version currently supported by the library
const FH_PROTO_VERSION_SUPPORTED: u32 = 1;

// Parsers are kept separate for more flexibility (e.g. log replay analysis)

/// Fetch an attribute and parse it, attaching context on a missing key or a
/// value that fails to parse.
fn parse_attr<T>(attrs: &IndexMap<String, String>, key: &str) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let val = attrs
        .get(key)
        .with_context(|| format!("Device response is missing the '{key}' attribute"))?;
    val.parse::<T>()
        .with_context(|| format!("Couldn't parse '{key}' attribute value \"{val}\""))
}

/// Check "value" for ack/nak (generic)
pub fn firehose_parser_ack_nak<T: QdlChan>(
    _: &mut T,
    attrs: &IndexMap<String, String>,
) -> Result<FirehoseStatus, anyhow::Error> {
    let val = attrs.get("value");
    match val.map(|s| s.as_str()) {
        Some("ACK") => Ok(FirehoseStatus::Ack),
        Some("NAK") => Ok(FirehoseStatus::Nak),
        _ => bail!("Got malformed data: {:?}", attrs),
    }
}

/// Parse the \<configure\> response
pub fn firehose_parser_configure_response<T: QdlChan>(
    channel: &mut T,
    attrs: &IndexMap<String, String>,
) -> Result<FirehoseStatus, anyhow::Error> {
    if let Ok(status) = firehose_parser_ack_nak(channel, attrs) {
        // The device can't handle that big of a buffer and it auto-reconfigures to the max it can
        if status == FirehoseStatus::Nak {
            if attrs.contains_key("MaxPayloadSizeToTargetInBytes") {
                channel.mut_fh_config().send_buffer_size =
                    parse_attr(attrs, "MaxPayloadSizeToTargetInBytes")?;
            } else {
                firehose_reset(channel, &FirehoseResetMode::ResetToEdl, 0)?;
                bail!("firehose <configure> failed, try again with  --verbose-firehose")
            }
        }
    }

    let device_max_write_payload_size: usize =
        parse_attr(attrs, "MaxPayloadSizeToTargetInBytesSupported")?;

    // TODO: define version of the spec we support and validate it
    let version = attrs
        .get("Version")
        .context("Device response is missing the 'Version' attribute")?;
    let min_version_supported: u32 = parse_attr(attrs, "MinVersionSupported")?;

    println!("Found protocol version {}", version.bright_blue());

    if min_version_supported > FH_PROTO_VERSION_SUPPORTED {
        bail!(
            "Device requires protocol version >= {}, the library only supports up to v{}",
            min_version_supported.bright_red(),
            FH_PROTO_VERSION_SUPPORTED.bright_blue()
        );
    }

    // TODO: MaxPayloadSizeFromTargetInBytes seems useless when xfers are abstracted through libusb
    // TODO: ^ is usually 1kiB (reaaally small), newer (citation needed) devices don't advertise it

    channel.mut_fh_config().xml_buf_size = parse_attr(attrs, "MaxXMLSizeInBytes")?;
    channel.mut_fh_config().send_buffer_size = parse_attr(attrs, "MaxPayloadSizeToTargetInBytes")?;

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
