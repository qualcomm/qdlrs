// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use clap_num::maybe_hex;
use itertools::Itertools;
use owo_colors::OwoColorize;
use qdl::parsers::{firehose_parser_ack_nak, firehose_parser_configure_response};
use qdl::sahara::{SaharaCmdModeCmd, SaharaMode, sahara_run, sahara_send_hello_rsp};
use qdl::types::{FirehoseResetMode, FirehoseStorageType, QdlBackend, QdlDevice};
use qdl::{firehose_configure, firehose_read, firehose_reset, types::FirehoseConfiguration};
use qdl::{
    firehose_get_default_sector_size, firehose_nop, firehose_peek, firehose_program_storage,
    firehose_set_bootable, setup_target_device,
};
use util::{
    find_part, print_partition_table, read_gpt_from_storage, read_storage_logical_partition,
};

use std::fs::{self, File};
use std::{path::Path, str::FromStr};

mod flasher;
mod programfile;
mod util;

#[derive(Debug, Subcommand, PartialEq)]
enum Command {
    /// Dump the entire storage
    Dump {
        #[arg(short, default_value = "out/")]
        outdir: String,
    },

    /// Dump a single partition
    DumpPart {
        #[arg()]
        name: String,

        #[arg(short, default_value = "out/")]
        outdir: String,
    },

    /// Invoke the flasher
    Flasher {
        #[arg(short, long, num_args = 1..=128, value_name = "FILE")]
        program_file_paths: Vec<String>,

        #[arg(short = 'x', long, num_args = 0..=128, value_name = "FILE")]
        patch_file_paths: Vec<String>,

        #[arg(long, default_value = "false")]
        verbose_flasher: bool,
    },

    /// Erase a partition
    Erase {
        #[arg()]
        name: String,
    },

    /// Ask the device to do nothing, hopefully successfully
    Nop,

    /// Overwrite the storage physical partition contents with a raw image
    /// Similar to Flasher, but this one only takes a partition dump as input
    /// and performs no real validation on the input data
    OverwriteStorage {
        #[arg()]
        file_path: String,
    },

    /// Peek at memory
    Peek {
        #[arg(value_parser=maybe_hex::<u64>)]
        base: u64,

        #[arg(default_value = "1", value_parser=maybe_hex::<u64>)]
        len: u64,
    },

    /// Print the GPT table
    PrintGpt,

    /// Restart the device
    Reset {
        #[arg(default_value = "system", value_name = "edl/off/system")]
        reset_mode: String,
    },

    /// Mark physical storage partition as bootable
    SetBootablePart {
        #[arg()]
        idx: u8,
    },

