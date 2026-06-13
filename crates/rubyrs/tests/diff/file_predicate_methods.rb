# Core-only twin for the File class predicates (rack_spec_lib_fixes.rb
# covers them too but requires stringio/tempfile and therefore doesn't
# run in the bare / Coverage configurations). Pins the vm/fileops.rs
# arms readable?/writable?/executable?/size?/__mtime_f with ZERO
# requires — the per-file coverage ratchet sees them in every build.
# Real FS via the Tier-1 File.binwrite primitive into /tmp.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 70]}"; end; puts "#{l}: #{r}"; end

fp = "/tmp/rubyrs-file-pred-#{Process.pid}"
File.binwrite(fp, "0123456789")
t("readable?")     { File.readable?(fp) }
t("writable?")     { File.writable?(fp) }
t("executable?")   { File.executable?(fp) }
t("readable miss") { File.readable?(fp + "-nope") }
t("size?")         { File.size?(fp) }
t("size? miss")    { File.size?(fp + "-nope") }
t("size? zero")    { File.binwrite(fp + "-e", ""); r = File.size?(fp + "-e"); File.delete(fp + "-e"); r }
t("mtime class")   { File.mtime(fp).class }
t("mtime istime")  { File.mtime(fp).is_a?(Time) }
t("mtime diff0")   { File.mtime(fp) - File.mtime(fp) }
t("mtime miss")    { begin; File.mtime(fp + "-nope"); rescue SystemCallError => e; e.is_a?(Errno::ENOENT); end }
File.delete(fp)
