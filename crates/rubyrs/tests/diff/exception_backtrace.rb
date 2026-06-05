# Exception#backtrace — Array<String> of frames captured at the
# raise site. Populated by `Vm::unwind_with_exception` from the
# live frame stack on first unwind hop; survives rescue and is
# readable via `e.backtrace`.
#
# Tests structural properties (presence, class, ordering,
# content matchers) rather than byte-identical frame strings
# because CRuby renders `'Object#f'` qualified method names
# while rubyrs renders `'f'` (proto.name vs proto.defining_class
# walk — documented divergence in SUBSET.md).

# 1. Basic capture — `raise "msg"` from inside a method records
# a non-nil Array. Innermost frame first.
def f1
  raise "boom1"
end
begin
  f1
rescue => e
  puts "is_array=#{e.backtrace.is_a?(Array)}"
  puts "nonempty=#{!e.backtrace.empty?}"
  puts "frame_class=#{e.backtrace.first.class}"
  puts "contains_method=#{e.backtrace.first.include?('f1')}"
  puts "contains_filename=#{e.backtrace.first.include?('exception_backtrace.rb')}"
end

# 2. Multi-frame — innermost (raise site) first, outermost
# (main) last. Verify both ends.
def deep_a
  raise StandardError, "deep"
end
def deep_b
  deep_a
end
def deep_c
  deep_b
end
begin
  deep_c
rescue => e
  bt = e.backtrace
  puts "depth=#{bt.length}"
  puts "innermost_has_a=#{bt[0].include?('deep_a')}"
  puts "next_has_b=#{bt[1].include?('deep_b')}"
  puts "third_has_c=#{bt[2].include?('deep_c')}"
end

# 3. Re-raise from inside rescue preserves the original
# backtrace (CRuby semantics — `raise` with no args inside a
# rescue body re-raises `$!` unchanged). Use a global to
# stash the inner exception across the def boundary — toplevel
# ivars vs nested-method-body ivars have a documented gap.
$captured = nil
def reraiser
  begin
    raise "orig"
  rescue => e
    $captured = e
    raise
  end
end
begin
  reraiser
rescue => e
  inner_bt = $captured.backtrace
  outer_bt = e.backtrace
  puts "reraise_same_bt=#{inner_bt == outer_bt}"
end

# 4. Exception constructed but never raised has nil backtrace —
# matches CRuby (`.new`-only exceptions carry no frame info).
e_synth = StandardError.new("never raised")
puts "synth_bt=#{e_synth.backtrace.inspect}"

# 5. Caller-supplied / VM-generated traps also carry backtrace
# (TypeError raised by primitive ops, ZeroDivisionError, etc.).
begin
  1 / 0
rescue => e
  puts "vm_trap_class=#{e.class}"
  puts "vm_trap_has_bt=#{e.backtrace.is_a?(Array) && !e.backtrace.empty?}"
end

# 6. Exception from inside a block — the captured frames include
# the block frame, not just the method frame.
def with_block
  [1, 2, 3].each do |x|
    raise "in block: #{x}"
  end
end
begin
  with_block
rescue => e
  puts "block_bt_has_with_block=#{e.backtrace.any? { |f| f.include?('with_block') }}"
end

# 7. full_message uses backtrace — verify the head line has the
# trap-line shape `path:line:in '...': msg (Class)` and at least
# one `\tfrom ...` continuation when there are multiple frames.
def fm1
  raise "fm_msg"
end
def fm2
  fm1
end
begin
  fm2
rescue => e
  fm = e.full_message(highlight: false)
  puts "fm_has_msg=#{fm.include?('fm_msg (RuntimeError)')}"
  puts "fm_has_from=#{fm.include?("\tfrom ")}"
  puts "fm_ends_newline=#{fm.end_with?("\n")}"
end
