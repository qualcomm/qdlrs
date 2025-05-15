// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Result, bail};
use gptman::{self, GPT, GPTHeader, GPTPartitionEntry};
use owo_colors::OwoColorize;
use std::io::{Cursor, Error, ErrorKind, Read, Seek, Write};

use qdl::{self, firehose_read_storage, types::QdlChan};

pub fn read_gpt_from_storage<T: Read + Write + QdlChan>(
    channel: &mut T,
    phys_part_idx: u8,
) -> Result<GPT> {
    let mut buf = Cursor::new(Vec::<u8>::new());

    // First, probe sector 1 to retrieve the GPT size
    // Note, sector 0 contains a fake MBR as per the GPT spec ("Protective MBR")
    firehose_read_storage(channel, &mut buf, 1, phys_part_idx, 1)?;

    buf.rewind()?;
    let header = match GPTHeader::read_from(&mut buf) {
        Ok(h) => h,
        Err(e) => bail!("Couldn't parse the GPT header: {}", e),
    };

    // The entire primary GPT is located between sectors 0 and first_usable_lba
    let gpt_len = header.first_usable_lba as usize;

    // Then, read the entire GPT and parse it
    buf.rewind()?;
    firehose_read_storage(channel, &mut buf, gpt_len, phys_part_idx, 0)?;

    // Ignore the aforementioned MBR sector
    buf.set_position(channel.fh_config().storage_sector_size as u64);
    GPT::read_from(&mut buf, channel.fh_config().storage_sector_size as u64).map_err(|e| e.into())
}

pub fn find_part<T: Read + Write + QdlChan>(
    channel: &mut T,
    name: &str,
    phys_part_idx: u8,
) -> Result<GPTPartitionEntry> {
    match read_gpt_from_storage(channel, phys_part_idx)?
        .iter()
        .find(|(_, p)| p.partition_name.to_string() == name)
    {
        Some(p) => Ok(p.1.clone()),
        None => Err(Error::from(ErrorKind::NotFound).into()),
    }
}

pub fn print_partition_table<T: Read + Write + QdlChan>(
    channel: &mut T,
    phys_part_idx: u8,
) -> Result<()> {
    let gpt = read_gpt_from_storage(channel, phys_part_idx)?;

    println!(
        "GPT on physical partition {} of {}:",
        phys_part_idx.bright_yellow(),
        channel.fh_config().storage_type.to_string().bright_yellow()
    );
    for (idx, part) in gpt.iter() {
        println!(
            "{}] {}: start_sector = {}, {} bytes ({} kiB)",
            idx,
            part.partition_name.as_str(),
            part.starting_lba,
            part.size().unwrap() * gpt.sector_size,
            part.size().unwrap() * gpt.sector_size / 1024,
        );
    }

    Ok(())
}

pub fn read_storage_logical_partition<T: Read + Write + QdlChan>(
    channel: &mut T,
    out: &mut impl Write,
    name: &str,
    phys_part_idx: u8,
) -> Result<()> {
    let gpt = read_gpt_from_storage(channel, phys_part_idx)?;

    let part = gpt
        .iter()
        .find(|(_, p)| p.partition_name.as_str() == name)
        .ok_or(Error::from(ErrorKind::NotFound))?
        .1;

    firehose_read_storage(
        channel,
        out,
        (part.ending_lba - part.starting_lba + 1) as usize,
        phys_part_idx,
        part.starting_lba as u32,
    )
}
