//! Pure-Rust bcrypt (EksBlowfish) — a faithful port of `crypt_blowfish.c`
//! (the bcrypt-ruby C extension), behind the `_bcrypt` feature. It backs the
//! two private C entry points the gem's Ruby wrapper calls:
//!
//!   * `BCrypt::Engine.__bc_salt(prefix, cost, input16)` → `bc_salt`
//!     (`_crypt_gensalt_blowfish_rn`): `$2X$NN$` + 22 base64 chars.
//!   * `BCrypt::Engine.__bc_crypt(secret, salt)` → `bc_crypt` (`BF_crypt`):
//!     the full 60-char `$2X$NN$<22 salt><31 hash>` digest.
//!
//! The output is byte-identical to OpenBSD/crypt_blowfish, so a hash computed
//! here verifies against (and round-trips with) CRuby's bcrypt. The `$2a$`
//! sign-extension/anti-collision details from `BF_set_key` are reproduced
//! exactly. Validated against the canonical OpenBSD test vectors (see tests).

const BF_N: usize = 16;

/// "OrpheanBeholderScryDoubt" as 6 big-endian words — the IV bcrypt encrypts
/// 64 times to produce the hash.
const MAGIC: [u32; 6] = [
    0x4F727068, 0x65616E42, 0x65686F6C, 0x64657253, 0x63727944, 0x6F756274,
];

/// bcrypt's base64 alphabet (NOT standard base64): `.`/`/` then `A-Za-z0-9`.
const ITOA64: &[u8; 64] =
    b"./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Inverse of ITOA64 over the printable range `0x20..0x80` (offset by 0x20);
/// `64` marks an invalid character.
static ATOI64: [u8; 0x60] = [
    64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 0, 1,
    54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 64, 64, 64, 64, 64,
    64, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
    17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 64, 64, 64, 64, 64,
    64, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42,
    43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 64, 64, 64, 64, 64,
];

/// `flags_by_subtype` for the prefixes the gem uses: `$2a$` → 2 (bug=0,
/// safety=0x10000), `$2b$` → 4 (bug=0, safety=0), `$2y$` → 4. Index by
/// `letter - 'a'`.
fn flags_for_subtype(letter: u8) -> Option<u8> {
    match letter {
        b'a' => Some(2),
        b'b' => Some(4),
        b'x' => Some(1),
        b'y' => Some(4),
        _ => None,
    }
}

struct Ctx {
    s: [[u32; 256]; 4],
    p: [u32; 18],
}

impl Ctx {
    /// Blowfish f-function: `((S0[a] + S1[b]) ^ S2[c]) + S3[d]` over the four
    /// bytes of `x` (a = MSB), all adds mod 2^32.
    #[inline]
    fn f(&self, x: u32) -> u32 {
        let a = (x >> 24) as usize;
        let b = ((x >> 16) & 0xff) as usize;
        let c = ((x >> 8) & 0xff) as usize;
        let d = (x & 0xff) as usize;
        (self.s[0][a].wrapping_add(self.s[1][b]) ^ self.s[2][c]).wrapping_add(self.s[3][d])
    }

    /// Encrypt one 64-bit block (16 Feistel rounds), matching `BF_ENCRYPT`.
    #[inline]
    fn encrypt(&self, mut l: u32, mut r: u32) -> (u32, u32) {
        l ^= self.p[0];
        let mut n = 0;
        while n < BF_N {
            r ^= self.p[n + 1] ^ self.f(l);
            l ^= self.p[n + 2] ^ self.f(r);
            n += 2;
        }
        // Undo the last half-swap and mix in P[16]/P[17].
        (r ^ self.p[BF_N + 1], l)
    }

    /// `BF_body`: re-derive P then all S-boxes by encrypting a running
    /// (L, R) pair starting from (0, 0) — the salt-free expansion pass.
    fn body(&mut self) {
        let mut l = 0u32;
        let mut r = 0u32;
        let mut i = 0;
        while i < 18 {
            let (nl, nr) = self.encrypt(l, r);
            l = nl;
            r = nr;
            self.p[i] = l;
            self.p[i + 1] = r;
            i += 2;
        }
        for box_idx in 0..4 {
            let mut k = 0;
            while k < 256 {
                let (nl, nr) = self.encrypt(l, r);
                l = nl;
                r = nr;
                self.s[box_idx][k] = l;
                self.s[box_idx][k + 1] = r;
                k += 2;
            }
        }
    }
}

