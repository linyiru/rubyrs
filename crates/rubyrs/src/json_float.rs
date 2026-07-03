//! Exact Rust port of the `json` gem's float serializer
//! (`ext/json/ext/vendor/fpconv.c`, itself extracted from
//! github.com/night-shift/fpconv, Boost Software License 1.0 —
//! see the license text in that file).
//!
//! Why a port and not `ryu` / `format!`: CRuby's `JSON.generate`
//! does NOT emit `Float#to_s` — it runs fpconv's Grisu2 digit
//! generation plus fpconv's own fixed/scientific window rule
//! (probed on CRuby 3.4.1 / json 2.20.0):
//!
//! ```text
//! 1e15    → "1e+15"        (Float#to_s: "1.0e+15")
//! 1.5e-5  → "0.000015"     (Float#to_s: "1.5e-05")
//! 5e-324  → "5e-324"       (Float#to_s: "5.0e-324")
//! 1e14    → "100000000000000.0"
//! ```
//!
//! Grisu2 also occasionally picks DIFFERENT shortest digits than
//! Ryū (both round-trip): CRuby json emits `1234567890123456.7`
//! where Ryū (and `Float#to_s`) says `1234567890123456.8`. Byte
//! parity with CRuby therefore requires this exact algorithm, not
//! a reshape of Rust's `{:e}` output.
//!
//! The port is 1:1 with the C source, including its unsigned
//! wrapping arithmetic (`wrapping_*` where C could wrap), so the
//! emitted bytes are bit-for-bit what CRuby's generator produces.
//! Verified against CRuby by a 10M-random-double differential run
//! plus the curated edge corpus in the tests below.
//!
//! Not feature-gated: the pure-Ruby JSON canon consumes it too
//! (via the always-registered `__rubyrs_json_float_repr` host fn)
//! so canon and `_json_native` builds emit identical floats.

#[derive(Copy, Clone)]
struct Fp {
    frac: u64,
    exp: i32,
}

const NPOWERS: i32 = 87;
const STEPPOWERS: i32 = 8;
const FIRSTPOWER: i32 = -348; // 10^-348
const EXPMAX: i32 = -32;
const EXPMIN: i32 = -60;

const FRACMASK: u64 = 0x000F_FFFF_FFFF_FFFF;
const EXPMASK: u64 = 0x7FF0_0000_0000_0000;
const HIDDENBIT: u64 = 0x0010_0000_0000_0000;
const SIGNMASK: u64 = 0x8000_0000_0000_0000;
const EXPBIAS: i32 = 1023 + 52;

