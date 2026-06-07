# `Module#constants` inherit semantics + the NameError that must
# STILL raise when no scope (lexical, ancestor, or toplevel)
# defines the constant.

module RM
  X = 1
end
class RC
  Y = 2
  include RM
end

# inherit=true (default): own + included, own listed first.
p RC.constants            # [:Y, :X]
# inherit=false: own only.
p RC.constants(false)     # [:Y]
# qualified read through the include still resolves.
p RC::X                   # 1

# --- NameError still raises for a truly-undefined constant ---
module NoConst
end
class NeedsNone
  include NoConst
  def f = DEFINITELY_NOT_DEFINED
end
begin
  NeedsNone.new.f
rescue NameError => e
  puts e.message          # "uninitialized constant ...DEFINITELY_NOT_DEFINED"
end

# Qualified miss through an included module that lacks it.
module HasOther
  PRESENT = 1
end
class QC
  include HasOther
end
begin
  QC::ABSENT
rescue NameError => e
  puts e.message          # "uninitialized constant QC::ABSENT"
end
