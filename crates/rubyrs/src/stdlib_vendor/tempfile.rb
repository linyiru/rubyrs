# Tier 3 Tempfile — a buffered handle over a real file in the temp
# directory (filesystem access rides the existing
# `allow_filesystem_io` capability; with it off, flush/read raise
# the same IOError every File call does). Names combine pid + a
# process-local sequence + the basename — collision-free within a
# run and deterministic in ORDER (the path embeds the pid, which is
# host state the consumer explicitly asked for by using Tempfile).
#
# Covers the minitest surface: assertions#diff writes a pair and
# shells out (`Tempfile.open { |f| f.puts ...; f.flush }` + #path),
# and capture_subprocess_io reopens stdio onto one (#16). Out of
# scope: encoding modes, real IO inheritance to subprocesses beyond
# path-based access, auto-finalizer unlink (close!/unlink are
# explicit — minitest always calls them).
class Tempfile
  @@seq = 0

  def self.open(basename = "tmp")
    f = new(basename)
    return f unless block_given?
    begin
      yield f
    ensure
      f.close!
    end
  end

  def initialize(basename = "tmp")
    @@seq += 1
    dir = ENV["TMPDIR"] || "/tmp"
    dir = dir.chomp("/")
    @path = "#{dir}/rubyrs-#{Process.pid}-#{@@seq}-#{basename}"
    @buf = +""
    @flushed = false
    @closed = false
    @pos = 0
  end

  attr_reader :path

  def write(s)
    s = s.to_s
    @buf << s
    s.length
  end

  def <<(s)
    write(s)
    self
  end

  def puts(*args)
    if args.empty?
      write("\n")
    else
      args.each do |a|
        s = a.to_s
        write(s)
        write("\n") unless s.end_with?("\n")
      end
    end
    nil
  end

  def print(*args)
    args.each { |a| write(a.to_s) }
    nil
  end

  def flush
    File.write(@path, @buf)
    @flushed = true
    self
  end

  def rewind
    @pos = 0
    0
  end

  def read
    flush unless @flushed
    content = File.read(@path)
    out = content[@pos..] || ""
    @pos = content.length
    out
  end

  def size
    @buf.length
  end
  alias_method :length, :size

  def close
    flush unless @flushed
    @closed = true
    nil
  end

  def closed?
    @closed
  end

  def unlink
    File.delete(@path) if File.exist?(@path)
    nil
  rescue StandardError
    nil
  end
  alias_method :delete, :unlink

  def close!
    close
    unlink
  end
end
