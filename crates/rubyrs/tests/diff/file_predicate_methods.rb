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

# File.stat -> File::Stat query surface (Rack::Directory listing).
t("stat size")     { File.stat(fp).size }
t("stat file?")    { s = File.stat(fp); [s.file?, s.directory?] }
t("stat mtime cls"){ File.stat(fp).mtime.class }
t("stat dir")      { s = File.stat("."); [s.directory?, s.file?] }
t("stat miss")     { begin; File.stat(fp + "-nope"); rescue SystemCallError => e; e.class; end }

# Dir.mkdir / rmdir / foreach (native).
dd = fp + "-d"
t("dir mkdir")     { Dir.mkdir(dd); File.directory?(dd) }
t("dir foreach")   { File.binwrite("#{dd}/a", "1"); File.binwrite("#{dd}/b", "2"); ns = []; Dir.foreach(dd) { |e| ns << e }; ns.sort }
File.delete("#{dd}/a"); File.delete("#{dd}/b")
t("dir rmdir")     { Dir.rmdir(dd); File.directory?(dd) }

# Symlink: broken target -> ENOENT, self-referential -> ELOOP, both
# of which Rack::Directory's listing rescues to skip the entry.
bl = fp + "-broken"
File.symlink(fp + "-no-such-target", bl)
t("stat broken")   { begin; File.stat(bl); rescue SystemCallError => e; e.class; end }
File.delete(bl)
lp = fp + "-loop"
File.symlink(File.basename(lp), lp)
t("stat eloop")    { begin; File.stat(lp); rescue SystemCallError => e; e.class; end }
File.delete(lp)

# mkfifo: a FIFO is not a regular file, so Rack serves 404. stat must
# NOT block (access(2)-based readable?, never opens the pipe).
ff = fp + "-fifo"
File.mkfifo(ff)
t("fifo not file") { s = File.stat(ff); [s.file?, s.directory?, s.readable?] }
File.delete(ff)

# FileUtils.copy_file (rack's UploadedFile copies into a Tempfile).
t("copy_file")     { d = fp + "-cp"; FileUtils.copy_file(fp, d); r = File.binread(d); File.delete(d); r }
t("copy_file 3arg"){ d = fp + "-cp2"; FileUtils.copy_file(fp, d, true); r = File.size(d); File.delete(d); r }

# chmod (last — it mutates fp's perms): set 0600, read the mode back.
t("chmod mode")    { File.chmod(0o600, fp); File.stat(fp).mode & 0o777 }
File.delete(fp)
