// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::Result;
use clap::{Parser, command};
use qdl::{
    self,
    vip::{calc_hashes, gen_hash_tables},
};
use std::path::Path;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg()]
    input_xml: String,

    #[arg(short, default_value = "out/")]
    output_dir: String,

    #[arg(short, default_value = "1048576")]
    send_buffer_size: usize,
}

pub fn main() -> Result<()> {
    let args = Args::parse();

    let hashes = calc_hashes(Path::new(&args.input_xml), args.send_buffer_size)?;

    gen_hash_tables(hashes, Path::new(&args.output_dir), 8192)
}