#[rustfmt::skip]
const POWERS_TEN: [Fp; 87] = [
    Fp { frac: 18054884314459144840, exp: -1220 }, Fp { frac: 13451937075301367670, exp: -1193 },
    Fp { frac: 10022474136428063862, exp: -1166 }, Fp { frac: 14934650266808366570, exp: -1140 },
    Fp { frac: 11127181549972568877, exp: -1113 }, Fp { frac: 16580792590934885855, exp: -1087 },
    Fp { frac: 12353653155963782858, exp: -1060 }, Fp { frac: 18408377700990114895, exp: -1034 },
    Fp { frac: 13715310171984221708, exp: -1007 }, Fp { frac: 10218702384817765436, exp: -980 },
    Fp { frac: 15227053142812498563, exp: -954 },  Fp { frac: 11345038669416679861, exp: -927 },
    Fp { frac: 16905424996341287883, exp: -901 },  Fp { frac: 12595523146049147757, exp: -874 },
    Fp { frac: 9384396036005875287,  exp: -847 },  Fp { frac: 13983839803942852151, exp: -821 },
    Fp { frac: 10418772551374772303, exp: -794 },  Fp { frac: 15525180923007089351, exp: -768 },
    Fp { frac: 11567161174868858868, exp: -741 },  Fp { frac: 17236413322193710309, exp: -715 },
    Fp { frac: 12842128665889583758, exp: -688 },  Fp { frac: 9568131466127621947,  exp: -661 },
    Fp { frac: 14257626930069360058, exp: -635 },  Fp { frac: 10622759856335341974, exp: -608 },
    Fp { frac: 15829145694278690180, exp: -582 },  Fp { frac: 11793632577567316726, exp: -555 },
    Fp { frac: 17573882009934360870, exp: -529 },  Fp { frac: 13093562431584567480, exp: -502 },
    Fp { frac: 9755464219737475723,  exp: -475 },  Fp { frac: 14536774485912137811, exp: -449 },
    Fp { frac: 10830740992659433045, exp: -422 },  Fp { frac: 16139061738043178685, exp: -396 },
    Fp { frac: 12024538023802026127, exp: -369 },  Fp { frac: 17917957937422433684, exp: -343 },
    Fp { frac: 13349918974505688015, exp: -316 },  Fp { frac: 9946464728195732843,  exp: -289 },
    Fp { frac: 14821387422376473014, exp: -263 },  Fp { frac: 11042794154864902060, exp: -236 },
    Fp { frac: 16455045573212060422, exp: -210 },  Fp { frac: 12259964326927110867, exp: -183 },
    Fp { frac: 18268770466636286478, exp: -157 },  Fp { frac: 13611294676837538539, exp: -130 },
    Fp { frac: 10141204801825835212, exp: -103 },  Fp { frac: 15111572745182864684, exp: -77 },
    Fp { frac: 11258999068426240000, exp: -50 },   Fp { frac: 16777216000000000000, exp: -24 },
    Fp { frac: 12500000000000000000, exp: 3 },     Fp { frac: 9313225746154785156,  exp: 30 },
    Fp { frac: 13877787807814456755, exp: 56 },    Fp { frac: 10339757656912845936, exp: 83 },
    Fp { frac: 15407439555097886824, exp: 109 },   Fp { frac: 11479437019748901445, exp: 136 },
    Fp { frac: 17105694144590052135, exp: 162 },   Fp { frac: 12744735289059618216, exp: 189 },
    Fp { frac: 9495567745759798747,  exp: 216 },   Fp { frac: 14149498560666738074, exp: 242 },
    Fp { frac: 10542197943230523224, exp: 269 },   Fp { frac: 15709099088952724970, exp: 295 },
    Fp { frac: 11704190886730495818, exp: 322 },   Fp { frac: 17440603504673385349, exp: 348 },
    Fp { frac: 12994262207056124023, exp: 375 },   Fp { frac: 9681479787123295682,  exp: 402 },
    Fp { frac: 14426529090290212157, exp: 428 },   Fp { frac: 10748601772107342003, exp: 455 },
    Fp { frac: 16016664761464807395, exp: 481 },   Fp { frac: 11933345169920330789, exp: 508 },
    Fp { frac: 17782069995880619868, exp: 534 },   Fp { frac: 13248674568444952270, exp: 561 },
    Fp { frac: 9871031767461413346,  exp: 588 },   Fp { frac: 14708983551653345445, exp: 614 },
    Fp { frac: 10959046745042015199, exp: 641 },   Fp { frac: 16330252207878254650, exp: 667 },
    Fp { frac: 12166986024289022870, exp: 694 },   Fp { frac: 18130221999122236476, exp: 720 },
    Fp { frac: 13508068024458167312, exp: 747 },   Fp { frac: 10064294952495520794, exp: 774 },
    Fp { frac: 14996968138956309548, exp: 800 },   Fp { frac: 11173611982879273257, exp: 827 },
    Fp { frac: 16649979327439178909, exp: 853 },   Fp { frac: 12405201291620119593, exp: 880 },
    Fp { frac: 9242595204427927429,  exp: 907 },   Fp { frac: 13772540099066387757, exp: 933 },
    Fp { frac: 10261342003245940623, exp: 960 },   Fp { frac: 15290591125556738113, exp: 986 },
    Fp { frac: 11392378155556871081, exp: 1013 },  Fp { frac: 16975966327722178521, exp: 1039 },
    Fp { frac: 12648080533535911531, exp: 1066 },
];

