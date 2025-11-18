// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use rusb::{self, Device, DeviceHandle, GlobalContext};
use std::{
    io::{BufRead, Read, Write},
    time::Duration,
};

use crate::types::QdlReadWrite;

pub struct QdlUsbConfig {
    dev_handle: rusb::DeviceHandle<GlobalContext>,
    in_ep: u8,
    out_ep: u8,
    buf: Vec<u8>,
    pos: usize,
    cap: usize,
}

// TODO: timeouts?
impl Write for QdlUsbConfig {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        self.dev_handle
            .write_bulk(self.out_ep, buf, Duration::from_secs(10))
            .map_err(rusb_err_xlate)
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        Ok(())
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
        self.dev_handle
            .read_bulk(self.in_ep, out, Duration::from_secs(10))
            .map_err(rusb_err_xlate)
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
            match self
                .dev_handle
                .read_bulk(self.in_ep, &mut self.buf, Duration::from_secs(10))
            {
                Ok(n) => {
                    self.cap = n;
                }
                Err(e) => return Err(rusb_err_xlate(e)),
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
    devices: &mut dyn Iterator<Item = Device<GlobalContext>>,
    serial_no: String,
) -> Result<DeviceHandle<GlobalContext>, UsbSetupError> {
    for d in devices {
        let dh = d.open().map_err(UsbSetupError::Open)?;

        let prod_str = dh
            .read_product_string_ascii(&d.device_descriptor().unwrap())
            .map_err(UsbSetupError::MiscRusb)?;
        let sn = &prod_str[prod_str.find("_SN:").unwrap() + "_SN:".len()..];
        if sn.eq_ignore_ascii_case(&serial_no) {
            return Ok(dh);
        }
    }

    Err(UsbSetupError::NoDevicesSerial { serial_no })
}

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum UsbSetupError {
    #[error("libusb error")]
    MiscRusb(#[source] rusb::Error),

    #[error("Failed to find devices in EDL mode with serial number {serial_no}")]
    NoDevicesSerial { serial_no: String },
    #[error("Failed to find devices in EDL mode")]
    NoDevices,

    #[error("Missing {0}")]
    Missing(&'static str),

    #[error("Failed to enumerate devices")]
    Enumeration(#[source] rusb::Error),
    #[error("Failed to open a device")]
    Open(#[source] rusb::Error),
    #[error("Failed to claim interface {0}")]
    ClaimInterface(u8, #[source] rusb::Error),
}

pub fn setup_usb_device(serial_no: Option<String>) -> Result<QdlUsbConfig, UsbSetupError> {
    let rusb_devices = rusb::devices().map_err(UsbSetupError::Enumeration)?;
    let mut devices = rusb_devices
        .iter()
        .filter(|d: &rusb::Device<GlobalContext>| {
            d.device_descriptor().unwrap().vendor_id() == USB_VID_QCOM
                && USB_PID_EDL.contains(&d.device_descriptor().unwrap().product_id())
        });

    let dev_handle = match serial_no {
        Some(s) => find_usb_handle_by_sn(&mut devices, s)?,
        None => {
            let d = devices.next().ok_or(UsbSetupError::NoDevices)?;
            d.open().map_err(UsbSetupError::Open)?
        }
    };

    // TODO: is there always precisely one interface like this?
    let cfg_desc = dev_handle
        .device()
        .active_config_descriptor()
        .map_err(UsbSetupError::MiscRusb)?;
    let intf_desc = cfg_desc
        .interfaces()
        .next()
        .unwrap()
        .descriptors()
        .find(|d| {
            d.class_code() == 0xFF
                && d.sub_class_code() == 0xFF
                && INTF_DESC_PROTO_CODES.contains(&d.protocol_code())
                && d.num_endpoints() >= 2
        })
        .ok_or(UsbSetupError::Missing("matching interface"))?;

    let in_ep = intf_desc
        .endpoint_descriptors()
        .find(|e| {
            e.direction() == rusb::Direction::In && e.transfer_type() == rusb::TransferType::Bulk
        })
        .ok_or(UsbSetupError::Missing("USB endpoint"))?
        .address();
    let out_ep = intf_desc
        .endpoint_descriptors()
        .find(|e| {
            e.direction() == rusb::Direction::Out && e.transfer_type() == rusb::TransferType::Bulk
        })
        .ok_or(UsbSetupError::Missing("USB endpoint"))?
        .address();

    // Make sure we can actually poke at the device
    dev_handle.set_auto_detach_kernel_driver(true).ok();
    dev_handle
        .claim_interface(intf_desc.interface_number())
        .map_err(|err| UsbSetupError::ClaimInterface(intf_desc.interface_number(), err))?;
    Ok(QdlUsbConfig {
        dev_handle,
        in_ep,
        out_ep,
        buf: Vec::new(),
        pos: 0,
        cap: 0,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("libusb error")]
struct RusbErrorWrap(#[source] rusb::Error);

// TODO: fix this upstream?
pub fn rusb_err_xlate(e: rusb::Error) -> std::io::Error {
    std::io::Error::other(RusbErrorWrap(e))
}
