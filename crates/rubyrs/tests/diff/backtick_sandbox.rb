# Backtick / %x on a MISSING bare-word command raises Errno::ENOENT
# on both runtimes (under the CLI's allow_process_spawn capability a
# bare word execs directly, same split as Kernel#system; with the
# capability off the dispatch raises a catchable RuntimeError
# instead — still StandardError, so the probe shape is identical).
# Discovery: P3 Jekyll spike — safe_yaml's libyaml_checker does
# `(`which dpkg` rescue '').empty?`.

# A definitely-absent command raises; a bare `rescue` catches.
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
