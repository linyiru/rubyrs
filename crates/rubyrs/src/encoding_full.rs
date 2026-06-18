//! `_encoding_full` — ADR 0020 Tier 2: the registry behind
//! `EncodingTag::Other(u8)` and real transcoding for
//! `String#encode`.
//!
//! v1 registry: index 0 = ISO-8859-1 (Latin-1), implemented by
//! hand — Latin-1 bytes ARE the first 256 Unicode codepoints, so
//! both directions are table-free and exactly match CRuby (no
//! WHATWG quirk surface; notably the WHATWG world maps the
//! "latin1" LABEL to windows-1252, which is precisely the trap
//! the amended ADR 0020 says to probe around — hand-writing the
//! real ISO-8859-1 sidesteps it entirely). The encoding_rs-backed
//! encodings land in a follow-up behind CRuby differential
//! probes.

use crate::value::EncodingTag;

/// Registry names by `Other(idx)`. Index stability is part of the
/// tag's meaning — append only. Index 0 (ISO-8859-1) is the
/// hand-written identity codec; 1..=7 transcode through
/// `encoding_rs`. Naming follows CRUBY: the WHATWG "shift_jis"
/// table carries windows-31j semantics, so it registers as
/// Windows-31J (CRuby's alias CP932/SJIS world) and CRuby's
/// STRICT Shift_JIS is deliberately absent — `find("Shift_JIS")`
/// answering "unknown" beats silently encoding ① where strict
/// Shift_JIS would raise. Same logic gives WHATWG big5 (≈ HKSCS
/// superset on decode) the plain Big5 slot: the differential
/// fixtures pin the common plane; the HKSCS fringe is documented
/// in SUBSET.md rather than guessed at.
const NAMES: &[&str] = &[
    "ISO-8859-1",
    "Windows-1252",
    "ISO-8859-15",
    "KOI8-R",
    "Windows-31J",
    "EUC-JP",
    "GBK",
    "Big5",
    // indices 8/9 are the fixed-endianness UTF-16 forms, transcoded
    // by hand (encoding_rs is decode-only for UTF-16 — its encoder
    // emits UTF-8 per the WHATWG spec, useless for CRuby's byte
    // semantics). Unicode round-trips losslessly, so no unmappable
    // surface; only an odd byte count / lone surrogate is invalid.
    "UTF-16LE",
    "UTF-16BE",
    // index 10 is the BOM-form "UTF-16": encode emits a big-endian
    // BOM (FE FF) followed by BE code units (empty string → no BOM);
    // decode sniffs the leading BOM to pick endianness and a missing
    // BOM is an InvalidByteSequenceError (CRuby's "dummy" UTF-16).
    "UTF-16",
];

/// True for the hand-rolled UTF-16 registry indices; the bool is
/// `true` for little-endian (idx 8 = UTF-16LE), `false` for
/// big-endian (idx 9 = UTF-16BE).
fn utf16_endianness(idx: u8) -> Option<bool> {
    match idx {
        8 => Some(true),
        9 => Some(false),
        _ => None,
    }
}

/// True for any UTF-16 registry index (the fixed-endianness LE/BE
/// pair and the BOM form). Callers use it to route a failed decode
/// to InvalidByteSequenceError instead of the generic decline path.
pub(crate) fn is_utf16_family(idx: u8) -> bool {
    matches!(idx, 8 | 9 | 10)
}

/// encoding_rs codec for a registry index (None = the hand-written
/// index 0, or unregistered).
fn codec(idx: u8) -> Option<&'static encoding_rs::Encoding> {
    match idx {
        1 => Some(encoding_rs::WINDOWS_1252),
        2 => Some(encoding_rs::ISO_8859_15),
        3 => Some(encoding_rs::KOI8_R),
        4 => Some(encoding_rs::SHIFT_JIS), // WHATWG table = windows-31j semantics
        5 => Some(encoding_rs::EUC_JP),
        6 => Some(encoding_rs::GBK),
        7 => Some(encoding_rs::BIG5),
        _ => None,
    }
}

/// Canonical name for a registry index (None = unregistered index
/// — unreachable through normal construction).
pub(crate) fn name(idx: u8) -> Option<&'static str> {
    NAMES.get(idx as usize).copied()
}

