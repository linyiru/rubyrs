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

impl Vm {
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
            ("read", [p]) => {
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
                match std::fs::read(&path) {
                    Ok(b) => Value::new_str_bytes(b),
                    Err(e) => return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("File.read({}): {}", path, e),
                    })),
                }
            }
            ("write", [p, body]) => {
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
                match std::fs::write(&path, &contents) {
                    Ok(()) => Value::Int(contents.len() as i64),
                    Err(e) => return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("File.write({}): {}", path, e),
                    })),
                }
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
                    Err(e) => return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("File.size({}): {}", path, e),
                    })),
                }
            }
            ("basename", [p]) => {
                let path = path_arg(p)?;
                let name = Path::new(&path).file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Value::new_str(name)
            }
            ("dirname", [p]) => {
                let path = path_arg(p)?;
                let dir = Path::new(&path).parent()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ".".to_string());
                Value::new_str(dir)
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
                    match v {
                        Value::Str(s) => comps.push(s.to_string_lossy()),
                        Value::Array(id) => {
                            let elems: Vec<Value> = self.heap.array(id).clone();
                            for e in elems.into_iter().rev() {
                                work.push(e);
                            }
                        }
                        other => {
                            return Err(self.trap(RubyError::TypeError {
                                msg: format!(
                                    "no implicit conversion of {} into String",
                                    other.type_name()
                                ),
                            }));
                        }
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
                Value::Array(self.heap.alloc(HeapObj::Array(elems)))
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
                        return Err(self.trap(RubyError::RuntimeError {
                            msg: format!("Dir.{}({}): {}", name, path, e),
                        }));
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
                Value::Array(self.heap.alloc(HeapObj::Array(elems)))
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
            _ => return Ok(None),
        }))
    }
}
