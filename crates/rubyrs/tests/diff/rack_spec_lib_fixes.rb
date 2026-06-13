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
require "uri"

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

# 2b — File class predicates + mtime + seek/pos family (Rack::Files
# gates serving on File.file?/readable? and serves Range requests via
# seek + chunked read; mtime drives Last-Modified / If-Modified-Since).
fpath = "/tmp/rubyrs-rack-file-#{Process.pid}"
File.binwrite(fpath, "0123456789")
t("file readable?")     { File.readable?(fpath) }
t("file writable?")     { File.writable?(fpath) }
t("file executable?")   { File.executable?(fpath) }
t("file readable miss") { File.readable?(fpath + "-nope") }
t("file size?")         { File.size?(fpath) }
t("file size? miss")    { File.size?(fpath + "-nope") }
t("file size? zero")    { File.binwrite(fpath + "-e", ""); r = File.size?(fpath + "-e"); File.delete(fpath + "-e"); r }
t("file mtime class")   { File.mtime(fpath).class }
t("file mtime istime")  { File.mtime(fpath).is_a?(Time) }
t("file mtime diff0")   { File.mtime(fpath) - File.mtime(fpath) }
File.open(fpath) do |f|
  t("file seek set")  { f.seek(3); f.read(4) }
  t("file seek cur")  { f.seek(2); f.seek(2, 1); f.read(2) }
  t("file seek end")  { f.seek(-2, 2); f.read }
  t("file pos/tell")  { f.rewind; f.read(5); [f.pos, f.tell] }
  t("file pos=")      { f.pos = 7; f.read }
end
File.delete(fpath)

# 2c — Dir.mktmpdir (require "tempfile" pulls in tmpdir, as in CRuby).
# Block form yields a real dir and removes it on exit; non-block
# returns the path. spec_directory builds its scratch tree this way.
t("mktmpdir blk")  { inner = nil; r = Dir.mktmpdir("rk") { |d| inner = d; File.directory?(d) }; [r, File.directory?(inner)] }
t("mktmpdir path") { d = Dir.mktmpdir; ok = File.directory?(d); Dir.rmdir(d); ok }

# 3 — Tempfile surface
# Array basename [prefix, suffix] + encoding kwarg (rack's UploadedFile
# does `Tempfile.new([name, ext], encoding: Encoding::BINARY)`); path
# must keep the suffix for File.extname.
t("tmp arr name") { t2 = Tempfile.new(["pre", ".txt"], encoding: Encoding::BINARY); ok = File.extname(t2.path); t2.close!; ok }
tf = Tempfile.new("rack-lib-fix")
tf << "ab\ncd\n"
t("tmp chmod")   { tf.chmod(0o000) }
t("tmp binmode") { tf.binmode; tf.set_encoding(Encoding::BINARY); :accepted }
t("tmp gets")    { tf.flush; tf.rewind; [tf.gets, tf.gets, tf.gets] }
t("tmp read buf"){ tf.rewind; b = +""; r = tf.read(3, b); [r, b] }
tf.close!

# 4 — IO.pipe
t("pipe") { r, w = IO.pipe; w.write("zz\nq"); w.close; out = [r.gets, r.read]; r.close; out }

# 4b — Kernel#URI (require "uri" installs it; rack's Recursive uses
# `URI(url)`). String -> parsed URI; an existing URI is returned as-is.
t("URI str")  { u = URI("http://example.com/a?b=1"); [u.scheme, u.host, u.path, u.query] }
t("URI idem") { u = URI("http://x/"); URI(u).equal?(u) }
t("URI bad")  { begin; URI(42); rescue ArgumentError => e; e.class.name; end }

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