fn find_cachedpow10(exp: i32, k: &mut i32) -> Fp {
    const ONE_LOG_TEN: f64 = 0.30102999566398114;

    let approx = (-((exp + NPOWERS) as f64) * ONE_LOG_TEN) as i32;
    let mut idx = (approx - FIRSTPOWER) / STEPPOWERS;

    loop {
        let current = exp + POWERS_TEN[idx as usize].exp + 64;

        if current < EXPMIN {
            idx += 1;
            continue;
        }
        if current > EXPMAX {
            idx -= 1;
            continue;
        }

        *k = FIRSTPOWER + idx * STEPPOWERS;
        return POWERS_TEN[idx as usize];
    }
}

#[rustfmt::skip]
const TENS: [u64; 20] = [
    10000000000000000000, 1000000000000000000, 100000000000000000,
    10000000000000000, 1000000000000000, 100000000000000,
    10000000000000, 1000000000000, 100000000000,
    10000000000, 1000000000, 100000000,
    10000000, 1000000, 100000,
    10000, 1000, 100,
    10, 1,
];

fn build_fp(d: f64) -> Fp {
    let bits = d.to_bits();
    let mut fp = Fp {
        frac: bits & FRACMASK,
        exp: ((bits & EXPMASK) >> 52) as i32,
    };
    if fp.exp != 0 {
        fp.frac += HIDDENBIT;
        fp.exp -= EXPBIAS;
    } else {
        fp.exp = -EXPBIAS + 1;
    }
    fp
}

fn normalize(fp: &mut Fp) {
    while (fp.frac & HIDDENBIT) == 0 {
        fp.frac <<= 1;
        fp.exp -= 1;
    }
    let shift = 64 - 52 - 1;
    fp.frac <<= shift;
    fp.exp -= shift;
}

fn get_normalized_boundaries(fp: &Fp, lower: &mut Fp, upper: &mut Fp) {
    upper.frac = (fp.frac << 1) + 1;
    upper.exp = fp.exp - 1;

    while (upper.frac & (HIDDENBIT << 1)) == 0 {
        upper.frac <<= 1;
        upper.exp -= 1;
    }

    let u_shift = 64 - 52 - 2;
    upper.frac <<= u_shift;
    upper.exp -= u_shift;

    let l_shift = if fp.frac == HIDDENBIT { 2 } else { 1 };
    lower.frac = (fp.frac << l_shift) - 1;
    lower.exp = fp.exp - l_shift;

    lower.frac <<= lower.exp - upper.exp;
    lower.exp = upper.exp;
}

fn multiply(a: &Fp, b: &Fp) -> Fp {
    const LOMASK: u64 = 0x0000_0000_FFFF_FFFF;

    let ah_bl = (a.frac >> 32).wrapping_mul(b.frac & LOMASK);
    let al_bh = (a.frac & LOMASK).wrapping_mul(b.frac >> 32);
    let al_bl = (a.frac & LOMASK).wrapping_mul(b.frac & LOMASK);
    let ah_bh = (a.frac >> 32).wrapping_mul(b.frac >> 32);

    let mut tmp = (ah_bl & LOMASK) + (al_bh & LOMASK) + (al_bl >> 32);
    // round up
    tmp += 1 << 31;

    Fp {
        frac: ah_bh
            .wrapping_add(ah_bl >> 32)
            .wrapping_add(al_bh >> 32)
            .wrapping_add(tmp >> 32),
        exp: a.exp + b.exp + 64,
    }
}

/// C's unsigned arithmetic wraps by definition; mirror it exactly
/// (the comparisons below are reachable with operands the C code
/// also wraps on) so the emitted digits can't diverge from CRuby.
fn round_digit(digits: &mut [u8], ndigits: usize, delta: u64, mut rem: u64, kappa: u64, frac: u64) {
    while rem < frac
        && delta.wrapping_sub(rem) >= kappa
        && (rem.wrapping_add(kappa) < frac || frac - rem > rem.wrapping_add(kappa).wrapping_sub(frac))
    {
        digits[ndigits - 1] -= 1;
        rem = rem.wrapping_add(kappa);
    }
}

