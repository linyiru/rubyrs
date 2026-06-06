# Backtick / %x command execution compiles but raises a StandardError
# at runtime (rubyrs is a Tier-1 sandbox — no subprocess). A bare
# `rescue` catches it, just as CRuby's Errno::ENOENT is caught when a
# command is absent — so guarded probes degrade identically. We assert
# only the rescued VALUE (parity-safe); the raised error CLASS differs
# across runtimes and isn't asserted. Discovery: P3 Jekyll spike —
# safe_yaml's libyaml_checker does `(`which dpkg` rescue '').empty?`.

# A definitely-absent command: CRuby raises Errno::ENOENT, rubyrs
# raises RuntimeError — both StandardError, both caught.
p((`this_command_truly_does_not_exist_xyz_42` rescue "rescued"))
p((`this_command_truly_does_not_exist_xyz_42` rescue "").empty?)

# %x{...} form compiles the same way.
p((%x{this_command_truly_does_not_exist_xyz_42} rescue :handled))

# the rescue flows into normal control flow
result = begin
  `nope_nope_nope_xyz`
  "ran"
rescue StandardError
  "fell-back"
end
p result
