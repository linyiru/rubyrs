//! Debug renderer: `cargo run -p rostdown --example render -- <file>
//! [--gfm]` prints the HTML, or the decline reason on stderr (exit 1).

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: render <file> [--gfm]");
    let gfm = std::env::args().any(|a| a == "--gfm");
    let src = std::fs::read_to_string(&path).expect("input file readable");
    let opts = if gfm {
        rostdown::Options::jekyll()
    } else {
        rostdown::Options::core()
    };
    match rostdown::to_html(&src, &opts, &mut rostdown::NoHighlight) {
        Ok(html) => print!("{html}"),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
