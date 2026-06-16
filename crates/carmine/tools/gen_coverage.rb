# Coverage-harness generator (companion to examples/coverage.rs). For every
# rouge lexer, extract its rule table into carmine's JSON format AND record
# rouge's own token stream over the lexer's demo (the golden) + the demo
# text itself. The Rust harness then lexes each demo with carmine and diffs
# against the golden — a measurable carmine↔rouge coverage baseline for the
# drop-in-replacement work. Output is self-contained (no rouge path needed
# by the Rust side).
#
# Run under `--parser=parse.y`: the wordlist upgrade VALIDATES each
# classifier block via `RubyVM::AbstractSyntaxTree.of(block)`, which Ruby
# 3.4's default Prism backend can't provide for compiled iseqs. Without it
# the AST gate returns nil → blocks stay callbacks (still SOUND, just less
# coverage).
#
#   ruby --parser=parse.y crates/carmine/tools/gen_coverage.rb [ROUGE_LIB_DIR]
#   CARMINE_COV_DIR=/tmp/carmine_cov cargo run -p carmine --example coverage
#
# Env: CARMINE_COV_DIR (output dir, default /tmp/carmine_cov). Optional
# ROUGE_LIB_DIR arg prepends a rouge checkout to the load path.
require "json"
$LOAD_PATH.unshift(ARGV[0]) if ARGV[0] && File.directory?(ARGV[0])
require "rouge"
OUT = ENV["CARMINE_COV_DIR"] || "/tmp/carmine_cov"
require "fileutils"; FileUtils.mkdir_p(OUT)

# --- recording DSL (mirrors tools/extract.rb) ---
class TraceCtx
  attr_reader :actions
  class Bail < StandardError; end
  def initialize; @actions = []; end
  def groups(*toks); @actions << ["groups", toks.map(&:qualname)]; end
  def token(tok, val = :__default__); raise Bail unless val == :__default__; @actions << ["token", tok.qualname]; end
  def push(st = :__self__); @actions << ["push", st.to_s]; end
  def pop!(n = 1); @actions << ["pop", n]; end
  def goto(st); @actions << ["goto", st.to_s]; end
  def method_missing(*); raise Bail; end
  def respond_to_missing?(*); true; end
end

# Stand-in for the MatchData `|m|` the rouge block receives. ANY access
# flips `touched` — a block that reads the match is match-dependent and
# must be a `callback` (its emitted tokens vary per match), never recorded
# as static `actions`. Returns benign values so the block runs far enough
# to reveal the access (the recorded actions are discarded once touched).
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
# The most common rouge callback is `do |m| if SET.include?(m[0]) then
# token A elsif SET2.include?(m[0]) then token B else token Name end end`.
# carmine's engine has a native `wordlist` rule (classify the match by set
# membership), so detecting this shape turns a decline into a MATCH.
#
# SOUNDNESS (the drop-in must never diverge): a block is only upgraded if
# EVERY use of `m[0]` is as an `include?` ARGUMENT and it emits exactly one
# `token T` (no value, no groups/push/pop). Enforced by:
#  - WLWord (the value of m[0]) raises on every method → any non-include?
#    use (==, =~, .start_with?, value-token) bails to callback;
#  - the include? hook captures the receiver SETS (returning false so the
#    word's methods are never touched), giving the COMPLETE candidate words
#    via `set.to_a`;
#  - tabulation re-runs the REAL block per candidate word (real sets) so the
#    recorded token is exactly what rouge emits; a non-member probes default.
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
      return false # take the else/next branch so every set is visited
    end
    super
  end
end
[::Array, ::Set, ::Hash].each { |k| k.prepend(WLIncludeHook) }

# m[0] → the word (WL_WORD in discovery, a real String in tabulation);
# any other match access bails (capture groups, pre_match, …).
class WLMatch
  def initialize(w); @w = w; end
  def [](i); i == 0 ? @w : raise(WLBail); end
  def method_missing(*); raise WLBail; end
  def respond_to_missing?(*); true; end
end

# Records the single emitted token; bails on anything else (value-token,
# groups, push, pop, goto …) so only pure classifiers survive.
class WLCtx
  attr_reader :tok
  def token(t, val = :__d__); raise WLBail if val != :__d__ || @tok; @tok = t.qualname; end
  def method_missing(*); raise WLBail; end
  def respond_to_missing?(*); true; end
