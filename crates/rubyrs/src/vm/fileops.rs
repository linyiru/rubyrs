//! `File` class-method shims. Path-based file operations used by
//! gemspec evaluation, Rakefiles, and other build-tool style
//! scripts. Pulled out of `vm.rs` for readability; the dispatch
//! itself happens in `do_call` (vm.rs) and routes to
//! `Vm::file_class_dispatch` here.

use std::path::{Path, PathBuf};

use crate::error::{RubyError, Trap};
use crate::heap::HeapObj;
use crate::value::Value;

use super::Vm;

/// Match a single glob path-segment pattern (`*`, `?`, literals)
/// against a filename. `*` matches any run of non-`/` chars, `?` one
/// char. (Brace/bracket classes `{a,b}` / `[..]` are not yet
/// supported — documented gap.)
fn glob_seg_match(pat: &[u8], txt: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_pi, mut star_ti): (Option<usize>, usize) = (None, 0);
    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

/// Whether glob segment `pat` matches directory entry `name`,
/// honouring Ruby's rule that wildcard segments don't match names
/// beginning with `.` unless the pattern itself begins with `.`.
fn glob_name_match(pat: &str, name: &str) -> bool {
    if name.starts_with('.') && !pat.starts_with('.') {
        return false;
    }
    glob_seg_match(pat.as_bytes(), name.as_bytes())
}

/// Sorted directory entries (name, full path) of `base`, or empty on
/// any read error (Ruby's glob silently skips unreadable dirs).
fn sorted_entries(base: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = match std::fs::read_dir(base) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Recursive glob walk. `segs` is the remaining pattern segments;
/// matches accumulate as PathBufs into `results`.
fn glob_walk(base: &Path, segs: &[String], results: &mut Vec<PathBuf>) {
    let Some(seg) = segs.first() else { return };
    let rest = &segs[1..];
    if seg == "**" {
        // `**` matches zero path components: apply the rest here.
        if rest.is_empty() {
            // Trailing `**` — match every descendant directory.
            for (name, p) in sorted_entries(base) {
                if name.starts_with('.') || !p.is_dir() {
                    continue;
                }
                results.push(p.clone());
                glob_walk(&p, segs, results);
            }
        } else {
            glob_walk(base, rest, results);
            // …and one-or-more components: descend keeping `**`.
            for (name, p) in sorted_entries(base) {
                if name.starts_with('.') || !p.is_dir() {
                    continue;
                }
                glob_walk(&p, segs, results);
            }
        }
        return;
    }
    for (name, p) in sorted_entries(base) {
        if !glob_name_match(seg, &name) {
            continue;
        }
        if rest.is_empty() {
            results.push(p);
        } else if p.is_dir() {
            glob_walk(&p, rest, results);
        }
    }
}

/// Split a brace group's interior on top-level commas (commas not
/// inside a nested `{...}`).
fn split_top_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    parts
}

/// Expand `{a,b,c}` brace alternations into concrete patterns
/// (cartesian over multiple/nested groups). `*.{rb,txt}` →
/// `["*.rb", "*.txt"]`.
fn expand_braces(pattern: &str, out: &mut Vec<String>) {
    let Some(open) = pattern.find('{') else {
        out.push(pattern.to_string());
        return;
    };
    // Find the matching close brace, honouring nesting.
    let mut depth = 0i32;
    let mut close = None;
    for (i, c) in pattern[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        // Unbalanced brace — treat literally.
        out.push(pattern.to_string());
        return;
    };
    let pre = &pattern[..open];
    let post = &pattern[close + 1..];
    let inner = &pattern[open + 1..close];
    for alt in split_top_commas(inner) {
        expand_braces(&format!("{}{}{}", pre, alt, post), out);
    }
}

/// Expand a single glob pattern into matching path strings (Ruby
/// `Dir.glob` semantics for `*` / `?` / `**` / `{a,b}` / literal
/// segments). Absolute patterns yield absolute paths; relative
/// patterns yield paths without a leading `./`. Results are deduped
/// and sorted (Ruby 3.0+).
fn glob_expand(pattern: &str) -> Vec<String> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let mut patterns: Vec<String> = Vec::new();
    expand_braces(pattern, &mut patterns);
    let mut out: Vec<String> = Vec::new();
    for pat in &patterns {
        let absolute = pat.starts_with('/');
        let segs: Vec<String> = pat
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        if segs.is_empty() {
            continue;
        }
        let root = if absolute { PathBuf::from("/") } else { PathBuf::from(".") };
        let mut results: Vec<PathBuf> = Vec::new();
        glob_walk(&root, &segs, &mut results);
        for p in results {
            let s = if absolute {
                p.to_string_lossy().into_owned()
            } else {
                // Strip the synthetic "./" root we walked from.
                p.strip_prefix(".").unwrap_or(&p).to_string_lossy().into_owned()
            };
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out.sort();
    out
}

/// CRuby `File.basename` base computation — a pure byte-level
/// string op, NOT `Path::file_name()`: the std method returns
/// `None` for `"/"` and `".."` (rubyrs then rendered `""`), while
/// CRuby returns `"/"` and `".."`. Rule (probed vs ruby 3.4):
/// strip ALL trailing slashes (a path that was nothing but slashes
/// is `"/"`), then take everything after the last remaining slash
/// — `"."` / `".."` are ordinary names, no normalization.
fn ruby_basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "" } else { "/" };
    }
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// CRuby `File.dirname` — byte-level twin of `ruby_basename`
/// (`Path::parent()` returned `Some("")` for `"a"` → rubyrs
/// rendered `""` where CRuby says `"."`, and `None` for `"/"` →
/// `"."` where CRuby says `"/"`). Rule (probed vs ruby 3.4):
/// strip trailing slashes; cut the last component AND the whole
/// separator run before it (`"a//b"` → `"a"`); empty result →
/// `"/"` for absolute, `"."` for relative (also `"a"` → `"."`);
/// a LEADING separator run collapses to a single `"/"`
/// (`"//a/b"` → `"/a"`) while interior runs away from the cut
/// are preserved (`"a//b/c"` → `"a//b"`).
fn ruby_dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { ".".to_string() } else { "/".to_string() };
    }
    let Some(cut) = trimmed.rfind('/') else {
        return ".".to_string();
    };
    let head = trimmed[..cut].trim_end_matches('/');
    if head.is_empty() {
        return "/".to_string();
    }
    let non_slash = head.find(|c| c != '/').unwrap_or(head.len());
    if non_slash > 1 {
        format!("/{}", &head[non_slash..])
    } else {
        head.to_string()
    }
}

