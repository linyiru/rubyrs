# GC-rooting regression for BeginBaseline.saved_dollar_bang: the
# Op::EnterBegin snapshot of `$!` must be a GC root — once the inner
# `raise E2` replaces the global, the snapshots are the ONLY refs to
# the E1 instance, and Op::ExitBegin would restore a swept ObjId.
# Reproduces under STRESS_GC=1 as "ICE: class_of called on
# non-Object slot" at the `$!.message` dispatch. The outer rescue is
# deliberately UNBOUND (no `=> e` local) so nothing else roots E1.
class E1 < StandardError; end
class E2 < StandardError; end
begin
  raise E1, "one"
rescue
  begin
    begin
      raise E2, "two"
    rescue
      # allocate hard inside the window where E1 lives ONLY in the
      # BeginBaseline snapshots
      200.times { [1, 2, 3].map { |x| x.to_s * 3 } }
    end
  end
  p $!.message
end
p "done"
