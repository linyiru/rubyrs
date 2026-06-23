//! `_openssl` battery — pure-Rust AES-256 (CTR mode) + HMAC-SHA256.
//!
//! The minimal symmetric-crypto slice Rack 3's `Rack::Session::Encryptor`
//! drives: it encrypts session cookies with `aes-256-ctr` and authenticates
//! them with `HMAC(SHA256)`. Both primitives are exposed to the openssl
//! preamble as host fns (`__rubyrs_aes256_ctr` / `__rubyrs_hmac_sha256`),
//! which the `OpenSSL::Cipher` / `OpenSSL::HMAC` veneers call.
//!
//! AES is implemented straight from FIPS-197 (no T-tables — clarity over
//! the throughput a cookie round-trip never needs). CTR turns the block
//! cipher into a stream cipher, so encrypt and decrypt are the same XOR
//! operation; only the forward (encrypt) block transform is needed.
//! HMAC-SHA256 is the textbook RFC 2104 construction over
//! [`crate::digest::sha256`].

// FIPS-197 S-box.
#[rustfmt::skip]
const SBOX: [u8; 256] = [
    0x63,0x7c,0x77,0x7b,0xf2,0x6b,0x6f,0xc5,0x30,0x01,0x67,0x2b,0xfe,0xd7,0xab,0x76,
    0xca,0x82,0xc9,0x7d,0xfa,0x59,0x47,0xf0,0xad,0xd4,0xa2,0xaf,0x9c,0xa4,0x72,0xc0,
    0xb7,0xfd,0x93,0x26,0x36,0x3f,0xf7,0xcc,0x34,0xa5,0xe5,0xf1,0x71,0xd8,0x31,0x15,
    0x04,0xc7,0x23,0xc3,0x18,0x96,0x05,0x9a,0x07,0x12,0x80,0xe2,0xeb,0x27,0xb2,0x75,
    0x09,0x83,0x2c,0x1a,0x1b,0x6e,0x5a,0xa0,0x52,0x3b,0xd6,0xb3,0x29,0xe3,0x2f,0x84,
    0x53,0xd1,0x00,0xed,0x20,0xfc,0xb1,0x5b,0x6a,0xcb,0xbe,0x39,0x4a,0x4c,0x58,0xcf,
    0xd0,0xef,0xaa,0xfb,0x43,0x4d,0x33,0x85,0x45,0xf9,0x02,0x7f,0x50,0x3c,0x9f,0xa8,
    0x51,0xa3,0x40,0x8f,0x92,0x9d,0x38,0xf5,0xbc,0xb6,0xda,0x21,0x10,0xff,0xf3,0xd2,
    0xcd,0x0c,0x13,0xec,0x5f,0x97,0x44,0x17,0xc4,0xa7,0x7e,0x3d,0x64,0x5d,0x19,0x73,
    0x60,0x81,0x4f,0xdc,0x22,0x2a,0x90,0x88,0x46,0xee,0xb8,0x14,0xde,0x5e,0x0b,0xdb,
    0xe0,0x32,0x3a,0x0a,0x49,0x06,0x24,0x5c,0xc2,0xd3,0xac,0x62,0x91,0x95,0xe4,0x79,
    0xe7,0xc8,0x37,0x6d,0x8d,0xd5,0x4e,0xa9,0x6c,0x56,0xf4,0xea,0x65,0x7a,0xae,0x08,
    0xba,0x78,0x25,0x2e,0x1c,0xa6,0xb4,0xc6,0xe8,0xdd,0x74,0x1f,0x4b,0xbd,0x8b,0x8a,
    0x70,0x3e,0xb5,0x66,0x48,0x03,0xf6,0x0e,0x61,0x35,0x57,0xb9,0x86,0xc1,0x1d,0x9e,
    0xe1,0xf8,0x98,0x11,0x69,0xd9,0x8e,0x94,0x9b,0x1e,0x87,0xe9,0xce,0x55,0x28,0xdf,
    0x8c,0xa1,0x89,0x0d,0xbf,0xe6,0x42,0x68,0x41,0x99,0x2d,0x0f,0xb0,0x54,0xbb,0x16,
];