impl Vm {
    /// E3 `ext:int` read transcode: decode the bytes as `ext`,
    /// re-encode into `int`. UTF-8/US-ASCII/BINARY sources decode
    /// trivially; registry sources go through encoding_full. An
    /// undecodable byte raises CRuby's
    /// Encoding::InvalidByteSequenceError; an unmappable char on
    /// the encode side raises UndefinedConversionError.
    fn transcode_read(
        &mut self,
        bytes: &[u8],
        ext: crate::value::EncodingTag,
        int: crate::value::EncodingTag,
        path: &str,
    ) -> Result<Vec<u8>, crate::error::Trap> {
        use crate::value::EncodingTag;
        if ext == int {
            return Ok(bytes.to_vec());
        }
        // Decode to UTF-8 text first (the pivot CRuby uses too).
        let text: String = match ext {
            EncodingTag::Utf8 => match std::str::from_utf8(bytes) {
                Ok(t) => t.to_string(),
                Err(e) => {
                    // CRuby names the offending byte: `"\xFF" on
                    // UTF-8`.
                    let b = bytes.get(e.valid_up_to()).copied().unwrap_or(0);
                    let _ = path;
                    return Err(self.trap(RubyError::HostException {
                        class_name: "Encoding::InvalidByteSequenceError".to_string(),
                        message: format!("\"\\x{b:02X}\" on UTF-8"),
                    }));
                }
            },
            EncodingTag::UsAscii | EncodingTag::Binary => {
                if let Some(&b) = bytes.iter().find(|&&b| b >= 0x80) {
                    return Err(self.trap(RubyError::HostException {
                        class_name: "Encoding::UndefinedConversionError".to_string(),
                        message: format!(
                            "\"\\x{b:02X}\" to UTF-8 in conversion from ASCII-8BIT to UTF-8"
                        ),
                    }));
                }
                String::from_utf8_lossy(bytes).into_owned()
            }
            #[cfg(feature = "_encoding_full")]
            EncodingTag::Other(idx) => {
                match crate::encoding_full::decode_to_utf8(idx, bytes) {
                    Some(t) => t,
                    None => {
                        let name = crate::encoding_full::name(idx).unwrap_or("OTHER");
                        let b = bytes.iter().copied().find(|&b| b >= 0x80).unwrap_or(0);
                        return Err(self.trap(RubyError::HostException {
                            class_name: "Encoding::InvalidByteSequenceError".to_string(),
                            message: format!("\"\\x{b:02X}\" on {name}"),
                        }));
                    }
                }
            }
            #[cfg(not(feature = "_encoding_full"))]
            EncodingTag::Other(_) => {
                return Err(self.trap(RubyError::ArgumentError {
                    msg: "registry encodings need the _encoding_full feature".to_string(),
                }));
            }
        };
        // Encode the pivot text into the internal coding.
        match int {
            EncodingTag::Utf8 => Ok(text.into_bytes()),
            EncodingTag::UsAscii | EncodingTag::Binary => {
                match text.chars().find(|c| !c.is_ascii()) {
                    None => Ok(text.into_bytes()),
                    Some(c) => Err(self.trap(RubyError::HostException {
                        class_name: "Encoding::UndefinedConversionError".to_string(),
                        message: format!(
                            "U+{:04X} from UTF-8 to {}",
                            c as u32,
                            if int == EncodingTag::UsAscii { "US-ASCII" } else { "ASCII-8BIT" },
                        ),
                    })),
                }
            }
            #[cfg(feature = "_encoding_full")]
            EncodingTag::Other(idx) => {
                match crate::encoding_full::encode_from_utf8(idx, &text, None) {
                    Ok(out) => Ok(out),
                    Err((cp, to)) => Err(self.trap(RubyError::HostException {
                        class_name: "Encoding::UndefinedConversionError".to_string(),
                        message: format!("U+{cp:04X} from UTF-8 to {to}"),
                    })),
                }
            }
            #[cfg(not(feature = "_encoding_full"))]
            EncodingTag::Other(_) => Err(self.trap(RubyError::ArgumentError {
                msg: "registry encodings need the _encoding_full feature".to_string(),
            })),
        }
    }
}

impl Vm {
    /// Coerce a `File`-path argument to a String the way CRuby does:
    /// a `String` passes through; any other object is asked for
    /// `to_path` then `to_str` (a `Pathname` answers `to_path`).
    /// Returns `Ok(None)` when neither conversion exists, so the caller
    /// raises the CRuby `no implicit conversion of <Class> into String`
    /// TypeError; `Ok(None)` likewise when a conversion returned a
    /// non-String (CRuby then raises the same TypeError).
    ///
    /// The `to_path`/`to_str` call re-enters the interpreter
    /// (`invoke_method` + `dispatch_until`, the `invoke_inherited_hook`
    /// pattern), so `maybe_gc` can run: `v` and the caller's
    /// not-yet-processed `also_pin` args are pinned for the duration.
    fn coerce_path_string(&mut self, v: &Value, also_pin: &[Value]) -> Result<Option<String>, Trap> {
        if let Value::Str(s) = v {
            return Ok(Some(s.to_string_lossy()));
        }
        let cls = match v {
            Value::Object(id) => self.heap.class_of(*id),
            _ => return Ok(None),
        };
        for conv in ["to_path", "to_str"] {
            let sym = self.interner.intern(conv);
            let m = match self.lookup_method_uncached(&cls, sym) {
                Some(m) => m,
                None => continue,
            };
            let pre_frames = self.frames.len();
            let result = {
                let mut g = crate::vm::PinGuard::new(self);
                g.pin(v.clone());
                for p in also_pin {
                    g.pin(p.clone());
                }
                g.vm.invoke_method(m, v.clone(), Vec::new())?;
                g.vm.dispatch_until(pre_frames)?;
                g.vm.stack.pop()
            };
            return match result {
                Some(Value::Str(s)) => Ok(Some(s.to_string_lossy())),
                _ => Ok(None),
            };
        }
        Ok(None)
    }

