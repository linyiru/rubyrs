mod ast;
mod bytecode;
mod compiler;
mod error;
mod heap;
mod value;
mod vm;

use std::env;
use std::fs;
use std::process;

use crate::ast::tr;
use crate::bytecode::Proto;
use crate::compiler::compile_proto;
use crate::vm::Vm;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: rubyrs <file.rb>");
        process::exit(1);
    }
    let source = fs::read_to_string(&args[1]).expect("cannot read file");
    let result = ruby_prism::parse(source.as_bytes());
    let errs: Vec<_> = result.errors().collect();
    if !errs.is_empty() {
        for e in errs { eprintln!("parse error: {:?}", e); }
        process::exit(2);
    }
    let prog = tr(&result.node());

    if env::var("DEBUG_AST").is_ok() {
        eprintln!("{:#?}", prog);
    }

    let filename: std::rc::Rc<str> = std::rc::Rc::from(args[1].clone());
    let mut protos: Vec<Proto> = vec![];
    let entry = compile_proto("<main>".into(), vec![], &[prog], filename, &mut protos);
    if env::var("DEBUG_BC").is_ok() {
        for (i, p) in protos.iter().enumerate() {
            eprintln!("proto {} {}", i, p.name);
            for (j, op) in p.code.iter().enumerate() {
                eprintln!("  {:04} {:?}", j, op);
            }
        }
    }
    let mut vm = Vm::new(protos);
    vm.run(entry);
    if env::var("GC_STATS").is_ok() {
        eprintln!(
            "gc: live={} slots={} freed_slots={}",
            vm.heap.live_count, vm.heap.slots.len(), vm.heap.free.len()
        );
    }
}