// FIPS-197 inverse S-box (for the decryption path / CBC).
#[rustfmt::skip]
const INV_SBOX: [u8; 256] = [
    0x52,0x09,0x6a,0xd5,0x30,0x36,0xa5,0x38,0xbf,0x40,0xa3,0x9e,0x81,0xf3,0xd7,0xfb,
    0x7c,0xe3,0x39,0x82,0x9b,0x2f,0xff,0x87,0x34,0x8e,0x43,0x44,0xc4,0xde,0xe9,0xcb,
    0x54,0x7b,0x94,0x32,0xa6,0xc2,0x23,0x3d,0xee,0x4c,0x95,0x0b,0x42,0xfa,0xc3,0x4e,
    0x08,0x2e,0xa1,0x66,0x28,0xd9,0x24,0xb2,0x76,0x5b,0xa2,0x49,0x6d,0x8b,0xd1,0x25,
    0x72,0xf8,0xf6,0x64,0x86,0x68,0x98,0x16,0xd4,0xa4,0x5c,0xcc,0x5d,0x65,0xb6,0x92,
    0x6c,0x70,0x48,0x50,0xfd,0xed,0xb9,0xda,0x5e,0x15,0x46,0x57,0xa7,0x8d,0x9d,0x84,
    0x90,0xd8,0xab,0x00,0x8c,0xbc,0xd3,0x0a,0xf7,0xe4,0x58,0x05,0xb8,0xb3,0x45,0x06,
    0xd0,0x2c,0x1e,0x8f,0xca,0x3f,0x0f,0x02,0xc1,0xaf,0xbd,0x03,0x01,0x13,0x8a,0x6b,
    0x3a,0x91,0x11,0x41,0x4f,0x67,0xdc,0xea,0x97,0xf2,0xcf,0xce,0xf0,0xb4,0xe6,0x73,
    0x96,0xac,0x74,0x22,0xe7,0xad,0x35,0x85,0xe2,0xf9,0x37,0xe8,0x1c,0x75,0xdf,0x6e,
    0x47,0xf1,0x1a,0x71,0x1d,0x29,0xc5,0x89,0x6f,0xb7,0x62,0x0e,0xaa,0x18,0xbe,0x1b,
    0xfc,0x56,0x3e,0x4b,0xc6,0xd2,0x79,0x20,0x9a,0xdb,0xc0,0xfe,0x78,0xcd,0x5a,0xf4,
    0x1f,0xdd,0xa8,0x33,0x88,0x07,0xc7,0x31,0xb1,0x12,0x10,0x59,0x27,0x80,0xec,0x5f,
    0x60,0x51,0x7f,0xa9,0x19,0xb5,0x4a,0x0d,0x2d,0xe5,0x7a,0x9f,0x93,0xc9,0x9c,0xef,
    0xa0,0xe0,0x3b,0x4d,0xae,0x2a,0xf5,0xb0,0xc8,0xeb,0xbb,0x3c,0x83,0x53,0x99,0x61,
    0x17,0x2b,0x04,0x7e,0xba,0x77,0xd6,0x26,0xe1,0x69,0x14,0x63,0x55,0x21,0x0c,0x7d,
];

// AES key-schedule round constants, Rcon[1..=10] (index 0 unused). Ten
// entries cover AES-128's 10 groups; AES-192/256 use fewer.
const RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

