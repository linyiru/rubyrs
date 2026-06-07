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
            // `File.read(path)` plus the optional `length` / `offset`
            // positionals and a trailing options Hash. The opts Hash
            // (encoding/mode keywords, e.g. jekyll's
            // `File.read(f, **Utils.merged_file_read_opts(...))`) is
            // accepted and ignored — rubyrs always reads raw bytes.
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
                match std::fs::read(&path) {
                    Ok(b) => {
                        if offset == 0 && length.is_none() {
                            Value::new_str_bytes(b)
                        } else {
                            let start = offset.min(b.len());
                            let slice = &b[start..];
                            let out = match length {
                                Some(n) => &slice[..n.min(slice.len())],
                                None => slice,
                            };
                            Value::new_str_bytes(out.to_vec())
                        }
                    }
                    Err(e) => return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("File.read({}): {}", path, e),
                    })),
                }
            }
            // `File.write(path, content)` and the keyword-opts form
            // `File.write(path, content, mode: "wb")` (trailing opts
            // Hash is accepted and ignored — we always write the bytes
            // verbatim). jekyll's page/document writer uses the latter.
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
                match std::fs::write(&path, &contents) {
                    Ok(()) => Value::Int(contents.len() as i64),
                    Err(e) => return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("File.write({}): {}", path, e),
                    })),
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
                let pattern = path_arg(pat)?;
                let target = path_arg(path)?;
                let flags = match args.get(2) {
                    Some(Value::Int(n)) => *n,
                    _ => 0,
                };
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
                        return Err(self.trap(RubyError::RuntimeError {
                            msg: format!("Dir.chdir({path}): {e}"),
                        }));
                    }
                }
            }
            _ => return Ok(None),
        }))
    }

    /// `FileUtils` module-method shims — the directory/file mutation
    /// surface site generators reach for: `mkdir_p` / `mkdir` /
    /// `rm_rf` / `rm_f` / `rm` / `cp` / `cp_r` / `touch`. Each path
    /// goes through the filesystem capability gate. Returns
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
                    std::fs::create_dir_all(p).map_err(|e| self.trap(RubyError::RuntimeError {
                        msg: format!("FileUtils.mkdir_p({}): {}", p, e),
                    }))?;
                }
                a.clone()
            }
            ("mkdir", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.mkdir", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.mkdir", Some(Path::new(p)))?;
                    std::fs::create_dir(p).map_err(|e| self.trap(RubyError::RuntimeError {
                        msg: format!("FileUtils.mkdir({}): {}", p, e),
                    }))?;
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
                let s = paths(self, src)?.into_iter().next().unwrap_or_default();
                let d = paths(self, dst)?.into_iter().next().unwrap_or_default();
                self.check_filesystem_io_allowed("FileUtils.cp", Some(Path::new(&s)))?;
                self.check_filesystem_io_allowed("FileUtils.cp", Some(Path::new(&d)))?;
                // If dst is an existing directory, copy INTO it (CRuby).
                let dest = if Path::new(&d).is_dir() {
                    Path::new(&d).join(Path::new(&s).file_name().unwrap_or_default())
                        .to_string_lossy().into_owned()
                } else { d };
                std::fs::copy(&s, &dest).map_err(|e| self.trap(RubyError::RuntimeError {
                    msg: format!("FileUtils.cp({}, {}): {}", s, dest, e),
                }))?;
                Value::Nil
            }
            ("touch", [a]) => {
                self.check_filesystem_io_allowed("FileUtils.touch", None)?;
                let ps = paths(self, a)?;
                for p in &ps {
                    self.check_filesystem_io_allowed("FileUtils.touch", Some(Path::new(p)))?;
                    // Create if absent; leave content untouched otherwise.
                    if !Path::new(p).exists() {
                        std::fs::write(p, b"").map_err(|e| self.trap(RubyError::RuntimeError {
                            msg: format!("FileUtils.touch({}): {}", p, e),
                        }))?;
                    }
                }
                a.clone()
            }
            _ => return Ok(None),
        }))
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
                while pi < p.len() && p[pi] == '*' {
                    pi += 1;
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

/// CRuby `File.fnmatch(pattern, path, flags)` — glob-style match.
pub(crate) fn fnmatch(pattern: &str, path: &str, flags: i64) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = path.chars().collect();
    fnm_match(&p, 0, &s, 0, flags)
}