    /// Write a partition
    Write {
        #[arg()]
        part_name: String,

        #[arg()]
        file_path: String,
    },
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, value_name = "usb/serial")]
    backend: Option<String>,

    /// Accept storage r/w operations, but make them never actually execute (useful for testing USB throughput)
    #[arg(long, default_value = "false")]
    bypass_storage: bool,

    #[arg(short, long, help = "E.g. COM4 on Windows")]
    dev_path: Option<String>,

    #[arg(short, long, value_name = "FILE")]
    loader_path: String,

    #[arg(long, default_value = "false", help = "Validate every packet. Slow.")]
    hash_packets: bool,

    #[arg(
        short = 'L',
        long,
        default_value = "0",
        help = "e.g. LUN index for UFS"
    )]
    phys_part_idx: u8,

    #[arg(long, default_value = "false")]
    print_firehose_log: bool,

    #[arg(
        long,
        default_value = "false",
        help = "Every <program> operation is read back. VERY SLOW!"
    )]
    read_back_verify: bool,

    /// WARNING: Will be deprecated in release v1.0.0
    #[arg(long, default_value = "edl", value_name = "edl/off/system")]
    reset_mode: String,

    // Only applies to the USB backend
    #[arg(long)]
    serial_no: Option<String>,

    #[arg(
        short = 'A',
        long,
        default_value = "false",
        help = "Work around missing HELLO packet"
    )]
    skip_hello_wait: bool,

    #[arg(short, long, value_name = "emmc/ufs/nvme/nand")]
    storage_type: String,

    #[arg(
        short = 'S',
        long,
        default_value = "0",
        help = "Index of the physical device (e.g. 1 for secondary UFS)"
    )]
    storage_slot: u8,

    #[arg(long)]
    sector_size: Option<usize>,

    #[arg(
        long,
        default_value = "false",
        help = "Required for unprovisioned storage media."
    )]
    skip_storage_init: bool,

    #[arg(long, default_value = "false")]
    verbose_sahara: bool,

    #[arg(long, default_value = "false")]
    verbose_firehose: bool,

    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let backend = match args.backend {
        Some(b) => QdlBackend::from_str(&b)?,
        None => QdlBackend::default(),
    };
    let reset_mode = FirehoseResetMode::from_str(&args.reset_mode)?;

    // Get the MBN loader binary
    let mbn_loader = match fs::read(args.loader_path) {
        Ok(m) => m,
        Err(e) => bail!("Couldn't open the programmer binary: {}", e.to_string()),
    };

    println!(
        "{} {}",
        env!("CARGO_PKG_NAME").green(),
        env!("CARGO_PKG_VERSION").yellow()
    );

    // Set up the device
    let mut rw_channel = match setup_target_device(backend, args.serial_no, args.dev_path) {
        Ok(c) => c,
        Err(e) => bail!("Couldn't set up device: {}", e.to_string()),
    };
    let mut qdl_dev = QdlDevice {
        rw: rw_channel.as_mut(),
        fh_cfg: FirehoseConfiguration {
            hash_packets: args.hash_packets,
            read_back_verify: args.read_back_verify,
            storage_type: FirehoseStorageType::from_str(&args.storage_type)?,
            storage_sector_size: match args.sector_size {
                Some(n) => n,
                None => {
                    let sector_size = firehose_get_default_sector_size(&args.storage_type);
                    if let Some(m) = sector_size {
                        println!("{} {}", "Using a default sector size of".bright_black(), m);
                        m
                    } else {
                        bail!("Specify storage sector size with --sector-size <n>");
                    }
                }
            },
            storage_slot: args.storage_slot,
            bypass_storage: args.bypass_storage,
            backend,
            skip_firehose_log: !args.print_firehose_log,
            verbose_firehose: args.verbose_firehose,
            // The remaining values are overwritten at runtime through a <configure> handshake
            ..Default::default()
        },
        reset_on_drop: false,
    };

    // In case another program on the system has already consumed the HELLO packet,
    // send a HELLO response upfront, to appease the state machine
    if args.skip_hello_wait {
        sahara_send_hello_rsp(&mut qdl_dev, SaharaMode::Command)?;
    }

    // Get some info about the device
    let sn = sahara_run(
        &mut qdl_dev,
        SaharaMode::Command,
        Some(SaharaCmdModeCmd::ReadSerialNum),
        &mut [],
        vec![],
        args.verbose_sahara,
    )?;
    let sn = u32::from_le_bytes([sn[0], sn[1], sn[2], sn[3]]);
    println!("Chip serial number: 0x{sn:x}");

    let key_hash = sahara_run(
        &mut qdl_dev,
        SaharaMode::Command,
        Some(SaharaCmdModeCmd::ReadOemKeyHash),
        &mut [],
        vec![],
        args.verbose_sahara,
    )?;
    println!(
        "OEM Private Key hash: 0x{:02x}",
        key_hash[..key_hash.len() / 3].iter().format("")
    );

    // Send the loader (and any other images)
    sahara_run(
        &mut qdl_dev,
        SaharaMode::WaitingForImage,
        None,
        &mut [mbn_loader],
        vec![],
        args.verbose_sahara,
    )?;

    // If we're past Sahara, activate the Firehose reset-on-drop listener
    qdl_dev.reset_on_drop = true;

    // Get any "welcome" logs
    firehose_read(&mut qdl_dev, firehose_parser_ack_nak)?;

    // Send the host capabilities to the device
    firehose_configure(&mut qdl_dev, args.skip_storage_init)?;

    // Parse some information from the device
    firehose_read(&mut qdl_dev, firehose_parser_configure_response)?;

    match args.command {
        Command::Dump { outdir } => {
            fs::create_dir_all(&outdir)?;
            let outpath = Path::new(&outdir);

            for (_, p) in
                read_gpt_from_storage(&mut qdl_dev, args.storage_slot, args.phys_part_idx)?.iter()
            {
                // *sigh*
                if p.partition_name.as_str().is_empty() || p.size()? == 0 {
                    continue;
                }

                let mut out = File::create(outpath.join(p.partition_name.to_string()))?;
                read_storage_logical_partition(
                    &mut qdl_dev,
                    &mut out,
                    &p.partition_name.to_string(),
                    args.storage_slot,
                    args.phys_part_idx,
                )?
            }
            // TODO: create an xml file
        }
        Command::DumpPart { name, outdir } => {
            fs::create_dir_all(&outdir)?;
            let outpath = Path::new(&outdir);
            let mut out = File::create(outpath.join(&name))?;

            read_storage_logical_partition(
                &mut qdl_dev,
                &mut out,
                &name,
                args.storage_slot,
                args.phys_part_idx,
            )?
        }
        Command::Erase { name } => {
            let part = find_part(&mut qdl_dev, &name, args.storage_slot, args.phys_part_idx)?;

            firehose_program_storage(
                &mut qdl_dev,
                &mut &[0u8][..],
                &name,
                (part.ending_lba - part.starting_lba + 1) as usize,
                args.storage_slot,
                args.phys_part_idx,
                &part.starting_lba.to_string(),
            )?;
        }
        Command::Flasher {
            program_file_paths,
            patch_file_paths,
            verbose_flasher,
        } => {
            flasher::run_flash(
                &mut qdl_dev,
                program_file_paths,
                patch_file_paths,
                verbose_flasher,
            )?;
        }
        Command::Nop => println!(
            "Your nop was {}",
            firehose_nop(&mut qdl_dev)
                .map(|_| "successful".bright_green())
                .map_err(|_| "unsuccessful".bright_red())
                .unwrap()
        ),
        Command::OverwriteStorage { file_path } => {
            let mut file = File::open(file_path)?;
            let file_len_sectors = file
                .metadata()?
                .len()
                .div_ceil(qdl_dev.fh_cfg.storage_sector_size as u64);

            firehose_program_storage(
                &mut qdl_dev,
                &mut file,
                "",
                file_len_sectors as usize,
                args.storage_slot,
                args.phys_part_idx,
                "0",
            )?;
        }
        Command::Peek { base, len } => firehose_peek(&mut qdl_dev, base, len)?,
        Command::PrintGpt => {
            print_partition_table(&mut qdl_dev, args.storage_slot, args.phys_part_idx)?
        }
        Command::Reset { reset_mode } => {
            firehose_reset(&mut qdl_dev, &FirehoseResetMode::from_str(&reset_mode)?, 0)?
        }
        Command::SetBootablePart { idx } => firehose_set_bootable(&mut qdl_dev, idx)?,
        Command::Write {
            part_name,
            file_path,
        } => {
            let part: gptman::GPTPartitionEntry = find_part(
                &mut qdl_dev,
                &part_name,
                args.storage_slot,
                args.phys_part_idx,
            )?;
            let mut file = File::open(file_path)?;
            let file_len_sectors = file
                .metadata()?
                .len()
                .div_ceil(qdl_dev.fh_cfg.storage_sector_size as u64);
            let part_len_sectors = part.ending_lba - part.starting_lba + 1;

            if file_len_sectors > part_len_sectors {
                bail!(
                    "Partition {} is too small for the specified image ({} > {})",
                    part_name,
                    file_len_sectors,
                    part_len_sectors
                );
            }

            firehose_program_storage(
                &mut qdl_dev,
                &mut file,
                &part_name,
                file_len_sectors as usize,
                args.storage_slot,
                args.phys_part_idx,
                &part.starting_lba.to_string(),
            )?;
        }
    };

    // Finally, reset the device
    qdl_dev.reset_on_drop = false;
    firehose_reset(&mut qdl_dev, &reset_mode, 0)?;

    println!(
        "{} {}",
        "All went well! Resetting to".green(),
        reset_mode.to_string().bright_yellow()
    );

    Ok(())
}