/// Expand an AES key (16 / 24 / 32 bytes → AES-128 / 192 / 256) into its
/// round-key words. Returns `(round_keys, nr)` where `nr` is the round
/// count (10 / 12 / 14); `round_keys.len() == 4 * (nr + 1)`. The block
/// transforms are identical across key sizes — only the schedule and
/// round count differ.
fn expand_key(key: &[u8]) -> (Vec<[u8; 4]>, usize) {
    let nk = key.len() / 4; // key words: 4, 6, or 8
    let nr = nk + 6; // rounds: 10, 12, or 14
    let total = 4 * (nr + 1);
    let mut w = vec![[0u8; 4]; total];
    for i in 0..nk {
        w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
    }
    for i in nk..total {
        let mut t = w[i - 1];
        if i % nk == 0 {
            // RotWord + SubWord + Rcon.
            t = [t[1], t[2], t[3], t[0]];
            for b in &mut t {
                *b = SBOX[*b as usize];
            }
            t[0] ^= RCON[i / nk];
        } else if nk > 6 && i % nk == 4 {
            // AES-256 only: SubWord at the mid-group word.
            for b in &mut t {
                *b = SBOX[*b as usize];
            }
        }
        for j in 0..4 {
            w[i][j] = w[i - nk][j] ^ t[j];
        }
    }
    (w, nr)
}

#[inline]
fn xtime(x: u8) -> u8 {
    // Multiply by 2 in GF(2^8) with the AES reduction polynomial.
    (x << 1) ^ if x & 0x80 != 0 { 0x1b } else { 0x00 }
}

#[inline]
fn gmul2(x: u8) -> u8 { xtime(x) }
#[inline]
fn gmul3(x: u8) -> u8 { xtime(x) ^ x }

/// Encrypt one 16-byte block in place under the expanded key schedule
/// (`nr` rounds).
fn encrypt_block(w: &[[u8; 4]], nr: usize, block: &mut [u8; 16]) {
    add_round_key(block, w, 0);
    for round in 1..nr {
        sub_bytes(block);
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, w, round);
    }
    sub_bytes(block);
    shift_rows(block);
    add_round_key(block, w, nr);
}

#[inline]
fn add_round_key(s: &mut [u8; 16], w: &[[u8; 4]], round: usize) {
    // State columns map to round-key words 4*round + col.
    for col in 0..4 {
        let k = w[4 * round + col];
        for row in 0..4 {
            s[col * 4 + row] ^= k[row];
        }
    }
}

#[inline]
fn sub_bytes(s: &mut [u8; 16]) {
    for b in s.iter_mut() {
        *b = SBOX[*b as usize];
    }
}

#[inline]
fn shift_rows(s: &mut [u8; 16]) {
    // State is column-major (s[col*4 + row]); row r rotates left by r.
    let orig = *s;
    for row in 1..4 {
        for col in 0..4 {
            s[col * 4 + row] = orig[((col + row) % 4) * 4 + row];
        }
    }
}

#[inline]
fn mix_columns(s: &mut [u8; 16]) {
    for col in 0..4 {
        let c = col * 4;
        let a0 = s[c];
        let a1 = s[c + 1];
        let a2 = s[c + 2];
        let a3 = s[c + 3];
        s[c] = gmul2(a0) ^ gmul3(a1) ^ a2 ^ a3;
        s[c + 1] = a0 ^ gmul2(a1) ^ gmul3(a2) ^ a3;
        s[c + 2] = a0 ^ a1 ^ gmul2(a2) ^ gmul3(a3);
        s[c + 3] = gmul3(a0) ^ a1 ^ a2 ^ gmul2(a3);
    }
}

/// AES-256-CTR keystream XOR. `iv` is the initial 128-bit counter block
/// (big-endian); `byte_offset` is how many keystream bytes have already
/// been consumed by previous `update` calls on the same cipher, so the
/// counter and intra-block position resume exactly where they left off.
/// Encrypt and decrypt are identical (stream cipher).
pub fn aes_ctr_xor(key: &[u8], iv: &[u8; 16], byte_offset: u64, data: &[u8]) -> Vec<u8> {
    let (w, nr) = expand_key(key);
    let mut out = Vec::with_capacity(data.len());
    let mut consumed = byte_offset;
    let mut keystream = [0u8; 16];
    let mut ks_valid_from = u64::MAX; // block index currently in `keystream`
    for &byte in data {
        let block_index = consumed / 16;
        let within = (consumed % 16) as usize;
        if ks_valid_from != block_index {
            // Counter block = IV + block_index (big-endian add).
            let mut ctr = *iv;
            add_counter(&mut ctr, block_index);
            keystream = ctr;
            encrypt_block(&w, nr, &mut keystream);
            ks_valid_from = block_index;
        }
        out.push(byte ^ keystream[within]);
        consumed += 1;
    }
    out
}

