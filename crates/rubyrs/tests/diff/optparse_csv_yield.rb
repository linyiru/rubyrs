# RuboCop's Options#option nesting: `opts.on(*args) { |arg| @o[k] =
# arg; yield arg if block_given? }`, where the on-block is stored and
# run later by parse!. The deferred `yield arg if block_given?` must
# fire so the CSV-splitting outer block overwrites the raw String with
# an Array (rubocop/options.rb add_cop_selection_csv_option). Needs the
# vendored OptionParser (--features stdlib).
require "optparse"

class Driver
  def initialize
    @o = {}
    @p = OptionParser.new do |o|
      csv(o, "--only [COP1,COP2,...]") { |list| @o[:only] = list.split(",") }
      csv(o, "--except [COP1,COP2,...]") { |list| @o[:except] = list.split(",") }
    end
  end

  def csv(opts, *args)
    opts.on(*args) do |arg|
      @o[args_key(args)] = arg          # raw String first
      yield arg if block_given?         # then the CSV block overwrites
    end
  end

  def args_key(args)
    args.first[/--(\w+)/, 1].to_sym
  end

  def run(argv)
    @p.parse!(argv)
    @o
  end
end

p Driver.new.run(["--only", "Style/StringLiterals"])
p Driver.new.run(["--only", "Style/Foo,Style/Bar", "--except", "Lint/Void"])
