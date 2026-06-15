//! Coverage for the native StringScanner search hook,
//! `String#__strscan_search(regex, byte_pos)` — the no-slice search
//! that keeps `StringScanner#scan_until` linear over a large binary
//! buffer (rack multipart). It runs the byte engine on a `&bytes[pos..]`
//! VIEW (so `\A`/`^` anchor at the scan position, like CRuby's
//! StringScanner), and returns the absolute match start (Integer), nil
//! for no match, or false when the pattern has no byte engine
//! (lookaround/backref → fancy). This lives in an embed test (not a
//! diff_cruby fixture) because the method is rubyrs-internal with no
//! CRuby equivalent — and because it must be exercised in the DEFAULT
//! feature build, where the stdlib StringScanner fixtures don't load.

use super::SharedBuf;

#[test]
fn strscan_search_native_contract() {
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        s = "xx--bnd\r\nyy--bnd--".b
        p s.__strscan_search(/--bnd(?:\r\n|--)/, 0)   # 2
        p s.__strscan_search(/--bnd(?:\r\n|--)/, 3)   # 11 (next at/after 3)
        p s.__strscan_search(/--bnd--/, 0)            # 11
        p s.__strscan_search(/\A--bnd/, 2)            # 2  (scan-pos \A at offset 2)
        p s.__strscan_search(/\A--bnd/, 0)            # nil ("xx" at 0)
        p s.__strscan_search(/(.*?)bnd/, 0)           # 0  (capture group)
        p s.__strscan_search(/zzzz/, 0)               # nil (no match)
        p s.__strscan_search(/(?<=x)--bnd/, 0)        # false (no byte engine)
        p s.__strscan_search(/--bnd/, 9999)           # nil (offset clamped)
        p "".b.__strscan_search(/--bnd/, 0)           # nil (empty subject)
        "#,
        "strscan_native.rb",
    )
    .expect("eval");
    assert_eq!(
        buf.snapshot(),
        "2\n11\n11\n2\nnil\n0\nnil\nfalse\nnil\nnil\n",
    );
}

#[test]
fn strscan_search_sets_byte_faithful_dollar_tilde() {
    // The match-found branch records `$~` with byte-faithful captures
    // (region bytes + re-based spans), ASCII-8BIT tagged.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        m = "name=\"f\xC3.txt\"".b
        off = m.__strscan_search(/name="(.*?)"/, 0)
        p off                 # 0
        p $~[1].bytes         # [102, 195, 46, 116, 120, 116]
        p $~[1].encoding.to_s # "ASCII-8BIT"
        "#,
        "strscan_native_caps.rb",
    )
    .expect("eval");
    assert_eq!(
        buf.snapshot(),
        "0\n[102, 195, 46, 116, 120, 116]\n\"ASCII-8BIT\"\n",
    );
}
