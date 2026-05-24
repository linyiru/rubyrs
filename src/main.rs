use std::env;
use std::path::Path;
use std::process;

use rubyrs::Runtime;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: rubyrs <file.rb>");
        process::exit(1);
    }
    let path = Path::new(&args[1]);

    let mut rt = Runtime::new();
    match rt.eval_file(path) {
        Ok(_) => {}
        Err(trap) => {
            eprint!("{}", rt.format_trap(&trap));
            process::exit(1);
        }
    }
}
