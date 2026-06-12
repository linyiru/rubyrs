# Kernel#system + backticks behind the allow_process_spawn
# capability (the CLI opts in — this fixture runs under it on both
# runtimes) and the Tempfile pair + `diff -u` pipeline minitest's
# assert_equal failure output is built from.
require "tempfile"
p system("true")
p system("false")
p system("definitely-not-a-cmd-zz")
p system("echo", "direct-form")
p `echo captured`
tool = "diff -u"
Tempfile.open "expect" do |a|
  a.puts "hello\nworld"
  a.flush
  Tempfile.open "butwas" do |b|
    b.puts "hello\nthere"
    b.flush
    d = `#{tool} #{a.path} #{b.path}`
    d.sub!(/^\-\-\- .+/, "--- expected")
    d.sub!(/^\+\+\+ .+/, "+++ actual")
    puts d
  end
end