    /// File class-method shims. Implements the half-dozen path-
    /// based File operations idiomatic Ruby scripts reach for
    /// (read / write / exist? / size / basename / dirname /
    /// extname / open-with-block). Returns `Ok(Some(v))` on a
    /// handled call, `Ok(None)` if the method name isn't in
    /// our subset so dispatch can keep walking.
    pub(crate) fn file_class_dispatch(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        let path_arg = |a: &Value| -> Result<String, Trap> {
            match a {
                Value::Str(s) => Ok(s.to_string_lossy()),
                _ => Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", a.type_name()),
                })),
            }
        };
        Ok(Some(match (name, args) {
            // `File.read(path)` plus the optional `length` / `offset`
            // positionals and a trailing options Hash. From the opts
            // Hash (encoding/mode keywords, e.g. jekyll's
            // `File.read(f, **Utils.merged_file_read_opts(...))`)
            // only `encoding: "bom|utf-8"` changes behaviour (BOM
            // strip, below); everything else is accepted and ignored
            // — rubyrs reads raw bytes.
            ("read", [p])
            | ("read", [p, _])
            | ("read", [p, _, _]) => {
                // First-gate: bool capability. Runs BEFORE path_arg so
                // a wrong-type arg under sandbox-on traps with IOError,
                // not TypeError (PR #257 F6 ordering contract).
                self.check_filesystem_io_allowed("File.read", None)?;
                let path = path_arg(p)?;
                // Second-gate: allowlist scope (no-op when `allowed_paths:
                // None`). The redundant bool re-check inside is one
                // branch — negligible vs the syscall below.
                self.check_filesystem_io_allowed(
                    "File.read",
                    Some(Path::new(&path)),
                )?;
                // Positional `length` (bytes to read) / `offset` (byte
                // start). Non-Integer 2nd/3rd args are the options
                // Hash and contribute nothing.
                let length = match args.get(1) {
                    Some(Value::Int(n)) if *n >= 0 => Some(*n as usize),
                    _ => None,
                };
                let offset = match args.get(2) {
                    Some(Value::Int(n)) if *n >= 0 => *n as usize,
                    _ => 0,
                };
                // L3-G follow-up: read raw bytes, not UTF-8-validated
                // String. msgpack/protobuf/binary-protocol fixtures
                // are not valid UTF-8; the previous read_to_string
                // path raised on every binary file. RStr now backs
                // arbitrary bytes via Vec<u8>, so we can store the
                // content verbatim and let downstream code that
                // needs string semantics call to_string_lossy.
                //
                // Post-PR-#63 code-review finding F5: this is a
                // SEMANTIC FLIP from the prior raise-on-binary
                // behavior. Existing scripts that did
                // `File.read(cfg).split("\n")` relied on the raise
                // as a fast-fail when a binary file was passed by
                // mistake; they now get U+FFFD-laden output from
                // to_string_lossy and parse on. The trade-off is
                // intentional (msgpack/protobuf fixtures need
                // binary-safe read), but the safety net is gone —
                // no `File.binread` exists as the explicit binary
                // path yet. Adding one is a follow-up so callers
                // can opt back into the strict-text mode if they
                // want it.
                // `encoding: "bom|utf-8"` (Symbol or String key, any
                // case) strips a leading UTF-8 BOM — the shape
                // Jekyll's `Utils.merged_file_read_opts` produces for
                // every document read. CRuby ground truth: with the
                // bom| prefix a leading EF BB BF disappears from the
                // returned string; without it the BOM is content.
                // Discovered by the front-matter differential (a
                // BOM-prefixed post matched YAML_FRONT_MATTER_REGEXP
                // on CRuby but not on rubyrs, which kept the BOM).
                // Other bom| encodings (utf-16/32) would need real
                // transcoding — rubyrs stays raw-bytes there, same
                // as before. The `File.open(path, "r:bom|utf-8")`
                // mode-string spelling is NOT handled yet.
                // The Symbol-keyed `encoding:` opt (CRuby ignores a
                // String "encoding" key — fixture-verified). E3
                // semantics, probed on CRuby 3.4.1:
                //   - single name  → raw bytes + that TAG (no
                //     transcode; an invalid-in-encoding read keeps
                //     its bytes and reports valid_encoding?=false)
                //   - "ext:int"    → decode as ext, ENCODE to int
                //     (the only transcoding form)
                //   - "bom|utf-8"  → BOM strip (pre-existing)
                //   - absent       → Encoding.default_external tag
                let enc_opt: Option<String> = args.iter().skip(1).find_map(|a| {
                    let Value::Hash(hid) = a else { return None };
                    self.heap.hash(*hid).iter().find_map(|(k, v)| {
                        if matches!(k, Value::Sym(s)
                            if &**self.interner.resolve(*s) == "encoding")
                            && let Value::Str(s) = v
                        {
                            Some(s.to_string_lossy())
                        } else {
                            None
                        }
                    })
                });
                let bom_utf8 = enc_opt
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case("bom|utf-8"));
                // Resolve the read's resulting tag (and the
                // optional transcode pair) BEFORE touching the disk
                // so an unknown name raises without a stray read.
                let mut transcode: Option<(crate::value::EncodingTag, crate::value::EncodingTag)> = None;
                let read_tag: crate::value::EncodingTag = match enc_opt.as_deref() {
                    None => self.default_external,
                    Some(e) if e.eq_ignore_ascii_case("bom|utf-8") => {
                        crate::value::EncodingTag::Utf8
                    }
                    // Unknown names WARN and fall back to the
                    // default tag — CRuby's read path is lenient
                    // ("warning: Unsupported encoding NOPE
                    // ignored"); only `Encoding.default_external=`
                    // raises (probed on 3.4.1).
                    Some(e) => match e.split_once(':') {
                        Some((ext, int)) => {
                            let ext_tag = Self::encoding_tag_from_str(ext);
                            let int_tag = Self::encoding_tag_from_str(int);
                            match (ext_tag, int_tag) {
                                (Some(x), Some(i)) => {
                                    transcode = Some((x, i));
                                    i
                                }
                                _ => {
                                    eprintln!("warning: Unsupported encoding {e} ignored");
                                    self.default_external
                                }
                            }
                        }
                        None => match Self::encoding_tag_from_str(e) {
                            Some(t) => t,
                            None => {
                                eprintln!("warning: Unsupported encoding {e} ignored");
                                self.default_external
                            }
                        },
                    },
                };
                // `Encoding.default_internal` (when set) upgrades a
                // tag-only read into a transcode: the resolved tag
                // above becomes the EXTERNAL side, the internal
                // default the destination (probed: it applies to
                // single-name `encoding:` forms too; an explicit
                // ext:int pair already decided and wins).
                let read_tag = match (transcode, self.default_internal) {
                    (None, Some(int)) if int != read_tag && !bom_utf8 => {
                        transcode = Some((read_tag, int));
                        int
                    }
                    _ => read_tag,
                };
                match std::fs::read(&path) {
                    Ok(b) => {
                        // BOM strip happens at the stream head,
                        // before length/offset slicing — mirroring
                        // CRuby, where the converter consumes the
                        // BOM at open time.
                        let b = match (bom_utf8, b.strip_prefix(b"\xef\xbb\xbf")) {
                            (true, Some(rest)) => rest.to_vec(),
                            _ => b,
                        };
                        let b = if offset == 0 && length.is_none() {
                            b
                        } else {
                            let start = offset.min(b.len());
                            let slice = &b[start..];
                            match length {
                                Some(n) => slice[..n.min(slice.len())].to_vec(),
                                None => slice.to_vec(),
                            }
                        };
                        // `ext:int` — the transcoding form. Decode
                        // with the external coding, re-encode into
                        // the internal one (E2's per-char machinery;
                        // an undecodable byte raises CRuby's
                        // InvalidByteSequenceError class).
                        let b = match transcode {
                            None => b,
                            Some((ext, int)) => self.transcode_read(&b, ext, int, &path)?,
                        };
                        let v = Value::new_str_bytes(b);
                        if let Value::Str(ref ns) = v {
                            ns.encoding.set(read_tag);
                        }
                        v
                    }
                    Err(e) => return Err(self.trap(io_error(&e, Some(Path::new(&path))))),
                }
            }
            // `File.write(path, content)` and the keyword-opts form
            // `File.write(path, content, mode: "a")`. The trailing opts
            // Hash's `mode:` is honoured — append ("a"/"ab"/"a+") vs the
            // default truncate ("w"/"wb") — since silently truncating an
            // append write overwrites prior content. Other opts (perm:,
            // binmode:) are still accepted and ignored. jekyll's
            // page/document writer uses the keyword form.
            // `File.binread(path[, length[, offset]])` — binary-mode
            // read: same raw-bytes read as File.read (rubyrs never
            // transcodes) but the result is TAGGED ASCII-8BIT, the
            // CRuby contract. Closes the "no File.binread" gap noted
            // back in the P0 review.
            ("binread", [p]) | ("binread", [p, _]) | ("binread", [p, _, _]) => {
                self.check_filesystem_io_allowed("File.binread", None)?;
                let path = path_arg(p)?;
                self.check_filesystem_io_allowed(
                    "File.binread",
                    Some(Path::new(&path)),
                )?;
                let length = match args.get(1) {
                    Some(Value::Int(n)) if *n >= 0 => Some(*n as usize),
                    _ => None,
                };
                let offset = match args.get(2) {
                    Some(Value::Int(n)) if *n >= 0 => *n as usize,
                    _ => 0,
                };
                match std::fs::read(&path) {
                    Ok(b) => {
                        let start = offset.min(b.len());
                        let slice = &b[start..];
                        let out = match length {
                            Some(n) => &slice[..n.min(slice.len())],
                            None => slice,
                        };
                        Value::new_str_bytes_binary(out.to_vec())
                    }
                    Err(e) => return Err(self.trap(io_error(&e, Some(Path::new(&path))))),
                }
            }
            // `File.binwrite(path, content)` — rubyrs writes raw
            // bytes unconditionally, so this is File.write minus the
            // append-mode opts handling (binwrite has no mode: opt).
            ("binwrite", [p, body]) => {
                self.check_filesystem_io_allowed("File.binwrite", None)?;
                let path = path_arg(p)?;
                self.check_filesystem_io_allowed(
                    "File.binwrite",
                    Some(Path::new(&path)),
                )?;
                let contents: Vec<u8> = match body {
                    Value::Str(s) => s.content.borrow().clone(),
                    _ => body.to_display(&self.heap, &self.interner).into_bytes(),
                };
                match std::fs::write(&path, &contents) {
                    Ok(()) => Value::Int(contents.len() as i64),
                    Err(e) => return Err(self.trap(io_error(&e, Some(Path::new(&path))))),
                }
            }
            // `File.delete(*paths)` / alias `File.unlink` — removes
            // each named file, returns the count removed (CRuby
            // contract). A missing file raises Errno::ENOENT via the
            // shared io_error mapping, partial work included (files
            // before the failing one stay deleted — same as CRuby's
            // left-to-right processing).
            ("delete" | "unlink", paths) if !paths.is_empty() => {
                self.check_filesystem_io_allowed("File.delete", None)?;
                let mut count: i64 = 0;
                for p in paths {
                    let path = path_arg(p)?;
                    self.check_filesystem_io_allowed(
                        "File.delete",
                        Some(Path::new(&path)),
                    )?;
                    match std::fs::remove_file(&path) {
                        Ok(()) => count += 1,
                        Err(e) => {
                            return Err(self.trap(io_error(&e, Some(Path::new(&path)))));
                        }
                    }
                }
                Value::Int(count)
            }
            ("write", [p, body]) | ("write", [p, body, _]) => {
                self.check_filesystem_io_allowed("File.write", None)?;
                let path = path_arg(p)?;
                self.check_filesystem_io_allowed(
                    "File.write",
                    Some(Path::new(&path)),
                )?;
                let contents: Vec<u8> = match body {
                    Value::Str(s) => s.content.borrow().clone(),
                    _ => body.to_display(&self.heap, &self.interner).into_bytes(),
                };
                let append = if let Some(Value::Hash(hid)) = args.get(2) {
                    let hid = *hid;
                    let mode_key = Value::Sym(self.interner.intern("mode"));
                    match self.heap.hash_index_lookup(hid, &mode_key) {
                        Some(pos) => matches!(
                            &self.heap.hash(hid)[pos].1,
                            Value::Str(s) if s.to_string_lossy().starts_with('a')
                        ),
                        None => false,
                    }
                } else {
                    false
                };
                let result = if append {
                    use std::io::Write as _;
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .and_then(|mut f| f.write_all(&contents))
                } else {
                    std::fs::write(&path, &contents)
                };
                match result {
                    Ok(()) => Value::Int(contents.len() as i64),
                    Err(e) => return Err(self.trap(io_error(&e, Some(Path::new(&path))))),
                }
            }
            // `File.fnmatch?(pattern, path)` / `fnmatch(pattern, path,
            // flags)` — glob-style match. Pure string work (no disk
            // access, no capability gate). `flags` is the FNM_* bitmask
            // (default 0); both method names are the same operation.
            ("fnmatch", [pat, path])
            | ("fnmatch?", [pat, path])
            | ("fnmatch", [pat, path, _])
            | ("fnmatch?", [pat, path, _]) => {
                let flags = match args.get(2) {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
                // Str/Str fast extraction: borrow both contents
                // directly (no `path_arg` String copies — this arm
                // runs N exclusion globs per Jekyll document).
                // Non-Str args (Pathname `to_path` coercion) and
                // non-UTF-8 take the general path below.
                if let (Value::Str(a), Value::Str(b)) = (pat, path) {
                    let ab = a.content.borrow();
                    let bb = b.content.borrow();
                    if let (Ok(ap), Ok(bp)) =
                        (std::str::from_utf8(&ab), std::str::from_utf8(&bb))
                    {
                        return Ok(Some(Value::Bool(fnmatch(ap, bp, flags))));
                    }
                }
                let pattern = path_arg(pat)?;
                let target = path_arg(path)?;
                Value::Bool(fnmatch(&pattern, &target, flags))
            }
            ("exist?", [p]) | ("exists?", [p]) | ("file?", [p]) => {
                // Resolve to a `&'static str` upfront — passing
                // `&format!("File.{name}")` would allocate a
                // fresh String on every call even on the
                // sandbox-off happy path where the check
                // immediately returns Ok. Three branches; the
                // arm-guard above guarantees the unreachable
                // arm cannot fire.
                let op = match name {
                    "exist?" => "File.exist?",
                    "exists?" => "File.exists?",
                    "file?" => "File.file?",
                    _ => unreachable!(),
                };
                self.check_filesystem_io_allowed(op, None)?;
                let path = path_arg(p)?;
                self.check_filesystem_io_allowed(op, Some(Path::new(&path)))?;
                let exists = std::fs::metadata(&path)
                    .map(|m| if name == "file?" { m.is_file() } else { true })
                    .unwrap_or(false);
                Value::Bool(exists)
            }
            ("directory?", [p]) => {
                self.check_filesystem_io_allowed("File.directory?", None)?;
                let path = path_arg(p)?;
                self.check_filesystem_io_allowed(
                    "File.directory?",
                    Some(Path::new(&path)),
                )?;
                let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
                Value::Bool(is_dir)
            }
            ("size", [p]) => {
                self.check_filesystem_io_allowed("File.size", None)?;
                let path = path_arg(p)?;
                self.check_filesystem_io_allowed(
                    "File.size",
                    Some(Path::new(&path)),
                )?;
                match std::fs::metadata(&path) {
                    Ok(m) => Value::Int(m.len() as i64),
                    Err(e) => return Err(self.trap(io_error(&e, Some(Path::new(&path))))),
                }
            }
            ("basename", [p]) => {
                let path = path_arg(p)?;
                Value::new_str(ruby_basename(&path).to_string())
            }
            // Two-arg form: strip `suffix` off the basename. `".*"`
            // strips the last extension — the final `.` and what
            // follows — unless the dot is at index 0 OR everything
            // before it is dots (dotfiles & dot-dirs:
            // `basename(".hidden", ".*")` → ".hidden",
            // `basename("..", ".*")` → ".."); any other suffix
            // strips on exact tail match only when it isn't the
            // whole name (`basename("c.md", "c.md")` → "c.md").
            // Ground-truth probed vs ruby 3.4
            // (file_basename_suffix fixture). Discovery: Jekyll's
            // `Document#basename_without_ext` uses
            // `File.basename(path, ".*")` — was NoMethodError.
            ("basename", [p, suffix]) => {
                let path = path_arg(p)?;
                let sfx = match suffix {
                    Value::Str(s) => s.to_string_lossy(),
                    other => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!(
                                "no implicit conversion of {} into String",
                                other.type_name()
                            ),
                        }))
                    }
                };
                let name = ruby_basename(&path);
                let stripped = if sfx == ".*" {
                    match name.rfind('.') {
                        Some(i)
                            if i > 0 && name[..i].bytes().any(|b| b != b'.') =>
                        {
                            &name[..i]
                        }
                        _ => name,
                    }
                } else if name.len() > sfx.len() && name.ends_with(sfx.as_str()) {
                    &name[..name.len() - sfx.len()]
                } else {
                    name
                };
                Value::new_str(stripped.to_string())
            }
            ("dirname", [p]) => {
                let path = path_arg(p)?;
                Value::new_str(ruby_dirname(&path))
            }
            ("extname", [p]) => {
                let path = path_arg(p)?;
                let p = Path::new(&path);
                let ext = p.extension()
                    .map(|s| format!(".{}", s.to_string_lossy()))
                    .unwrap_or_default();
                Value::new_str(ext)
            }
            ("join", parts) => {
                // CRuby `File.join(*parts)` — concatenate path
                // components with "/", collapsing a doubled separator
                // only at each join boundary (`File.join("a/", "/b")`
                // → "a/b", but internal "//" is preserved). Nested
                // Array args are flattened left-to-right; non-String/
                // Array leaves raise TypeError. Pure string op — no
                // filesystem access, so no capability gate. Discovery:
                // P3 Jekyll spike — Liquid's i18n.rb builds its
                // DEFAULT_LOCALE via `File.join(...)`.
                let mut comps: Vec<String> = Vec::new();
                let mut work: Vec<Value> = parts.iter().rev().cloned().collect();
                while let Some(v) = work.pop() {
                    match &v {
                        Value::Str(s) => comps.push(s.to_string_lossy()),
                        Value::Array(id) => {
                            let elems: Vec<Value> = self.heap.array(*id).clone();
                            for e in elems.into_iter().rev() {
                                work.push(e);
                            }
                        }
                        // Pathname (or any object answering to_path/to_str):
                        // CRuby coerces File.join args via to_path then
                        // to_str. `coerce_path_string` pins `v` + the
                        // unprocessed `work` across the re-entrant call.
                        // rouge's `load_lexer` does
                        // `File.join(BASE_DIR, pathname)`.
                        _ => match self.coerce_path_string(&v, &work)? {
                            Some(s) => comps.push(s),
                            None => {
                                // CRuby names the actual class (e.g.
                                // "NoConv"), not the generic "Object".
                                let cls_name = match self.class_of(&v) {
                                    Value::Class(c) => {
                                        c.effective_name().unwrap_or_else(|| c.name.clone())
                                    }
                                    _ => v.type_name().to_string(),
                                };
                                return Err(self.trap(RubyError::TypeError {
                                    msg: format!(
                                        "no implicit conversion of {} into String",
                                        cls_name
                                    ),
                                }));
                            }
                        },
                    }
                }
                let mut result = String::new();
                for (i, c) in comps.iter().enumerate() {
                    if i == 0 {
                        result.push_str(c);
                        continue;
                    }
                    let left = result.ends_with('/');
                    let right = c.starts_with('/');
                    if left && right {
                        result.pop();
                        result.push_str(c);
                    } else if left || right {
                        result.push_str(c);
                    } else {
                        result.push('/');
                        result.push_str(c);
                    }
                }
                Value::new_str(result)
            }
            ("expand_path", [p]) | ("expand_path", [p, _]) => {
                // `File.expand_path(path, base=cwd)`. CRuby
                // doesn't require the path to exist — it just
                // resolves relative paths and `..`/`.`
                // components.
                //
                // Mode selection: cwd-read + canonicalize are
                // "full host FS" operations. They only fire when
                // `allow_filesystem_io: true` AND `allowed_paths:
                // None` (the fully-open shape). In any other
                // shape — sandbox off OR scoped-by-allowlist —
                // fall back to the lexical-only form (root-anchored,
                // no symlink resolve), matching what CRuby returns
                // for paths that don't exist on disk. This closes
                // an info-leak under `allowed_paths: Some(_)`:
                // `File.expand_path('.')` previously returned the
                // host's actual cwd to script code (outside the
                // allowlist scope), and the canonicalize call
                // followed symlinks anywhere on the host FS.
                use std::path::{Path, PathBuf};
                let path = path_arg(p)?;
                // Wide-open shape: sandbox on AND no allowlist.
                let wide_open = self.allow_filesystem_io && self.allowed_paths.is_none();
                let base: String = match args.get(1) {
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    // When the host explicitly didn't supply a
                    // base, fall back to cwd only in the wide-open
                    // shape. Otherwise use `/` as the sentinel so
                    // the lexical expansion still produces an
                    // ABSOLUTE path (CRuby's contract for
                    // File.expand_path — gems and
                    // `$LOAD_PATH.unshift` consumers rely on the
                    // absolute-shape).
                    _ if wide_open => std::env::current_dir()
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| ".".to_string()),
                    _ => "/".to_string(),
                };
                let p_path = Path::new(&path);
                let joined: PathBuf = if p_path.is_absolute() {
                    p_path.to_path_buf()
                } else {
                    let mut b = PathBuf::from(&base);
                    // `~` is the home-directory shortcut CRuby
                    // expands. Not modelled — leaves it as-is.
                    b.push(p_path);
                    b
                };
                // Shared lexical collapse (single source of
                // truth; gc.rs's `check_path_in_allowlist` uses
                // the same helper so scope-check and visible
                // string stay in lockstep).
                let resolved = crate::lexically_resolve_path(&joined);
                let final_path = if wide_open {
                    std::fs::canonicalize(&resolved).unwrap_or(resolved)
                } else {
                    resolved
                };
                Value::new_str(final_path.to_string_lossy().into_owned())
            }
            _ => return Ok(None),
        }))
    }

    /// `Dir` class-method shims — `glob` / `[]` / `entries` /
    /// `children` / `exist?` / `pwd`. Returns `Ok(Some(v))` on a
    /// handled method, `Ok(None)` to let dispatch keep walking.
    /// Discovery: P3 Jekyll spike — Liquid loads its tags via
    /// `Dir["…/tags/*.rb"]` and Jekyll globs site sources.
    pub(crate) fn dir_class_dispatch(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        let str_arg = |a: &Value| -> Result<String, Trap> {
            match a {
                Value::Str(s) => Ok(s.to_string_lossy()),
                _ => Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", a.type_name()),
                })),
            }
        };
        Ok(Some(match (name, args) {
            // `Dir.glob(pat)` / `Dir[pat]` (+ ignored flags arg).
            // A single String pattern, or an Array of patterns whose
            // results union. No block form (Ruby's block-yield
            // variant) — returns the Array.
            ("glob", [pat]) | ("glob", [pat, _]) | ("[]", [pat]) | ("[]", [pat, _]) => {
                self.check_filesystem_io_allowed("Dir.glob", None)?;
                let patterns: Vec<String> = match pat {
                    Value::Str(s) => vec![s.to_string_lossy()],
                    Value::Array(id) => {
                        let elems: Vec<Value> = self.heap.array(*id).clone();
                        let mut ps = Vec::with_capacity(elems.len());
                        for e in &elems {
                            ps.push(str_arg(e)?);
                        }
                        ps
                    }
                    _ => {
                        return Err(self.trap(RubyError::TypeError {
                            msg: format!("no implicit conversion of {} into String", pat.type_name()),
                        }));
                    }
                };
                let mut paths: Vec<String> = Vec::new();
                for p in &patterns {
                    for m in glob_expand(p) {
                        if !paths.contains(&m) {
                            paths.push(m);
                        }
                    }
                }
                // A multi-pattern glob unions in pattern order; a
                // single pattern is already sorted by glob_expand.
                if patterns.len() > 1 {
                    paths.sort();
                }
                let elems: Vec<Value> = paths.into_iter().map(Value::new_str).collect();
                self.maybe_gc();
                self.check_alloc()?;
                Value::Array(self.heap.alloc(HeapObj::Array(elems.into())))
            }
            // `Dir.entries(path)` — names in the directory, INCLUDING
            // "." and ".." (CRuby). `Dir.children(path)` — same but
            // without "." / "..".
            ("entries", [p]) | ("children", [p]) => {
                self.check_filesystem_io_allowed("Dir.entries", None)?;
                let path = str_arg(p)?;
                self.check_filesystem_io_allowed("Dir.entries", Some(Path::new(&path)))?;
                let mut names: Vec<String> = match std::fs::read_dir(&path) {
                    Ok(rd) => rd
                        .filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect(),
                    Err(e) => {
                        return Err(self.trap(io_error(&e, Some(Path::new(&path)))));
                    }
                };
                names.sort();
                if name == "entries" {
                    names.insert(0, "..".to_string());
                    names.insert(0, ".".to_string());
                }
                let elems: Vec<Value> = names.into_iter().map(Value::new_str).collect();
                self.maybe_gc();
                self.check_alloc()?;
                Value::Array(self.heap.alloc(HeapObj::Array(elems.into())))
            }
            ("exist?", [p]) | ("exists?", [p]) | ("directory?", [p]) => {
                self.check_filesystem_io_allowed("Dir.exist?", None)?;
                let path = str_arg(p)?;
                self.check_filesystem_io_allowed("Dir.exist?", Some(Path::new(&path)))?;
                Value::Bool(std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false))
            }
            ("pwd", []) | ("getwd", []) => {
                self.check_filesystem_io_allowed("Dir.pwd", None)?;
                Value::new_str(
                    std::env::current_dir()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            }
            // `Dir.__chdir(path)` — the OS-level `set_current_dir`
            // backing the `Dir.chdir` veneer (preamble). The block /
            // non-block bracketing (save cwd, restore via
            // begin/ensure) lives in Ruby so it reuses `yield`; this
            // primitive only performs the move. Capability-gated.
            // Discovery: P3 Jekyll spike — `layout_reader.rb#within`
            // does `Dir.chdir(dir) { Dir["**/*.*"] }`.
            ("__chdir", [p]) => {
                self.check_filesystem_io_allowed("Dir.chdir", None)?;
                let path = str_arg(p)?;
                self.check_filesystem_io_allowed("Dir.chdir", Some(Path::new(&path)))?;
                match std::env::set_current_dir(&path) {
                    Ok(()) => Value::new_str(path),
                    Err(e) => {
                        return Err(self.trap(io_error(&e, Some(Path::new(&path)))));
                    }
                }
            }
            _ => return Ok(None),
        }))
    }

    /// `FileUtils` module-method shims — the directory/file mutation
    /// surface site generators reach for: `mkdir_p` / `mkdir` /
    /// `rm_rf` / `rm_f` / `rm` / `cp` / `cp_r` / `mv` / `touch`. Each
    /// path goes through the filesystem capability gate. Returns
    /// `Ok(Some(v))` on a handled method, `Ok(None)` otherwise.
    /// Discovery: P3 Jekyll spike — jekyll writes the cache dir +
    /// `_site` output via `FileUtils.mkdir_p` etc.
    pub(crate) fn fileutils_class_dispatch(&mut self, name: &str, args: &[Value]) -> Result<Option<Value>, Trap> {
        // Each path arg may be a String or an Array of Strings (CRuby
        // FileUtils accepts both for most ops). Flatten to a Vec.
        let paths = |vm: &Vm, a: &Value| -> Result<Vec<String>, Trap> {
            match a {
                Value::Str(s) => Ok(vec![s.to_string_lossy()]),
                Value::Array(id) => {
                    let mut out = Vec::new();
                    for e in vm.heap.array(*id).clone() {
                        if let Value::Str(s) = e {
                            out.push(s.to_string_lossy());
                        } else {
                            return Err(vm.trap(RubyError::TypeError {
                                msg: format!("no implicit conversion of {} into String", e.type_name()),
                            }));
                        }
                    }
                    Ok(out)
                }
                other => Err(vm.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", other.type_name()),
                })),
            }
        };
        // Trailing options Hash (e.g. `mkdir_p(path, mode: 0755)`) is
        // accepted and ignored — strip it from the positional args.
        let positional: &[Value] = match args.last() {
            Some(Value::Hash(_)) => &args[..args.len() - 1],
            _ => args,
        };
        Ok(Some(match (name, positional) {
            ("mkdir_p" | "makedirs" | "mkpath", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.mkdir_p", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.mkdir_p", Some(Path::new(p)))?;
                    std::fs::create_dir_all(p).map_err(|e| self.trap(io_error(&e, Some(Path::new(p)))))?;
                }
                a.clone()
            }
            ("mkdir", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.mkdir", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.mkdir", Some(Path::new(p)))?;
                    std::fs::create_dir(p).map_err(|e| self.trap(io_error(&e, Some(Path::new(p)))))?;
                }
                a.clone()
            }
            ("rm_rf" | "remove_entry_secure", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.rm_rf", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.rm_rf", Some(Path::new(p)))?;
                    // rm_rf ignores missing paths (CRuby :force).
                    if Path::new(p).is_dir() {
                        let _ = std::fs::remove_dir_all(p);
                    } else {
                        let _ = std::fs::remove_file(p);
                    }
                }
                a.clone()
            }
            ("rm" | "rm_f" | "remove" | "safe_unlink", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.rm", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.rm", Some(Path::new(p)))?;
                    let _ = std::fs::remove_file(p);
                }
                a.clone()
            }
            ("cp" | "copy", [src, dst]) => {
                self.check_filesystem_io_allowed("FileUtils.cp", None)?;
                let srcs = paths(self, src)?;
                let d = paths(self, dst)?.into_iter().next().unwrap_or_default();
                self.check_filesystem_io_allowed("FileUtils.cp", Some(Path::new(&d)))?;
                // CRuby: a *list* of sources (or an existing-directory
                // dest) always joins dest/basename(src) — copying only
                // the first source, as the old code did, silently dropped
                // the rest. A single source to a non-dir dest copies to
                // dest verbatim.
                let into_dir = matches!(src, Value::Array(_)) || Path::new(&d).is_dir();
                for s in &srcs {
                    self.check_filesystem_io_allowed("FileUtils.cp", Some(Path::new(s)))?;
                    let dest = if into_dir {
                        Path::new(&d)
                            .join(Path::new(s).file_name().unwrap_or_default())
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        d.clone()
                    };
                    std::fs::copy(s, &dest)
                        .map_err(|e| self.trap(io_error(&e, Some(Path::new(s)))))?;
                }
                Value::Nil
            }
            ("cp_r", [src, dst]) => {
                self.check_filesystem_io_allowed("FileUtils.cp_r", None)?;
                let srcs = paths(self, src)?;
                let d = paths(self, dst)?.into_iter().next().unwrap_or_default();
                self.check_filesystem_io_allowed("FileUtils.cp_r", Some(Path::new(&d)))?;
                let into_dir = matches!(src, Value::Array(_)) || Path::new(&d).is_dir();
                for s in &srcs {
                    self.check_filesystem_io_allowed("FileUtils.cp_r", Some(Path::new(s)))?;
                    let target = if into_dir {
                        Path::new(&d).join(Path::new(s).file_name().unwrap_or_default())
                    } else {
                        Path::new(&d).to_path_buf()
                    };
                    copy_tree(Path::new(s), &target)
                        .map_err(|e| self.trap(io_error(&e, Some(Path::new(s)))))?;
                }
                Value::Nil
            }
            ("mv" | "move", [src, dst]) => {
                self.check_filesystem_io_allowed("FileUtils.mv", None)?;
                let srcs = paths(self, src)?;
                let d = paths(self, dst)?.into_iter().next().unwrap_or_default();
                self.check_filesystem_io_allowed("FileUtils.mv", Some(Path::new(&d)))?;
                let into_dir = matches!(src, Value::Array(_)) || Path::new(&d).is_dir();
                for s in &srcs {
                    self.check_filesystem_io_allowed("FileUtils.mv", Some(Path::new(s)))?;
                    let sp = Path::new(s);
                    let target = if into_dir {
                        Path::new(&d).join(sp.file_name().unwrap_or_default())
                    } else {
                        Path::new(&d).to_path_buf()
                    };
                    // Same-filesystem rename is atomic; fall back to a
                    // recursive copy + remove across devices.
                    if std::fs::rename(sp, &target).is_err() {
                        copy_tree(sp, &target)
                            .map_err(|e| self.trap(io_error(&e, Some(sp))))?;
                        let rm = if sp.is_dir() {
                            std::fs::remove_dir_all(sp)
                        } else {
                            std::fs::remove_file(sp)
                        };
                        rm.map_err(|e| self.trap(io_error(&e, Some(sp))))?;
                    }
                }
                Value::Nil
            }
            ("touch", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.touch", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.touch", Some(Path::new(p)))?;
                    // Create if absent; leave content untouched otherwise.
                    if !Path::new(p).exists() {
                        std::fs::write(p, b"").map_err(|e| self.trap(io_error(&e, Some(Path::new(p)))))?;
                    }
                }
                a.clone()
            }
            _ => return Ok(None),
        }))
    }
}

