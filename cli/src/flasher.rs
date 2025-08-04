// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Result, bail};
use programfile::parse_program_xml;
use qdl::firehose_set_bootable;
use qdl::types::QdlChan;

use std::fs::{self};
use std::path::Path;

use crate::programfile;

/// Iterates through program/patch files and executes the instructions therein.
pub(crate) fn run_flash<T: QdlChan>(
    channel: &mut T,
    program_file_paths: Vec<String>,
    patch_file_paths: Vec<String>,
    verbose: bool,
) -> Result<()> {
    // Check if the required files are present
    let file_paths = [&program_file_paths[..], &patch_file_paths[..]].concat();
    if let Some(f) = file_paths.iter().find(|f| !Path::new(f).is_file()) {
        bail!("{} doesn't exist", f);
    }
    let tmp_path_string = match cfg!(target_os = "windows") {
        true => "C:\\Temp\\",
        false => "/tmp/out/",
    };

    let mut bootable_part_idx: Option<u8> = None;
    for program_file_path in file_paths {
        let path = Path::new(&program_file_path);
        if !path.is_file() {
            bail!("Program file doesn't exist");
        }

        // Get the program files that we need
        let program_file_dir = path.parent().unwrap();
        let program_file = fs::read(path)?;
        let xml = xmltree::Element::parse(&program_file[..])?;

        // Parse the program/patch XMLs and flash away
        if let Some(n) = parse_program_xml(
            channel,
            &xml,
            program_file_dir,
            Path::new(tmp_path_string), // TODO
            true,                       // TODO
            verbose,
        )? {
            bootable_part_idx = Some(n)
        };
    }

    // Mark the correct LUN (or any other kind of physical partition) as bootable
    if bootable_part_idx.is_some() {
        println!(
            "Setting partition {} as bootable!",
            bootable_part_idx.unwrap()
        );
        firehose_set_bootable(channel, bootable_part_idx.unwrap())?;
    }

    Ok(())
}
