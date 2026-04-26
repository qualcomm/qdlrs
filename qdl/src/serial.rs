// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Result, bail};
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

pub fn setup_serial_device(dev_path: Option<String>) -> Result<QdlSerialConfig> {
    if dev_path.is_none() {
        bail!("Serial port path unspecified");
    }
    let path = dev_path.unwrap();

    // Two-stage open so a partial-apply termios error doesn't kill
    // the session before any byte flows.
    //
    // Stage 1: open with an identity settings callback (returns
    // current termios verbatim). serial2's `set_configuration`
    // path will then write back what it just read, so its strict
    // readback `matches_requested` check trivially passes — no
    // matter how exotic the kernel driver is about which bits it
    // actually honours.
    //
    // Stage 2: best-effort apply raw + 115200 baud. If the kernel
    // driver silently downgrades any bit (qcserial under VMware
    // USB passthrough is the confirmed culprit, where serial2 errors
    // out with `failed to apply some or all settings`), log to
    // stderr and proceed: Sahara/Firehose is a raw byte stream over
    // USB-CDC, and the kernel termios layer is advisory for those
    // drivers. The bytes still flow.
    let mut serport = SerialPort::open(&path, |s| Ok(s))?;
    if let Ok(mut applied) = serport.get_configuration() {
        applied.set_raw();
        let _ = applied.set_baud_rate(115200);
        if let Err(e) = serport.set_configuration(&applied) {
            eprintln!(
                "[qdl] serial: best-effort termios apply on {path} failed ({e}); proceeding (kernel termios is advisory for USB-CDC drivers)"
            );
        }
    }

    Ok(QdlSerialConfig {
        serport,
        buf: Vec::new(),
        pos: 0,
        cap: 0,
    })
}
