# perf "boot" workload — isolates BASE / startup RSS.
#
# `Runtime::new` loads every always-on preamble and builds the
# class / method / interner tables; this near-empty program then runs
# and exits. So its peak RSS is essentially the base runtime cost with
# ~zero workload allocation on top.
#
# Why it earns its own baseline row: the 2026-06-29 RSS analysis found
# the JIT-arc's RSS growth was UNIFORM across every workload (even
# fizzbuzz) because it's base cost — rubyrs's own `.text` grew
# 1.7 -> 2.1 MiB, paging in more resident code at startup. A dedicated
# boot row surfaces that base growth in ISOLATION (a normal workload
# row mixes base + per-workload churn), so a future uniform bump shows
# up here first and attributably. Keep this script trivial on purpose.
nil