/// Add `n` to a 128-bit big-endian counter block in place (wrapping).
fn add_counter(ctr: &mut [u8; 16], n: u64) {
    let mut carry = n as u128;
    for i in (0..16).rev() {
        if carry == 0 {
            break;
        }
        let sum = ctr[i] as u128 + (carry & 0xff);
        ctr[i] = sum as u8;
        carry = (carry >> 8) + (sum >> 8);
    }
}

// ---- AES-256-GCM (NIST SP 800-38D) ----
//
// GCM authenticates with GHASH over GF(2^128) and encrypts with a
// 32-bit-incrementing counter mode (GCTR). Only the forward block
// cipher is needed (both encrypt and decrypt run GCTR). The hash
// subkey is H = E_K(0^128); for a 96-bit IV (the common / Rails case)
// the pre-counter block J0 = IV || 0^31 || 1, else J0 = GHASH_H of the
// padded IV. The 128-bit tag T = E_K(J0) XOR GHASH_H(A,C).

/// GF(2^128) multiply under the GCM bit ordering (reduction poly
/// x^128 + x^7 + x^2 + x + 1, represented big-endian as 0xe1...).
fn gcm_mult(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *y;
    for i in 0..128 {
        if (x[i / 8] >> (7 - (i % 8))) & 1 == 1 {
            for j in 0..16 {
                z[j] ^= v[j];
            }
        }
        // V >>= 1 over the full 128-bit word, then fold in R on underflow.
        let lsb = v[15] & 1;
        let mut carry = 0u8;
        for j in 0..16 {
            let next = v[j] & 1;
            v[j] = (v[j] >> 1) | (carry << 7);
            carry = next;
        }
        if lsb == 1 {
            v[0] ^= 0xe1;
        }
    }
    z
}

/// GHASH_H over `data` (which the caller has already padded to a 16-byte
/// boundary and length-framed). Y_0 = 0; Y_i = (Y_{i-1} XOR B_i) • H.
fn ghash(h: &[u8; 16], data: &[u8]) -> [u8; 16] {
    let mut y = [0u8; 16];
    for chunk in data.chunks(16) {
        for (j, &b) in chunk.iter().enumerate() {
            y[j] ^= b;
        }
        y = gcm_mult(&y, h);
    }
    y
}

/// Increment the rightmost 32 bits of a counter block (big-endian, wrapping).
fn inc32(cb: &mut [u8; 16]) {
    let n = u32::from_be_bytes([cb[12], cb[13], cb[14], cb[15]]).wrapping_add(1);
    cb[12..16].copy_from_slice(&n.to_be_bytes());
}

/// GCTR: counter mode keyed by the expanded schedule, starting from the
/// initial counter block `icb`, incrementing the low 32 bits per block.
fn gctr(w: &[[u8; 4]], nr: usize, icb: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut cb = *icb;
    for chunk in data.chunks(16) {
        let mut ks = cb;
        encrypt_block(w, nr, &mut ks);
        for (i, &b) in chunk.iter().enumerate() {
            out.push(b ^ ks[i]);
        }
        inc32(&mut cb);
    }
    out
}

/// Derive (H, J0) for a key schedule and IV.
fn gcm_setup(w: &[[u8; 4]], nr: usize, iv: &[u8]) -> ([u8; 16], [u8; 16]) {
    let mut h = [0u8; 16];
    encrypt_block(w, nr, &mut h);
    let j0 = if iv.len() == 12 {
        let mut j = [0u8; 16];
        j[..12].copy_from_slice(iv);
        j[15] = 1;
        j
    } else {
        let mut data = iv.to_vec();
        while data.len() % 16 != 0 {
            data.push(0);
        }
        data.extend_from_slice(&[0u8; 8]);
        data.extend_from_slice(&((iv.len() as u64) * 8).to_be_bytes());
        ghash(&h, &data)
    };
    (h, j0)
}

