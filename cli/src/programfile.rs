// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Context, bail};
use indexmap::IndexMap;
use std::{
    fs,
    io::{Seek, SeekFrom},
    path::Path,
    str::FromStr,
};
use xmltree::{self, Element, XMLNode};

use qdl::{
    firehose_checksum_storage, firehose_patch, firehose_program_storage, firehose_read_storage,
    types::QdlChan,
};

/// Fetch an attribute and parse it, attaching context on a missing key or a
/// value that fails to parse, so a malformed program/patch file yields a
/// clear error instead of a panic.
fn parse_attr<T>(attrs: &IndexMap<String, String>, key: &str) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let val = attrs
        .get(key)
        .with_context(|| format!("Program file entry is missing the '{key}' attribute"))?;
    val.parse::<T>()
        .with_context(|| format!("Couldn't parse '{key}' attribute value \"{val}\""))
}

/// Fetch a required string attribute, attaching context on a missing key.
fn get_attr<'a>(attrs: &'a IndexMap<String, String>, key: &str) -> anyhow::Result<&'a String> {
    attrs
        .get(key)
        .with_context(|| format!("Program file entry is missing the '{key}' attribute"))
}

fn parse_read_cmd<T: QdlChan>(
    channel: &mut T,
    out_dir: &Path,
    attrs: &IndexMap<String, String>,
    checksum_only: bool,
) -> anyhow::Result<()> {
    let num_sectors = parse_attr::<usize>(attrs, "num_partition_sectors")?;
    let slot = match attrs.get("slot") {
        Some(_) => parse_attr::<u8>(attrs, "slot")?,
        None => 0,
    };
    let phys_part_idx = parse_attr::<u8>(attrs, "physical_partition_number")?;
    let start_sector = parse_attr::<u64>(attrs, "start_sector")?;

    if checksum_only {
        return firehose_checksum_storage(channel, num_sectors, phys_part_idx, start_sector);
    }

    if !attrs.contains_key("filename") {
        bail!("Got '<read>' tag without a filename");
    }
    let mut outfile = fs::File::create(out_dir.join(get_attr(attrs, "filename")?))?;

    firehose_read_storage(
        channel,
        &mut outfile,
        num_sectors,
        slot,
        phys_part_idx,
        start_sector,
    )
}

fn parse_patch_cmd<T: QdlChan>(
    channel: &mut T,
    attrs: &IndexMap<String, String>,
    verbose: bool,
) -> anyhow::Result<()> {
    if let Some(filename) = attrs.get("filename") {
        if filename != "DISK" {
            if verbose {
                println!("Skipping <patch> tag trying to alter {filename} on Host filesystem");
            }
            return Ok(());
        }
    } else {
        bail!("Got '<patch>' tag without a filename");
    }

    let byte_off = parse_attr::<u64>(attrs, "byte_offset")?;
    let slot = match attrs.get("slot") {
        Some(_) => parse_attr::<u8>(attrs, "slot")?,
        None => 0,
    };
    let phys_part_idx = parse_attr::<u8>(attrs, "physical_partition_number")?;
    let size = parse_attr::<u64>(attrs, "size_in_bytes")?;
    let start_sector = get_attr(attrs, "start_sector")?;
    let val = get_attr(attrs, "value")?;

    firehose_patch(
        channel,
        byte_off,
        slot,
        phys_part_idx,
        size,
        start_sector,
        val,
    )
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
    let sector_size = parse_attr::<usize>(attrs, "SECTOR_SIZE_IN_BYTES")?;
    if sector_size != channel.fh_config().storage_sector_size {
        bail!(
            "Mismatch in storage sector size! Programfile requests {}",
            sector_size
        );
    }
    let num_sectors = parse_attr::<usize>(attrs, "num_partition_sectors")?;
    let slot = match attrs.get("slot") {
        Some(_) => parse_attr::<u8>(attrs, "slot")?,
        None => 0,
    };
    let phys_part_idx = parse_attr::<u8>(attrs, "physical_partition_number")?;
    let start_sector = get_attr(attrs, "start_sector")?;
    // file_sector_offset is optional; treat a missing or unparseable value as 0
    let file_sector_offset = attrs
        .get("file_sector_offset")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    let label = get_attr(attrs, "label")?;
    if num_sectors == 0 {
        println!("Skipping 0-length entry for {label}");
        return Ok(());
    }
    if BOOTABLE_PART_NAMES.contains(&&label[..]) {
        *bootable_part_idx = Some(phys_part_idx);
    }

    let filename = get_attr(attrs, "filename")?;
    let file_path = program_file_dir.join(filename);
    if allow_missing_files {
        if filename.is_empty() {
            if verbose {
                println!("Skipping bogus entry for {label}");
            }
            return Ok(());
        } else if !file_path.exists() {
            if verbose {
                println!("Skipping non-existent file {}", file_path.display());
            }
            return Ok(());
        }
    }

    let mut buf = fs::File::open(&file_path)
        .with_context(|| format!("Couldn't open program file {}", file_path.display()))?;
    buf.seek(SeekFrom::Current(
        sector_size as i64 * file_sector_offset as i64,
    ))?;

    firehose_program_storage(
        channel,
        &mut buf,
        label,
        num_sectors,
        slot,
        phys_part_idx,
        start_sector,
    )
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

                    let filename = get_attr(&e.attributes, "filename")?;
                    let file_path = program_file_dir.join(filename);

                    if !file_path.exists() && !allow_missing_files {
                        bail!("{} doesn't exist!", file_path.display())
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
