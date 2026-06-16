# frozen_string_literal: true

# Deterministically generate a benchmark corpus that every engine can
# parse (CommonMark-safe subset: headings, prose, emphasis, links,
# inline + fenced code, lists, blockquotes). No tables/footnotes/math so
# the comparison stays apples-to-apples and rostdown stays on its native
# path. Output size is controlled by REPEAT.

REPEAT = Integer(ENV.fetch("REPEAT", "24"))
OUT = File.expand_path("corpus/bench.md", __dir__)

BLOCK = <<~MD
  ## Section %<n>d: shipping a renderer

  We've been running a Rust renderer in *production* for a while, and
  it's **fast**. Here's the [write-up](https://example.com/post/%<n>d)
  with the gory details, including why we didn't just rewrite the whole
  parser from scratch --- that way lies madness.

  ### Why bother at all?

  Pure-Ruby parsing dominates build time on large sites. A few hundred
  posts and you wait *seconds* per build. The fix isn't to replace the
  parser; it's to accelerate the common case and fall back for the rest.

  - Prose renders natively, with `inline code` sprinkled throughout.
  - Anything exotic declines to the reference implementation.
  - Output is identical, so nobody notices the swap.

  1. First, scan the source.
  2. Then highlight each block.
  3. Finally, splice and emit.

  > Don't optimize what you can't measure. We measured, then measured
  > again, and only *then* did we ship the thing to production.

  Here is the hot loop, lightly edited for clarity:

  ```ruby
  def render(source, profile)
    sid = Native.rd_scan(source, profile)
    return nil if sid.negative?
    each_block(sid) { |lang, code| supply(sid, highlight(lang, code)) }
    Native.rd_render(sid)
  end
  ```

  And the equivalent shape on the Rust side of the boundary:

  ```rust
  pub extern "C" fn rd_scan(src: *const u8, len: usize) -> i64 {
      let src = unsafe { slice::from_raw_parts(src, len) };
      parse(src).map_or(-1, store_session)
  }
  ```

  That is more or less the whole trick: keep the parser honest, keep the
  fallback boring, and let the profiler tell you where the time actually
  goes.
MD

File.write(OUT, (1..REPEAT).map { |n| format(BLOCK, n: n) }.join("\n"))
bytes = File.size(OUT)
puts "wrote #{OUT} (#{bytes} bytes, REPEAT=#{REPEAT})"