fn generate_digits(fp: &Fp, upper: &Fp, lower: &Fp, digits: &mut [u8], k: &mut i32) -> usize {
    let wfrac = upper.frac - fp.frac;
    let delta = upper.frac - lower.frac;

    let one = Fp {
        frac: 1u64 << -upper.exp,
        exp: upper.exp,
    };

    let mut part1 = upper.frac >> -one.exp;
    let mut part2 = upper.frac & (one.frac - 1);

    let mut idx: usize = 0;
    let mut kappa: i32 = 10;

    // 1000000000 — the kappa > 0 loop walks TENS[10..]
    let mut divp = 10usize;
    while kappa > 0 {
        let div = TENS[divp];
        let digit = part1 / div;

        if digit != 0 || idx != 0 {
            digits[idx] = digit as u8 + b'0';
            idx += 1;
        }

        part1 -= digit * div;
        kappa -= 1;

        let tmp = (part1 << -one.exp).wrapping_add(part2);
        if tmp <= delta {
            *k += kappa;
            round_digit(digits, idx, delta, tmp, div.wrapping_shl((-one.exp) as u32), wfrac);
            return idx;
        }
        divp += 1;
    }

    // 10 — the fractional-digit loop
    let mut unit = 18usize;
    let mut delta = delta;
    loop {
        part2 = part2.wrapping_mul(10);
        delta = delta.wrapping_mul(10);
        kappa -= 1;

        let digit = part2 >> -one.exp;
        if digit != 0 || idx != 0 {
            digits[idx] = digit as u8 + b'0';
            idx += 1;
        }

        part2 &= one.frac - 1;
        if part2 < delta {
            *k += kappa;
            round_digit(digits, idx, delta, part2, one.frac, wfrac.wrapping_mul(TENS[unit]));
            return idx;
        }
        unit -= 1;
    }
}

fn grisu2(d: f64, digits: &mut [u8], k: &mut i32) -> usize {
    let mut w = build_fp(d);

    let mut lower = Fp { frac: 0, exp: 0 };
    let mut upper = Fp { frac: 0, exp: 0 };
    get_normalized_boundaries(&w, &mut lower, &mut upper);

    normalize(&mut w);

    let mut cached_k = 0;
    let cp = find_cachedpow10(upper.exp, &mut cached_k);

    w = multiply(&w, &cp);
    upper = multiply(&upper, &cp);
    lower = multiply(&lower, &cp);

    lower.frac += 1;
    upper.frac -= 1;

    *k = -cached_k;

    generate_digits(&w, &upper, &lower, digits, k)
}

fn emit_digits(digits: &[u8], mut ndigits: usize, dest: &mut [u8], k: i32, neg: bool) -> usize {
    let exp = (k + ndigits as i32 - 1).abs();

    // write plain integer (with a ".0" to mark it as a float)
    if k >= 0 && exp < 15 {
        let ku = k as usize;
        dest[..ndigits].copy_from_slice(&digits[..ndigits]);
        dest[ndigits..ndigits + ku].fill(b'0');
        dest[ndigits + ku] = b'.';
        dest[ndigits + ku + 1] = b'0';
        return ndigits + ku + 2;
    }

    // write decimal w/o scientific notation
    if k < 0 && (k > -7 || exp < 10) {
        let offset = ndigits as i32 - (-k);
        // fp < 1.0 -> write leading zero
        if offset <= 0 {
            let offset = (-offset) as usize;
            dest[0] = b'0';
            dest[1] = b'.';
            dest[2..2 + offset].fill(b'0');
            dest[2 + offset..2 + offset + ndigits].copy_from_slice(&digits[..ndigits]);
            return ndigits + 2 + offset;
        }
        // fp > 1.0
        let offset = offset as usize;
        dest[..offset].copy_from_slice(&digits[..offset]);
        dest[offset] = b'.';
        dest[offset + 1..ndigits + 1].copy_from_slice(&digits[offset..ndigits]);
        return ndigits + 1;
    }

    // write decimal w/ scientific notation
    ndigits = ndigits.min(18 - usize::from(neg));

    let mut idx: usize = 0;
    dest[idx] = digits[0];
    idx += 1;

    if ndigits > 1 {
        dest[idx] = b'.';
        idx += 1;
        dest[idx..idx + ndigits - 1].copy_from_slice(&digits[1..ndigits]);
        idx += ndigits - 1;
    }

    dest[idx] = b'e';
    idx += 1;

    let sign = if k + ndigits as i32 - 1 < 0 { b'-' } else { b'+' };
    dest[idx] = sign;
    idx += 1;

    let mut exp = exp;
    let mut cent = 0;
    if exp > 99 {
        cent = exp / 100;
        dest[idx] = cent as u8 + b'0';
        idx += 1;
        exp -= cent * 100;
    }
    if exp > 9 {
        let dec = exp / 10;
        dest[idx] = dec as u8 + b'0';
        idx += 1;
        exp -= dec * 10;
    } else if cent != 0 {
        dest[idx] = b'0';
        idx += 1;
    }

    dest[idx] = (exp % 10) as u8 + b'0';
    idx += 1;

    idx
}