end

# --- value-token IR (straight-line blocks) ---
# The other big callback family is the VALUE-token rule:
#   do |m| token Name::Function, m[1]; push :params end
# (emit a captured group as a token's text, optionally with state moves).
# carmine's engine already executes a Conditional-Action IR (see src/ir.rs);
# a STRAIGHT-LINE block (no control flow) compiles 1:1 to that IR and runs
# natively — turning the decline into a MATCH.
#
# SOUNDNESS: a value match probe can't bail on truthiness branches
# (`if m[1] …` calls no method on the value), so runtime tracing alone is
# UNSOUND. The gate is therefore the AST (`ClassifierAST.straight_line?`):
# only a block whose body is a flat sequence of token/groups/push/pop!/goto
# calls — with NO IF/CASE/loop/&&/|| anywhere and values restricted to
# `m[N]` / string-literal / `+`-concatenations — is accepted. With no
# control flow, running the block ONCE captures its complete behaviour, so
# the recorded ops are exactly what rouge emits for every input.
class IRBail < StandardError; end

# Wraps an IR value expression. `+` builds a `cat`; any other method bails
# (the AST gate already forbids them, this is defence in depth).
class IRCap
  attr_reader :ir
  def initialize(ir); @ir = ir; end
  def +(other)
    rhs = case other
          when IRCap then other.ir
          when String then ["lit", other]
          else raise IRBail
          end
    base = (@ir.is_a?(Array) && @ir[0] == "cat") ? @ir[1..] : [@ir]
    IRCap.new(["cat", *base, rhs])
  end
  def coerce(*); raise IRBail; end
  def method_missing(*); raise IRBail; end
  def respond_to_missing?(*); false; end
end

# The `|m|` stand-in while building IR: `m[i]` (Integer i ≥ 0) → an IRCap
# for capture group i; anything else bails.
class IRMatch
  def [](i); (i.is_a?(Integer) && i >= 0) ? IRCap.new(["g", i]) : raise(IRBail); end
  def method_missing(*); raise IRBail; end
  def respond_to_missing?(*); true; end
end

# Records the ops a straight-line block performs.
class IRCtx
  attr_reader :ops
  def initialize; @ops = []; end
  def token(t, val = :__whole__)
    @ops << (val == :__whole__ ? ["token", t.qualname] : ["token", t.qualname, to_expr(val)])
  end
  def groups(*toks); @ops << ["groups", toks.map(&:qualname)]; end
  def push(st = :__self__); @ops << ["push", st == :__self__ ? nil : st.to_s]; end
  def pop!(n = 1); @ops << ["pop", n]; end
  def goto(st); @ops << ["goto", st.to_s]; end
  def method_missing(*); raise IRBail; end
  def respond_to_missing?(*); true; end

  private

  def to_expr(v)
    case v
    when IRCap then v.ir
    when String then ["lit", v]
    else raise IRBail
    end
  end
end

def try_ir(blk)
  return nil unless ClassifierAST.straight_line?(blk)
  ctx = IRCtx.new
  begin
    ctx.instance_exec(IRMatch.new, &blk)
  rescue Exception
    return nil
  end
  ops = ctx.ops
  ops.empty? ? nil : ops
end

