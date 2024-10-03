// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Context, Result, bail};
use rusb::{self, Device, DeviceHandle, GlobalContext};
use std::{
    io::{Error, ErrorKind, Read, Write},
    time::Duration,
};

use crate::types::QdlReadWrite;

pub struct QdlUsbConfig {
    dev_handle: rusb::DeviceHandle<GlobalContext>,
    in_ep: u8,
    out_ep: u8,
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
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        self.dev_handle
            .read_bulk(self.in_ep, buf, Duration::from_secs(10))
            .map_err(rusb_err_xlate)
    }
}

impl QdlReadWrite for QdlUsbConfig {}

const USB_VID_QCOM: u16 = 0x05c6;
const USB_PID_EDL: [u16; 2] = [0x9008 /* EDL */, 0x900e /* Ramdump */];
const INTF_DESC_PROTO_CODES: [u8; 3] = [0x10, 0x11, 0xFF];

fn find_usb_handle_by_sn(
    devices: &mut dyn Iterator<Item = Device<GlobalContext>>,
    serial_no: String,
) -> Result<DeviceHandle<GlobalContext>> {
    let mut dev_handle: Option<DeviceHandle<GlobalContext>> = None;

    for d in devices {
        let dh = d.open()?;

        let prod_str = dh.read_product_string_ascii(&d.device_descriptor().unwrap())?;
        let sn = &prod_str[prod_str.find("_SN:").unwrap() + "_SN:".len()..];
        if sn.eq_ignore_ascii_case(&serial_no) {
            dev_handle = Some(dh);
            break;
        }
    }

    match dev_handle {
        Some(h) => Ok(h),
        None => bail!(
            "Found no devices in EDL mode with serial number {}",
            serial_no
        ),
    }
}

pub fn setup_usb_device(serial_no: Option<String>) -> Result<QdlUsbConfig> {
    let rusb_devices = rusb::devices()?;
    let mut devices = rusb_devices
        .iter()
        .filter(|d: &rusb::Device<GlobalContext>| {
            d.device_descriptor().unwrap().vendor_id() == USB_VID_QCOM
                && USB_PID_EDL.contains(&d.device_descriptor().unwrap().product_id())
        });

    let dev_handle = match serial_no {
        Some(s) => find_usb_handle_by_sn(&mut devices, s),
        None => {
            let Some(d) = devices.next() else {
                bail!("Found no devices in EDL mode")
            };
            d.open().map_err(|e| rusb_err_xlate(e).into())
        }
    }?;

    // TODO: is there always precisely one interface like this?
    let cfg_desc = dev_handle.device().active_config_descriptor()?;
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
        .ok_or::<anyhow::Error>(Error::from(ErrorKind::NotFound).into())?;

    let in_ep = intf_desc
        .endpoint_descriptors()
        .find(|e| {
            e.direction() == rusb::Direction::In && e.transfer_type() == rusb::TransferType::Bulk
        })
        .unwrap()
        .address();
    let out_ep = intf_desc
        .endpoint_descriptors()
        .find(|e| {
            e.direction() == rusb::Direction::Out && e.transfer_type() == rusb::TransferType::Bulk
        })
        .unwrap()
        .address();

    // Make sure we can actually poke at the device
    dev_handle.set_auto_detach_kernel_driver(true).ok();
    dev_handle
        .claim_interface(intf_desc.interface_number())
        .with_context(|| format!("Couldn't claim interface{}", intf_desc.interface_number()))?;

    Ok(QdlUsbConfig {
        dev_handle,
        in_ep,
        out_ep,
    })
}

// TODO: fix this upstream?
pub fn rusb_err_xlate(e: rusb::Error) -> std::io::Error {
    std::io::Error::from(match e {
        rusb::Error::Timeout => std::io::ErrorKind::TimedOut,
        rusb::Error::Access => std::io::ErrorKind::PermissionDenied,
        rusb::Error::NoDevice => std::io::ErrorKind::NotConnected,
        _ => std::io::ErrorKind::Other,
    })
}
