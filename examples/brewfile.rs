//! Brewfile DSL host — the pivot-validation demo.
//!
//! A Brewfile is a tiny DSL: a Ruby script whose top-level "API" is just
//! `tap`, `brew`, `cask`, and `mas`, each of which records a package the
//! user wants installed. Homebrew itself runs the Brewfile under CRuby
//! to gather that list.
//!
//! This example runs the same shape of script *embedded in a Rust
//! application* using rubyrs. The host registers four host functions
//! that mutate a `Brewfile` struct, evaluates a sample Brewfile script,
//! and prints a summary.
//!
//! ```text
//!     cargo run --release --example brewfile
//! ```
//!
//! Why this matters: it demonstrates rubyrs's actual product niche —
//! "Ruby DSL hosted in a Rust application" — using only the public
//! embedding API (`Runtime::register_fn`, `eval_file`). No special
//! plumbing.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use rubyrs::{Runtime, Value};

#[derive(Default, Debug)]
struct Brewfile {
    taps: Vec<String>,
    formulae: Vec<String>,
    casks: Vec<String>,
    mas_apps: Vec<(String, i64)>,
}

impl Brewfile {
    fn print_summary(&self) {
        println!("Collected Brewfile contents:");
        println!("  {:>3} taps", self.taps.len());
        println!("  {:>3} formulae (brew)", self.formulae.len());
        println!("  {:>3} casks", self.casks.len());
        println!("  {:>3} mas apps", self.mas_apps.len());
        println!();
        if !self.formulae.is_empty() {
            println!("first 5 brews: {:?}", &self.formulae[..self.formulae.len().min(5)]);
        }
        if !self.casks.is_empty() {
            println!("first 5 casks: {:?}", &self.casks[..self.casks.len().min(5)]);
        }
    }
}

fn main() {
    let bf = Rc::new(RefCell::new(Brewfile::default()));

    let mut rt = Runtime::new();

    // ---- register the DSL surface ----
    {
        let b = bf.clone();
        rt.register_fn("tap", move |args| {
            if let [Value::Str(s)] = args {
                b.borrow_mut().taps.push(s.to_string());
            }
            Ok(Value::Nil)
        });
    }
    {
        let b = bf.clone();
        rt.register_fn("brew", move |args| {
            if let [Value::Str(s)] = args {
                b.borrow_mut().formulae.push(s.to_string());
            }
            Ok(Value::Nil)
        });
    }
    {
        let b = bf.clone();
        rt.register_fn("cask", move |args| {
            if let [Value::Str(s)] = args {
                b.borrow_mut().casks.push(s.to_string());
            }
            Ok(Value::Nil)
        });
    }
    {
        let b = bf.clone();
        rt.register_fn("mas", move |args| {
            if let [Value::Str(name), Value::Int(id)] = args {
                b.borrow_mut().mas_apps.push((name.to_string(), *id));
            }
            Ok(Value::Nil)
        });
    }

    // ---- run the user's Brewfile ----
    let bf_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/brewfile/Brewfile.rb");
    let start = Instant::now();
    if let Err(trap) = rt.eval_file(&bf_path) {
        eprint!("{}", rt.format_trap(&trap));
        std::process::exit(1);
    }
    let elapsed = start.elapsed();

    bf.borrow().print_summary();
    println!();
    println!("rubyrs ran the Brewfile in {:.2} ms", elapsed.as_secs_f64() * 1000.0);
}
