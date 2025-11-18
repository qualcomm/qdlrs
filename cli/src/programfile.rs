// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::bail;
use indexmap::IndexMap;
use std::{
    fs,
    io::{Seek, SeekFrom},
    path::Path,
};
use xmltree::{self, Element, XMLNode};

use qdl::{
    firehose_checksum_storage, firehose_patch, firehose_program_storage, firehose_read_storage,
    types::QdlChan,
};

fn parse_read_cmd<T: QdlChan>(
    channel: &mut T,
    out_dir: &Path,
    attrs: &IndexMap<String, String>,
    checksum_only: bool,
) -> anyhow::Result<()> {
    let num_sectors = attrs
        .get("num_partition_sectors")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let slot = attrs.get("slot").map_or(0, |a| a.parse::<u8>().unwrap());
    let phys_part_idx = attrs
        .get("physical_partition_number")
        .unwrap()
        .parse::<u8>()
        .unwrap();
    let start_sector = attrs.get("start_sector").unwrap().parse::<u32>().unwrap();

    if checksum_only {
        return Ok(firehose_checksum_storage(
            channel,
            num_sectors,
            phys_part_idx,
            start_sector,
        )?);
    }

    if !attrs.contains_key("filename") {
        bail!("Got '<read>' tag without a filename");
    }
    let mut outfile = fs::File::create(out_dir.join(attrs.get("filename").unwrap()))?;

    Ok(firehose_read_storage(
        channel,
        &mut outfile,
        num_sectors,
        slot,
        phys_part_idx,
        start_sector,
    )?)
}

fn parse_patch_cmd<T: QdlChan>(
    channel: &mut T,
    attrs: &IndexMap<String, String>,
    verbose: bool,
) -> anyhow::Result<()> {
    if let Some(filename) = attrs.get("filename") {
        if filename != "DISK" && verbose {
            println!("Skipping <patch> tag trying to alter {filename} on Host filesystem");
            return Ok(());
        }
    } else {
        bail!("Got '<patch>' tag without a filename");
    }

    let byte_off = attrs.get("byte_offset").unwrap().parse::<u64>().unwrap();
    let slot = attrs.get("slot").map_or(0, |a| a.parse::<u8>().unwrap());
    let phys_part_idx = attrs
        .get("physical_partition_number")
        .unwrap()
        .parse::<u8>()
        .unwrap();
    let size = attrs.get("size_in_bytes").unwrap().parse::<u64>().unwrap();
    let start_sector = attrs.get("start_sector").unwrap();
    let val = attrs.get("value").unwrap();

    Ok(firehose_patch(
        channel,
        byte_off,
        slot,
        phys_part_idx,
        size,
        start_sector,
        val,
    )?)
}

const BOOTABLE_PART_NAMES: [&str; 3] = ["xbl", "xbl_a", "sbl1"];

// TODO: readbackverify
fn parse_program_cmd<T: QdlChan>(
    channel: &mut T,
    program_file_dir: &Path,
    attrs: &IndexMap<String, String>,
    allow_missing_files: bool,
    bootable_part_idx: &mut Option<u8>,
    verbose: bool,
) -> anyhow::Result<()> {
    let sector_size = attrs
        .get("SECTOR_SIZE_IN_BYTES")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    if sector_size != channel.fh_config().storage_sector_size {
        bail!(
            "Mismatch in storage sector size! Programfile requests {}",
            sector_size
        );
    }
    let num_sectors = attrs
        .get("num_partition_sectors")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let slot = attrs.get("slot").map_or(0, |a| a.parse::<u8>().unwrap());
    let phys_part_idx = attrs
        .get("physical_partition_number")
        .unwrap()
        .parse::<u8>()
        .unwrap();
    let start_sector = attrs.get("start_sector").unwrap();
    let file_sector_offset = attrs
        .get("file_sector_offset")
        .unwrap_or(&"".to_owned())
        .parse::<u32>()
        .unwrap_or(0);

    let label = attrs.get("label").unwrap();
    if num_sectors == 0 {
        println!("Skipping 0-length entry for {label}");
        return Ok(());
    }
    if BOOTABLE_PART_NAMES.contains(&&label[..]) {
        *bootable_part_idx = Some(phys_part_idx);
    }

    let filename = attrs.get("filename").unwrap();
    let file_path = program_file_dir.join(filename);
    if allow_missing_files {
        if filename.is_empty() {
            if verbose {
                println!("Skipping bogus entry for {label}");
            }
            return Ok(());
        } else if !file_path.exists() {
            if verbose {
                println!("Skipping non-existent file {}", file_path.to_str().unwrap());
            }
            return Ok(());
        }
    }

    let mut buf = fs::File::open(file_path)?;
    buf.seek(SeekFrom::Current(
        sector_size as i64 * file_sector_offset as i64,
    ))?;

    Ok(firehose_program_storage(
        channel,
        &mut buf,
        label,
        num_sectors,
        slot,
        phys_part_idx,
        start_sector,
    )?)
}

// TODO: there's some funny optimizations to make here, such as OoO loading files into memory, or doing things while we're waiting on the device to finish
pub fn parse_program_xml<T: QdlChan>(
    channel: &mut T,
    xml: &Element,
    program_file_dir: &Path,
    out_dir: &Path,
    allow_missing_files: bool,
    verbose: bool,
) -> anyhow::Result<Option<u8>> {
    let mut bootable_part_idx: Option<u8> = None;

    // First make sure we have all the necessary files (and fail unless specified otherwise)
    for node in xml.children.iter() {
        if let XMLNode::Element(e) = node {
            match e.name.to_lowercase().as_str() {
                "program" => {
                    if !e.attributes.contains_key("filename") {
                        bail!("Got '<program>' tag without a filename");
                    }

                    let filename = e.attributes.get("filename").unwrap();
                    let file_path = program_file_dir.join(filename);

                    if !file_path.exists() && !allow_missing_files {
                        bail!("{} doesn't exist!", file_path.to_str().unwrap())
                    }
                }
                _ => continue,
            }
        }
    }

    // At last, do the things we're supposed to do
    for node in xml.children.iter() {
        if let XMLNode::Element(e) = node {
            match e.name.to_lowercase().as_str() {
                "getsha256digest" => parse_read_cmd(channel, out_dir, &e.attributes, true)?,
                "patch" => parse_patch_cmd(channel, &e.attributes, verbose)?,
                "program" => parse_program_cmd(
                    channel,
                    program_file_dir,
                    &e.attributes,
                    allow_missing_files,
                    &mut bootable_part_idx,
                    verbose,
                )?,
                "read" => parse_read_cmd(channel, out_dir, &e.attributes, false)?,

                unknown => bail!(
                    "Got unknown instruction ({}), failing to prevent damage",
                    unknown
                ),
            };
        }
    }

    Ok(bootable_part_idx)
}
