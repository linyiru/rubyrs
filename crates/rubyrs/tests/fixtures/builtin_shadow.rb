# Regression guard for PR #155's toplevel fixed-arity fast path.
#
# `sprintf`, `format`, and `__time_now_raw` are handled by
# `Vm::builtin_call`. They must also appear in `Vm::is_builtin_name`
# so the fast path skips them — otherwise a fixed-arity user `def`
# would be cached and silently shadow the builtin, diverging from
# master's "builtin always wins" dispatch order.
#
# (CRuby would call the user def in all three cases — that is a
# documented, pre-existing divergence tracked separately, not in
# scope for this PR.)
#
# We use `p` to print observation lines so the test is self-contained
# even if `puts` were overridden in a future variant of this fixture.

def sprintf(fmt, x); "USER_SPRINTF" end
def format(fmt, x); "USER_FORMAT" end
def __time_now_raw; "USER_TIME_RAW" end

# Call twice each so the second call exercises the cache path
# (cache_hit on the first `if no_recv` block in do_call).
p sprintf("%d", 5)
p sprintf("%d", 5)
p format("%d", 5)
p format("%d", 5)

# `__time_now_raw` returns a non-deterministic wall-clock pair, so we
# can't compare its value directly; compare its class instead. Builtin
# returns Array (`[secs, nanos]`); a user `def` returning a String
# would flip the class to String and fail this test.
p __time_now_raw.class
p __time_now_raw.class
