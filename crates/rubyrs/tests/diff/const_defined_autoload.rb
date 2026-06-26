# const_defined? reports a registered-but-unloaded autoload as DEFINED, and
# must NOT trigger the load — for both the own-only (inherit=false) and the
# inheriting (default) forms. CRuby semantics; zeitwerk's const_defined?
# checks rely on it. The autoload target file does not exist, so any
# accidental trigger would raise LoadError instead of printing cleanly.
$loaded = false
Object.autoload(:Zzz, File.expand_path("nonexistent_zzz_target.rb", __dir__))

puts Object.const_defined?(:Zzz, false)   # true (own table + autoload)
puts Object.const_defined?(:Zzz)          # true (inherit) — must not load
puts(Object.autoload?(:Zzz) ? "still armed" : "fired")
puts $loaded                              # false — never triggered

# A bare name with no constant and no autoload is still not defined.
puts Object.const_defined?(:NoSuchConstAtAll, false)