/// Resolve an encoding NAME (any case; CRuby's alias fold set for
/// the registered encodings) to a tag. Returns None for names
/// this build doesn't know — the caller raises CRuby's
/// ArgumentError shape.
pub(crate) fn find(name: &str) -> Option<EncodingTag> {
    let idx = match name.to_ascii_uppercase().as_str() {
        // CRuby accepts the hyphen-less aliases too.
        "ISO-8859-1" | "ISO8859-1" => 0,
        "WINDOWS-1252" | "CP1252" => 1,
        "ISO-8859-15" | "ISO8859-15" => 2,
        "KOI8-R" | "KOI8R" => 3,
        // CRuby: alias SJIS/CP932 point at Windows-31J (NOT at the
        // strict Shift_JIS encoding, which this registry declines).
        "WINDOWS-31J" | "CP932" | "SJIS" => 4,
        "EUC-JP" | "EUCJP" => 5,
        "GBK" | "CP936" => 6,
        "BIG5" => 7,
        "UTF-16LE" | "UTF16LE" => 8,
        "UTF-16BE" | "UTF16BE" => 9,
        "UTF-16" | "UTF16" => 10,
        _ => return None,
    };
    Some(EncodingTag::Other(idx))
}

/// UTF-8 → registry encoding. `replace` carries the
/// `undef: :replace` option's replacement bytes (CRuby default
/// "?"); None = raise on the first unmappable char, reported as
/// `(codepoint, target_name)` so the caller formats CRuby's
/// "U+XXXX from UTF-8 to <enc>" message.
pub(crate) fn encode_from_utf8(
    idx: u8,
    text: &str,
    replace: Option<&[u8]>,
) -> Result<Vec<u8>, (u32, &'static str)> {
    if idx == 0 {
        let mut out = Vec::with_capacity(text.len());
        for c in text.chars() {
            let cp = c as u32;
            if cp <= 0xFF {
                out.push(cp as u8);
            } else if let Some(r) = replace {
                out.extend_from_slice(r);
            } else {
                return Err((cp, "ISO-8859-1"));
            }
        }
        return Ok(out);
    }
    // UTF-16LE/BE: every Unicode scalar value has a UTF-16 form, so
    // there's no unmappable case (`replace` is unused). `encode_utf16`
    // already expands astral chars to the correct surrogate pair.
    if let Some(le) = utf16_endianness(idx) {
        let mut out = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            let b = if le { unit.to_le_bytes() } else { unit.to_be_bytes() };
            out.extend_from_slice(&b);
        }
        return Ok(out);
    }
    // BOM-form "UTF-16": a leading big-endian BOM then BE units;
    // empty input emits no bytes (CRuby: `"".encode("UTF-16")` is "").
    if idx == 10 {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(text.len() * 2 + 2);
        out.extend_from_slice(&[0xFE, 0xFF]);
        for unit in text.encode_utf16() {
            out.extend_from_slice(&unit.to_be_bytes());
        }
        return Ok(out);
    }
    let (enc, enc_name) = match (codec(idx), name(idx)) {
        (Some(e), Some(n)) => (e, n),
        _ => return Err((0, "unregistered")),
    };
    // Per-char encoding: encoding_rs's whole-string `encode` would
    // substitute `&#NNNN;` HTML entities for unmappables — useless
    // for CRuby semantics. Char-at-a-time keeps the offender's
    // codepoint for the error message and lets `replace` splice
    // exactly like CRuby's undef: :replace.
    let mut out = Vec::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for c in text.chars() {
        let s = c.encode_utf8(&mut buf);
        let (bytes, _, unmappable) = enc.encode(s);
        if unmappable {
            if let Some(r) = replace {
                out.extend_from_slice(r);
            } else {
                return Err((c as u32, enc_name));
            }
        } else {
            out.extend_from_slice(&bytes);
        }
    }
    Ok(out)
}

