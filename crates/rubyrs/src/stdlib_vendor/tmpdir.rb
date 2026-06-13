# Vendored minimal `tmpdir` — Dir.tmpdir + Dir.mktmpdir, backed by the
# native Dir.mkdir primitive (+ FileUtils for block-form cleanup).
# rack's spec_directory builds a throwaway tree with Dir.mktmpdir; the
# tempfile-style specs reach for it as scratch space too.
require "fileutils"

class Dir
  # System temp directory — TMPDIR/TMP/TEMP override, else the POSIX
  # default /tmp (rubyrs targets Unix; no Windows temp probing).
  def self.tmpdir
    # CRuby strips a trailing slash (ENV["TMPDIR"] is often
    # "/var/folders/.../T/"); leaving it breaks path-prefix matching —
    # rack's Sendfile builds an x-accel-mapping regex from
    # "#{Dir.tmpdir}/" and a doubled slash never matches the served
    # path. Also avoids the doubled slash in our own mktmpdir join.
    dir = (ENV["TMPDIR"] || ENV["TMP"] || ENV["TEMP"] || "/tmp").to_s
    dir = dir.chomp("/")
    dir = "/tmp" if dir.empty?
    dir
  end

  @@__mktmpdir_seq = 0

  # `Dir.mktmpdir(prefix_suffix = nil, tmpdir = nil)`
  #   prefix_suffix : String prefix, or [prefix, suffix] pair, or nil
  #                   (default prefix "d", empty suffix).
  #   tmpdir        : parent dir (default Dir.tmpdir).
  # Creates a uniquely-named directory and, in block form, yields the
  # path then removes the tree afterwards (even on raise), returning
  # the block's value. Non-block form returns the path. The unique
  # stamp folds rand + pid + a process-local sequence so concurrent /
  # repeated calls don't collide; creation retries on a name clash.
  def self.mktmpdir(prefix_suffix = nil, tmpdir = nil)
    prefix, suffix =
      case prefix_suffix
      when nil   then ["d", ""]
      when Array then [prefix_suffix[0].to_s, (prefix_suffix[1] || "").to_s]
      else            [prefix_suffix.to_s, ""]
      end
    base = (tmpdir || Dir.tmpdir).to_s
    path = nil
    attempts = 0
    while attempts < 32
      attempts += 1
      @@__mktmpdir_seq += 1
      stamp = "#{rand(0x100000000).to_s(36)}-#{Process.pid}-#{@@__mktmpdir_seq}"
      candidate = "#{base}/#{prefix}#{stamp}#{suffix}"
      begin
        Dir.mkdir(candidate)
        path = candidate
        break
      rescue SystemCallError
        # Name collision or transient error — retry with a fresh stamp.
      end
    end
    raise "could not make a temporary directory in #{base}" if path.nil?
    if block_given?
      begin
        yield path
      ensure
        FileUtils.remove_entry_secure(path) if File.directory?(path)
      end
    else
      path
    end
  end
end
