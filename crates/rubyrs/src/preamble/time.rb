# Tier 1 `Time` class. Per ADR 0017 row 130 the seeded /
# capability-injected form lives in Tier 1; the wall-clock source
# itself is a host capability (`Config::time_now`) consumed via
# the `__time_now_raw` Kernel primitive defined in `vm/kernel.rs`.
#
# Pure Ruby per the Path A decision documented in
# `perf/time_microbench_results.md` (workload-mix ratio 2.0× vs
# realistic Path B, but the absolute cost stays sub-millisecond
# for the niche workloads rubyrs targets — break-even crosses
# ~10k Time ops per script run, niche scripts make 0-1k).
#
# Loaded unconditionally by `Runtime::load_preamble`. Without a
# host-injected `Config::time_now`, `Time.now` raises
# `RuntimeError` — the deterministic Tier 1 default. Scripts that
# want fixed-clock behavior inject a constant closure; the CLI
# binary injects `SystemTime::now()` so `rubyrs script.rb`
# matches CRuby semantics.

class Time
  include Comparable

  def initialize(sec, nsec = 0)
    unless sec.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{sec.class} into Integer"
    end
    unless nsec.is_a?(Integer)
      raise TypeError, "no implicit conversion of #{nsec.class} into Integer"
    end
    # Normalise nsec into 0..999_999_999, carrying overflow into
    # sec. Matches CRuby's "Time.at(0, 1e10).nsec → 0; sec += 10"
    # behaviour for any int input.
    if nsec >= 1_000_000_000 || nsec < 0
      extra_sec = nsec / 1_000_000_000
      nsec_remainder = nsec - extra_sec * 1_000_000_000
      if nsec_remainder < 0
        extra_sec -= 1
        nsec_remainder += 1_000_000_000
      end
      sec += extra_sec
      nsec = nsec_remainder
    end
    @sec = sec
    @nsec = nsec
  end

  # Class methods. `Time.now` calls into the host-injected
  # capability via the `__time_now_raw` Kernel primitive; if no
  # injection is in effect, `__time_now_raw` itself raises
  # RuntimeError.
  def self.now
    sec, nsec = __time_now_raw
    new(sec, nsec)
  end

  # `Time.at(seconds)` / `Time.at(seconds, subsec)` — entry point
  # matching CRuby's signature. The 2-arg form's subsec is in
  # MICROSECONDS by default (NOT nanoseconds) — CRuby's
  # `Time.at(sec, usec)` shape. The internal `initialize` takes
  # nsec directly; `Time.at` is the public CRuby-compatible
  # adapter that multiplies usec → nsec before delegating.
  #
  # CRuby's full signature is `Time.at(sec, subsec, unit = :usec)`
  # where unit ∈ {:usec, :millisecond, :nsec}; the unit-keyword
  # form is a follow-up. For now the 2-arg form is usec-only.
  def self.at(sec, subsec = nil)
    case sec
    when Time
      # `Time.at(other_time)` returns a fresh copy.
      new(sec.tv_sec, sec.tv_nsec)
    when Integer
      # 2-arg subsec is MICROSECONDS — multiply by 1000 to get
      # nsec for the internal builder. nil subsec → 0.
      new(sec, (subsec || 0) * 1_000)
    when Float
      total_ns = (sec * 1_000_000_000).to_i
      whole_sec = total_ns / 1_000_000_000
      ns_remainder = total_ns - whole_sec * 1_000_000_000
      if ns_remainder < 0
        whole_sec -= 1
        ns_remainder += 1_000_000_000
      end
      new(whole_sec, ns_remainder)
    else
      raise TypeError, "can't convert #{sec.class} into an exact number"
    end
  end

  # Accessors mirroring CRuby's surface. `tv_sec` / `tv_nsec`
  # are the POSIX names; `sec` / `nsec` / `usec` are the
  # human-friendly ones. All are integer-typed.
  def tv_sec; @sec; end
  def tv_nsec; @nsec; end
  alias_method :nsec, :tv_nsec
  def usec; @nsec / 1_000; end

  # Component accessors. `sec` / `min` / `hour` / `day` /
  # `month` / `year` — derived from `@sec` (UTC epoch) via the
  # civil-from-days algorithm below. Tier 1 has no timezone
  # capability, so all components are UTC.
  def year;  decompose[:year];  end
  def month; decompose[:month]; end
  alias_method :mon, :month
  def day;   decompose[:day];   end
  alias_method :mday, :day
  def hour;  decompose[:hour];  end
  def min;   decompose[:min];   end
  def sec;   decompose[:sec];   end
  def wday;  decompose[:wday];  end

  def to_i; @sec; end
  alias_method :to_int, :to_i

  def to_f
    @sec.to_f + @nsec.to_f / 1_000_000_000.0
  end

  # Arithmetic. `Time + n` and `Time - n` (n in seconds, possibly
  # Float) return a new Time. `Time - Time` returns the Float
  # delta in seconds. CRuby distinguishes these by type.
  def +(other)
    if other.is_a?(Time)
      raise TypeError, "no implicit conversion of Time into Integer"
    end
    case other
    when Integer
      Time.new(@sec + other, @nsec)
    when Float
      total_ns = (other * 1_000_000_000).to_i
      extra_sec = total_ns / 1_000_000_000
      extra_ns = total_ns - extra_sec * 1_000_000_000
      Time.new(@sec + extra_sec, @nsec + extra_ns)
    else
      raise TypeError, "can't convert #{other.class} into an exact number"
    end
  end

  def -(other)
    case other
    when Time
      (@sec - other.tv_sec).to_f + (@nsec - other.tv_nsec) / 1_000_000_000.0
    when Integer
      Time.new(@sec - other, @nsec)
    when Float
      total_ns = (other * 1_000_000_000).to_i
      extra_sec = total_ns / 1_000_000_000
      extra_ns = total_ns - extra_sec * 1_000_000_000
      Time.new(@sec - extra_sec, @nsec - extra_ns)
    else
      raise TypeError, "can't convert #{other.class} into an exact number"
    end
  end

  # `<=>` defines the entire ordering surface; Comparable provides
  # `<` / `<=` / `>` / `>=` / `between?` automatically.
  def <=>(other)
    return nil unless other.is_a?(Time)
    sec_cmp = @sec <=> other.tv_sec
    return sec_cmp unless sec_cmp == 0
    @nsec <=> other.tv_nsec
  end

  def ==(other)
    other.is_a?(Time) && @sec == other.tv_sec && @nsec == other.tv_nsec
  end
  alias_method :eql?, :==

  def hash
    # Mix sec and nsec — XOR is order-independent enough for the
    # rare hash-of-Time use case the embed niche actually carries.
    @sec ^ @nsec
  end

  # Identity helpers.
  def utc?; true; end          # Tier 1 is UTC-only
  def gmt?; true; end
  def zone; "UTC"; end
  def utc_offset; 0; end
  alias_method :gmt_offset, :utc_offset
  alias_method :gmtoff, :utc_offset

  # `utc` / `getutc` / `gmtime` — CRuby uses these to convert
  # local-tz Times into the UTC zone for comparable display.
  # Tier 1 has no local timezone (everything is UTC already),
  # so these are no-ops that return `self`. Lets diff_cruby
  # fixtures call `t.utc.to_s` and get byte-identical output
  # across rubyrs / CRuby without writing tz-aware assertions.
  def utc; self; end
  alias_method :getutc, :utc
  alias_method :gmtime, :utc
  alias_method :getgm, :utc

  # Stringification. CRuby's `Time#to_s` default format is local-
  # time `"YYYY-MM-DD HH:MM:SS ±HHMM"`; Tier 1's UTC-only form is
  # `"YYYY-MM-DD HH:MM:SS UTC"`. `inspect` matches `to_s` (CRuby
  # 3.x parity — inspect used to be different but converged).
  def to_s
    d = decompose
    sprintf(
      "%04d-%02d-%02d %02d:%02d:%02d UTC",
      d[:year], d[:month], d[:day], d[:hour], d[:min], d[:sec],
    )
  end
  alias_method :inspect, :to_s

  private

  # Decompose `@sec` (Unix epoch UTC) into year/month/day/hour/
  # min/sec via Howard Hinnant's civil-from-days algorithm. The
  # standard reference: http://howardhinnant.github.io/date_algorithms.html
  # `days_from_civil` for the inverse direction; we use the
  # `civil_from_days` variant. Handles negative epochs (pre-1970)
  # via the floor-divide normalisation.
  def decompose
    secs_in_day = 86_400
    # Floor-divide so pre-1970 timestamps decompose correctly.
    days = @sec / secs_in_day
    seconds_of_day = @sec - days * secs_in_day
    if seconds_of_day < 0
      days -= 1
      seconds_of_day += secs_in_day
    end
    # Day-of-week — 1970-01-01 was a Thursday (wday 4).
    wday = ((days % 7) + 4) % 7

    # Hinnant's civil_from_days. Input: days since 1970-01-01.
    # Output: (year, month, day) with month in 1..12.
    z = days + 719_468
    era = (z >= 0 ? z : z - 146_096) / 146_097
    doe = z - era * 146_097                                    # 0..146_096
    yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365  # 0..399
    y = yoe + era * 400
    doy = doe - (365 * yoe + yoe / 4 - yoe / 100)              # 0..365
    mp = (5 * doy + 2) / 153                                   # 0..11
    d = doy - (153 * mp + 2) / 5 + 1                           # 1..31
    m = mp < 10 ? mp + 3 : mp - 9                              # 1..12
    y += (m <= 2 ? 1 : 0)

    hh = seconds_of_day / 3_600
    rem = seconds_of_day - hh * 3_600
    mm = rem / 60
    ss = rem - mm * 60

    {
      year:  y,
      month: m,
      day:   d,
      hour:  hh,
      min:   mm,
      sec:   ss,
      wday:  wday,
    }
  end
end