/// GHASH over AAD || pad || C || pad || [bitlen(A)]_64 || [bitlen(C)]_64.
fn gcm_tag(w: &[[u8; 4]], nr: usize, h: &[u8; 16], j0: &[u8; 16], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut g = Vec::with_capacity(aad.len() + ct.len() + 48);
    g.extend_from_slice(aad);
    while g.len() % 16 != 0 {
        g.push(0);
    }
    g.extend_from_slice(ct);
    while g.len() % 16 != 0 {
        g.push(0);
    }
    g.extend_from_slice(&((aad.len() as u64) * 8).to_be_bytes());
    g.extend_from_slice(&((ct.len() as u64) * 8).to_be_bytes());
    let s = ghash(h, &g);
    let mut ej0 = *j0;
    encrypt_block(w, nr, &mut ej0);
    let mut tag = [0u8; 16];
    for i in 0..16 {
        tag[i] = ej0[i] ^ s[i];
    }
    tag
}

/// AES-256-GCM encrypt → (ciphertext, 16-byte tag).
pub fn aes_gcm_encrypt(key: &[u8], iv: &[u8], aad: &[u8], plaintext: &[u8]) -> (Vec<u8>, [u8; 16]) {
    let (w, nr) = expand_key(key);
    let (h, j0) = gcm_setup(&w, nr, iv);
    let mut cb = j0;
    inc32(&mut cb);
    let ct = gctr(&w, nr, &cb, plaintext);
    let tag = gcm_tag(&w, nr, &h, &j0, aad, &ct);
    (ct, tag)
}

/// AES-256-GCM decrypt with tag verification → plaintext, or `None` if
/// the tag doesn't authenticate (constant-time compare).
pub fn aes_gcm_decrypt(
    key: &[u8],
    iv: &[u8],
    aad: &[u8],
    ct: &[u8],
    tag: &[u8],
) -> Option<Vec<u8>> {
    let (w, nr) = expand_key(key);
    let (h, j0) = gcm_setup(&w, nr, iv);
    let expected = gcm_tag(&w, nr, &h, &j0, aad, ct);
    let mut diff = 0u8;
    if tag.len() != 16 {
        return None;
    }
    for i in 0..16 {
        diff |= expected[i] ^ tag[i];
    }
    if diff != 0 {
        return None;
    }
    let mut cb = j0;
    inc32(&mut cb);
    Some(gctr(&w, nr, &cb, ct))
}

// ---- AES-256-CBC ----
//
// CBC chains each plaintext block with the previous ciphertext block
// (or the IV for the first), then runs the forward cipher; decryption
// is the inverse cipher followed by the XOR. PKCS#7 padding is applied
// by the OpenSSL::Cipher veneer, so these operate on block-aligned
// (16-byte-multiple) data.

/// General GF(2^8) multiply (used by InvMixColumns' 9/11/13/14 factors).
#[inline]
fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

#[inline]
fn inv_sub_bytes(s: &mut [u8; 16]) {
    for b in s.iter_mut() {
        *b = INV_SBOX[*b as usize];
    }
}

#[inline]
fn inv_shift_rows(s: &mut [u8; 16]) {
    // Inverse of shift_rows: row r rotates RIGHT by r.
    let orig = *s;
    for row in 1..4 {
        for col in 0..4 {
            s[col * 4 + row] = orig[((col + 4 - row) % 4) * 4 + row];
        }
    }
}

#[inline]
fn inv_mix_columns(s: &mut [u8; 16]) {
    for col in 0..4 {
        let c = col * 4;
        let (a0, a1, a2, a3) = (s[c], s[c + 1], s[c + 2], s[c + 3]);
        s[c] = gmul(a0, 14) ^ gmul(a1, 11) ^ gmul(a2, 13) ^ gmul(a3, 9);
        s[c + 1] = gmul(a0, 9) ^ gmul(a1, 14) ^ gmul(a2, 11) ^ gmul(a3, 13);
        s[c + 2] = gmul(a0, 13) ^ gmul(a1, 9) ^ gmul(a2, 14) ^ gmul(a3, 11);
        s[c + 3] = gmul(a0, 11) ^ gmul(a1, 13) ^ gmul(a2, 9) ^ gmul(a3, 14);
    }
}

