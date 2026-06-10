//! C-locale strftime for Unix timestamps (UTC clock — the rubyrs
//! Tier-1 contract; zone-rendering directives decline). Ported from
//! rubyrs' preamble/time.rb (Howard Hinnant civil_from_days).

use crate::Error;

const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub(crate) struct Civil {
    pub year: i64,
    pub month: usize, // 1..12
    pub day: i64,
    pub hour: i64,
    pub min: i64,
    pub sec: i64,
    pub wday: usize, // 0=Sunday
    pub yday: i64,   // 1..366
}

pub(crate) fn decompose(sec: i64) -> Civil {
    let days = sec.div_euclid(86_400);
    let secs = sec.rem_euclid(86_400);
    // civil_from_days (proleptic Gregorian).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let wday = (days + 4).rem_euclid(7) as usize; // 1970-01-01 = Thursday
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let cum = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut yday = cum[(m - 1) as usize] + d;
    if leap && m > 2 {
        yday += 1;
    }
    Civil {
        year,
        month: m as usize,
        day: d,
        hour: secs / 3600,
        min: (secs % 3600) / 60,
        sec: secs % 60,
        wday,
        yday,
    }
}

/// Format `sec` (Unix seconds, UTC) with a C-locale strftime subset.
/// Unsupported directives decline.
pub(crate) fn strftime(sec: i64, fmt: &str) -> Result<String, Error> {
    let c = decompose(sec);
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        // Optional flag: '-' (no pad), '_' (space pad), '0' (zero pad).
        let mut flag = ' ';
        if matches!(chars.peek(), Some('-' | '_' | '0')) {
            flag = chars.next().expect("peeked");
        }
        let Some(dir) = chars.next() else {
            return Err(Error::Declined("strftime-trailing-percent"));
        };
        match dir {
            '%' => out.push('%'),
            'Y' => out.push_str(&c.year.to_string()),
            'C' => pad2(&mut out, c.year.div_euclid(100), flag, '0'),
            'y' => pad2(&mut out, c.year.rem_euclid(100), flag, '0'),
            'm' => pad2(&mut out, c.month as i64, flag, '0'),
            'd' => pad2(&mut out, c.day, flag, '0'),
            'e' => pad2(&mut out, c.day, flag, ' '),
            'H' => pad2(&mut out, c.hour, flag, '0'),
            'k' => pad2(&mut out, c.hour, flag, ' '),
            'I' => pad2(&mut out, hour12(c.hour), flag, '0'),
            'l' => pad2(&mut out, hour12(c.hour), flag, ' '),
            'M' => pad2(&mut out, c.min, flag, '0'),
            'S' => pad2(&mut out, c.sec, flag, '0'),
            'j' => pad_n(&mut out, c.yday, 3, flag, '0'),
            'B' => out.push_str(MONTH_NAMES[c.month - 1]),
            'b' | 'h' => out.push_str(&MONTH_NAMES[c.month - 1][..3]),
            'A' => out.push_str(DAY_NAMES[c.wday]),
            'a' => out.push_str(&DAY_NAMES[c.wday][..3]),
            'p' => out.push_str(if c.hour < 12 { "AM" } else { "PM" }),
            'P' => out.push_str(if c.hour < 12 { "am" } else { "pm" }),
            'u' => out.push_str(&(if c.wday == 0 { 7 } else { c.wday }).to_string()),
            'w' => out.push_str(&c.wday.to_string()),
            's' => out.push_str(&sec.to_string()),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'F' => {
                out.push_str(&c.year.to_string());
                out.push('-');
                pad2(&mut out, c.month as i64, ' ', '0');
                out.push('-');
                pad2(&mut out, c.day, ' ', '0');
            }
            'T' | 'X' => {
                pad2(&mut out, c.hour, ' ', '0');
                out.push(':');
                pad2(&mut out, c.min, ' ', '0');
                out.push(':');
                pad2(&mut out, c.sec, ' ', '0');
            }
            'R' => {
                pad2(&mut out, c.hour, ' ', '0');
                out.push(':');
                pad2(&mut out, c.min, ' ', '0');
            }
            'D' | 'x' => {
                pad2(&mut out, c.month as i64, ' ', '0');
                out.push('/');
                pad2(&mut out, c.day, ' ', '0');
                out.push('/');
                pad2(&mut out, c.year.rem_euclid(100), ' ', '0');
            }
            // Zone-rendering (%z/%Z), locale-composite (%c), and the
            // ISO-week family (%G/%g/%U/%V/%W) are out of subset.
            _ => return Err(Error::Declined("strftime-directive")),
        }
    }
    Ok(out)
}

fn hour12(h: i64) -> i64 {
    let h = h % 12;
    if h == 0 { 12 } else { h }
}

fn pad2(out: &mut String, n: i64, flag: char, default_pad: char) {
    pad_n(out, n, 2, flag, default_pad);
}

fn pad_n(out: &mut String, n: i64, width: usize, flag: char, default_pad: char) {
    let s = n.to_string();
    let pad = match flag {
        '-' => {
            out.push_str(&s);
            return;
        }
        '_' => ' ',
        '0' => '0',
        _ => default_pad,
    };
    for _ in s.len()..width {
        out.push(pad);
    }
    out.push_str(&s);
}
