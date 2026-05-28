//! `File` class-method shims. Path-based file operations used by
//! gemspec evaluation, Rakefiles, and other build-tool style
//! scripts. Pulled out of `vm.rs` for readability; the dispatch
//! itself happens in `do_call` (vm.rs) and routes to
//! `Vm::file_class_dispatch` here.

use std::path::Path;

use crate::error::{RubyError, Trap};
use crate::value::Value;

use super::Vm;

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
}
