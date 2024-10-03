// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use std::str::FromStr;

use anyhow::{Result, bail};

use clap::{Parser, command};
use qdl::{
    self,
    sahara::{SaharaMode, sahara_run},
    setup_target_device,
    types::{FirehoseConfiguration, FirehoseDevice, QdlBackend},
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, value_name = "usb/serial")]
    backend: Option<String>,

    #[arg(short, long, help = "E.g. COM4 on Windows")]
    dev_path: Option<String>,

    #[arg()]
    regions_to_dump: Vec<String>,

    // Only applies to the USB backend
    #[arg(long)]
    serial_no: Option<String>,

    #[arg(long, default_value = "false")]
    verbose_sahara: bool,
}

pub fn main() -> Result<()> {
    let args = Args::parse();
    let backend = match args.backend {
        Some(b) => QdlBackend::from_str(&b)?,
        None => QdlBackend::default(),
    };

    let mut rw_channel = match setup_target_device(backend, args.serial_no, args.dev_path) {
        Ok(c) => c,
        Err(e) => bail!("Couldn't set up device: {}", e.to_string()),
    };

    let mut fh_dev = FirehoseDevice {
        rw: rw_channel.as_mut(),
        fh_cfg: FirehoseConfiguration::default(),
        session_done: true,
    };

    sahara_run(
        &mut fh_dev,
        SaharaMode::MemoryDebug,
        None,
        &mut [],
        args.regions_to_dump,
        args.verbose_sahara,
    )?;

    Ok(())
}