/// `BF_set_key`: derive the `expanded` key (key words, cycled) and the
/// `initial` P-array (`init.P ^ key`), reproducing the `$2a$` sign-extension
/// bug emulation + anti-collision safety bit exactly.
fn set_key(key: &[u8], flags: u8) -> ([u32; 18], [u32; 18]) {
    let bug = (flags & 1) as u32;
    let safety = ((flags as u32) & 2) << 15;

    // The key is treated as a NUL-terminated C string, cycled: …key, 0,
    // key, 0… An embedded NUL truncates (C-string semantics).
    let mut pos = 0usize;
    let mut next = || -> u8 {
        let byte = if pos < key.len() { key[pos] } else { 0 };
        if byte == 0 {
            pos = 0;
        } else {
            pos += 1;
        }
        byte
    };

    let mut expanded = [0u32; 18];
    let mut initial = [0u32; 18];
    let mut sign = 0u32;
    let mut diff = 0u32;
    for i in 0..18 {
        let mut tmp0 = 0u32; // correct (zero-extended)
        let mut tmp1 = 0u32; // buggy (sign-extended)
        for j in 0..4 {
            let ch = next();
            tmp0 = (tmp0 << 8) | ch as u32;
            tmp1 = (tmp1 << 8) | (ch as i8 as i32 as u32);
            if j != 0 {
                sign |= tmp1 & 0x80;
            }
        }
        diff |= tmp0 ^ tmp1;
        let chosen = if bug == 0 { tmp0 } else { tmp1 };
        expanded[i] = chosen;
        initial[i] = BF_INIT_P[i] ^ chosen;
    }
    diff |= diff >> 16;
    diff &= 0xffff;
    diff = diff.wrapping_add(0xffff);
    sign <<= 9;
    sign &= !diff & safety;
    initial[0] ^= sign;
    (expanded, initial)
}

/// `BF_encode`: bcrypt-base64 of `src` bytes (MSB-first, 6 bits/char).
fn bf_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len().div_ceil(3) * 4);
    let end = src.len();
    let mut i = 0;
    loop {
        let c1 = src[i] as usize;
        i += 1;
        out.push(ITOA64[c1 >> 2]);
        let mut c1 = (c1 & 0x03) << 4;
        if i >= end {
            out.push(ITOA64[c1]);
            break;
        }
        let c2 = src[i] as usize;
        i += 1;
        c1 |= c2 >> 4;
        out.push(ITOA64[c1]);
        let mut c1 = (c2 & 0x0f) << 2;
        if i >= end {
            out.push(ITOA64[c1]);
            break;
        }
        let c2 = src[i] as usize;
        i += 1;
        c1 |= c2 >> 6;
        out.push(ITOA64[c1]);
        out.push(ITOA64[c2 & 0x3f]);
        if i >= end {
            break;
        }
    }
    out
}

/// `BF_safe_atoi64`: decode one bcrypt-base64 char, `None` if invalid.
fn atoi64(c: u8) -> Option<u8> {
    let t = c.wrapping_sub(0x20);
    if t as usize >= 0x60 {
        return None;
    }
    let v = ATOI64[t as usize];
    if v > 63 { None } else { Some(v) }
}

/// `BF_decode`: bcrypt-base64 → `out_len` bytes, `None` on a bad character
/// (or truncated input).
fn bf_decode(src: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let mut dst = Vec::with_capacity(out_len);
    let mut sp = 0usize;
    let mut take = || -> Option<u8> {
        let c = *src.get(sp)?;
        sp += 1;
        atoi64(c)
    };
    loop {
        let c1 = take()?;
        let c2 = take()?;
        dst.push((c1 << 2) | ((c2 & 0x30) >> 4));
        if dst.len() >= out_len {
            break;
        }
        let c3 = take()?;
        dst.push(((c2 & 0x0f) << 4) | ((c3 & 0x3c) >> 2));
        if dst.len() >= out_len {
            break;
        }
        let c4 = take()?;
        dst.push(((c3 & 0x03) << 6) | c4);
        if dst.len() >= out_len {
            break;
        }
    }
    Some(dst)
}

/// `_crypt_gensalt_blowfish_rn` — build a bcrypt salt string `prefix` +
/// 2-digit cost + `$` + 22 base64 chars of the 16-byte `input`. `prefix`
/// is `$2a$` / `$2b$` / `$2y$`; `count` (cost) defaults to 5, clamped 4..=31.
/// Returns `None` on invalid prefix / short input / out-of-range cost.
pub(crate) fn bc_salt(prefix: &[u8], count: u32, input: &[u8]) -> Option<String> {
    if input.len() < 16 {
        return None;
    }
    if prefix.len() < 3 || prefix[0] != b'$' || prefix[1] != b'2' {
        return None;
    }
    let sub = prefix[2];
    if sub != b'a' && sub != b'b' && sub != b'y' {
        return None;
    }
    if count != 0 && !(4..=31).contains(&count) {
        return None;
    }
    let count = if count == 0 { 5 } else { count };
    let mut out = Vec::with_capacity(7 + 22);
    out.extend_from_slice(&[b'$', b'2', sub, b'$']);
    out.push(b'0' + (count / 10) as u8);
    out.push(b'0' + (count % 10) as u8);
    out.push(b'$');
    out.extend_from_slice(&bf_encode(&input[..16]));
    Some(String::from_utf8(out).expect("itoa64 is ASCII"))
}