/// Decrypt one 16-byte block in place (inverse cipher, FIPS-197 §5.3).
fn decrypt_block(w: &[[u8; 4]], nr: usize, block: &mut [u8; 16]) {
    add_round_key(block, w, nr);
    for round in (1..nr).rev() {
        inv_shift_rows(block);
        inv_sub_bytes(block);
        add_round_key(block, w, round);
        inv_mix_columns(block);
    }
    inv_shift_rows(block);
    inv_sub_bytes(block);
    add_round_key(block, w, 0);
}

/// AES-256-CBC encrypt of block-aligned `data` (len multiple of 16).
pub fn aes_cbc_encrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let (w, nr) = expand_key(key);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = *iv;
    for chunk in data.chunks_exact(16) {
        let mut blk = [0u8; 16];
        for i in 0..16 {
            blk[i] = chunk[i] ^ prev[i];
        }
        encrypt_block(&w, nr, &mut blk);
        out.extend_from_slice(&blk);
        prev = blk;
    }
    out
}

/// AES-256-CBC decrypt of block-aligned `data` (len multiple of 16).
/// Returns the still-PKCS#7-padded plaintext (the veneer strips it).
pub fn aes_cbc_decrypt(key: &[u8], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let (w, nr) = expand_key(key);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = *iv;
    for chunk in data.chunks_exact(16) {
        let mut blk = [0u8; 16];
        blk.copy_from_slice(chunk);
        let ct = blk;
        decrypt_block(&w, nr, &mut blk);
        for i in 0..16 {
            out.push(blk[i] ^ prev[i]);
        }
        prev = ct;
    }
    out
}

