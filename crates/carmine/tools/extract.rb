# Extract a rouge lexer's state machine into carmine's JSON rule-table
# format. Run against a rouge checkout/gem:
#
#   ruby tools/extract.rb [ROUGE_LIB_DIR] LexerName > python.json
#
# rouge stores each state's definition BLOCK lazily in
# `state_definitions`; we instance_eval those blocks against a recording
# DSL. Declarative rules record (re, token, next). Proc rules are traced
# ONCE against a stub context: if the proc only performs static DSL calls
# (groups / token-without-value / push / pop! / goto with constant args)
# we record the action list — otherwise the rule is marked "callback"
# (match-dependent; carmine delegates those to its Callback hook).
#
# Curated upgrades: the universal identifier-classification idiom
# (`if keywords.include?(m[0]) ...`) is match-dependent but its word sets
# are CLASS-LEVEL DATA — WORDLIST_UPGRADES turns those callbacks into
# native "wordlist" rules.
#
# Tables derived from rouge are subject to rouge's MIT license
# (https://github.com/rouge-ruby/rouge — © Jeanine Adkisson and
# contributors).
require "json"

if ARGV[0] && File.directory?(ARGV[0])
  $LOAD_PATH.unshift(ARGV.shift)
end
require "rouge"

lexer_name = ARGV[0] or abort "usage: extract.rb [ROUGE_LIB] LexerName"
lexer = Rouge::Lexers.const_get(lexer_name)

# state name (string) → { regex source prefix → wordlist spec }.
WORDLIST_UPGRADES = {
  "Python" => {
    root: lambda do |re_source|
      return nil unless re_source.include?('(?<!\.)')
      {
        sets: [
          ["Keyword", Rouge::Lexers::Python.keywords],
          ["Name.Builtin", Rouge::Lexers::Python.exceptions],
          ["Name.Builtin", Rouge::Lexers::Python.builtins],
          ["Name.Builtin.Pseudo", Rouge::Lexers::Python.builtins_pseudo],
        ],
        default: "Name",
      }
    end,
  },
}.freeze

class TraceCtx
  attr_reader :actions
  class Bail < StandardError; end
  def initialize; @actions = []; end
  def groups(*toks); @actions << ["groups", toks.map(&:qualname)]; end
  def token(tok, val = :__default__)
    raise Bail unless val == :__default__ # match-dependent value
    @actions << ["token", tok.qualname]
  end
  def push(st = :__self__); @actions << ["push", st.to_s]; end
  def pop!(n = 1); @actions << ["pop", n]; end
  def goto(st); @actions << ["goto", st.to_s]; end
  def method_missing(*); raise Bail; end
  def respond_to_missing?(*); true; end
end

class Recorder
  attr_reader :rules
  def initialize(lexer_name, state_name)
    @rules = []
    @upgrade = WORDLIST_UPGRADES.dig(lexer_name, state_name)
  end

  def rule(re, tok = nil, next_state = nil, &blk)
    if blk
      if @upgrade && (spec = @upgrade.call(re.source))
        @rules << { kind: "wordlist", re: re.source, opts: re.options,
                    sets: spec[:sets], default: spec[:default] }
        return
      end
      ctx = TraceCtx.new
      begin
        ctx.instance_exec(:__stream_stub__, &blk)
        @rules << { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
      rescue StandardError
        @rules << { kind: "callback", re: re.source, opts: re.options }
      end
    else
      ns = case next_state
           when nil then nil
           when Array then next_state.map(&:to_s)
           else next_state.to_s
           end
      @rules << { kind: "tok", re: re.source, opts: re.options, tok: tok.qualname, next: ns }
    end
  end

  def mixin(name); @rules << { kind: "mixin", state: name.to_s }; end
end

states = {}
lexer.state_definitions.each do |name, dsl|
  defn = dsl.instance_variable_get(:@defn)
  rec = Recorder.new(lexer_name, name)
  rec.instance_eval(&defn)
  states[name] = rec.rules
end

shortnames = {}
Rouge::Token.each_token { |t| shortnames[t.qualname] = t.shortname }

puts JSON.pretty_generate(
  lexer: lexer_name,
  rouge_version: Rouge.version,
  states: states,
  shortnames: shortnames,
)