# --- AST classifier validator (run the tool under `--parser=parse.y`) ---
# Runtime tracing alone is UNSOUND for `m[0] == "lit"` / `case m[0] when
# "lit"` branches: the literal owns the comparison (`"lit" == word` /
# `"lit" === word`), so the word probe never sees it → those words would be
# silently misclassified as default. The AST sees every branch explicitly,
# so we VALIDATE that a block is purely an if/case tree over `m[0]` whose
# conditions are only `==` / `include?` / `when`-literals with single-`token`
# leaves, and COLLECT the `==`/`when` literals (the `include?` sets are still
# captured at runtime). If anything is unrecognized → nil (→ callback). With
# the literals + sets forming a COMPLETE candidate set, the real-block
# tabulation below is sound.
module ClassifierAST
  N = RubyVM::AbstractSyntaxTree::Node
  def self.node?(x); x.is_a?(N); end
  def self.str?(x); node?(x) && x.type == :STR; end

  # The matched word: `m[0]` (CALL :[] on a `params` var, index 0) OR a bare
  # alias var `w` where `w = m[0]` was seen (in `aliases`).
  def self.m0?(n, params, aliases)
    return false unless node?(n)
    return true if %i[DVAR LVAR].include?(n.type) && aliases.include?(n.children[0])
    return false unless n.type == :CALL && n.children[1] == :[]
    recv, _mid, args = n.children
    return false unless node?(recv) && %i[DVAR LVAR].include?(recv.type) && params.include?(recv.children[0])
    a = node?(args) && args.type == :LIST ? args.children.compact : []
    a.size == 1 && node?(a[0]) && a[0].type == :INTEGER && a[0].children[0] == 0
  end

  # A recognized condition over the matched word; collects == literals.
  # `SET.include?(word)` is accepted (set captured at runtime) — SET is any
  # expression. `word == "lit"` / `"lit" == word` collects the literal.
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

  # A leaf must be a single `token X` call (FCALL/VCALL :token).
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
      items = list.children.compact # LIST has a trailing nil terminator
      return false unless items.all? { |e| str?(e) && (lits << e.children[0]) }
      branch(body, params, aliases, lits) && branch(nxt, params, aliases, lits)
    else
      false
    end
  end

  # --- straight-line value-token validator (for try_ir) ---
  # The args of a DSL call: a LIST's non-nil children, a lone node, or none.
  def self.arg_list(n)
    return [] if n.nil?
    return n.children.compact if node?(n) && n.type == :LIST
    [n]
  end

  def self.const_like?(n); node?(n) && %i[CONST COLON2 COLON3].include?(n.type); end
  def self.sym?(n); node?(n) && n.type == :SYM; end
  def self.int?(n); node?(n) && n.type == :INTEGER; end

  # A value expression for `token T, <value>`: `m[N]` (N ≥ 0), a string
  # literal, or a `+`-concatenation of those. NOTHING else (no method calls
  # on the match, no interpolation — those would need transforms the IR
  # can't express, so they stay callbacks).
  def self.value?(n, params)
    return false unless node?(n)
    case n.type
    when :STR then true
    when :CALL
      return false unless n.children[1] == :[]
      recv, _mid, args = n.children
      return false unless node?(recv) && %i[DVAR LVAR].include?(recv.type) && params.include?(recv.children[0])
      a = arg_list(args)
      a.size == 1 && node?(a[0]) && a[0].type == :INTEGER && a[0].children[0] >= 0
    when :OPCALL
      recv, op, args = n.children
      return false unless op == :+
      b = arg_list(args)[0]
      value?(recv, params) && !b.nil? && value?(b, params)
    else
      false
    end
  end

  # A single straight-line statement: token / groups / push / pop! / goto
  # with constant token names and value-only arguments.
  def self.stmt?(n, params)
    return false unless node?(n) && %i[FCALL VCALL].include?(n.type)
    mid = n.children[0]
    args = n.type == :FCALL ? arg_list(n.children[1]) : []
    case mid
    when :token
      return false unless args.size.between?(1, 2) && const_like?(args[0])
      args.size == 1 || value?(args[1], params)
    when :groups
      !args.empty? && args.all? { |a| const_like?(a) }
    when :push
      args.empty? || (args.size == 1 && sym?(args[0]))
    when :goto
      args.size == 1 && sym?(args[0])
    when :pop!
      args.empty? || (args.size == 1 && int?(args[0]))
    else
      false
    end
  end

  # True iff `blk` is a flat sequence of safe DSL statements with NO control
  # flow — so running it once is its complete, input-independent behaviour.
  def self.straight_line?(blk)
    scope = begin
      RubyVM::AbstractSyntaxTree.of(blk)
    rescue StandardError
      return false
    end
    return false unless node?(scope) && scope.type == :SCOPE
    params = Array(scope.children[0])[0, 1]
    body = scope.children[2]
    stmts = (node?(body) && body.type == :BLOCK) ? body.children : [body]
    stmts = stmts.compact
    !stmts.empty? && stmts.all? { |s| stmt?(s, params) }
  end

  # Returns the ==/when literals if `blk` is a validated pure classifier,
  # else nil. nil also when the AST is unavailable (Prism build w/o parse.y).
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
    # Unwrap a BLOCK body's leading `w = m[0]` alias assignments.
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
  # SOUNDNESS gate: only a structurally-validated pure classifier (AST)
  # may be upgraded — this captures the ==/when literals runtime tracing
  # can't see, so the candidate set is COMPLETE.
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
  def initialize; @rules = []; end
  def rule(re, tok = nil, next_state = nil, &blk)
    if blk
      @rules << classify_block(re, blk)
    else
      ns = case next_state when nil then nil when Array then next_state.map(&:to_s) else next_state.to_s end
      @rules << { kind: "tok", re: re.source, opts: re.options, tok: tok.qualname, next: ns }
    end
  end

  # Classify a rule BLOCK. A block that performs only static DSL calls
  # (no match access) records as `actions`. Otherwise it's match-dependent
  # — TraceCtx#token even Bails on a value token (`token T, m[1]`), so the
  # match-dependent path is reached BOTH on `probe.touched?` AND on a Bail.
  # We then try a sound upgrade (wordlist classifier / straight-line IR);
  # anything else stays a `callback`.
  def classify_block(re, blk)
    ctx = TraceCtx.new
    probe = MatchProbe.new
    begin
      ctx.instance_exec(probe, &blk)
      unless probe.touched?
        return { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
      end
    rescue StandardError
      # match-dependent (value token, dynamic state, unmodeled call) → fall
      # through to the sound-upgrade attempts below.
    end
    if (wl = try_wordlist(blk))
      { kind: "wordlist", re: re.source, opts: re.options, sets: wl[:sets], default: wl[:default] }
    elsif (ir = try_ir(blk))
      { kind: "ir", re: re.source, opts: re.options, ops: ir }
    else
      { kind: "callback", re: re.source, opts: re.options }
    end
  end
  def mixin(name); @rules << { kind: "mixin", state: name.to_s }; end
end

def extract_table(lx)
  states = {}
  lx.state_definitions.each do |name, dsl|
    rec = Recorder.new
    rec.instance_eval(&dsl.instance_variable_get(:@defn))
    states[name] = rec.rules
  end
  # `start { push :foo }` blocks set the initial stack above :root. Trace
  # each (no-arg, run on the lexer at reset) for its pushes; ivar inits
  # like `@q = []` are no-ops here (carmine lazily treats ivars as empty).
  # An uncapturable start (calls beyond push/ivar) Bails → empty (best
  # effort; surfaces as a divergence in the harness rather than silently).
  start_push = []
  (lx.start_procs || []).each do |pr|
    ctx = TraceCtx.new
    begin
      ctx.instance_exec(&pr)
      ctx.actions.each { |a| start_push << a[1] if a[0] == "push" }
    rescue StandardError
      # leave whatever pushes were captured before the Bail
    end
  end
  shortnames = {}
  Rouge::Token.each_token { |t| shortnames[t.qualname] = t.shortname }
  { lexer: lx.name, start_push: start_push, states: states, shortnames: shortnames }
end

manifest = []
Rouge::Lexer.all.sort_by(&:tag).each do |lx|
  # rouge's own demo reader; lexers without a demo file raise → skip.
  demo = (lx.demo rescue nil)
  next if demo.nil?
  tag = lx.tag
  rec = { tag: tag, class: lx.name }
  File.write(File.join(OUT, "#{tag}.demo"), demo)
  begin
    table = extract_table(lx)
    rec[:callback_rules] = table[:states].values.flatten.count { |r| r[:kind] == "callback" }
    rec[:total_rules]    = table[:states].values.flatten.size
    File.write(File.join(OUT, "#{tag}.table.json"), JSON.generate(table))
  rescue => e
    rec[:extract_error] = "#{e.class}: #{e.message[0, 60]}"
  end
  begin
    golden = lx.new.lex(demo).map { |tok, val| [tok.qualname, val] }
    File.write(File.join(OUT, "#{tag}.golden.json"), JSON.generate(golden))
    rec[:golden_tokens] = golden.size
  rescue => e
    rec[:golden_error] = "#{e.class}: #{e.message[0, 60]}"
  end
  manifest << rec
end
File.write(File.join(OUT, "manifest.json"), JSON.pretty_generate(manifest))
cb = manifest.count { |r| (r[:callback_rules] || 0) > 0 }
puts "lexers=#{manifest.size} with_callback_rules=#{cb} extract_errors=#{manifest.count { |r| r[:extract_error] }} golden_errors=#{manifest.count { |r| r[:golden_error] }}"
