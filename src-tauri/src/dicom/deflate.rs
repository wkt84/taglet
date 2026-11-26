//! Deflated Explicit VR Little Endian (#8).
//!
//! The data set after the file meta information is compressed with Deflate (RFC 1951,
//! without a zlib or gzip wrapper; PS3.5 A.5). dicom-rs 0.7 knows this transfer syntax
//! but can neither read nor write it, so Taglet inflates the data set itself: reading
//! sees "the file meta information, then the inflated data set", which is Explicit VR
//! Little Endian, and saving splices into that and deflates the result again.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

use dicom_dictionary_std::uids;
use dicom_object::FileMetaTable;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

pub fn is_deflated(transfer_syntax: &str) -> bool {
    transfer_syntax.trim_end_matches(['\0', ' ']) == uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN
}

/// A deflated Part 10 file with its data set inflated.
pub struct InflatedFile {
    pub meta: FileMetaTable,
    /// The preamble, "DICM" and the file meta information, as in the file, followed by
    /// the inflated data set.
    pub bytes: Vec<u8>,
    /// Where the data set starts in `bytes` (and in the file).
    pub dataset_start: usize,
}

impl InflatedFile {
    pub fn dataset(&self) -> &[u8] {
        &self.bytes[self.dataset_start..]
    }
}

/// Read a Part 10 file (with a preamble) whose transfer syntax is Deflated Explicit VR
/// Little Endian, inflating its data set. `Ok(None)` for any other file, including
/// files that are not Part 10 or whose file meta information cannot be read: those are
/// left to the usual readers, which report their own errors.
pub fn read_inflated(path: &str) -> Result<Option<InflatedFile>, String> {
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let mut reader = BufReader::new(file);
    let mut preamble = [0_u8; 128];
    if reader.read_exact(&mut preamble).is_err() {
        return Ok(None);
    }
    // `FileMetaTable::from_reader` starts at "DICM" and reads exactly the group.
    let Ok(meta) = FileMetaTable::from_reader(&mut reader) else {
        return Ok(None);
    };
    if !is_deflated(meta.transfer_syntax()) {
        return Ok(None);
    }
    let dataset_start = reader
        .stream_position()
        .map_err(|error| error.to_string())? as usize;
    let mut bytes = vec![0_u8; dataset_start];
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    reader
        .read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    inflate_into(&mut reader, &mut bytes)?;
    Ok(Some(InflatedFile {
        meta,
        bytes,
        dataset_start,
    }))
}

/// Inflate a deflated data set from `source`, appending it to `out`. A byte after the
/// end of the Deflate stream (the padding to an even length) is ignored.
pub fn inflate_into(source: &mut dyn Read, out: &mut Vec<u8>) -> Result<(), String> {
    DeflateDecoder::new(source)
        .read_to_end(out)
        .map_err(|error| format!("Could not inflate the deflated data set: {error}"))?;
    Ok(())
}

/// Deflate a data set, padded to an even length with a trailing zero (PS3.5 A.5).
pub fn deflate(dataset: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(dataset)
        .map_err(|error| error.to_string())?;
    let mut out = encoder.finish().map_err(|error| error.to_string())?;
    if out.len() % 2 == 1 {
        out.push(0);
    }
    Ok(out)
}