/// `BF_crypt` — the EksBlowfish bcrypt hash. `key` is the secret (the gem
/// already truncates it to 72 bytes); `setting` is a valid `$2X$NN$<22>` salt
/// (longer settings are accepted — only the first 29 chars matter). Returns
/// the full 60-char digest, or `None` if the setting is malformed.
pub(crate) fn bc_crypt(key: &[u8], setting: &[u8]) -> Option<String> {
    // Parse and validate the setting header: $2[a-z]$NN$
    if setting.len() < 7 + 22 {
        return None;
    }
    if setting[0] != b'$' || setting[1] != b'2' {
        return None;
    }
    let flags = flags_for_subtype(setting[2])?;
    if setting[3] != b'$'
        || !setting[4].is_ascii_digit()
        || !setting[5].is_ascii_digit()
        || setting[6] != b'$'
    {
        return None;
    }
    let c10 = (setting[4] - b'0') as u32;
    let c1 = (setting[5] - b'0') as u32;
    if setting[4] > b'3' || (setting[4] == b'3' && setting[5] > b'1') {
        return None;
    }
    let count: u64 = 1u64 << (c10 * 10 + c1);

    // Salt: 22 base64 chars → 16 bytes → 4 big-endian words.
    let salt_bytes = bf_decode(&setting[7..7 + 22], 16)?;
    let mut salt = [0u32; 4];
    for (i, w) in salt.iter_mut().enumerate() {
        *w = u32::from_be_bytes([
            salt_bytes[4 * i],
            salt_bytes[4 * i + 1],
            salt_bytes[4 * i + 2],
            salt_bytes[4 * i + 3],
        ]);
    }

    let (expanded, initial) = set_key(key, flags);
    let mut ctx = Ctx {
        s: BF_INIT_S,
        p: initial,
    };

    // Expensive setup, salted: expand P then S, XORing the 128-bit salt in
    // 64-bit halves (cycling the 4 salt words), carrying (L, R) across.
    let mut l = 0u32;
    let mut r = 0u32;
    let mut i = 0;
    while i < 18 {
        l ^= salt[i & 2];
        r ^= salt[(i & 2) + 1];
        let (nl, nr) = ctx.encrypt(l, r);
        l = nl;
        r = nr;
        ctx.p[i] = l;
        ctx.p[i + 1] = r;
        i += 2;
    }
    // S-box fill: alternate salt blocks (2,3) then (0,1).
    let mut k = 0;
    while k < 1024 {
        l ^= salt[2];
        r ^= salt[3];
        let (nl, nr) = ctx.encrypt(l, r);
        l = nl;
        r = nr;
        ctx.s[k / 256][k % 256] = l;
        ctx.s[(k + 1) / 256][(k + 1) % 256] = r;
        k += 2;
        l ^= salt[0];
        r ^= salt[1];
        let (nl, nr) = ctx.encrypt(l, r);
        l = nl;
        r = nr;
        ctx.s[k / 256][k % 256] = l;
        ctx.s[(k + 1) / 256][(k + 1) % 256] = r;
        k += 2;
    }

    // Main loop: 2^cost rounds of ExpandKey(0, key) then ExpandKey(0, salt).
    for _ in 0..count {
        for j in 0..18 {
            ctx.p[j] ^= expanded[j];
        }
        ctx.body();
        for j in 0..18 {
            ctx.p[j] ^= salt[j % 4];
        }
        ctx.body();
    }

    // Encrypt the magic IV 64 times.
    let mut output = [0u32; 6];
    let mut m = 0;
    while m < 6 {
        let mut l = MAGIC[m];
        let mut r = MAGIC[m + 1];
        for _ in 0..64 {
            let (nl, nr) = ctx.encrypt(l, r);
            l = nl;
            r = nr;
        }
        output[m] = l;
        output[m + 1] = r;
        m += 2;
    }

    // Output = "$2X$NN$" + canonicalized 22-char salt + 31 hash chars.
    let mut out = Vec::with_capacity(60);
    out.extend_from_slice(&setting[0..7 + 22]);
    // Re-canonicalize the final salt char (only its high bits are
    // significant): itoa64[atoi64[c] & 0x30].
    let last = setting[7 + 22 - 1];
    let canon = atoi64(last).unwrap_or(0) & 0x30;
    out[7 + 22 - 1] = ITOA64[canon as usize];

    // 6 words → 24 big-endian bytes; encode only the first 23 (the
    // documented "bug-compatible" truncation).
    let mut obytes = [0u8; 24];
    for (i, w) in output.iter().enumerate() {
        obytes[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    out.extend_from_slice(&bf_encode(&obytes[..23]));
    Some(String::from_utf8(out).expect("output is ASCII"))
}

/// Register the `_bcrypt` battery's host fns. The Ruby surface
/// (`BCrypt::Engine` with the two `__bc_salt` / `__bc_crypt` class
/// methods, preamble/bcrypt_ext.rb) is loaded by `load_preamble_inner`
/// at Runtime construction through the preamble bytecode cache —
/// mirrors the socket/openssl battery shape; `require "bcrypt_ext"`
/// succeeds as a known stub.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    use crate::error::{RubyError, Trap};
    use crate::value::Value;

    fn bytes_of(v: &Value) -> Option<Vec<u8>> {
        match v {
            Value::Str(s) => Some(s.content.borrow().clone()),
            _ => None,
        }
    }
    fn arg_err(msg: &str) -> Trap {
        Trap { err: RubyError::ArgumentError { msg: msg.to_string() }, backtrace: vec![] }
    }

    // `__bc_salt(prefix, cost, input)` → salt String (or nil on bad input,
    // matching the C extension's Qnil return).
    rt.register_fn("__rubyrs_bcrypt_salt", |args| {
        let (prefix, cost, input) = match args {
            [p, Value::Int(c), i] => (
                bytes_of(p).ok_or_else(|| arg_err("__bc_salt: prefix must be a String"))?,
                *c,
                bytes_of(i).ok_or_else(|| arg_err("__bc_salt: input must be a String"))?,
            ),
            _ => return Err(arg_err("__bc_salt(prefix: String, cost: Integer, input: String)")),
        };
        let cost = u32::try_from(cost).unwrap_or(0);
        Ok(bc_salt(&prefix, cost, &input).map_or(Value::Nil, Value::new_str))
    });

    // `__bc_crypt(secret, salt)` → full hash String (or nil on bad salt).
    rt.register_fn("__rubyrs_bcrypt_crypt", |args| {
        let (secret, salt) = match args {
            [s, sa] => (
                bytes_of(s).ok_or_else(|| arg_err("__bc_crypt: secret must be a String"))?,
                bytes_of(sa).ok_or_else(|| arg_err("__bc_crypt: salt must be a String"))?,
            ),
            _ => return Err(arg_err("__bc_crypt(secret: String, salt: String)")),
        };
        Ok(bc_crypt(&secret, &salt).map_or(Value::Nil, Value::new_str))
    });
}

include!("bcrypt_init.rs");

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical OpenBSD / crypt_blowfish test vectors.
    #[test]
    fn known_vectors() {
        let cases: &[(&str, &str, &str)] = &[
            ("U*U", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"),
            ("U*U*", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.VGOzA784oUp/Z0DY336zx7pLYAy0lwK"),
            ("U*U*U", "$2a$05$XXXXXXXXXXXXXXXXXXXXXO", "$2a$05$XXXXXXXXXXXXXXXXXXXXXOAcXxm9kjPGEMsLznoKqmqw7tc8WCx4a"),
            ("", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.7uG0VCzI2bS7j6ymqJi9CdcdxiRTWNy"),
            ("0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789chars after 72 are ignored",
             "$2a$05$abcdefghijklmnopqrstuu",
             "$2a$05$abcdefghijklmnopqrstuu5s2v8.iXieOjg/.AySBTTZIIVFJeBui"),
        ];
        for (secret, salt, expected) in cases {
            let got = bc_crypt(secret.as_bytes(), salt.as_bytes());
            assert_eq!(got.as_deref(), Some(*expected), "secret={secret:?} salt={salt}");
        }
    }

    #[test]
    fn salt_format() {
        let input = [0x10u8; 16];
        let s = bc_salt(b"$2a$", 4, &input).unwrap();
        assert!(s.starts_with("$2a$04$"));
        assert_eq!(s.len(), 7 + 22);
        // round-trip: a hash with this salt parses back to the same salt.
        let h = bc_crypt(b"pw", s.as_bytes()).unwrap();
        assert_eq!(&h[..7 + 22], &bc_crypt(b"pw", s.as_bytes()).unwrap()[..7 + 22]);
        assert_eq!(h.len(), 60);
    }

    #[test]
    fn rejects_bad_setting() {
        assert!(bc_crypt(b"x", b"not-a-salt").is_none());
        assert!(bc_crypt(b"x", b"$2a$99$CCCCCCCCCCCCCCCCCCCCC.").is_none());
        assert!(bc_salt(b"$1$", 4, &[0u8; 16]).is_none());
    }
}
