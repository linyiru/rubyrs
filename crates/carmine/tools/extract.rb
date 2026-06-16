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

# --- AST→IR compiler (value-token + conditional/stateful blocks) ---
# After the keyword classifier, the big callback families are VALUE-tokens
# (`token Name::Function, m[1]`) and CONDITIONAL/STATEFUL blocks (heredoc
# queues, `state?`-guards, keyword-classifier-with-push). carmine's engine
# runs a Conditional-Action IR (src/ir.rs); this COMPILES a rule block's AST
# directly to that IR — it does NOT run the block, because with conditionals
# the branch taken depends on the match, so running once would be unsound.
# Conditions compile to `if` ops the ENGINE evaluates per match.
#
# SOUNDNESS: the decline boundary IS the compiler — any AST shape it doesn't
# recognize returns nil → the rule stays a `callback` (never wrong). The
# translation faithfully mirrors rouge's semantics (and the Rust twin
# crates/rubyrs/src/rouge_ir.rs, already trusted for fence highlighting):
# m[i]→capture i (0 = whole match), engine `emit` merges adjacent same-type
# tokens exactly like rouge's lex stream, and `self.class.<data>` keyword
# sets resolve to literals (match-independent, like the wordlist hook). Run
# under `--parser=parse.y` (Prism iseqs have no `.of`).
module IrCompiler
  N = RubyVM::AbstractSyntaxTree::Node
  def self.node?(x); x.is_a?(N); end

  # The lexer CLASS being extracted — lets `self.class.keywords`-style
  # CLASS-LEVEL data resolve to literal word lists at extract time.
  class << self; attr_accessor :klass; end

  # block → ops array (JSON-ready), or nil to decline.
  def self.compile(blk)
    scope = begin
      RubyVM::AbstractSyntaxTree.of(blk)
    rescue StandardError
      return nil
    end
    return nil unless node?(scope) && scope.type == :SCOPE
    mvar = (scope.children[0] || [])[0] # the `|m|` match param
    @aliases = {} # leading `name = m[i]` binds (per block)
    body = scope.children[2]
    stmts = ((node?(body) && body.type == :BLOCK) ? body.children : [body]).compact
    out = []
    peeling = true
    stmts.each do |s|
      next if peeling && alias_bind?(s, mvar) # `name = m[i]` → record, drop
      peeling = false
      next if debug_print?(s)
      op = stmt(s, mvar); return nil if op.nil?
      out << op
    end
    out.empty? ? nil : out
  end

  # A leading `lvar = m[i]` pure alias: record `lvar → group i` and drop the
  # binding from the op stream. A non-leading reassignment is not peeled →
  # its LASGN reaches stmt → nil → decline, so an alias is never stale.
  def self.alias_bind?(s, mvar)
    return false unless node?(s) && %i[LASGN DASGN].include?(s.type)
    gi = group_index(s.children[1], mvar)
    return false if gi.nil?
    @aliases[s.children[0]] = gi
    true
  end

  # A body (BLOCK of statements, or a single statement) → [ops] or nil.
  def self.stmt_list(body, mvar)
    stmts = (node?(body) && body.type == :BLOCK) ? body.children : [body]
    out = []
    stmts.each do |s|
      next if s.nil? || debug_print?(s)
      op = stmt(s, mvar)
      return nil if op.nil?
      out << op
    end
    out
  end

  # `puts/p/print/pp …` (optionally `… if @debug`) — stdout diagnostics that
  # never touch the token stream; elided like every native path.
  def self.debug_print?(n)
    return false unless node?(n)
    if %i[IF UNLESS].include?(n.type)
      cond, body, = n.children
      return false unless node?(cond) && cond.type == :IVAR
      ss = ((node?(body) && body.type == :BLOCK) ? body.children : [body]).compact
      return !ss.empty? && ss.all? { |x| print_call?(x) }
    end
    print_call?(n)
  end

  def self.print_call?(n)
    node?(n) && %i[FCALL VCALL].include?(n.type) && %i[puts p print pp].include?(n.children[0])
  end

  def self.stmt(n, mvar)
    return nil unless node?(n)
    case n.type
    when :FCALL, :VCALL, :CALL, :OPCALL
      call_stmt(n, mvar)
    when :IF, :UNLESS
      pred, a, b = n.children
      c = cond(pred, mvar); return nil if c.nil?
      t = branch(a, mvar); return nil if t.nil?
      e = branch(b, mvar); return nil if e.nil?
      ["if", n.type == :UNLESS ? ["not", c] : c, t, e]
    when :IASGN
      v = expr(n.children[1], mvar); return nil if v.nil?
      ["iset", n.children[0].to_s.sub(/\A@/, ""), v]
    end
  end

  # An if/unless branch → [ops]; a nil branch (no else) is empty; failure
  # propagates as nil (distinct from the empty `[]`).
  def self.branch(n, mvar)
    n.nil? ? [] : stmt_list(n, mvar)
  end

  def self.call_stmt(n, mvar)
    # `@ivar << [a, b, …]`
    if n.type == :OPCALL && n.children[1] == :<<
      recv, _op, args = n.children
      return nil unless node?(recv) && recv.type == :IVAR
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :LIST
      tuple = a[0].children.compact.map { |e| expr(e, mvar) }
      return nil if tuple.any?(&:nil?)
      return ["lpush", recv.children[0].to_s.sub(/\A@/, ""), tuple]
    end
    return nil unless %i[FCALL VCALL].include?(n.type) # bare-receiver DSL only
    mid = n.children[0]
    args = n.type == :FCALL ? arglist(n.children[1]) : []
    case mid
    when :token
      return nil unless args.size.between?(1, 2)
      tok = const_qualname(args[0]); return nil if tok.nil?
      return ["token", tok] if args.size == 1
      v = expr(args[1], mvar); return nil if v.nil?
      ["token", tok, v]
    when :groups
      return nil if args.empty?
      toks = args.map { |a| const_qualname(a) }
      toks.any?(&:nil?) ? nil : ["groups", toks]
    when :push
      return ["push", nil] if args.empty?
      (args.size == 1 && sym?(args[0])) ? ["push", args[0].children[0].to_s] : nil
    when :pop!
      return ["pop", 1] if args.empty?
      (args.size == 1 && int?(args[0])) ? ["pop", args[0].children[0]] : nil
    when :goto
      (args.size == 1 && sym?(args[0])) ? ["goto", args[0].children[0].to_s] : nil
    end
  end

  def self.cond(n, mvar)
    return nil unless node?(n)
    return ["ivar", n.children[0].to_s.sub(/\A@/, "")] if n.type == :IVAR
    if %i[FCALL VCALL].include?(n.type) && n.children[0] == :state?
      args = n.type == :FCALL ? arglist(n.children[1]) : []
      return (args.size == 1 && sym?(args[0])) ? ["instate", args[0].children[0].to_s] : nil
    end
    if n.type == :CALL && n.children[1] == :include?
      recv, _mid, args = n.children
      lits = string_set(recv); return nil if lits.nil?
      a = arglist(args); return nil unless a.size == 1
      g = group_index(a[0], mvar); return nil if g.nil?
      return ["gin", g, lits]
    end
    if n.type == :OPCALL && n.children[1] == :==
      recv, _op, args = n.children
      g = group_index(recv, mvar); return nil if g.nil?
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :STR
      return ["geq", g, a[0].children[0]]
    end
    if n.type == :OPCALL && n.children[1] == :!
      inner = cond(n.children[0], mvar)
      return inner.nil? ? nil : ["not", inner]
    end
    nil
  end

  def self.expr(n, mvar)
    return nil unless node?(n)
    # `m[i]` or a bare alias var → capture group value.
    g = group_index(n, mvar); return ["g", g] unless g.nil?
    case n.type
    when :STR then ["lit", n.children[0]]
    when :TRUE then ["bool", true]
    when :FALSE then ["bool", false]
    when :DSTR then interp(n, mvar)
    when :CALL
      if n.children[1] == :include?
        recv, _mid, args = n.children
        lits = string_set(recv); return nil if lits.nil?
        a = arglist(args); return nil unless a.size == 1
        gi = group_index(a[0], mvar); gi.nil? ? nil : ["gin", gi, lits]
      end
    end
  end

  # String interpolation `"a#{m[i]}b…"` → `["cat", parts…]`. Only literal
  # chunks and `#{m[i]}` group interpolations; anything else declines.
  # RubyVM DSTR nests: [head_str, EVSTR | LIST | nested DSTR, …].
  def self.interp(n, mvar)
    parts = []
    return nil unless flatten_dstr(n, mvar, parts)
    parts.empty? ? nil : ["cat", *parts]
  end

  def self.flatten_dstr(n, mvar, parts)
    return false unless node?(n)
    n.children.each do |c|
      case c
      when nil then next
      when String then parts << ["lit", c]
      else
        return false unless node?(c)
        case c.type
        when :STR then parts << ["lit", c.children[0]]
        when :EVSTR
          g = group_index(c.children[0], mvar); return false if g.nil?
          parts << ["g", g]
        when :LIST, :DSTR
          return false unless flatten_dstr(c, mvar, parts)
        else
          return false
        end
      end
    end
    true
  end

  # ---- leaf helpers ----
  def self.arglist(n)
    return [] if n.nil?
    return n.children.compact if node?(n) && n.type == :LIST
    [n]
  end

  def self.sym?(n); node?(n) && n.type == :SYM; end
  def self.int?(n); node?(n) && n.type == :INTEGER; end

  # `m[i]` (or a bare alias var bound to `m[i]`) → capture index i (≥ 0).
  def self.group_index(n, mvar)
    return nil unless node?(n)
    if n.type == :CALL && n.children[1] == :[]
      recv, _mid, args = n.children
      return nil unless node?(recv) && %i[DVAR LVAR].include?(recv.type) && recv.children[0] == mvar
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :INTEGER
      v = a[0].children[0]
      return (v.is_a?(Integer) && v >= 0) ? v : nil
    end
    if %i[DVAR LVAR].include?(n.type) && @aliases&.key?(n.children[0])
      return @aliases[n.children[0]]
    end
    nil
  end

  # An array/`%w` literal of string literals → [String], else nil.
  def self.string_array(n)
    return nil unless node?(n) && n.type == :LIST
    els = n.children.compact
    return nil if els.empty? || !els.all? { |e| node?(e) && e.type == :STR }
    els.map { |e| e.children[0] }
  end

  # A string set in an `include?` receiver position: an inline literal array,
  # OR a CLASS-LEVEL data expression (`self.class.keywords`, a `CONST`) which
  # we evaluate against the lexer class. Sound because such data is
  # match-independent and memoized — the same words the runtime wordlist hook
  # would capture. Non-string / instance-dependent / unresolvable → nil.
  def self.string_set(n)
    return string_array(n) if node?(n) && n.type == :LIST
    v = eval_class_data(n)
    return nil unless v.respond_to?(:to_a)
    a = v.to_a
    (!a.empty? && a.all? { |x| x.is_a?(String) }) ? a : nil
  rescue StandardError
    nil
  end

  # Resolve a match-INDEPENDENT class-data node to its value: `self.class.M`
  # (no-arg accessor) or a bare `CONST`, against the lexer class. Anything
  # instance-dependent (`self.M`) or with arguments is refused (nil).
  def self.eval_class_data(n)
    return nil unless node?(n) && klass
    case n.type
    when :CONST
      klass.const_get(n.children[0])
    when :CALL
      recv, mid, args = n.children
      return nil unless arglist(args).empty?
      return nil unless node?(recv) && recv.type == :CALL && recv.children[1] == :class &&
                        node?(recv.children[0]) && recv.children[0].type == :SELF
      klass.respond_to?(mid) ? klass.public_send(mid) : nil
    end
  rescue StandardError
    nil
  end

  # Resolve a token constant node (`Keyword`, `Name::Builtin`, `Str`) to its
  # rouge qualname via the live token tree (handles aliases like Str →
  # Literal::String). nil if it isn't a resolvable constant path.
  def self.const_qualname(n)
    path = const_path(n)
    return nil if path.nil?
    tok = path.reduce(Rouge::Token::Tokens) { |mod, seg| mod.const_get(seg) }
    tok.respond_to?(:qualname) ? tok.qualname : nil
  rescue StandardError
    nil
  end

  def self.const_path(n)
    return nil unless node?(n)
    case n.type
    when :CONST then [n.children[0]]
    when :COLON3 then [n.children[0]]
    when :COLON2
      parent, name = n.children
      pp = parent.nil? ? [] : const_path(parent)
      pp.nil? ? nil : pp + [name]
    end
  end
end

def try_ir(blk)
  IrCompiler.compile(blk)
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
      @rules << classify_block(re, blk)
    else
      ns = case next_state
           when nil then nil
           when Array then next_state.map(&:to_s)
           else next_state.to_s
           end
      @rules << { kind: "tok", re: re.source, opts: re.options, tok: tok.qualname, next: ns }
    end
  end

  # Classify a rule BLOCK. Only static DSL calls (no match access) record as
  # `actions`. Match-dependent blocks are reached BOTH via `probe.touched?`
  # AND via a Bail (TraceCtx#token Bails on a value token), then upgraded
  # soundly to `wordlist`/`ir` where possible, else left a `callback`.
  def classify_block(re, blk)
    ctx = TraceCtx.new
    probe = MatchProbe.new
    begin
      ctx.instance_exec(probe, &blk)
      unless probe.touched?
        return { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
      end
    rescue StandardError
      # match-dependent → fall through to the sound-upgrade attempts.
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

IrCompiler.klass = lexer # enable self.class.<data> resolution
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
