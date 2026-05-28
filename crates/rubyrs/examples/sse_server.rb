# rubyrs `_http_server + _fiber` — Server-Sent Events example.
#
# Demonstrates true async streaming per ADR 0023: each
# `yield` from the Rack body becomes one HTTP/1.1 chunked
# frame flushed to the client BEFORE the body proc finishes.
# Buffered Rack servers would batch all events into one
# write at end-of-body — useless for SSE.
#
# Run with:
#   cargo run --features _http_server,_fiber -p rubyrs -- \
#     crates/rubyrs/examples/sse_server.rb
#
# Then in another terminal:
#   curl -N http://127.0.0.1:9292/events
#   # -N disables curl's output buffering so you can SEE
#   # each chunk arrive as a separate Transfer-Encoding:
#   # chunked frame.
#
# Why this needs `_fiber`:
# - SSE bodies are open-ended generators. Buffered bodies
#   (Array, to_a) materialise the whole sequence up front
#   — impossible for an unbounded stream.
# - The `_fiber` feature lets the body suspend (`yield`)
#   between events and resume when hyper polls for the
#   next frame. See ADR 0023 §"Streaming bodies via Fiber".
#
# Pacing note: rubyrs's stdlib doesn't yet ship a `sleep`
# primitive, so this demo yields all events as fast as the
# fiber + tokio executor can deliver them. To see
# wall-clock-paced streaming, register a tiny `sleep_ms`
# host fn in your embedder (the test suite has an example —
# search for `__rubyrs_test_sleep_ms`). The wire-level
# streaming proof is independent of pacing: each `yield`
# is still its own chunked frame (see the timing test
# `p2b2b4_first_chunk_arrives_before_body_finishes`).

# A Rack 3 streaming body. Responds to `each`; each `yield`
# becomes one wire-level chunked frame.
class SSEStream
  def initialize(event_count)
    @event_count = event_count
  end

  def each
    i = 0
    while i < @event_count
      # SSE wire format: `data: <payload>\n\n`. The blank
      # line is the event terminator per the SSE spec.
      yield "data: tick #{i} at #{Time.now.to_i}\n\n"
      i += 1
    end
    yield "data: done\n\n"
  end

  def close
    # Rack 3 SPEC §"Body" — invoked once by the server
    # after the stream completes (or errors). rubyrs
    # honours this on both buffered and streaming paths.
    puts "[sse] body closed at #{Time.now.to_i}"
  end
end

# Alternative streaming shape: Rack 3's `call(stream)`
# protocol. The body's `call` method receives a writer
# (rubyrs's `RubyrsStreamingStream`) and pushes chunks
# via `stream.write(...)`. Each `write` suspends the
# fiber, hyper drains the chunk to the socket, then
# resumes — same wire effect as `each`, different API.
class WriteStyleStream
  def call(stream)
    5.times do |i|
      stream.write("data: write-style #{i}\n\n")
    end
    stream.write("data: done\n\n")
    stream.close
  end
end

app = ->(env) {
  case env["PATH_INFO"]
  when "/events"
    [
      200,
      {"Content-Type" => "text/event-stream", "Cache-Control" => "no-cache"},
      SSEStream.new(10),
    ]
  when "/write"
    [
      200,
      {"Content-Type" => "text/event-stream", "Cache-Control" => "no-cache"},
      WriteStyleStream.new,
    ]
  else
    [
      200,
      {"Content-Type" => "text/plain"},
      ["Endpoints:\n  /events  — `each`-shape SSE stream (10 events)\n  /write   — `call`-shape SSE stream (6 events)\n"],
    ]
  end
}

puts "[sse] listening on http://127.0.0.1:9292"
puts "[sse] try: curl -N http://127.0.0.1:9292/events"
# (addr, duration_secs, app). Duration is generous so the
# server stays up long enough to demo multiple connections.
__rubyrs_http_serve_with_app("127.0.0.1:9292", 600, app)
