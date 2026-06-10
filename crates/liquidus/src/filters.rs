//! Filter implementations, byte-aligned with Ruby liquid 4.0.4's
//! standardfilters.rb and Jekyll 4.4's filters. Inputs the exact
//! semantics can't be reproduced for (non-ASCII where Ruby uses
//! Unicode tables, absolute URLs needing Addressable normalization)
//! decline the render.

use crate::parse::Expr;
use crate::strftime;
use crate::{Error, LValue, Template};

fn declined(what: &'static str) -> Error {
    Error::Declined(what)
}

pub(crate) fn apply(
    tpl: &Template,
    name: &str,
    input: LValue,
    args: &[Expr],
) -> Result<LValue, Error> {
    match name {
        "upcase" => str_map(input, |s| {
            if s.is_ascii() {
                Ok(s.to_ascii_uppercase())
            } else {
                // Ruby String#upcase is Unicode-aware.
                Err(declined("upcase-non-ascii"))
            }
        }),
        "downcase" => str_map(input, |s| {
            if s.is_ascii() {
                Ok(s.to_ascii_lowercase())
            } else {
                Err(declined("downcase-non-ascii"))
            }
        }),
        "strip" => str_map(input, |s| {
            Ok(
                s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '\0'))
                    .to_string(),
            )
        }),
        "escape" => str_map(input, |s| {
            // liquid escape == CGI.escapeHTML: & < > " '
            let mut out = String::with_capacity(s.len() + 8);
            for c in s.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\'' => out.push_str("&#39;"),
                    _ => out.push(c),
                }
            }
            Ok(out)
        }),
        "slugify" => {
            if !args.is_empty() {
                return Err(declined("slugify-mode-arg"));
            }
            str_map(input, |s| {
                // Jekyll Utils.slugify mode "default":
                // [^\p{M}\p{L}\p{Nd}]+ → "-", strip edge hyphens,
                // downcase. The ASCII projection of that class is
                // [^A-Za-z0-9]+; any non-ASCII char would need the
                // Unicode property tables.
                if !s.is_ascii() {
                    return Err(declined("slugify-non-ascii"));
                }
                let mut slug = String::with_capacity(s.len());
                let mut in_run = false;
                for b in s.bytes() {
                    if b.is_ascii_alphanumeric() {
                        slug.push(b.to_ascii_lowercase() as char);
                        in_run = false;
                    } else if !in_run {
                        slug.push('-');
                        in_run = true;
                    }
                }
                let trimmed = slug.trim_matches('-');
                Ok(trimmed.to_string())
            })
        }
        "truncate" => {
            // liquid: l = max(0, length - ellipsis.len);
            // chars > length ? input[0...l] + ellipsis : input
            let length = match args.first() {
                Some(Expr::IntLit(n)) if *n >= 0 => *n as usize,
                None => 50,
                _ => return Err(declined("truncate-arg-shape")),
            };
            let ellipsis = match args.get(1) {
                Some(Expr::StrLit(s)) => s.clone(),
                None => "...".to_string(),
                _ => return Err(declined("truncate-arg-shape")),
            };
            str_map(input, move |s| {
                let chars: Vec<char> = s.chars().collect();
                if chars.len() > length {
                    let l = length.saturating_sub(ellipsis.chars().count());
                    let mut out: String = chars[..l.min(chars.len())].iter().collect();
                    out.push_str(&ellipsis);
                    Ok(out)
                } else {
                    Ok(s)
                }
            })
        }
        "number_of_words" => {
            if !args.is_empty() {
                return Err(declined("number_of_words-mode"));
            }
            str_map_to(input, |s| {
                // Jekyll default mode: input.split.length — but the
                // CJK regex classes in the other modes mean non-ASCII
                // input deserves the Ruby path.
                if !s.is_ascii() {
                    return Err(declined("number_of_words-non-ascii"));
                }
                Ok(LValue::Int(s.split_whitespace().count() as i64))
            })
        }
        "size" => Ok(match input {
            LValue::Array(items) => LValue::Int(items.len() as i64),
            LValue::Str(s) => LValue::Int(s.chars().count() as i64),
            LValue::Map(pairs) => LValue::Int(pairs.len() as i64),
            LValue::Nil => LValue::Int(0),
            _ => return Err(declined("size-shape")),
        }),
        "append" => {
            let Some(Expr::StrLit(suffix)) = args.first() else {
                return Err(declined("append-arg-shape"));
            };
            str_map(input, move |s| Ok(s + suffix))
        }
        "prepend" => {
            let Some(Expr::StrLit(prefix)) = args.first() else {
                return Err(declined("prepend-arg-shape"));
            };
            str_map(input, move |s| Ok(format!("{prefix}{s}")))
        }
        "relative_url" => {
            str_map(input, |s| {
                // Jekyll compute_relative_url: absolute URIs pass
                // through; otherwise ensure_leading_slash(baseurl) +
                // ensure_leading_slash(input), then Addressable
                // normalize. For the conservative character set below
                // normalize is the identity; anything else declines.
                if s.contains("://") {
                    return Err(declined("relative_url-absolute"));
                }
                if !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/-._~".contains(&b))
                {
                    return Err(declined("relative_url-charset"));
                }
                let mut out = String::new();
                push_with_leading_slash(&mut out, &tpl.config.baseurl);
                push_with_leading_slash(&mut out, &s);
                Ok(out)
            })
        }
        "date" => {
            let Some(Expr::StrLit(fmt)) = args.first() else {
                return Err(declined("date-arg-shape"));
            };
            let LValue::Time { sec, .. } = input else {
                // String inputs go through Liquid::Utils.to_date
                // parsing — decline, the embedder supplies real Times.
                return Err(declined("date-input-shape"));
            };
            Ok(LValue::Str(strftime::strftime(sec, fmt)?))
        }
        "date_to_xmlschema" => {
            let LValue::Time { sec, .. } = input else {
                return Err(declined("date-input-shape"));
            };
            // Jekyll: time(input).localtime.xmlschema — the localtime
            // pass forces the LOCAL flavour, which under the TZ=UTC
            // contract renders the zone as "+00:00".
            let mut out = strftime::strftime(sec, "%Y-%m-%dT%H:%M:%S")?;
            out.push_str("+00:00");
            Ok(LValue::Str(out))
        }
        _ => Err(declined("unsupported-filter")),
    }
}

fn push_with_leading_slash(out: &mut String, part: &str) {
    // Jekyll ensure_leading_slash: empty stays empty; otherwise a
    // leading "/" is guaranteed.
    if part.is_empty() {
        return;
    }
    if !part.starts_with('/') {
        out.push('/');
    }
    out.push_str(part);
}

fn str_map(
    input: LValue,
    f: impl FnOnce(String) -> Result<String, Error>,
) -> Result<LValue, Error> {
    str_map_to(input, |s| f(s).map(LValue::Str))
}

fn str_map_to(
    input: LValue,
    f: impl FnOnce(String) -> Result<LValue, Error>,
) -> Result<LValue, Error> {
    match input {
        LValue::Str(s) => f(s),
        // Liquid coerces filter inputs via to_s; nil → "" for most
        // string filters. Keeping it strict avoids reproducing every
        // coercion table — the embedder supplies strings.
        LValue::Nil => f(String::new()),
        LValue::Int(n) => f(n.to_string()),
        _ => Err(declined("filter-input-shape")),
    }
}