/// Recursively copy `src` to `dst` (files and directory trees),
/// backing `FileUtils.cp_r` and the cross-device fallback of
/// `FileUtils.mv`. Directories are created as the walk descends; a
/// plain file is copied directly.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// Map a `std::io::Error` to the matching `Errno::*` exception
/// (falling back to `SystemCallError`) and wrap it as a
/// `HostException` so Ruby `rescue Errno::ENOENT` / `rescue
/// SystemCallError` catches filesystem failures. Previously every
/// File/Dir/FileUtils failure was raised as a plain `RuntimeError`,
/// which the pervasive `rescue Errno::ENOENT` idiom silently missed.
pub(crate) fn io_error(e: &std::io::Error, path: Option<&Path>) -> RubyError {
    let (class, desc) = io_errno(e);
    let message = match path {
        Some(p) => format!("{} - {}", desc, p.display()),
        None => desc.to_string(),
    };
    RubyError::HostException {
        class_name: class.to_string(),
        message,
    }
}

/// `(Errno::* class name, strerror-like description)` for an io error.
/// The low POSIX errno numbers (2/13/17/20/21/22/28) are identical on
/// Linux and macOS, so the `raw_os_error` mapping is portable; the
/// `ErrorKind` fallback covers the platform-independent cases.
fn io_errno(e: &std::io::Error) -> (&'static str, &'static str) {
    use std::io::ErrorKind;
    if let Some(code) = e.raw_os_error() {
        match code {
            2 => return ("Errno::ENOENT", "No such file or directory"),
            13 => return ("Errno::EACCES", "Permission denied"),
            17 => return ("Errno::EEXIST", "File exists"),
            20 => return ("Errno::ENOTDIR", "Not a directory"),
            21 => return ("Errno::EISDIR", "Is a directory"),
            22 => return ("Errno::EINVAL", "Invalid argument"),
            28 => return ("Errno::ENOSPC", "No space left on device"),
            _ => {}
        }
    }
    match e.kind() {
        ErrorKind::NotFound => ("Errno::ENOENT", "No such file or directory"),
        ErrorKind::PermissionDenied => ("Errno::EACCES", "Permission denied"),
        ErrorKind::AlreadyExists => ("Errno::EEXIST", "File exists"),
        _ => ("SystemCallError", "Unknown error"),
    }
}

