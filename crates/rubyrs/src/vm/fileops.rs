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
                Value::Str(s) => Ok(s.content.borrow().clone()),
                _ => Err(self.trap(RubyError::TypeError {
                    msg: format!("no implicit conversion of {} into String", a.type_name()),
                })),
            }
        };
        Ok(Some(match (name, args) {
            ("read", [p]) => {
                let path = path_arg(p)?;
                match std::fs::read_to_string(&path) {
                    Ok(s) => Value::new_str(s),
                    Err(e) => return Err(self.trap(RubyError::RuntimeError {
                        msg: format!("File.read({}): {}", path, e),
                    })),
                }
            }
            ("write", [p, body]) => {
                let path = path_arg(p)?;
                let contents = match body {
                    Value::Str(s) => s.content.borrow().clone(),
                    _ => body.to_display(&self.heap, &self.interner),
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
            ("expand_path", [p]) => {
                let path = path_arg(p)?;
                let abs = std::fs::canonicalize(&path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(path);
                Value::new_str(abs)
            }
            _ => return Ok(None),
        }))
    }
}
