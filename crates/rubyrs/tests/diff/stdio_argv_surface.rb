# $stdout / STDOUT IO veneer + ARGV. stdout-only (stderr interleaves
# differently under pipe buffering and would flake the diff).
p ARGV
$stdout.puts "via $stdout"
STDOUT.print "no", "newline"
STDOUT.puts
$stdout.write("w1", "w2\n")
$stdout << "chained" << "\n"
p $stdout.sync
$stdout.sync = true
p $stdout.sync
p STDOUT.tty?
p STDOUT.fileno
p STDERR.fileno
$stdout.printf("%05d|%s\n", 42, "fmt")
$stdout.puts ["arr1", "arr2"]
$stdout.puts []
p $stdout.is_a?(IO)
p $stdout.equal?(STDOUT)
