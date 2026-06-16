# Extract a rouge lexer's state machine into carmine's JSON rule-table
# format. Run against a rouge checkout/gem:
#
#   ruby --parser=parse.y tools/extract.rb [ROUGE_LIB_DIR] LexerName > python.json
# (parse.y: the wordlist upgrade reads each block's AST via .of, which
#  Ruby 3.4's Prism backend can't provide; without it blocks stay callbacks.)
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

# Stand-in for the MatchData `|m|` a rouge rule block receives. ANY access
# flips `touched` — a block that reads the match is match-dependent (its
# emitted tokens vary per match), so it MUST be a `callback`, never a
# static `actions` list traced down one arbitrary branch. Returns benign
# values so the block runs far enough to reveal the access.
class MatchProbe
  def initialize; @touched = false; end
  def touched?; @touched; end
  def [](*); @touched = true; ""; end
  def to_s; @touched = true; ""; end
  def to_str; @touched = true; ""; end
  def to_ary; @touched = true; []; end
  def method_missing(*); @touched = true; self; end
  def respond_to_missing?(*); true; end
end

# --- generic wordlist (keyword-classifier) upgrade ---
# Detects the universal `do |m| SET.include?(m[0]) ? token A : token Name`
# shape and turns it into carmine's native `wordlist` rule. SOUND by
# construction: only blocks whose EVERY use of `m[0]` is an `include?`
# argument and which emit exactly one valueless `token` are upgraded
# (WLWord raises on any other use → callback); the candidate words come
# from the captured sets' `to_a` (complete), and each word's token is the
# REAL block's output. (Generalises the hand-written WORDLIST_UPGRADES.)
require "set"
class WLBail < StandardError; end
class WLWord < BasicObject
  def method_missing(*); ::Kernel.raise ::WLBail; end
end
WL_WORD = WLWord.new
module WLIncludeHook
  def include?(arg)
    if $wl_capture && arg.equal?(WL_WORD)
      $wl_sets << self
      return false
    end
    super
  end
end
[::Array, ::Set, ::Hash].each { |k| k.prepend(WLIncludeHook) }
class WLMatch
  def initialize(w); @w = w; end
  def [](i); i == 0 ? @w : raise(WLBail); end
  def method_missing(*); raise WLBail; end
  def respond_to_missing?(*); true; end
end
class WLCtx
  attr_reader :tok
  def token(t, val = :__d__); raise WLBail if val != :__d__ || @tok; @tok = t.qualname; end
  def method_missing(*); raise WLBail; end
  def respond_to_missing?(*); true; end
