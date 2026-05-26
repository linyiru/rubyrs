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
                let path = path_arg(p)?;
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
                let path = path_arg(p)?;
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
                let path = path_arg(p)?;
                let exists = std::fs::metadata(&path)
                    .map(|m| if name == "file?" { m.is_file() } else { true })
                    .unwrap_or(false);
                Value::Bool(exists)
            }
            ("directory?", [p]) => {
                let path = path_arg(p)?;
                let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
                Value::Bool(is_dir)
            }
            ("size", [p]) => {
                let path = path_arg(p)?;
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
                // components. Our previous `canonicalize`-only
                // shape would silently fall back to the raw
                // input when the path was missing, which trips
                // gem entry-point setup like
                // `$LOAD_PATH.unshift File.expand_path("..",
                // __dir__)`. Re-implement the lexical resolver:
                //   1. If path is absolute, use as-is.
                //   2. Else join with base (defaults to cwd).
                //   3. Manually collapse `.` and `..` segments.
                //   4. Try canonicalize for the "follows
                //      symlinks" guarantee; fall back to the
                //      lexically-resolved form when the file
                //      doesn't exist (CRuby's behavior).
                use std::path::{Component, Path, PathBuf};
                let path = path_arg(p)?;
                let base: String = match args.get(1) {
                    Some(Value::Str(s)) => s.to_string_lossy(),
                    _ => std::env::current_dir()
                        .map(|d| d.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| ".".to_string()),
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
                // Collapse Components into a normalised PathBuf
                // without touching the filesystem.
                let mut resolved = PathBuf::new();
                for c in joined.components() {
                    match c {
                        Component::ParentDir => { resolved.pop(); }
                        Component::CurDir => {}
                        other => resolved.push(other.as_os_str()),
                    }
                }
                // If the path exists, prefer `canonicalize`'s
                // symlink-resolved form (matches CRuby on
                // existent files); otherwise return the
                // lexically resolved form.
                let final_path = std::fs::canonicalize(&resolved)
                    .unwrap_or(resolved);
                Value::new_str(final_path.to_string_lossy().into_owned())
            }
            _ => return Ok(None),
        }))
    }
}
