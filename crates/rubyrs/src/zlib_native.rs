//! Zlib host primitives backed by `flate2` (pure-Rust miniz_oxide
//! backend — wasm-safe). The `stdlib` Zlib veneer
//! (`stdlib_vendor/zlib.rb`) does the Ruby-side window-bits / format
//! selection and routes the actual (de)compression here. rack reaches
//! this via Deflater (gzip/deflate responses) and Static (serve `.gz`).
#![cfg(feature = "stdlib")]

use std::cell::{Cell, RefCell};
use std::io::{Read, Write};

use flate2::{Compress, Compression, Crc, Decompress, FlushCompress, FlushDecompress, Status};

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

/// Standard IEEE CRC-32 (`Zlib.crc32`). `init` continues a prior crc
/// (`Zlib.crc32(b, Zlib.crc32(a)) == Zlib.crc32(a + b)`); pass 0 for a
/// fresh checksum. Bitwise (no table) — fine for the small inputs
/// `Zlib.crc32` is used on (RuboCop's result-cache file digest).
pub(crate) fn crc32(data: &[u8], init: u32) -> u32 {
    let mut crc = !init;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
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

// ----- stateful streaming handles (Zlib::GzipWriter / Zlib::Inflate) -----
//
// The miniz_oxide flate2 backend (wasm-safe, no `any_zlib`) exposes only
// raw / zlib `Compress`/`Decompress`, so gzip framing (10-byte header +
// CRC32 + ISIZE trailer) is done by hand here. These back the INCREMENTAL
// path that rack's Deflater relies on: with `:sync` true (the default) it
// calls `gzip.write(part); gzip.flush` per source chunk, and a consumer
// may stop reading early (the "client aborts reading" spec) — so each
// `flush` must emit a Z_SYNC_FLUSH boundary the consumer's stateful
// `Zlib::Inflate` can decode immediately, rather than buffering the whole
// body until `finish`.
//
// Handles live in a thread-local slab keyed by a monotonic id (they hold
// no GC values and are created + finished within a single response).
// Streams are freed explicitly on `finish`/`close`; ids are never reused,
// so a stale id can't alias a fresh stream.

struct GzDeflateState {
    comp: Compress,
    crc: Crc,
    mtime: u32,
    header_written: bool,
}

struct InflateState {
    decomp: Option<Decompress>,
    /// `Zlib::Inflate.new(window_bits)`: <0 raw, 8..=15 zlib,
    /// 16..=31 gzip, >=32 auto-detect gzip-vs-zlib from the magic.
    auto: bool,
    zlib_header: bool,
    is_gzip: bool,
    /// Undecoded input carried across `push` calls (a gzip header may
    /// not be complete in the first chunk; raw inflate may not consume
    /// everything if the output cap is hit).
    pending: Vec<u8>,
}

enum ZStream {
    Deflate(GzDeflateState),
    Inflate(InflateState),
}

thread_local! {
    static STREAMS: RefCell<std::collections::HashMap<u64, ZStream>> =
        RefCell::new(std::collections::HashMap::new());
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

fn alloc_stream(s: ZStream) -> u64 {
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    STREAMS.with(|m| m.borrow_mut().insert(id, s));
    id
}

/// A standard 10-byte gzip header with `FLG=0` (no extra fields) and the
/// supplied mtime; `OS=0xff` (unknown), matching flate2's `GzBuilder`.
fn gzip_header(mtime: u32) -> [u8; 10] {
    let m = mtime.to_le_bytes();
    [0x1f, 0x8b, 0x08, 0x00, m[0], m[1], m[2], m[3], 0x00, 0xff]
}

/// Open a streaming gzip COMPRESSOR; returns its handle id.
pub(crate) fn gz_deflate_new(lvl: i64, mtime: u32) -> u64 {
    alloc_stream(ZStream::Deflate(GzDeflateState {
        comp: Compress::new(level(lvl), false), // raw DEFLATE; gzip frame added by hand
        crc: Crc::new(),
        mtime,
        header_written: false,
    }))
}

/// Feed `data` to a streaming gzip compressor. `flush`: 0 = none (buffer),
/// 1 = sync flush (emit a decodable boundary), 2 = finish (final block +
/// CRC/ISIZE trailer, then the stream is freed). Returns the compressed
/// bytes produced by this call (to be written to the wrapped IO).
pub(crate) fn gz_deflate_push(id: u64, data: &[u8], flush: i64) -> Vec<u8> {
    STREAMS.with(|m| {
        let mut map = m.borrow_mut();
        let Some(ZStream::Deflate(st)) = map.get_mut(&id) else { return Vec::new(); };
        let mut out = Vec::new();
        if !st.header_written {
            out.extend_from_slice(&gzip_header(st.mtime));
            st.header_written = true;
        }
        st.crc.update(data);
        let mode = match flush {
            1 => FlushCompress::Sync,
            2 => FlushCompress::Finish,
            _ => FlushCompress::None,
        };
        // `compress_vec` writes only into the output Vec's SPARE
        // capacity, so we `reserve` each iteration (a fresh empty Vec
        // has 0 spare → would emit nothing). zlib flush idiom: keep
        // calling while the output buffer fills completely (more
        // pending output); once a call leaves spare room AND all input
        // is consumed, the (de)compressor has emitted everything for
        // this flush. We must NOT key termination on "produced == 0":
        // Z_SYNC_FLUSH re-emits an empty sync marker on every call, so
        // that would spin forever. FINISH loops until StreamEnd.
        let finishing = matches!(mode, FlushCompress::Finish);
        let mut input = data;
        loop {
            out.reserve(input.len().max(512) + 64);
            let cap = out.capacity();
            let before_in = st.comp.total_in();
            let before_out = st.comp.total_out();
            let status = match st.comp.compress_vec(input, &mut out, mode) {
                Ok(s) => s,
                Err(_) => break,
            };
            let consumed = (st.comp.total_in() - before_in) as usize;
            let produced = st.comp.total_out() - before_out;
            input = &input[consumed..];
            if matches!(status, Status::StreamEnd) {
                break; // FINISH complete
            }
            // Output buffer not filled to capacity ⇒ all pending output
            // for this flush has been written. Stop (unless finishing,
            // which must run to StreamEnd) once input is also drained.
            if out.len() < cap && input.is_empty() && !finishing {
                break;
            }
            // Defensive no-progress guard (shouldn't trigger once spare
            // is guaranteed, but prevents any spin).
            if consumed == 0 && produced == 0 && out.len() < cap {
                break;
            }
        }
        if flush == 2 {
            out.extend_from_slice(&st.crc.sum().to_le_bytes());
            out.extend_from_slice(&(st.comp.total_in() as u32).to_le_bytes());
            map.remove(&id);
        }
        out
    })
}

/// Open a streaming INFLATE decompressor for the given window-bits mode;
/// returns its handle id.
pub(crate) fn inflate_stream_new(wbits: i64) -> u64 {
    let auto = wbits >= 32;
    let is_gzip = (16..=31).contains(&wbits);
    let zlib_header = (8..=15).contains(&wbits);
    alloc_stream(ZStream::Inflate(InflateState {
        decomp: None,
        auto,
        zlib_header,
        is_gzip,
        pending: Vec::new(),
    }))
}

/// Length of a gzip header at the front of `buf`, or `None` if `buf`
/// doesn't yet hold the whole header (FLG extra fields can extend it).
fn gzip_header_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 10 {
        return None;
    }
    let flg = buf[3];
    let mut pos = 10usize;
    if flg & 0x04 != 0 {
        // FEXTRA: 2-byte little-endian length + that many bytes.
        if buf.len() < pos + 2 {
            return None;
        }
        let xlen = u16::from_le_bytes([buf[pos], buf[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    if flg & 0x08 != 0 {
        // FNAME: NUL-terminated.
        pos = skip_cstr(buf, pos)?;
    }
    if flg & 0x10 != 0 {
        // FCOMMENT: NUL-terminated.
        pos = skip_cstr(buf, pos)?;
    }
    if flg & 0x02 != 0 {
        // FHCRC: 2 bytes.
        pos += 2;
    }
    if buf.len() < pos {
        return None;
    }
    Some(pos)
}

fn skip_cstr(buf: &[u8], start: usize) -> Option<usize> {
    let nul = buf[start..].iter().position(|&b| b == 0)?;
    Some(start + nul + 1)
}

/// Push `data` into a streaming decompressor; returns the bytes decoded so
/// far (may be empty if more input is needed). The gzip CRC/ISIZE trailer
/// is not validated — a caller that stops reading early never sees it.
pub(crate) fn inflate_stream_push(id: u64, data: &[u8]) -> Result<Vec<u8>, String> {
    STREAMS.with(|m| {
        let mut map = m.borrow_mut();
        let Some(ZStream::Inflate(st)) = map.get_mut(&id) else { return Ok(Vec::new()); };
        st.pending.extend_from_slice(data);
        let mut out = Vec::new();
        if st.decomp.is_none() {
            if st.auto {
                if st.pending.len() < 2 {
                    return Ok(out); // need the magic bytes
                }
                st.is_gzip = st.pending[0] == 0x1f && st.pending[1] == 0x8b;
            }
            if st.is_gzip {
                match gzip_header_len(&st.pending) {
                    Some(n) => {
                        st.pending.drain(0..n);
                        st.decomp = Some(Decompress::new(false)); // raw body
                    }
                    None => return Ok(out), // header not complete yet
                }
            } else {
                st.decomp = Some(Decompress::new(st.zlib_header));
            }
        }
        let Some(d) = st.decomp.as_mut() else { return Ok(out); };
        loop {
            if st.pending.is_empty() {
                break;
            }
            // `decompress_vec` writes only into spare capacity; reserve
            // generously since inflate expands (and a fresh Vec has 0
            // spare → would decode nothing).
            out.reserve(st.pending.len().max(512) * 4 + 1024);
            let cap = out.capacity();
            let before_in = d.total_in();
            let before_out = d.total_out();
            let status = d
                .decompress_vec(&st.pending, &mut out, FlushDecompress::None)
                .map_err(|e| e.to_string())?;
            let consumed = (d.total_in() - before_in) as usize;
            st.pending.drain(0..consumed);
            let produced = d.total_out() - before_out;
            if matches!(status, Status::StreamEnd) {
                break; // body done; any trailer left in `pending` is ignored
            }
            // Output buffer left room ⇒ everything available was
            // decoded; wait for more input (or stop on no progress).
            if out.len() < cap {
                break;
            }
            if consumed == 0 && produced == 0 {
                break; // needs more input
            }
        }
        Ok(out)
    })
}

/// Free a streaming handle (Deflate or Inflate). No-op for an unknown id.
pub(crate) fn stream_free(id: u64) {
    STREAMS.with(|m| {
        m.borrow_mut().remove(&id);
    });
}
