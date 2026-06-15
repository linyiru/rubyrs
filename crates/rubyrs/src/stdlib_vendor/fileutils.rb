# FileUtils reflection veneer (ADR 0026 blessed reimpl). The actual
# file/dir mutation commands (cp, mkdir_p, rm_rf, ...) are NATIVE host
# primitives (vm/fileops.rs) dispatched on the FileUtils module; this
# file adds the REFLECTION surface that pure-Ruby callers metaprogram
# over. Discovery: rake's `Rake::FileUtilsExt` (rake/file_utils_ext.rb)
# iterates `FileUtils.commands`, reads `FileUtils.options_of(name)`, and
# generates verbose/noop-aware wrappers at load time.
#
# OPT_TABLE mirrors CRuby's FileUtils::OPT_TABLE, restricted to the
# commands rubyrs implements natively (so every wrapper rake generates
# resolves to a real command). Values are the option names each command
# accepts; the native primitives accept and ignore a trailing options
# Hash, so the generated `verbose:`/`noop:` defaults pass through
# harmlessly.
module FileUtils
  OPT_TABLE = {
    "cp"       => ["preserve", "noop", "verbose"],
    "copy"     => ["preserve", "noop", "verbose"],
    "cp_r"     => ["preserve", "noop", "verbose", "dereference_root", "remove_destination"],
    "mkdir"    => ["mode", "noop", "verbose"],
    "mkdir_p"  => ["mode", "noop", "verbose"],
    "makedirs" => ["mode", "noop", "verbose"],
    "mkpath"   => ["mode", "noop", "verbose"],
    "mv"       => ["force", "noop", "verbose", "secure"],
    "move"     => ["force", "noop", "verbose", "secure"],
    "rm"       => ["force", "noop", "verbose"],
    "remove"   => ["force", "noop", "verbose"],
    "rm_f"     => ["noop", "verbose"],
    "rm_rf"    => ["noop", "verbose", "secure"],
    "touch"    => ["noop", "verbose", "mtime", "nocreate"],
  }.freeze

  def self.commands
    OPT_TABLE.keys
  end

  def self.options
    OPT_TABLE.values.flatten.uniq
  end

  def self.options_of(mid)
    OPT_TABLE[mid.to_s] || []
  end

  def self.have_option?(mid, opt)
    (OPT_TABLE[mid.to_s] || []).include?(opt.to_s)
  end
end