// --- File.fnmatch glob-pattern matcher ---
//
// Pure string matching (no filesystem access) mirroring CRuby's
// `File.fnmatch` / `fnmatch?`. Supports `*`, `?`, `[set]` (with
// ranges, `!`/`^` negation, `\`-escapes) and the FNM_* flag bitmask
// below. The leading-period rule (a `.` at the start of a path
// segment must be matched explicitly, not by a wildcard) is honoured
// unless FNM_DOTMATCH. Discovery: P3 Jekyll spike —
// EntryFilter#glob_include? filters site entries with
// `File.fnmatch?`.

const FNM_NOESCAPE: i64 = 0x01;
const FNM_PATHNAME: i64 = 0x02;
const FNM_DOTMATCH: i64 = 0x04;
const FNM_CASEFOLD: i64 = 0x08;

#[inline]
fn fnm_char_eq(a: char, b: char, flags: i64) -> bool {
    if flags & FNM_CASEFOLD != 0 {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

#[inline]
fn fnm_in_range(ch: char, lo: char, hi: char, flags: i64) -> bool {
    if (lo..=hi).contains(&ch) {
        return true;
    }
    if flags & FNM_CASEFOLD != 0 {
        let c = ch.to_ascii_lowercase();
        let cu = ch.to_ascii_uppercase();
        (lo..=hi).contains(&c) || (lo..=hi).contains(&cu)
    } else {
        false
    }
}

/// True if a wildcard at `s[i]` is forbidden from matching because
/// `s[i]` is a leading period of a path segment (and FNM_DOTMATCH
/// is off).
fn fnm_period_blocked(s: &[char], i: usize, flags: i64) -> bool {
    if flags & FNM_DOTMATCH != 0 || i >= s.len() || s[i] != '.' {
        return false;
    }
    if i == 0 {
        return true;
    }
    flags & FNM_PATHNAME != 0 && s[i - 1] == '/'
}

/// Parse a `[set]` starting at `p[start]` ('[') against `ch`.
/// Returns `Some((matched, index_after_class))`, or `None` if the
/// bracket is malformed (no closing `]`) — the caller then treats
/// `[` as a literal.
fn fnm_bracket(p: &[char], start: usize, ch: char, flags: i64) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let mut negate = false;
    if i < p.len() && (p[i] == '!' || p[i] == '^') {
        negate = true;
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        // Escaped member.
        let lo = if p[i] == '\\' && flags & FNM_NOESCAPE == 0 && i + 1 < p.len() {
            i += 1;
            p[i]
        } else {
            p[i]
        };
        // Range `lo-hi` (a trailing `-` before `]` is a literal `-`).
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            i += 2;
            let hi = if p[i] == '\\' && flags & FNM_NOESCAPE == 0 && i + 1 < p.len() {
                i += 1;
                p[i]
            } else {
                p[i]
            };
            if fnm_in_range(ch, lo, hi, flags) {
                matched = true;
            }
            i += 1;
        } else {
            if fnm_char_eq(ch, lo, flags) {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

fn fnm_match(p: &[char], mut pi: usize, s: &[char], mut si: usize, flags: i64) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                let star_start = pi;
                while pi < p.len() && p[pi] == '*' {
                    pi += 1;
                }
                // `**/` under FNM_PATHNAME is the recursive token: it
                // matches zero or more complete directory components, so
                // the rest of the pattern can match at any depth. A `**`
                // that isn't a bounded path segment (`a**`, `**.rb`, a
                // trailing `**`) falls through to ordinary `*` semantics.
                if pi - star_start >= 2
                    && flags & FNM_PATHNAME != 0
                    && pi < p.len()
                    && p[pi] == '/'
                {
                    let rest = pi + 1; // pattern after the `**/`
                    // Zero directories consumed.
                    if fnm_match(p, rest, s, si, flags) {
                        return true;
                    }
                    // Consume one `<segment>/` at a time and retry.
                    let mut k = si;
                    loop {
                        while k < s.len() && s[k] != '/' {
                            k += 1;
                        }
                        if k >= s.len() {
                            return false;
                        }
                        k += 1; // past the `/`
                        if fnm_match(p, rest, s, k, flags) {
                            return true;
                        }
                    }
                }
                if fnm_period_blocked(s, si, flags) {
                    return false;
                }
                if pi == p.len() {
                    // Trailing `*` matches the rest; under FNM_PATHNAME
                    // it cannot cross a `/`.
                    return flags & FNM_PATHNAME == 0 || !s[si..].contains(&'/');
                }
                let mut k = si;
                loop {
                    if fnm_match(p, pi, s, k, flags) {
                        return true;
                    }
                    if k >= s.len() {
                        return false;
                    }
                    if flags & FNM_PATHNAME != 0 && s[k] == '/' {
                        return false;
                    }
                    k += 1;
                }
            }
            '?' => {
                if si >= s.len()
                    || (flags & FNM_PATHNAME != 0 && s[si] == '/')
                    || fnm_period_blocked(s, si, flags)
                {
                    return false;
                }
                pi += 1;
                si += 1;
            }
            '[' => {
                if si >= s.len()
                    || (flags & FNM_PATHNAME != 0 && s[si] == '/')
                    || fnm_period_blocked(s, si, flags)
                {
                    return false;
                }
                match fnm_bracket(p, pi, s[si], flags) {
                    Some((true, next)) => {
                        pi = next;
                        si += 1;
                    }
                    Some((false, _)) => return false,
                    // Unterminated `[` — CRuby fails the whole match
                    // (it does NOT fall back to a literal `[`):
                    // `File.fnmatch?("a[b", "a[b") == false`.
                    None => return false,
                }
            }
            '\\' if flags & FNM_NOESCAPE == 0 => {
                pi += 1;
                let lit = if pi < p.len() { p[pi] } else { '\\' };
                if si >= s.len() || !fnm_char_eq(s[si], lit, flags) {
                    return false;
                }
                pi += 1;
                si += 1;
            }
            c => {
                if si >= s.len() || !fnm_char_eq(s[si], c, flags) {
                    return false;
                }
                pi += 1;
                si += 1;
            }
        }
    }
    si == s.len()
}

thread_local! {
    // Reused char buffers for `fnmatch` — the recursive matcher
    // needs random access (`&[char]`), but collecting two fresh
    // `Vec<char>`s per call was ~half the cost of the hot
    // Jekyll path (`EntryFilter#glob_include?` runs N exclusion
    // globs per document). clear+extend keeps the capacity.
    static FNM_SCRATCH: std::cell::RefCell<(Vec<char>, Vec<char>)> =
        const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

/// CRuby `File.fnmatch(pattern, path, flags)` — glob-style match.
pub(crate) fn fnmatch(pattern: &str, path: &str, flags: i64) -> bool {
    FNM_SCRATCH.with(|cell| {
        let mut scratch = cell.borrow_mut();
        let (p, s) = &mut *scratch;
        p.clear();
        p.extend(pattern.chars());
        s.clear();
        s.extend(path.chars());
        fnm_match(p, 0, s, 0, flags)
    })
}