/// Registry encoding → UTF-8. None = the byte sequence isn't valid
/// in that encoding (the caller surfaces CRuby's
/// InvalidByteSequenceError shape) — except index 0, which is
/// total.
pub(crate) fn decode_to_utf8(idx: u8, bytes: &[u8]) -> Option<String> {
    if idx == 0 {
        return Some(bytes.iter().map(|&b| b as char).collect());
    }
    // UTF-16LE/BE: an odd byte count or a lone/ill-formed surrogate
    // is invalid (None → InvalidByteSequenceError at the call site).
    if let Some(le) = utf16_endianness(idx) {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units = bytes.chunks_exact(2).map(|c| {
            if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }
        });
        let mut out = String::with_capacity(bytes.len() / 2);
        for r in char::decode_utf16(units) {
            out.push(r.ok()?);
        }
        return Some(out);
    }
    // BOM-form "UTF-16": empty → ""; otherwise the leading BOM picks
    // the endianness (FE FF = BE, FF FE = LE) and is stripped. A
    // missing/short BOM is invalid (None → InvalidByteSequenceError).
    if idx == 10 {
        if bytes.is_empty() {
            return Some(String::new());
        }
        let le = match bytes.get(0..2) {
            Some([0xFE, 0xFF]) => false,
            Some([0xFF, 0xFE]) => true,
            _ => return None,
        };
        return decode_to_utf8(if le { 8 } else { 9 }, &bytes[2..]);
    }
    let enc = codec(idx)?;
    enc.decode_without_bom_handling_and_without_replacement(bytes)
        .map(|cow| cow.into_owned())
}

/// Is `bytes` well-formed under registry encoding `idx`? Drives
/// `String#valid_encoding?` for Other-tagged strings.
pub(crate) fn valid(idx: u8, bytes: &[u8]) -> bool {
    if idx == 0 {
        return true;
    }
    if utf16_endianness(idx).is_some() || idx == 10 {
        return decode_to_utf8(idx, bytes).is_some();
    }
    match codec(idx) {
        Some(enc) => enc
            .decode_without_bom_handling_and_without_replacement(bytes)
            .is_some(),
        None => false,
    }
}

/// Character count under registry encoding `idx` (None = invalid
/// sequence — callers fall back to byte length, mirroring CRuby's
/// lenient length-on-broken-strings behaviour).
pub(crate) fn char_count(idx: u8, bytes: &[u8]) -> Option<usize> {
    if idx == 0 {
        return Some(bytes.len());
    }
    decode_to_utf8(idx, bytes).map(|s| s.chars().count())
}

/// Per-character byte chunks under registry encoding `idx` —
/// `String#chars` for Other-tagged strings. Round-trips each
/// decoded char back through the encoder so the chunks are exact.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CaseMode {
    Up,
    Down,
    Capitalize,
    Swap,
}

/// Unicode case over a registry-tagged byte string, CRuby shape
/// (probed on 3.4.1 / ISO-8859-1): per source char, decode → apply
/// the FULL Unicode mapping (ß.upcase → "SS", output grows) →
/// re-encode; a mapped char the encoding can't hold keeps the
/// ORIGINAL char's bytes (ÿ.upcase stays ÿ — Ÿ is unmappable in
/// latin1). `None` = the input isn't valid in the encoding; the
/// caller falls back to its pre-existing lossy route.
pub(crate) fn case_other(idx: u8, bytes: &[u8], mode: CaseMode) -> Option<Vec<u8>> {
    let chunks = char_chunks(idx, bytes)?;
    let mut out = Vec::with_capacity(bytes.len());
    let mut first = true;
    for ch_bytes in &chunks {
        let s = decode_to_utf8(idx, ch_bytes)?;
        let c = s.chars().next()?;
        let mapped: String = match mode {
            CaseMode::Up => c.to_uppercase().collect(),
            CaseMode::Down => c.to_lowercase().collect(),
            CaseMode::Capitalize => {
                if first {
                    c.to_uppercase().collect()
                } else {
                    c.to_lowercase().collect()
                }
            }
            CaseMode::Swap => {
                if c.is_uppercase() {
                    c.to_lowercase().collect()
                } else if c.is_lowercase() {
                    c.to_uppercase().collect()
                } else {
                    c.to_string()
                }
            }
        };
        match encode_from_utf8(idx, &mapped, None) {
            Ok(b) => out.extend_from_slice(&b),
            Err(_) => out.extend_from_slice(ch_bytes),
        }
        first = false;
    }
    Some(out)
}

