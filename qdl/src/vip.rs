use anyhow::Result;
use bincode::serialize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, Read, Write},
    path::Path,
};
use xmltree::XMLNode;

use crate::firehose_xml_setup;

pub fn calc_hashes(xml_path: &Path, send_buffer_size: usize) -> Result<Vec<Vec<u8>>> {
    let program_file = fs::read(xml_path)?;
    let xml = xmltree::Element::parse(&program_file[..])?;

    let mut digests: Vec<Vec<u8>> = vec![];
    for node in xml.children.iter() {
        if let XMLNode::Element(e) = node {
            let args: Vec<(&str, &str)> = e
                .attributes
                .as_slice()
                .into_iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            let packet = firehose_xml_setup(&e.name.to_ascii_lowercase(), &args)?;

            let hash = Sha256::digest(packet);
            digests.push(hash.to_vec());

            // SAFETY: if the program file exists, it must have a parent dir
            let xml_dir = xml_path.parent().unwrap();
            if let Some(filename) = &e.attributes.get("filename") {
                let file_path = xml_dir.join(filename);

                if filename.is_empty() {
                    continue;
                } else {
                    if !file_path.exists() {
                        println!("WARNING: {filename} doesn't exist - assuming that's intended");
                        continue;
                    }

                    println!("Processing {filename}...");
                }
                let mut buf = vec![0u8; send_buffer_size];
                let mut br = BufReader::new(File::open(file_path)?);
                loop {
                    let n = br.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    digests.push(Sha256::digest(&buf[..n]).to_vec());
                }
            }
        }
    }

    Ok(digests)
}

#[derive(Serialize)]
#[repr(C)]
struct MbnHeaderV3 {
    image_id: u32,
    header_ver_num: u32,
    image_src: u32,
    image_dest_ptr: u32,
    image_size: u32,
    code_size: u32,
    signature_ptr: u32,
    signature_size: u32,
    cert_chain_ptr: u32,
    cert_chain_size: u32,
}

/// The number of hashes in a single table of digests
/// The 54th entry is reserved for hashing the other 53
const MAX_DIGESTS_PER_FILE: usize = 54 - 1;

pub fn gen_hash_tables(
    digests: Vec<Vec<u8>>,
    output_dir: &Path,
    max_table_size: usize,
) -> Result<()> {
    let chained_table_elem_count = max_table_size / Sha256::output_size();
    let mut processed_chained_tables: Vec<Vec<u8>> = vec![];
    let primary_digests: Vec<Vec<u8>>;
    let aux_digests: Vec<Vec<u8>>;

    if digests.len() >= MAX_DIGESTS_PER_FILE {
        primary_digests = digests[..MAX_DIGESTS_PER_FILE].to_vec();
        aux_digests = digests[MAX_DIGESTS_PER_FILE..].to_vec();
    } else {
        primary_digests = digests;
        aux_digests = vec![];
    }

    // The last digest in the table is the hash of the next table
    // Add a - 1 to accomodate for the last entry being the next table's hash
    let chained_tables = aux_digests.chunks(chained_table_elem_count - 1);
    let mut hash: Vec<u8> = vec![];

    // Note this loop starts from the last table
    for tbl in chained_tables.rev() {
        // Add the digests
        let mut entry = tbl.concat();

        // Add the hash of the table that follows (add nothing in the first iteration)
        // TODO: use the explicit init/update/finalize to avoid sad copies
        entry.append(&mut hash);

        processed_chained_tables.push(entry);

        // Hash the current table to include in the next one
        // The variable will contain the hash of the first table at the end of
        // execution (may be an empty vector)
        hash = Sha256::digest(tbl.concat()).to_vec();
    }

    let mbn_table_size = match aux_digests.is_empty() {
        true => size_of_val(&primary_digests),
        false => size_of_val(&primary_digests) + Sha256::output_size(),
    };

    let hdr = MbnHeaderV3 {
        image_id: 26,
        header_ver_num: 3,
        // Offset of the first hash table
        image_src: 40,
        image_dest_ptr: 0,
        image_size: mbn_table_size as u32,
        code_size: mbn_table_size as u32,
        // The file will be signed externally, leave signature fields empty
        signature_ptr: 0,
        signature_size: 0,
        cert_chain_ptr: 0,
        cert_chain_size: 0,
    };

    if !output_dir.exists() {
        std::fs::create_dir(output_dir)?;
    }

    let mut mbn = File::create(output_dir.join("signme.mbn"))?;
    mbn.write_all(&serialize(&hdr)?)?;
    mbn.write_all(&primary_digests.concat())?;
    if let Some(hash) = processed_chained_tables.last() {
        mbn.write_all(hash)?;

        let mut aux_tbl_file = File::create(output_dir.join("tables.bin"))?;
        aux_tbl_file.write_all(&aux_digests.concat())?;
    }

    Ok(())
}
