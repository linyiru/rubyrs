# Library-surface behaviours rack's self-suite batch 2 exposed
# (spec_multipart / spec_rewindable_input / spec_mock_request /
# spec_headers / spec_lock). Each row was an E in the dashboard:
#   1. Hash: assoc / rassoc / shift / value? / select! / reject! /
#      keep_if / delete_if (Headers is a Hash subclass that supers
#      into all of these)
#   2. IO#read(length, outbuf) contract on StringIO and File —
#      result REPLACED into outbuf, same object returned; EOF-nil
#      clears the buffer; read(0) is "" not nil
#   3. Tempfile: chmod / binmode / set_encoding accepted, gets /
#      read are byte-based (RewindableInput buffers request bodies
#      through one)
#   4. IO.pipe in-memory pair (write side then read side)
#   5. YAML round-trip: Hash#to_yaml → YAML.unsafe_load
#   6. CGI::Cookie (Array subclass; value returns self)
#   7. Time.httpdate parse + Time.utc carries the UTC flavour
#   8. `undef :name` inside instance_eval undefs on the eigenclass
require "stringio"
require "tempfile"
require "yaml"
require "cgi/cookie"
require "time"

def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 70]}"; end; puts "#{l}: #{r}"; end

# 1 — Hash surface
h = { "a" => "1", "b" => "2" }
t("assoc hit")  { h.assoc("a") }
t("assoc miss") { h.assoc("z") }
t("rassoc")     { h.rassoc("2") }
t("shift")      { x = h.dup; [x.shift, x] }
t("shift empty"){ {}.shift }
t("value?")     { [h.value?("1"), h.value?("9")] }
t("select!")    { x = h.dup; [x.select! { |k, _| k == "a" }, x] }
t("select! nil"){ x = h.dup; x.select! { true } }
t("keep_if")    { x = h.dup; [x.keep_if { true }.size, x.keep_if { |k, _| k == "b" }] }
t("reject!")    { x = h.dup; [x.reject! { |k, _| k == "a" }, x.reject! { false }] }
t("delete_if")  { x = h.dup; x.delete_if { |_, v| v == "2" } }

# 2 — read(length, outbuf)
io = StringIO.new("hello world")
buf = +"XXXX"
t("sio read buf")   { r = io.read(5, buf); [r, buf, r.equal?(buf)] }
t("sio rest buf")   { r = io.read(nil, buf); [r, buf] }
t("sio eof buf")    { r = io.read(3, buf); [r, buf] }
path = "/tmp/rubyrs-rack-lib-#{Process.pid}"
File.binwrite(path, "0123456789")
File.open(path) do |f|
  t("file read buf")  { r = f.read(4, buf); [r, buf, r.equal?(buf)] }
  t("file read zero") { f.read(0) }
  t("file rest buf")  { r = f.read(nil, buf); [r, buf] }
  t("file eof buf")   { r = f.read(2, buf); [r, buf] }
end
File.delete(path)

# 3 — Tempfile surface
tf = Tempfile.new("rack-lib-fix")
tf << "ab\ncd\n"
t("tmp chmod")   { tf.chmod(0o000) }
t("tmp binmode") { tf.binmode; tf.set_encoding(Encoding::BINARY); :accepted }
t("tmp gets")    { tf.flush; tf.rewind; [tf.gets, tf.gets, tf.gets] }
t("tmp read buf"){ tf.rewind; b = +""; r = tf.read(3, b); [r, b] }
tf.close!

# 4 — IO.pipe
t("pipe") { r, w = IO.pipe; w.write("zz\nq"); w.close; out = [r.gets, r.read]; r.close; out }

# 5 — YAML round-trip (loose: our emitter isn't psych-byte-equal,
# but load(dump(x)) must reproduce x on both runtimes)
t("yaml rt") {
  env = { "A" => "GET", "n" => nil, "i" => 5, "f" => 1.5, "b" => true,
          "list" => [1, "two", nil], "deep" => { "k" => "v" } }
  YAML.unsafe_load(YAML.dump(env)) == env
}
t("yaml esc") { s = "a\"b\\c\nd\te"; YAML.unsafe_load(YAML.dump([{ "x" => s }])) == [{ "x" => s }] }

# 6 — CGI::Cookie
c = CGI::Cookie.new("name" => "sid", "value" => "abc", "path" => "/x", "secure" => false)
t("cookie basics") { [c.value[0], c.name, c.path, c.secure, c.expires, c.is_a?(Array)] }
t("cookie multi")  { CGI::Cookie.new("name" => "m", "value" => %w[v1 v2]).to_a }

# 7 — Time
t("httpdate parse") { Time.httpdate("Thu, 31 Oct 2021 07:28:00 GMT") }
t("httpdate bad")   { begin; Time.httpdate("nope"); rescue ArgumentError; :argerr; end }
t("utc flavour")    { [Time.utc(2021, 1, 2).utc?, Time.utc(2021, 1, 2).to_s] }

# 8 — undef inside instance_eval
sio = StringIO.new("q")
sio.instance_eval { undef :rewind }
t("undef ie") { [sio.respond_to?(:rewind), StringIO.new("w").respond_to?(:rewind)] }