pub(crate) fn char_chunks(idx: u8, bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    if idx == 0 {
        return Some(bytes.iter().map(|&b| vec![b]).collect());
    }
    let text = decode_to_utf8(idx, bytes)?;
    let mut out = Vec::new();
    for c in text.chars() {
        let s: String = c.to_string();
        match encode_from_utf8(idx, &s, None) {
            Ok(chunk) => out.push(chunk),
            Err(_) => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_roundtrip_and_errors() {
        assert_eq!(name(0), Some("ISO-8859-1"));
        assert_eq!(name(9), Some("UTF-16BE"));
        assert_eq!(name(11), None);
        assert_eq!(find("iso-8859-1"), Some(EncodingTag::Other(0)));
        assert_eq!(find("ISO8859-1"), Some(EncodingTag::Other(0)));
        assert_eq!(find("KLINGON"), None);
        // identity transcode both ways
        assert_eq!(encode_from_utf8(0, "héllo", None), Ok(vec![104, 0xE9, 108, 108, 111]));
        assert_eq!(decode_to_utf8(0, &[104, 0xE9]), Some("hé".to_string()));
        // unmappable: raise tuple / replace / custom replacement
        assert_eq!(encode_from_utf8(0, "日", None), Err((0x65E5, "ISO-8859-1")));
        assert_eq!(encode_from_utf8(0, "日x", Some(b"?")), Ok(vec![b'?', b'x']));
        assert_eq!(encode_from_utf8(0, "日x", Some(b"_")), Ok(vec![b'_', b'x']));
        // unregistered index (registry is 0..=10 with the UTF-16 trio)
        assert!(encode_from_utf8(99, "x", None).is_err());
        assert_eq!(decode_to_utf8(99, b"x"), None);
        // v2 spot checks: the WHATWG-backed codecs round-trip the
        // probe chars (full behaviour is pinned by the
        // encoding_full_latin1 + seven-encoding diff fixtures).
        assert_eq!(find("SJIS"), Some(EncodingTag::Other(4)));
        let sj = encode_from_utf8(4, "日", None).unwrap();
        assert_eq!(decode_to_utf8(4, &sj).as_deref(), Some("日"));
        assert!(!valid(4, &sj[..1]));
        assert_eq!(char_count(4, &sj), Some(1));
        assert_eq!(char_chunks(4, &sj).map(|c| c.len()), Some(1));
    }

    #[test]
    fn utf16_transcode() {
        assert_eq!(find("UTF-16LE"), Some(EncodingTag::Other(8)));
        assert_eq!(find("utf16be"), Some(EncodingTag::Other(9)));
        assert_eq!(find("UTF-16"), Some(EncodingTag::Other(10)));
        // LE/BE round-trip a BMP char + an astral surrogate pair.
        assert_eq!(encode_from_utf8(8, "h", None), Ok(vec![0x68, 0x00]));
        assert_eq!(encode_from_utf8(9, "h", None), Ok(vec![0x00, 0x68]));
        assert_eq!(decode_to_utf8(8, &[0x68, 0x00]).as_deref(), Some("h"));
        let smiley_le = encode_from_utf8(8, "😀", None).unwrap();
        assert_eq!(smiley_le, vec![0x3D, 0xD8, 0x00, 0xDE]);
        assert_eq!(decode_to_utf8(8, &smiley_le).as_deref(), Some("😀"));
        // odd length / lone surrogate are invalid.
        assert_eq!(decode_to_utf8(8, &[0x00]), None);
        assert_eq!(decode_to_utf8(9, &[0xD8, 0x3D]), None);
        assert!(!valid(8, &[0x00]));
        // BOM form: BE BOM + BE bytes; empty stays empty; decode sniffs.
        assert_eq!(encode_from_utf8(10, "h", None), Ok(vec![0xFE, 0xFF, 0x00, 0x68]));
        assert_eq!(encode_from_utf8(10, "", None), Ok(vec![]));
        assert_eq!(decode_to_utf8(10, &[0xFE, 0xFF, 0x00, 0x68]).as_deref(), Some("h"));
        assert_eq!(decode_to_utf8(10, &[0xFF, 0xFE, 0x68, 0x00]).as_deref(), Some("h"));
        assert_eq!(decode_to_utf8(10, &[]).as_deref(), Some(""));
        assert_eq!(decode_to_utf8(10, &[0x00, 0x68]), None); // no BOM
        assert!(is_utf16_family(8) && is_utf16_family(9) && is_utf16_family(10));
        assert!(!is_utf16_family(0) && !is_utf16_family(4));
    }
}
