## Section 1: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/1)
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

## Section 2: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/2)
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

## Section 3: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/3)
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

## Section 4: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/4)
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

## Section 5: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/5)
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

## Section 6: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/6)
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

## Section 7: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/7)
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

## Section 8: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/8)
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

## Section 9: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/9)
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

## Section 10: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/10)
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

## Section 11: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/11)
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

## Section 12: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/12)
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

## Section 13: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/13)
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

## Section 14: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/14)
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

## Section 15: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/15)
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

## Section 16: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/16)
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

## Section 17: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/17)
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

## Section 18: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/18)
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

## Section 19: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/19)
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

## Section 20: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/20)
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

## Section 21: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/21)
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

## Section 22: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/22)
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

## Section 23: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/23)
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

## Section 24: shipping a renderer

We've been running a Rust renderer in *production* for a while, and
it's **fast**. Here's the [write-up](https://example.com/post/24)
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