/// HMAC-SHA256 (RFC 2104). Returns the 32-byte MAC.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64; // SHA-256 block size.
    // Keys longer than the block size are hashed first.
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let h = crate::digest::sha256(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    // inner = SHA256(ipad || data)
    let mut inner_in = Vec::with_capacity(BLOCK + data.len());
    inner_in.extend_from_slice(&ipad);
    inner_in.extend_from_slice(data);
    let inner = crate::digest::sha256(&inner_in);
    // outer = SHA256(opad || inner)
    let mut outer_in = Vec::with_capacity(BLOCK + 32);
    outer_in.extend_from_slice(&opad);
    outer_in.extend_from_slice(&inner);
    crate::digest::sha256(&outer_in)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn aes256_block_fips197_vector() {
        // FIPS-197 Appendix C.3 (AES-256) known-answer.
        let key: [u8; 32] = unhex(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        ).try_into().unwrap();
        let mut block: [u8; 16] = unhex("00112233445566778899aabbccddeeff").try_into().unwrap();
        let (w, nr) = expand_key(&key);
        encrypt_block(&w, nr, &mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    #[test]
    fn aes128_block_and_modes() {
        // FIPS-197 Appendix C.1 (AES-128) known-answer for the block.
        let key: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let mut block: [u8; 16] = unhex("00112233445566778899aabbccddeeff").try_into().unwrap();
        let (w, nr) = expand_key(&key);
        assert_eq!(nr, 10);
        encrypt_block(&w, nr, &mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");

        // CBC round-trip with a 128-bit key.
        let iv = [0x11u8; 16];
        let pt = b"sixteen byte msg".to_vec();
        let ct = aes_cbc_encrypt(&key, &iv, &pt);
        assert_eq!(aes_cbc_decrypt(&key, &iv, &ct), pt);

        // GCM round-trip + tag verification with a 128-bit key.
        let giv = [0x22u8; 12];
        let (gct, tag) = aes_gcm_encrypt(&key, &giv, b"aad", b"hello aes-128-gcm");
        assert_eq!(aes_gcm_decrypt(&key, &giv, b"aad", &gct, &tag).unwrap(), b"hello aes-128-gcm");
        assert!(aes_gcm_decrypt(&key, &giv, b"wrong", &gct, &tag).is_none());
    }

    #[test]
    fn aes256_ctr_nist_sp800_38a_vector() {
        // NIST SP800-38A F.5.5 CTR-AES256.Encrypt.
        let key: [u8; 32] = unhex(
            "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
        ).try_into().unwrap();
        let iv: [u8; 16] = unhex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff").try_into().unwrap();
        let plaintext = unhex(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ));
        let ct = aes_ctr_xor(&key, &iv, 0, &plaintext);
        assert_eq!(hex(&ct), concat!(
            "601ec313775789a5b7a7f504bbf3d228",
            "f443e3ca4d62b59aca84e990cacaf5c5",
            "2b0930daa23de94ce87017ba2d84988d",
            "dfc9c58db67aada613c2dd08457941a6",
        ));
        // Round-trip: CTR decrypt == same XOR.
        let back = aes_ctr_xor(&key, &iv, 0, &ct);
        assert_eq!(back, plaintext);
    }

    #[test]
    fn aes256_ctr_resumes_across_offsets() {
        // Splitting `update` at an arbitrary byte boundary must produce the
        // same keystream as one shot (the byte_offset resume contract).
        let key = [7u8; 32];
        let iv = [3u8; 16];
        let data: Vec<u8> = (0..70u8).collect();
        let whole = aes_ctr_xor(&key, &iv, 0, &data);
        let a = aes_ctr_xor(&key, &iv, 0, &data[..23]);
        let b = aes_ctr_xor(&key, &iv, 23, &data[23..]);
        assert_eq!([a, b].concat(), whole);
    }

    #[test]
    fn aes256_cbc_nist_sp800_38a_vector() {
        // NIST SP800-38A F.2.5/F.2.6 CBC-AES256 (block-aligned, no pad).
        let key: [u8; 32] = unhex(
            "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
        ).try_into().unwrap();
        let iv: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let pt = unhex(concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ));
        let ct = aes_cbc_encrypt(&key, &iv, &pt);
        assert_eq!(hex(&ct), concat!(
            "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
            "9cfc4e967edb808d679f777bc6702c7d",
            "39f23369a9d9bacfa530e26304231461",
            "b2eb05e2c39be9fcda6c19078c6a9d1b",
        ));
        assert_eq!(aes_cbc_decrypt(&key, &iv, &ct), pt);
    }

    #[test]
    fn aes256_gcm_nist_test_case_14() {
        // GCM spec (McGrew/Viega) Test Case 14, AES-256: all-zero key/IV,
        // empty AAD/plaintext → empty ct, known tag.
        let key = [0u8; 32];
        let iv = [0u8; 12];
        let (ct, tag) = aes_gcm_encrypt(&key, &iv, &[], &[]);
        assert!(ct.is_empty());
        assert_eq!(hex(&tag), "530f8afbc74536b9a963b4f1c4cb738b");
    }

    #[test]
    fn aes256_gcm_nist_test_case_16() {
        // GCM Test Case 16, AES-256: non-empty plaintext + AAD.
        let key: [u8; 32] = unhex(
            "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308",
        ).try_into().unwrap();
        let iv = unhex("cafebabefacedbaddecaf888");
        let aad = unhex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
        let pt = unhex(concat!(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72",
            "1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
        ));
        let (ct, tag) = aes_gcm_encrypt(&key, &iv, &aad, &pt);
        assert_eq!(hex(&ct), concat!(
            "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa",
            "8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662",
        ));
        assert_eq!(hex(&tag), "76fc6ece0f4e1768cddf8853bb2d551b");
        // Round-trip with tag verification.
        let back = aes_gcm_decrypt(&key, &iv, &aad, &ct, &tag).unwrap();
        assert_eq!(back, pt);
        // A flipped tag byte must fail authentication.
        let mut bad = tag;
        bad[0] ^= 1;
        assert!(aes_gcm_decrypt(&key, &iv, &aad, &ct, &bad).is_none());
    }

    #[test]
    fn hmac_sha256_rfc4231_case2() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case1() {
        let key = [0x0bu8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        );
    }
}