fn filter_special(fp: f64, dest: &mut [u8]) -> usize {
    if fp == 0.0 {
        dest[0] = b'0';
        dest[1] = b'.';
        dest[2] = b'0';
        return 3;
    }

    let bits = fp.to_bits();
    let nan = (bits & EXPMASK) == EXPMASK;
    if !nan {
        return 0;
    }

    if bits & FRACMASK != 0 {
        dest[..3].copy_from_slice(b"nan");
    } else {
        dest[..3].copy_from_slice(b"inf");
    }
    3
}

/// The gem's `fpconv_dtoa`: writes the JSON representation of `d`
/// into `dest`, returns the byte length. Never writes more than 32
/// bytes. NaN / ±Infinity come out as `nan` / `inf` exactly like
/// the C function — callers that must raise (JSON's default) check
/// finiteness BEFORE calling.
pub(crate) fn fpconv_dtoa(d: f64, dest: &mut [u8; 32]) -> usize {
    let mut digits = [0u8; 18];

    let mut str_len: usize = 0;
    let neg = d.to_bits() & SIGNMASK != 0;
    if neg {
        dest[0] = b'-';
        str_len += 1;
    }

    let spec = filter_special(d, &mut dest[str_len..]);
    if spec != 0 {
        return str_len + spec;
    }

    let mut k = 0;
    let ndigits = grisu2(d, &mut digits, &mut k);

    str_len + emit_digits(&digits, ndigits, &mut dest[str_len..], k, neg)
}

/// Append CRuby-`JSON.generate` bytes for a finite `f` to `out`.
pub(crate) fn write_json_float(f: f64, out: &mut Vec<u8>) {
    let mut buf = [0u8; 32];
    let n = fpconv_dtoa(f, &mut buf);
    out.extend_from_slice(&buf[..n]);
}

/// `JSON.generate` float repr as a `String` — the canon-side
/// surface (`__rubyrs_json_float_repr` host fn).
pub(crate) fn json_float_to_string(f: f64) -> String {
    let mut buf = [0u8; 32];
    let n = fpconv_dtoa(f, &mut buf);
    // fpconv emits pure ASCII.
    unsafe { String::from_utf8_unchecked(buf[..n].to_vec()) }
}

