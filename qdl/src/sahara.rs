// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) Qualcomm Technologies, Inc. and/or its subsidiaries.
use anstream::println;
use owo_colors::OwoColorize;
use pbr::{ProgressBar, Units};
use std::{
    cmp::min,
    ffi::CStr,
    fs::File,
    io::{Read, Write},
    mem::{self, size_of_val},
};

use anyhow::{Result, bail};

use bincode::serialize;
use serde::{self, Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::types::{QdlBackend, QdlChan};

const SAHARA_STATUS_SUCCESS: u32 = 0;

#[derive(Copy, Clone, Debug, PartialEq, Deserialize_repr, Serialize_repr)]
#[repr(u32)]
pub enum SaharaMode {
    WaitingForImage = 0x0,
    MemoryDebug = 0x2,
    Command = 0x3,
}

#[derive(Copy, Clone, Debug, Deserialize_repr, Serialize_repr)]
#[repr(u32)]
pub enum SaharaCmdModeCmd {
    Nop = 0x0,
    ReadSerialNum = 0x1,
    ReadHwId = 0x2,
    ReadOemKeyHash = 0x3,
}

// (De)serialize_repr works on C-like enums (match by value instead of entry index)
#[derive(Copy, Clone, Debug, PartialEq, Deserialize_repr, Serialize_repr)]
#[repr(u32)]
pub enum SaharaCmd {
    SaharaHello = 0x1,      /* Device sends HELLO at init */
    SaharaHelloResp = 0x2,  /* Host responds with a version number */
    SaharaReadData = 0x3,   /* Device requests an image to read */
    SaharaEndOfImage = 0x4, /* Device signals EOF */
    SaharaDone = 0x5,       /* Host reassures the Device EOF was understood */
    SaharaDoneResp = 0x6,   /* Device requires more images if status == 0 */
    SaharaReset = 0x7,      /* Host asks Device to stop the current process */
    SaharaResetResp = 0x8,  /* Device acks the reset */
    /* Proto >= 2.0 */
    SaharaMemDebug = 0x9,
    SaharaMemRead = 0xa,
    /* Proto >= 2.1 */
    SaharaCommandReady = 0xb,
    SaharaSwitchMode = 0xc,
    SaharaExecute = 0xd,
    SaharaExecuteResp = 0xe,
    SaharaExecuteData = 0xf,
    /* Proto >= 2.5 */
    SaharaMemDebug64 = 0x10,
    SaharaMemRead64 = 0x11,
    /* Proto >= 2.8 */
    SaharaReadData64 = 0x12,
    /* Proto >= 2.9 */
    SaharaResetState = 0x13,
    /* Proto >= 3.0 */
    SaharaWriteData = 0x14,

    /* This isn't part of spec, but rather "<?xm" (XML) suggesting Sahara mode is over */
    SaharaXML = 0x6d783f3c,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct HelloReq {
    pub ver: u32,
    pub compatible: u32,
    pub max_len: u32,
    pub mode: SaharaMode,
    unk0: u32,
    unk1: u32,
    unk2: u32,
    unk3: u32,
    unk4: u32,
    unk5: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct HelloResp {
    ver: u32,
    compatible: u32,
    status: u32,
    mode: SaharaMode,
    unk0: u32,
    unk1: u32,
    unk2: u32,
    unk3: u32,
    unk4: u32,
    unk5: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct ReadReq {
    pub image: u32,
    pub offset: u32,
    pub len: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct Eoi {
    pub image: u32,
    pub status: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct DoneReq {}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct DoneResp {
    pub status: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct ResetReq {}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct ResetResp {}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct CommandReady {}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct SwitchMode {
    mode: SaharaMode,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct ExecResp {
    pub command: SaharaCmdModeCmd,
    pub len: u32,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct Debug64Req {
    addr: u64,
    len: u64,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct ReadMem64Req {
    addr: u64,
    len: u64,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct ReadData64Req {
    pub image: u64,
    pub offset: u64,
    pub len: u64,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
#[repr(C)]
pub enum SaharaPacketBody {
    // Packets accepted in WaitingForImage mode
    HelloReq(HelloReq),
    HelloResp(HelloResp),
    ReadReq(ReadReq),
    Eoi(Eoi),
    DoneReq(DoneReq),
    DoneResp(DoneResp),
    ResetReq(ResetReq),
    ResetResp(ResetResp),
    CommandReady(CommandReady),
    SwitchMode(SwitchMode),
    ExecResp(ExecResp),
    Debug64Req(Debug64Req),
    ReadMem64Req(ReadMem64Req),
    ReadData64Req(ReadData64Req),

    // Packets accepted in Command mode
    Command(SaharaCmdModeCmd),
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct SaharaPacket {
    pub cmd: SaharaCmd,
    pub len: u32,
    pub body: SaharaPacketBody,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[repr(C)]
pub struct RamdumpTable64 {
    save_pref: u64,
    base: u64,
    len: u64,
    description: [u8; 20],
    filename: [u8; 20],
}

fn get_u32(buf: &[u8]) -> u32 {
    u32::from_le_bytes(buf.try_into().unwrap())
}

pub fn sahara_send_img_to_device<T: Read + Write>(
    channel: &mut T,
    img_arr: &mut [Vec<u8>],
    image_idx: u64,
    image_offset: u64,
    image_len: u64,
) -> Result<usize, anyhow::Error> {
    let image = if img_arr.len() == 1 { 0 } else { image_idx };
    let buf = &mut img_arr[image as usize];
    if (image_offset + image_len) as usize > buf.len() {
        bail!(
            "Attempted OOB read {} > {}",
            image_offset + image_len,
            buf.len()
        );
    }

    channel
        .write(&buf[image_offset as usize..(image_offset + image_len) as usize])
        .map_err(|e| e.into())
}

fn sahara_send_generic<T: Read + Write>(
    channel: &mut T,
    cmd: SaharaCmd,
    body: SaharaPacketBody,
    body_len: usize,
) -> Result<usize> {
    let pkt = SaharaPacket {
        cmd,
        len: (size_of_val(&cmd) + size_of::<u32>() + body_len) as u32,
        body,
    };

    channel
        .write(&serialize(&pkt).expect("Error serializing packet"))
        .map_err(|e| e.into())
}

const SAHARA_VERSION: u32 = 2;
pub fn sahara_send_hello_rsp<T: Read + Write>(channel: &mut T, mode: SaharaMode) -> Result<usize> {
    let data = HelloResp {
        ver: SAHARA_VERSION,
        compatible: 1,
        status: SAHARA_STATUS_SUCCESS,
        mode,
        unk0: 0,
        unk1: 0,
        unk2: 0,
        unk3: 0,
        unk4: 0,
        unk5: 0,
    };

    sahara_send_generic(
        channel,
        SaharaCmd::SaharaHelloResp,
        SaharaPacketBody::HelloResp(data),
        size_of_val(&data),
    )
}

pub fn sahara_send_done<T: Read + Write>(channel: &mut T) -> Result<usize> {
    let data = DoneReq {};

    sahara_send_generic(
        channel,
        SaharaCmd::SaharaDone,
        SaharaPacketBody::DoneReq(data),
        size_of_val(&data),
    )
}

pub fn sahara_send_cmd_exec<T: Read + Write>(
    channel: &mut T,
    command: SaharaCmdModeCmd,
) -> Result<usize, anyhow::Error> {
    sahara_send_generic(
        channel,
        SaharaCmd::SaharaExecute,
        SaharaPacketBody::Command(command),
        size_of_val(&command),
    )
}

pub fn sahara_send_cmd_data<T: Read + Write>(
    channel: &mut T,
    command: SaharaCmdModeCmd,
) -> Result<usize, anyhow::Error> {
    sahara_send_generic(
        channel,
        SaharaCmd::SaharaExecuteData,
        SaharaPacketBody::Command(command),
        size_of_val(&command),
    )
}

pub fn sahara_reset<T: Read + Write>(channel: &mut T) -> Result<usize, anyhow::Error> {
    let data = ResetReq {};

    sahara_send_generic(
        channel,
        SaharaCmd::SaharaReset,
        SaharaPacketBody::ResetReq(data),
        size_of_val(&data),
    )
}

pub fn sahara_switch_mode<T: Read + Write>(
    channel: &mut T,
    mode: SaharaMode,
) -> Result<usize, anyhow::Error> {
    let data = SwitchMode { mode };

    sahara_send_generic(
        channel,
        SaharaCmd::SaharaSwitchMode,
        SaharaPacketBody::SwitchMode(data),
        size_of_val(&data),
    )
}

pub fn sahara_get_ramdump_tbl<T: Read + Write>(
    channel: &mut T,
    addr: u64,
    len: u64,
    verbose: bool,
) -> Result<Vec<RamdumpTable64>, anyhow::Error> {
    let data = ReadMem64Req { addr, len };

    sahara_send_generic(
        channel,
        SaharaCmd::SaharaMemRead64,
        SaharaPacketBody::ReadMem64Req(data),
        size_of_val(&data),
    )?;

    let entry_size = size_of::<RamdumpTable64>();
    let num_chunks = len as usize / entry_size;
    let mut tbl = Vec::<RamdumpTable64>::with_capacity(num_chunks);

    let mut buf = vec![0u8; len as usize];
    channel.read_exact(&mut buf)?;

    if verbose {
        println!("Available images:");
    }
    for i in 0..num_chunks {
        let entry = bincode::deserialize::<RamdumpTable64>(&buf[i * entry_size..])?;
        tbl.push(entry);
        if verbose {
            println!(
                "\t{} (0x{:x} @ 0x{:x}){}",
                String::from_utf8(entry.filename.to_vec())?,
                entry.len,
                entry.base,
                match entry.save_pref {
                    0 => "",
                    _ => " *",
                }
            );
        }
    }

    Ok(tbl)
}

fn sahara_dump_region<T: QdlChan>(
    channel: &mut T,
    entry: RamdumpTable64,
    output: &mut impl Write,
) -> Result<()> {
    let mut pb = ProgressBar::new(entry.len);
    pb.show_time_left = true;
    pb.message(&format!(
        "Dumping {}: ",
        String::from_utf8(entry.filename.to_vec())?
    ));
    pb.set_units(Units::Bytes);

    let mut bytes_read = 0usize;
    while bytes_read < entry.len as usize {
        let chunk_size = min(4096, entry.len as usize - bytes_read);
        let mut buf = vec![0u8; chunk_size];
        let data = ReadMem64Req {
            addr: entry.base + bytes_read as u64,
            len: chunk_size as u64,
        };

        sahara_send_generic(
            channel,
            SaharaCmd::SaharaMemRead64,
            SaharaPacketBody::ReadMem64Req(data),
            size_of_val(&data),
        )?;
        channel.flush()?;

        bytes_read += channel.read(&mut buf)?;

        // Issue a dummy read to consume the ZLP
        if channel.fh_config().backend == QdlBackend::Usb && buf.len().is_multiple_of(512) {
            let _ = channel.read(&mut []);
        }

        pb.set(bytes_read as u64);
        let _ = output.write(&buf)?;
    }

    Ok(())
}

pub fn sahara_dump_regions<T: QdlChan>(
    channel: &mut T,
    dump_tbl: Vec<RamdumpTable64>,
    regions_to_dump: Vec<String>,
) -> Result<()> {
    // Make all of them lowercase for better UX
    let regions_to_dump = regions_to_dump
        .iter()
        .map(|rname| rname.to_ascii_lowercase())
        .collect::<Vec<String>>();

    std::fs::create_dir_all("ramdump/")?;
    let filtered_list: Vec<RamdumpTable64> = match regions_to_dump.len() {
        // Dump everything with save_pref == true if no argument was provided
        0 => dump_tbl
            .iter()
            .filter(|e| e.save_pref != 0)
            .copied()
            .collect(),
        _ => dump_tbl
            .iter()
            .filter(|dump_entry| {
                regions_to_dump.contains(
                    &String::from_utf8(dump_entry.filename.to_vec())
                        .unwrap_or("".to_owned())
                        .to_ascii_lowercase()
                        .split('.') // Ignore file extensions proposed by ramdump
                        .next()
                        .unwrap_or("")
                        .to_owned(),
                )
            })
            .copied()
            .collect(),
    };
    for entry in filtered_list {
        let fname = CStr::from_bytes_until_nul(&entry.filename)
            .unwrap()
            .to_str()?
            .to_owned();

        let mut f = File::create(std::path::Path::new(&format!("ramdump/{fname}")))?;
        sahara_dump_region(channel, entry, &mut f)?;
    }

    Ok(())
}

pub fn sahara_run<T: QdlChan>(
    channel: &mut T,
    sahara_mode: SaharaMode,
    sahara_command: Option<SaharaCmdModeCmd>,
    images: &mut [Vec<u8>],
    filenames: Vec<String>,
    verbose: bool,
) -> Result<Vec<u8>> {
    let mut buf = vec![0; 4096];

    loop {
        let bytes_read = channel.read(&mut buf[..])?;
        let pkt = sahara_parse_packet(&buf[..bytes_read], verbose);
        let pktsize = size_of_val(&pkt.cmd) + size_of_val(&pkt.len);

        match pkt.cmd {
            SaharaCmd::SaharaHello => {
                if let SaharaPacketBody::HelloReq(req) = pkt.body {
                    assert_eq!(pkt.len as usize, pktsize + mem::size_of::<HelloReq>());

                    // MemoryDebug mode can only be entered if the device offers it
                    let mode = if sahara_mode == SaharaMode::MemoryDebug
                        && req.mode == SaharaMode::MemoryDebug
                    {
                        SaharaMode::MemoryDebug
                    } else {
                        sahara_mode
                    };
                    sahara_send_hello_rsp(channel, mode)?;
                }
            }
            SaharaCmd::SaharaReadData => {
                if let SaharaPacketBody::ReadReq(rr) = pkt.body {
                    assert_eq!(pkt.len as usize, pktsize + mem::size_of::<ReadReq>());
                    sahara_send_img_to_device(
                        channel,
                        images,
                        rr.image as u64,
                        rr.offset as u64,
                        rr.len as u64,
                    )?;
                }
            }
            SaharaCmd::SaharaEndOfImage => {
                if let SaharaPacketBody::Eoi(req) = pkt.body {
                    assert_eq!(pkt.len as usize, pktsize + mem::size_of::<Eoi>());

                    if req.status == 0 {
                        sahara_send_done(channel)?;
                    } else {
                        eprintln!("Received unsuccessful End of Image packet");
                        return Ok(vec![]);
                    }
                }
            }
            SaharaCmd::SaharaDoneResp => {
                if let SaharaPacketBody::DoneResp(req) = pkt.body
                    && (req.status == 1 /* COMPLETE */ /* 8916 bug */ ||
                     images.len() == 1)
                {
                    println!("{}", "Loader sent. Hack away!".green());
                    return Ok(vec![]);
                }
            }
            SaharaCmd::SaharaCommandReady => {
                assert_eq!(pkt.len as usize, pktsize);
                match sahara_command {
                    Some(cmd) => sahara_send_cmd_exec(channel, cmd),
                    None => bail!("Missing sahara command"),
                }?;
            }
            SaharaCmd::SaharaExecuteResp => {
                if let SaharaPacketBody::ExecResp(resp) = pkt.body {
                    let mut resp_buf = vec![0u8; resp.len as usize];

                    // Indicate we're ready to receive the requested amount of data
                    sahara_send_cmd_data(channel, resp.command)?;

                    let resp_len = channel.read(&mut resp_buf)?;
                    assert_eq!(resp_len, resp.len as usize);

                    // Got everything we want, exit command mode
                    sahara_switch_mode(channel, SaharaMode::WaitingForImage)?;

                    return Ok(resp_buf);
                }
            }
            SaharaCmd::SaharaMemDebug64 => {
                if let SaharaPacketBody::Debug64Req(req) = pkt.body {
                    assert_eq!(pkt.len as usize, pktsize + mem::size_of::<Debug64Req>());

                    // Receive the dump info table
                    let dump_tbl = sahara_get_ramdump_tbl(channel, req.addr, req.len, verbose)?;

                    // Grab some (possibly all) of the available regions
                    sahara_dump_regions(channel, dump_tbl, filenames)?;

                    return Ok(vec![]);
                }
            }
            SaharaCmd::SaharaReadData64 => {
                if let SaharaPacketBody::ReadData64Req(rr) = pkt.body {
                    assert_eq!(pkt.len as usize, pktsize + mem::size_of::<ReadData64Req>());
                    sahara_send_img_to_device(channel, images, rr.image, rr.offset, rr.len)?;
                }
            }
            SaharaCmd::SaharaResetResp => {
                assert_eq!(pkt.len as usize, pktsize);
            }
            SaharaCmd::SaharaXML => {
                // Todo: make this optionally "fine"
                println!("Device booted into the loader already");
                return Ok(vec![]);
            }
            _ => todo!("Got packet {:?}", pkt),
        }
    }
}

fn sahara_parse_packet(buf: &[u8], verbose: bool) -> SaharaPacket {
    let cmd = bincode::deserialize::<SaharaCmd>(&buf[0..4])
        .unwrap_or_else(|_| panic!("Got unknown command {}", get_u32(&buf[0..4])));

    let ret = SaharaPacket {
        cmd,
        len: get_u32(&buf[4..8]),
        body: match cmd {
            SaharaCmd::SaharaHello => {
                SaharaPacketBody::HelloReq(bincode::deserialize::<HelloReq>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaHelloResp => {
                SaharaPacketBody::HelloResp(bincode::deserialize::<HelloResp>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaReadData => {
                SaharaPacketBody::ReadReq(bincode::deserialize::<ReadReq>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaEndOfImage => {
                SaharaPacketBody::Eoi(bincode::deserialize::<Eoi>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaDone => SaharaPacketBody::DoneReq(DoneReq {}),
            SaharaCmd::SaharaDoneResp => {
                SaharaPacketBody::DoneResp(bincode::deserialize::<DoneResp>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaResetResp => SaharaPacketBody::ResetResp(ResetResp {}),
            SaharaCmd::SaharaCommandReady => SaharaPacketBody::CommandReady(CommandReady {}),
            SaharaCmd::SaharaExecuteResp => {
                SaharaPacketBody::ExecResp(bincode::deserialize::<ExecResp>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaExecuteData => SaharaPacketBody::Command(
                bincode::deserialize::<SaharaCmdModeCmd>(&buf[8..]).unwrap(),
            ),
            SaharaCmd::SaharaMemDebug64 => {
                SaharaPacketBody::Debug64Req(bincode::deserialize::<Debug64Req>(&buf[8..]).unwrap())
            }
            SaharaCmd::SaharaMemRead64 => SaharaPacketBody::ReadMem64Req(
                bincode::deserialize::<ReadMem64Req>(&buf[8..]).unwrap(),
            ),
            SaharaCmd::SaharaReadData64 => SaharaPacketBody::ReadData64Req(
                bincode::deserialize::<ReadData64Req>(&buf[8..]).unwrap(),
            ),
            _ => todo!("Got unknown command: {:?}", buf),
        },
    };

    if verbose {
        println!("{:?}", ret);
    }

    ret
}
