// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Context, Result, bail};
use nusb::{
    self, Device, DeviceInfo, MaybeFuture,
    io::{EndpointRead, EndpointWrite},
};
use std::{
    io::{BufRead, Error, ErrorKind, Read, Write},
    time::Duration,
};

use crate::types::QdlReadWrite;

pub struct QdlUsbConfig {
    _dev: nusb::Device,
    reader: EndpointRead<nusb::transfer::Bulk>,
    writer: EndpointWrite<nusb::transfer::Bulk>,
    buf: Vec<u8>,
    pos: usize,
    cap: usize,
}

// TODO: timeouts?
impl Write for QdlUsbConfig {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        let n = self.writer.write(buf);
        self.writer.submit_end();
        n
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        self.writer.flush()
    }
}
impl Read for QdlUsbConfig {
    fn read(&mut self, out: &mut [u8]) -> Result<usize, std::io::Error> {
        // Drain internal buffer first
        if self.pos < self.cap {
            let n = std::cmp::min(out.len(), self.cap - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        // Otherwise, read directly from USB
        self.reader.read(out)
    }
}

impl BufRead for QdlUsbConfig {
    fn fill_buf(&mut self) -> Result<&[u8], std::io::Error> {
        if self.pos >= self.cap {
            self.pos = 0;
            self.cap = 0;
            if self.buf.is_empty() {
                self.buf.resize(4096, 0);
            }
            match self.reader.read(&mut self.buf) {
                Ok(n) => {
                    self.cap = n;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(&self.buf[self.pos..self.cap])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = std::cmp::min(self.pos + amt, self.cap);
    }
}

impl QdlReadWrite for QdlUsbConfig {}

const USB_VID_QCOM: u16 = 0x05c6;
const USB_PID_EDL: [u16; 2] = [0x9008 /* EDL */, 0x900e /* Ramdump */];
const INTF_DESC_PROTO_CODES: [u8; 3] = [0x10, 0x11, 0xFF];

fn find_usb_handle_by_sn(
    devices: &mut dyn Iterator<Item = DeviceInfo>,
    serial_no: String,
) -> Result<Device> {
    let mut dev: Option<DeviceInfo> = None;

    for d in devices {
        // let prod_str = dh.read_product_string_ascii(&d.device_descriptor().unwrap())?;
        if let Some(prod_str) = d.product_string() {
            let sn = &prod_str[prod_str.find("_SN:").unwrap() + "_SN:".len()..];
            if sn.eq_ignore_ascii_case(&serial_no) {
                dev = Some(d);
                break;
            }
        }
    }

    match dev {
        Some(h) => Ok(h.open().wait()?),
        None => bail!(
            "Found no devices in EDL mode with serial number {}",
            serial_no
        ),
    }
}

pub fn setup_usb_device(serial_no: Option<String>) -> Result<QdlUsbConfig> {
    let mut devices = nusb::list_devices()
        .wait()
        .unwrap()
        .filter(|d| d.vendor_id() == USB_VID_QCOM && USB_PID_EDL.contains(&d.product_id()));

    let dev = match serial_no {
        Some(s) => find_usb_handle_by_sn(&mut devices, s)?,
        None => {
            let Some(d) = devices.next() else {
                bail!("Found no devices in EDL mode")
            };
            d.open().wait()?
        }
    };

    // TODO: is there always precisely one interface like this?
    let cfg_desc = dev.active_configuration()?;
    let intf_desc = cfg_desc
        .interface_alt_settings()
        .find(|d| {
            d.class() == 0xFF
                && d.subclass() == 0xFF
                && INTF_DESC_PROTO_CODES.contains(&d.protocol())
                && d.num_endpoints() >= 2
        })
        .ok_or::<anyhow::Error>(Error::from(ErrorKind::NotFound).into())?;

    let in_ep = intf_desc
        .endpoints()
        .find(|e| {
            e.direction() == nusb::transfer::Direction::In
                && e.transfer_type() == nusb::descriptors::TransferType::Bulk
        })
        .unwrap()
        .address();
    let out_ep = intf_desc
        .endpoints()
        .find(|e| {
            e.direction() == nusb::transfer::Direction::Out
                && e.transfer_type() == nusb::descriptors::TransferType::Bulk
        })
        .unwrap()
        .address();

    // Make sure we can actually poke at the device
    let intf = dev
        .detach_and_claim_interface(intf_desc.interface_number())
        .wait()
        .with_context(|| format!("Couldn't claim interface{}", intf_desc.interface_number()))?;

    let mut rd = intf.endpoint(in_ep)?.reader(1024 * 1024);
    let mut wr = intf.endpoint(out_ep)?.writer(1024 * 1024);

    rd.set_read_timeout(Duration::from_secs(10));
    wr.set_write_timeout(Duration::from_secs(10));

    Ok(QdlUsbConfig {
        _dev: dev,
        reader: rd,
        writer: wr,
        buf: Vec::new(),
        pos: 0,
        cap: 0,
    })
}