/// Register the always-on `__rubyrs_json_float_repr(float) → String`
/// host fn: the pure-Ruby JSON canon's `generate_with` Float arm
/// calls it (when defined) so canon output matches CRuby's fpconv
/// bytes exactly — including the rare doubles where Grisu2's digit
/// pick differs from `Float#to_s`'s (Ryū) digits. Not feature-gated:
/// the canon must emit identical bytes with or without
/// `_json_native` built in.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    use crate::error::{RubyError, Trap};
    use crate::value::Value;
    rt.register_fn("__rubyrs_json_float_repr", |args| {
        let f = match args {
            [Value::Float(f)] => *f,
            [Value::Int(n)] => *n as f64,
            _ => {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: "__rubyrs_json_float_repr(float)".to_string(),
                    },
                    backtrace: vec![],
                })
            }
        };
        if f.is_nan() || f.is_infinite() {
            return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: format!("{f} not allowed in JSON"),
                },
                backtrace: vec![],
            });
        }
        Ok(Value::new_str_us_ascii(json_float_to_string(f)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(f: f64) -> String {
        json_float_to_string(f)
    }

    /// Curated corpus — expected bytes captured from CRuby 3.4.1 +
    /// json 2.20.0 (`JSON.generate([v])`), 2026-07-03.
    #[test]
    #[allow(clippy::approx_constant)] // 3.14159 is corpus data, not a π stand-in
    fn curated_edge_corpus_matches_cruby() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (0.5, "0.5"),
            (0.1, "0.1"),
            (2.0 / 3.0, "0.6666666666666666"),
            (100.0, "100.0"),
            (12345.0, "12345.0"),
            (3.14159, "3.14159"),
            (1e14, "100000000000000.0"),
            (99999999999999.0, "99999999999999.0"),
            (999999999999999.0, "999999999999999.0"),
            (1e15, "1e+15"),
            (1.5e15, "1.5e+15"),
            (9999999999999998.0, "9.999999999999998e+15"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1e17, "1e+17"),
            (1e18, "1e+18"),
            (1e20, "1e+20"),
            (-1e20, "-1e+20"),
            (123456789012345.6, "123456789012345.6"),
            // Grisu2 digit pick differs from Ryū here (both
            // round-trip): CRuby json emits ...6.7, Float#to_s
            // says ...6.8. We must match CRuby json.
            (1234567890123456.8, "1234567890123456.7"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (12345678901234567890.0, "1.2345678901234567e+19"),
            (1e-4, "0.0001"),
            (1e-5, "0.00001"),
            (1e-6, "0.000001"),
            (1e-7, "0.0000001"),
            (1e-8, "0.00000001"),
            (1.5e-5, "0.000015"),
            (1.5e-6, "0.0000015"),
            (1.5e-7, "0.00000015"),
            (-1.5e-7, "-0.00000015"),
            (1.23456789e-5, "0.0000123456789"),
            (0.00012345678901234567, "0.00012345678901234567"),
            (1.2345678901234567e-7, "0.00000012345678901234566"),
            (5e-324, "5e-324"),                                   // min subnormal
            (2.2250738585072014e-308, "2.2250738585072014e-308"), // min normal
            (1.7976931348623157e308, "1.7976931348623157e+308"),  // f64::MAX
        ];
        for (f, want) in cases {
            assert_eq!(&s(*f), want, "float {f:?} (bits {:016x})", f.to_bits());
        }
    }

    /// Every emitted string must re-parse to the exact same f64 —
    /// over 1M+ random bit patterns. (Byte-parity vs CRuby is pinned
    /// by the 10M-sample differential run documented in the module
    /// header + the diff fixture corpus; this permanent test pins
    /// the round-trip property + panic-freedom on arbitrary bits.)
    #[test]
    fn round_trip_property_random_bits() {
        // xorshift64* — deterministic, no rand dep.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state = state.wrapping_mul(0x2545F4914F6CDD1D);
            state
        };
        let mut tested = 0u64;
        while tested < 1_200_000 {
            let bits = next();
            let f = f64::from_bits(bits);
            if f.is_nan() || f.is_infinite() {
                continue;
            }
            let out = s(f);
            let back: f64 = out.parse().unwrap_or_else(|_| panic!("unparseable {out:?} for bits {bits:016x}"));
            assert_eq!(
                back.to_bits(),
                f.to_bits(),
                "round-trip mismatch: {f:?} (bits {bits:016x}) emitted {out:?}"
            );
            tested += 1;
        }
    }

    /// Subnormals + boundaries sweep (denser than the random walk).
    #[test]
    fn round_trip_boundaries() {
        for bits in (0u64..5000)
            .chain((1u64 << 52) - 2500..(1u64 << 52) + 2500)
            .chain(0x7FEF_FFFF_FFFF_FFFFu64 - 2500..=0x7FEF_FFFF_FFFF_FFFF)
        {
            let f = f64::from_bits(bits);
            if f.is_nan() || f.is_infinite() {
                continue;
            }
            let out = s(f);
            assert_eq!(out.parse::<f64>().unwrap().to_bits(), bits, "bits {bits:016x} → {out:?}");
            let neg = f64::from_bits(bits | SIGNMASK);
            if !neg.is_nan() && !neg.is_infinite() {
                let out = s(neg);
                assert_eq!(out.parse::<f64>().unwrap().to_bits(), bits | SIGNMASK, "-bits {bits:016x} → {out:?}");
            }
        }
    }
}