end
# AST classifier validator — closes the runtime hole for `m[0] == "lit"` /
# `case m[0] when "lit"` branches (the literal owns the comparison, so the
# word probe never sees it). Validates a block is purely an if/case tree
# over m[0] (incl. a `w = m[0]` alias) with ==/include?/when conditions and
# single-`token` leaves, collecting the ==/when literals. Needs the tool run
# under `--parser=parse.y` (Prism iseqs have no `.of` AST); without it the
# gate returns nil → blocks stay callbacks (still sound).
module ClassifierAST
  N = RubyVM::AbstractSyntaxTree::Node
  def self.node?(x); x.is_a?(N); end
  def self.str?(x); node?(x) && x.type == :STR; end

  def self.m0?(n, params, aliases)
    return false unless node?(n)
    return true if %i[DVAR LVAR].include?(n.type) && aliases.include?(n.children[0])
    return false unless n.type == :CALL && n.children[1] == :[]
    recv, _mid, args = n.children
    return false unless node?(recv) && %i[DVAR LVAR].include?(recv.type) && params.include?(recv.children[0])
    a = node?(args) && args.type == :LIST ? args.children.compact : []
    a.size == 1 && node?(a[0]) && a[0].type == :INTEGER && a[0].children[0] == 0
  end

  def self.cond?(c, params, aliases, lits)
    return false unless node?(c)
    if c.type == :CALL && c.children[1] == :include?
      _set, _mid, args = c.children
      a = node?(args) && args.type == :LIST ? args.children.compact : []
      return a.size == 1 && m0?(a[0], params, aliases)
    end
    if c.type == :OPCALL && c.children[1] == :==
      a, _op, args = c.children
      b = (node?(args) && args.type == :LIST) ? args.children.compact[0] : nil
      if m0?(a, params, aliases) && str?(b) then lits << b.children[0]; return true end
      if m0?(b, params, aliases) && str?(a) then lits << a.children[0]; return true end
    end
    false
  end

  def self.leaf?(n)
    return true if n.nil?
    return false unless node?(n)
    %i[FCALL VCALL].include?(n.type) && n.children[0] == :token
  end

  def self.branch(n, params, aliases, lits)
    return true if leaf?(n)
    return false unless node?(n)
    case n.type
    when :IF, :UNLESS
      cond, a, b = n.children
      cond?(cond, params, aliases, lits) && branch(a, params, aliases, lits) && branch(b, params, aliases, lits)
    when :CASE, :CASE2
      subj = n.children[0]
      return false unless subj.nil? || m0?(subj, params, aliases)
      n.children[1..].all? { |w| branch(w, params, aliases, lits) }
    when :WHEN
      list, body, nxt = n.children
      return false unless node?(list) && list.type == :LIST
      items = list.children.compact
      return false unless items.all? { |e| str?(e) && (lits << e.children[0]) }
      branch(body, params, aliases, lits) && branch(nxt, params, aliases, lits)
    else
      false
    end
  end

  def self.literals(blk)
    scope = begin
      RubyVM::AbstractSyntaxTree.of(blk)
    rescue StandardError
      return nil
    end
    return nil unless node?(scope) && scope.type == :SCOPE
    param = (scope.children[0] || []).first
    return nil if param.nil?
    params = [param]
    aliases = []
    body = scope.children[2]
    if node?(body) && body.type == :BLOCK
      stmts = body.children
      stmts[0..-2].each do |s|
        return nil unless node?(s) && %i[LASGN DASGN].include?(s.type) && m0?(s.children[1], params, aliases)
        aliases << s.children[0]
      end
      body = stmts.last
    end
    lits = []
    branch(body, params, aliases, lits) ? lits.uniq : nil
  end
end

def try_wordlist(blk)
  ast_lits = ClassifierAST.literals(blk)
  return nil if ast_lits.nil?
  $wl_sets = []
  $wl_capture = true
  dctx = WLCtx.new
  begin
    dctx.instance_exec(WLMatch.new(WL_WORD), &blk)
  rescue Exception
    return nil
  ensure
    $wl_capture = false
  end
  default = dctx.tok
  return nil if default.nil?
  words = ast_lits.dup
  $wl_sets.each do |s|
    return nil unless s.respond_to?(:each)
    s.each { |w| words << w if w.is_a?(String) }
  end
  words.uniq!
  return nil if words.empty?
  order = []
  by_tok = {}
  words.each do |w|
    c = WLCtx.new
    begin
      c.instance_exec(WLMatch.new(w), &blk)
    rescue Exception
      return nil
    end
    return nil if c.tok.nil?
    next if c.tok == default
    (by_tok[c.tok] ||= (order << c.tok; []))
    by_tok[c.tok] << w
  end
  { sets: order.map { |t| [t, by_tok[t]] }, default: default }
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
      probe = MatchProbe.new
      begin
        ctx.instance_exec(probe, &blk)
        if probe.touched?
          if (wl = try_wordlist(blk))
            @rules << { kind: "wordlist", re: re.source, opts: re.options,
                        sets: wl[:sets], default: wl[:default] }
          else
            @rules << { kind: "callback", re: re.source, opts: re.options }
          end
        else
          @rules << { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
        end
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

# `start { push :foo }` initial-stack states (above :root). Trace each
# start proc (no args, run on the lexer at reset) for its pushes; ivar
# inits are no-ops here. carmine applies these in `Lexer::begin`.
start_push = []
(lexer.start_procs || []).each do |pr|
  ctx = TraceCtx.new
  begin
    ctx.instance_exec(&pr)
    ctx.actions.each { |a| start_push << a[1] if a[0] == "push" }
  rescue StandardError
    # keep pushes captured before the Bail
  end
end

shortnames = {}
Rouge::Token.each_token { |t| shortnames[t.qualname] = t.shortname }

puts JSON.pretty_generate(
  lexer: lexer_name,
  rouge_version: Rouge.version,
  start_push: start_push,
  states: states,
  shortnames: shortnames,
)
