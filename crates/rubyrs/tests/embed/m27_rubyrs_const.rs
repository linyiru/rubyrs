//! M27 B2 (ADR 0026 GAP #4): the `RUBYRS` sentinel constant for
//! library-author adapter shims. CRuby leaves it undefined; rubyrs
//! pins it to the frozen string "rubyrs". `defined?(RUBYRS)` is the
//! canonical detection idiom.

use crate::SharedBuf;

#[test]
fn rubyrs_constant_is_defined_with_canonical_value() {
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r##"
        puts defined?(RUBYRS).inspect
        puts RUBYRS
        puts RUBYRS.frozen?
        # RUBY_ENGINE still reports "ruby" (legacy gem-compat posture);
        # RUBYRS is the additive surface for unambiguous detection.
        puts RUBY_ENGINE
        "##,
        "m27_rubyrs_const.rb",
    )
    .expect("eval");
    assert_eq!(
        buf.snapshot(),
        "\"constant\"\nrubyrs\ntrue\nruby\n",
    );
}
