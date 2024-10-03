// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Result, bail};
use serial2::{self, SerialPort};
use std::io::{Read, Write};

use crate::types::QdlReadWrite;

pub struct QdlSerialConfig {
    serport: SerialPort,
}

// TODO: timeouts?
impl Write for QdlSerialConfig {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        self.serport.write(buf)
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        self.serport.flush()
    }
}

impl Read for QdlSerialConfig {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        self.serport.read(buf)
    }
}

impl QdlReadWrite for QdlSerialConfig {}

pub fn setup_serial_device(dev_path: Option<String>) -> Result<QdlSerialConfig> {
    if dev_path.is_none() {
        bail!("Serial port path unspecified");
    }

    let serport = SerialPort::open(dev_path.unwrap(), |mut settings: serial2::Settings| {
        settings.set_raw();
        settings.set_baud_rate(115200)?;
        Ok(settings)
    })?;

    Ok(QdlSerialConfig { serport })
}
