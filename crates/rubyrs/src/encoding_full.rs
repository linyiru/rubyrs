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
/// tag's meaning — append only.
const NAMES: &[&str] = &["ISO-8859-1"];

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
    match name.to_ascii_uppercase().as_str() {
        // CRuby accepts the hyphen-less alias too.
        "ISO-8859-1" | "ISO8859-1" => Some(EncodingTag::Other(0)),
        _ => None,
    }
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
    match idx {
        0 => {
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
            Ok(out)
        }
        _ => Err((0, "unregistered")),
    }
}

/// Registry encoding → UTF-8. Latin-1 is total (every byte maps),
/// so index 0 never fails; the signature leaves room for partial
/// encodings later.
pub(crate) fn decode_to_utf8(idx: u8, bytes: &[u8]) -> Option<String> {
    match idx {
        0 => Some(bytes.iter().map(|&b| b as char).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_roundtrip_and_errors() {
        assert_eq!(name(0), Some("ISO-8859-1"));
        assert_eq!(name(9), None);
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
        // unregistered index
        assert!(encode_from_utf8(7, "x", None).is_err());
        assert_eq!(decode_to_utf8(7, b"x"), None);
    }
}
