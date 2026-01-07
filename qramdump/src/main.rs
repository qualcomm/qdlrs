// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use std::str::FromStr;

use anyhow::{Result, bail};

use clap::Parser;
use qdl::{
    self,
    sahara::{SaharaMode, sahara_reset, sahara_run},
    setup_target_device,
    types::{FirehoseConfiguration, QdlBackend, QdlDevice},
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

    let rw_channel = match setup_target_device(backend, args.serial_no, args.dev_path) {
        Ok(c) => c,
        Err(e) => bail!("Couldn't set up device: {}", e.to_string()),
    };

    let mut qdl_dev = QdlDevice {
        rw: rw_channel,
        fh_cfg: FirehoseConfiguration::default(),
        reset_on_drop: false,
    };

    sahara_run(
        &mut qdl_dev,
        SaharaMode::MemoryDebug,
        None,
        &mut [],
        args.regions_to_dump,
        args.verbose_sahara,
    )?;

    sahara_reset(&mut qdl_dev)?;

    Ok(())
}
