# FileUtils reflection surface rake's FileUtilsExt metaprograms over.
# CRuby's full command list is larger than rubyrs's native subset, so
# assert membership/option invariants that hold for both rather than the
# exact list.
require "fileutils"
p FileUtils.commands.is_a?(Array)
%w[cp mkdir_p mv rm_rf touch].each { |c| p FileUtils.commands.include?(c) }
p FileUtils.options_of("cp").include?("verbose")
p FileUtils.options_of("cp").include?("noop")
p FileUtils.options_of("mkdir_p").include?("noop")
p FileUtils.have_option?(:cp, :preserve)
p FileUtils.have_option?(:rm_rf, :verbose)
