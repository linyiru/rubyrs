# $LOADED_FEATURES reflects COMPLETION order (CRuby): a file is added only
# AFTER its body runs, so a nested require inside an in-progress file sees
# the just-completed INNER file as `.last`, not the outer one. zeitwerk's
# decorated require reads `$LOADED_FEATURES.last` to identify the file it
# loaded; an early-add made a nested `require "time"` mid-load misfire.
base = "/tmp/_lf_fix"
Dir.mkdir(base) unless Dir.exist?(base)
File.write("#{base}/lf_inner.rb", "module LfInner; end\n")
File.write("#{base}/lf_outer.rb", <<~RB2)
  puts "outer-before: self-in-features=\#{$LOADED_FEATURES.any? { |f| f.to_s.end_with?('lf_outer.rb') }}"
  require "\#{__dir__}/lf_inner"
  puts "outer-after-inner: last-is-inner=\#{$LOADED_FEATURES.last.to_s.end_with?('lf_inner.rb')}"
RB2
require "#{base}/lf_outer"
puts "done: last-is-outer=#{$LOADED_FEATURES.last.to_s.end_with?('lf_outer.rb')}"
