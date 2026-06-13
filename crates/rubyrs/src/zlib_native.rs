//! Zlib host primitives backed by `flate2` (pure-Rust miniz_oxide
//! backend — wasm-safe). The `stdlib` Zlib veneer
//! (`stdlib_vendor/zlib.rb`) does the Ruby-side window-bits / format
//! selection and routes the actual (de)compression here. rack reaches
//! this via Deflater (gzip/deflate responses) and Static (serve `.gz`).
#![cfg(feature = "stdlib")]

use std::io::{Read, Write};

use flate2::Compression;

/// Map CRuby's level (`-1` = default ≈ 6, `0` = store, `1..9`) onto
/// flate2's `Compression`.
fn level(lvl: i64) -> Compression {
    if lvl < 0 {
        Compression::default()
    } else {
        Compression::new((lvl as u32).min(9))
    }
}

/// Raw DEFLATE (no zlib/gzip header) — `Zlib::Deflate.new(lvl,
/// -MAX_WBITS)`. rack's `deflate` content-encoding.
pub(crate) fn deflate_raw(data: &[u8], lvl: i64) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), level(lvl));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Raw INFLATE — `Zlib::Inflate.new(-MAX_WBITS)`.
pub(crate) fn inflate_raw(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = flate2::read::DeflateDecoder::new(data);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// zlib-wrapped DEFLATE (2-byte header + Adler32) — `Zlib::Deflate`
/// default window bits.
pub(crate) fn deflate_zlib(data: &[u8], lvl: i64) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), level(lvl));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// zlib-wrapped INFLATE.
pub(crate) fn inflate_zlib(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// gzip compress with an explicit header mtime — `Zlib::GzipWriter`.
pub(crate) fn gzip(data: &[u8], lvl: i64, mtime: u32) -> Vec<u8> {
    let mut e = flate2::GzBuilder::new()
        .mtime(mtime)
        .write(Vec::new(), level(lvl));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// gzip decompress, returning `(bytes, header_mtime)` —
/// `Zlib::GzipReader#read` + `#mtime`.
pub(crate) fn gunzip(data: &[u8]) -> Result<(Vec<u8>, u32), String> {
    let mut d = flate2::read::GzDecoder::new(data);
    let mtime = d.header().map(|h| h.mtime()).unwrap_or(0);
    let mut out = Vec::new();
    d.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok((out, mtime))
}

/// Auto-detecting INFLATE — `Zlib::Inflate.new(32 + MAX_WBITS)`
/// accepts either a gzip stream (magic `1f 8b`) or a zlib stream.
pub(crate) fn inflate_auto(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        gunzip(data).map(|(b, _)| b)
    } else {
        inflate_zlib(data)
    }
}
