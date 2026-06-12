# Tier 3 pure-Ruby OptionParser — the declarative-`on` +
# destructive-`parse!` subset CLI-ish gems actually exercise
# (minitest's process_args is the motivating consumer: short/long
# flags, required/optional arguments, Integer/String coercion
# tags, banner/separator/help text, InvalidOption on unknowns).
#
# Out of scope (NoMethodError / divergence by design): `order!`'s
# permutation modes, `accept` custom converters, abbreviation
# matching ("--verb" for "--verbose" — CRuby allows unambiguous
# prefixes; here options match exactly), summary indenting knobs,
# and `load`/`environment`.

class OptionParser
  class ParseError < StandardError; end
  class InvalidOption < ParseError; end
  class MissingArgument < ParseError; end
  class InvalidArgument < ParseError; end

  attr_accessor :banner, :version

  def initialize(banner = nil)
    @banner = banner
    @specs = []
    @separators = []
    yield self if block_given?
  end

  # `on("-s", "--seed SEED", Integer, "desc") { |v| ... }`
  # Argument shapes recognised:
  #   "--name"            flag (block gets true; "--no-name" of a
  #                       "--[no-]name" spec gets false)
  #   "--name ARG"        required argument
  #   "--name [ARG]"      optional argument
  #   "--name=ARG"        same as "--name ARG"
  #   "-n" / "-n ARG" / "-W[err]"  short forms (inline value "-nfoo"
  #                       supported for argument-taking shorts)
  #   Integer/Float/String  coercion tag for the block's value
  #   any other String    help text
  def on(*args, &block)
    spec = {
      shorts: [], longs: [], arg: :none, coerce: nil, desc: [],
      block: block,
    }
    args.each do |a|
      if a == Integer || a == Float || a == String
        spec[:coerce] = a
      elsif a.is_a?(String) && a.start_with?("--")
        head, argpart = a.split(/[ =]/, 2)
        if head.start_with?("--[no-]")
          spec[:longs] << head.sub("--[no-]", "--")
          spec[:longs] << head.sub("--[no-]", "--no-")
        else
          spec[:longs] << head
        end
        if argpart && !argpart.empty?
          spec[:arg] = argpart.start_with?("[") ? :optional : :required
        end
      elsif a.is_a?(String) && a.start_with?("-") && a.length >= 2
        spec[:shorts] << a[0, 2]
        rest = a[2..].to_s.strip
        unless rest.empty?
          spec[:arg] = rest.start_with?("[") ? :optional : :required
        end
      else
        spec[:desc] << a.to_s
      end
    end
    @specs << spec
    self
  end
  alias_method :on_tail, :on
  alias_method :on_head, :on

  def separator(text)
    @separators << text.to_s
    self
  end

  # Destructive parse: consumed options (and their values) are
  # deleted from argv; positional arguments stay. `--` ends option
  # processing (and is itself removed), matching CRuby.
  def parse!(argv)
    i = 0
    while i < argv.length
      arg = argv[i]
      if arg == "--"
        argv.delete_at(i)
        break
      elsif arg.start_with?("--")
        name, inline = arg.split("=", 2)
        spec = @specs.find { |s| s[:longs].include?(name) }
        raise InvalidOption, "invalid option: #{name}" unless spec
        argv.delete_at(i)
        val = inline
        if spec[:arg] == :required && val.nil?
          val = argv.delete_at(i)
          raise MissingArgument, "missing argument: #{name}" if val.nil?
        elsif spec[:arg] == :optional && val.nil?
          if i < argv.length && !argv[i].start_with?("-")
            val = argv.delete_at(i)
          end
        end
        invoke(spec, name, val)
      elsif arg.start_with?("-") && arg.length > 1
        short = arg[0, 2]
        spec = @specs.find { |s| s[:shorts].include?(short) }
        raise InvalidOption, "invalid option: #{short}" unless spec
        argv.delete_at(i)
        val = arg.length > 2 ? arg[2..] : nil
        if spec[:arg] == :required && val.nil?
          val = argv.delete_at(i)
          raise MissingArgument, "missing argument: #{short}" if val.nil?
        end
        invoke(spec, short, val)
      else
        i += 1
      end
    end
    argv
  end

  def parse(argv)
    rest = argv.dup
    parse!(rest)
    rest
  end
  alias_method :order!, :parse!
  alias_method :permute!, :parse!

  def to_s
    out = +""
    out << "#{@banner}\n" if @banner
    @specs.each do |s|
      names = (s[:shorts] + s[:longs]).join(", ")
      out << "    #{names}#{s[:desc].empty? ? "" : "  " + s[:desc].join(" ")}\n"
    end
    @separators.each { |t| out << "#{t}\n" }
    out
  end
  alias_method :help, :to_s

  private

  def invoke(spec, name, val)
    blk = spec[:block]
    return unless blk
    if spec[:arg] == :none
      blk.call(!name.start_with?("--no-"))
    else
      v = val
      if v && spec[:coerce] == Integer
        begin
          v = Integer(v)
        rescue ArgumentError
          raise InvalidArgument, "invalid argument: #{name} #{val}"
        end
      elsif v && spec[:coerce] == Float
        begin
          v = Float(v)
        rescue ArgumentError
          raise InvalidArgument, "invalid argument: #{name} #{val}"
        end
      end
      blk.call(v)
    end
  end
end
