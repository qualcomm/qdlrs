// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use serial2::{self, SerialPort};
use std::io::{BufRead, Read, Write};

use crate::types::QdlReadWrite;

pub struct QdlSerialConfig {
    serport: SerialPort,
    buf: Vec<u8>,
    pos: usize,
    cap: usize,
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
    fn read(&mut self, out: &mut [u8]) -> Result<usize, std::io::Error> {
        // Drain internal buffer first
        if self.pos < self.cap {
            let n = std::cmp::min(out.len(), self.cap - self.pos);
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        // Otherwise, read directly from serial port
        self.serport.read(out)
    }
}

impl BufRead for QdlSerialConfig {
    fn fill_buf(&mut self) -> Result<&[u8], std::io::Error> {
        if self.pos >= self.cap {
            self.pos = 0;
            self.cap = 0;
            if self.buf.is_empty() {
                self.buf.resize(4096, 0);
            }
            match self.serport.read(&mut self.buf) {
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

impl QdlReadWrite for QdlSerialConfig {}

pub fn setup_serial_device(dev_path: String) -> Result<QdlSerialConfig, std::io::Error> {
    let serport = SerialPort::open(dev_path, |mut settings: serial2::Settings| {
        settings.set_raw();
        settings.set_baud_rate(115200)?;
        Ok(settings)
    })?;

    Ok(QdlSerialConfig {
        serport,
        buf: Vec::new(),
        pos: 0,
        cap: 0,
    })
}
