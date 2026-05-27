# Adapted from ruby/spec core/string/upcase_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — baseline ASCII
# uppercase shape. The Unicode-aware `upcase(:lithuanian)` /
# `:turkic` / `:fold` option forms are dropped (Tier-2 Encoding
# work — ADR 0020).

describe "String#upcase" do
  it "returns a copy with ASCII letters folded to uppercase" do
    assert_eq("hello".upcase, "HELLO")
  end

  it "leaves already-uppercase chars unchanged" do
    assert_eq("HELLO".upcase, "HELLO")
    assert_eq("Hello World".upcase, "HELLO WORLD")
  end

  it "returns an empty String for an empty receiver" do
    assert_eq("".upcase, "")
  end

  it "leaves non-letter chars unchanged" do
    assert_eq("hi-5 OK?".upcase, "HI-5 OK?")
  end

  it "returns a new string (non-destructive)" do
    s = "hello"
    assert_eq(s.upcase, "HELLO")
    assert_eq(s, "hello")
  end

  # skipped (method-not-implemented): Unicode option forms
  # (`upcase(:ascii)` / `:turkic` / `:lithuanian` / `:fold`).
  #   Tier-2 Encoding work (ADR 0020).
  # skipped (method-not-implemented): destructive `upcase!`.
end
