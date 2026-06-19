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
    # CRuby's Time.at / Time.now / Time.parse return LOCAL-flavoured
    # times (under TZ=UTC: `to_s` "+0000", `utc?` false); only
    # explicit `.utc` / `Time.utc` carry the UTC flavour. Tier 1 has
    # one clock (UTC) but tracks the FLAVOUR so formatting matches
    # TZ=UTC CRuby byte-for-byte. See `localtime` / `utc`.
    @local = true
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

  # `Time.utc(year, month=1, day=1, hour=0, min=0, sec=0)` — civil
  # fields → Time, via days_from_civil. Fractional/string seconds
  # and the 10-arg `Time.gm` form are out of scope. `Time.local` /
  # `Time.mktime` alias to it: Tier-1 time is UTC-only (the TZ=UTC
  # CRuby flavour), so local-zone construction IS utc construction
  # here — documented divergence for hosts in other zones
  # (minitest's skip_until/fail_after only compare dates, where
  # this is exact).
  # `Time.local` keeps the local FLAVOUR (under TZ=UTC CRuby still
  # renders it "+0000"); `Time.utc` carries the UTC flavour
  # (renders "UTC", utc? true). Same single clock either way —
  # only the flavour bit differs (see initialize).
  def self.local(year, month = 1, day = 1, hour = 0, min = 0, sec = 0)
    days = days_from_civil(year, month, day)
    new(days * 86_400 + hour * 3600 + min * 60 + sec, 0)
  end

  def self.utc(year, month = 1, day = 1, hour = 0, min = 0, sec = 0)
    t = local(year, month, day, hour, min, sec)
    t.instance_variable_set(:@local, false)
    t
  end

  class << self
    alias_method :gm, :utc
    alias_method :mktime, :local
  end

  # Howard Hinnant's `days_from_civil` — the inverse of `decompose`'s
  # `civil_from_days`. Returns days since 1970-01-01 for a proleptic
  # Gregorian (year, month 1..12, day) triple, correct for pre-1970
  # dates too. Used by `Time.parse` to turn a parsed date back into a
  # Unix timestamp.
  def self.days_from_civil(y, m, d)
    y -= 1 if m <= 2
    era = (y >= 0 ? y : y - 399) / 400
    yoe = y - era * 400
    mp = m > 2 ? m - 3 : m + 9
    doy = (153 * mp + 2) / 5 + d - 1
    doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
    era * 146_097 + doe - 719_468
  end

  # `Time.parse(str)` — a focused parser for the ISO-8601-ish date /
  # datetime shapes the front-matter / filename-date world uses:
  # `YYYY-MM-DD`, optionally `[ T]HH:MM[:SS]`, optionally a `Z` /
  # `±HH:MM` / `±HHMM` offset. Tier 1 is UTC-only, so an offset is
  # normalised to UTC at parse time and the result carries no zone of
  # its own. Bad input raises ArgumentError (CRuby's contract, which
  # jekyll's `Utils.parse_date` rescues). The full `Time.parse`
  # natural-language surface (weekday names, `now`-relative fills) is
  # out of scope. Discovery: P3 Jekyll spike — `Utils.parse_date`
  # does `Time.parse(input).localtime`.
  # Hand-rolled (no Regexp) so this loads on no-`regex`-feature builds
  # too — the preamble must compile without the regex Cargo feature
  # (wasm32-wasip1 / Tier-1 minimal), and a `/.../` literal there is a
  # load-time SyntaxError.
  # `Time.httpdate(str)` — parse the RFC 7231 IMF-fixdate shape
  # Time#httpdate emits ("Sun, 06 Nov 1994 08:49:37 GMT"). Strict
  # on that one format, like CRuby's lib/time.rb (which also
  # accepts the obsolete RFC 850/asctime forms — out of subset
  # until a consumer needs them).
  #
  # String-walk parser, NO regex literal: the Time preamble loads
  # unconditionally, and a regex literal here breaks every
  # no-`regex`-feature build at boot (the wasm32-wasip1 CI smoke
  # caught exactly that).
  HTTPDATE_DAYS = %w[Sun, Mon, Tue, Wed, Thu, Fri, Sat,].freeze

  def self.httpdate(str)
    parts = str.to_s.split(" ")
    bad = lambda { raise ArgumentError, "not RFC 2616 compliant date: #{str.inspect}" }
    bad.call unless parts.length == 6 && parts[5] == "GMT"
    bad.call unless HTTPDATE_DAYS.include?(parts[0])
    digits = lambda do |s2, n|
      s2.length == n && s2.each_char.all? { |c| c >= "0" && c <= "9" }
    end
    day, mon_abbr, year, hms = parts[1], parts[2], parts[3], parts[4]
    mon = MONTH_ABBR.index(mon_abbr)
    bad.call unless mon && digits.call(day, 2) && digits.call(year, 4)
    hh, mm, ss = hms.split(":")
    bad.call unless ss && digits.call(hh, 2) && digits.call(mm, 2) && digits.call(ss, 2)
    utc(year.to_i, mon + 1, day.to_i, hh.to_i, mm.to_i, ss.to_i)
  end

  # `Time.rfc2822(str)` / `Time.rfc822` — parse the RFC 2822 date the
  # `#rfc2822` formatter emits ("Sat, 13 Jun 2026 19:00:10 +0000") and,
  # since the day-of-week prefix is optional and the zone may be GMT,
  # the RFC 7231 httpdate shape too. rack's ConditionalGet parses both
  # If-Modified-Since and Last-Modified with `Time.rfc2822(...) rescue
  # nil`, so a missing method silently disabled every 304. NO regex
  # literal: the Time preamble loads unconditionally and a literal
  # would break the no-`regex` / wasm builds (CI smoke caught this for
  # httpdate) — the zone is parsed with string ops.
  def self.rfc2822(str)
    parts = str.to_s.split(" ")
    # Drop an optional leading "Dow," token.
    parts.shift if parts[0] && parts[0].end_with?(",")
    bad = lambda { raise ArgumentError, "not RFC 2822 compliant date: #{str.inspect}" }
    bad.call unless parts.length >= 5
    day, mon_abbr, year, hms, zone = parts[0], parts[1], parts[2], parts[3], parts[4]
    mon = MONTH_ABBR.index(mon_abbr)
    bad.call unless mon
    hh, mm, ss = hms.to_s.split(":")
    bad.call if hh.nil? || mm.nil?
    t = utc(year.to_i, mon + 1, day.to_i, hh.to_i, mm.to_i, (ss || "0").to_i)
    # Numeric "+HHMM" / "-HHMM" offset → shift to UTC. GMT / UTC / Z /
    # +0000 are zero. Parsed without a regex (see note above).
    if zone && zone.length == 5 && (zone[0] == "+" || zone[0] == "-") &&
       zone[1..].each_char.all? { |c| c >= "0" && c <= "9" }
      off = zone[1, 2].to_i * 3600 + zone[3, 2].to_i * 60
      off = -off if zone[0] == "+" # east of UTC: subtract to normalise
      t += off
    end
    t
  end

  def self.parse(str, _now = nil)
    s = str.to_s.strip
    # Host fast path (`__rubyrs_time_parse_iso`, vm/kernel.rs):
    # strictly-digit ISO shapes compute epoch seconds natively with
    # the identical days_from_civil arithmetic; anything it isn't
    # certain about returns nil and the Ruby parser below stays the
    # source of truth (to_i quirks, junk suffixes, huge years).
    fast = __rubyrs_time_parse_iso(s)
    return new(fast, 0) if fast
    # RFC 2822 / RFC 7231 httpdate shapes carry a month NAME ("Jun")
    # rather than the ISO numeric `-` layout — e.g.
    # "Sat, 13 Jun 2026 22:00:05 GMT". rack's Response cache helpers
    # write `Time.now.httpdate` into a header then re-`Time.parse` it,
    # so without this `Time.parse` raised "no time information" on its
    # own output. Delegate to the rfc2822 parser (which also accepts
    # the httpdate variant) whenever a month-name token is present; it
    # raises ArgumentError on a true mismatch, which propagates as
    # CRuby's would.
    if s.split(" ").any? { |tk| MONTH_ABBR.include?(tk) }
      return rfc2822(s)
    end
    # Leniency (CRuby's Date._parse scans for a timestamp ANYWHERE in
    # the string, ignoring surrounding text). When the string doesn't
    # already start with an ISO date, extract the first embedded
    # `YYYY-MM-DD[ HH:MM:SS [zone]]` run and parse that — e.g. a Struct's
    # `"#<struct to_time=2026-06-18 22:45:32 +0000>"` (Sinatra's
    # `time_for` does `Time.parse(value.to_s)` on arbitrary objects).
    unless s =~ /\A\d{4}-\d{2}-\d{2}/
      m = s.match(/\d{4}-\d{2}-\d{2}(?:[ T]\d{2}:\d{2}:\d{2}(?:\s*[+-]\d{2}:?\d{2}|\s*Z)?)?/)
      raise ArgumentError, "no time information in #{str.inspect}" unless m
      s = m[0]
    end
    # Split the date from the time/zone at the first space or 'T'.
    sep = nil
    i = 0
    while i < s.length
      c = s[i]
      if c == " " || c == "T"
        sep = i
        break
      end
      i += 1
    end
    date_str = sep ? s[0...sep] : s
    rest = sep ? s[(sep + 1)..].to_s.strip : ""
    d = date_str.split("-")
    if d.length < 3 || d[0].empty? || d[1].empty? || d[2].empty?
      raise ArgumentError, "no time information in #{str.inspect}"
    end
    year  = d[0].to_i
    month = d[1].to_i
    day   = d[2].to_i
    hour = 0
    minute = 0
    second = 0
    off = 0
    unless rest.empty?
      # Peel a trailing zone: `Z`, or `±HHMM` / `±HH:MM`.
      if rest.end_with?("Z")
        rest = rest[0...-1].strip
      else
        tzpos = nil
        j = rest.length - 1
        while j >= 0
          ch = rest[j]
          if ch == "+" || ch == "-"
            tzpos = j
            break
          end
          j -= 1
        end
        if tzpos
          tz = rest[tzpos..]
          rest = rest[0...tzpos].strip
          sign = tz[0] == "-" ? -1 : 1
          body = tz[1..].to_s
          oh = body[0, 2].to_i
          mm_at = body[2] == ":" ? 3 : 2
          om = body[mm_at, 2].to_i
          off = sign * (oh * 3600 + om * 60)
        end
      end
      t = rest.split(":")
      hour   = (t[0] || "0").to_i
      minute = (t[1] || "0").to_i
      second = (t[2] || "0").to_i
    end
    total = days_from_civil(year, month, day) * 86_400 +
            hour * 3600 + minute * 60 + second - off
    new(total, 0)
  end

  # Tier-1 UTC-only: `#localtime` / `#getlocal` have no separate local
  # zone to convert into, so they return self (the time value is
  # unchanged; only the would-be zone label differs, and we don't
  # model zones). Accept and ignore an explicit-offset argument.
  # Tier 1 has no local timezone, so "converting" can't change the
  # clock value — but CRuby distinguishes the LOCAL flavour from the
  # UTC flavour even when TZ=UTC (utc? flips false, to_s renders
  # "+0000", xmlschema renders "+00:00" instead of "Z"). Jekyll's
  # date filters route every time through `.localtime`, so matching
  # the flavour is what keeps TZ=UTC CRuby builds byte-identical.
  # Explicit copy constructors: jekyll's date filters call
  # `time.dup.localtime` / `input.clone` on every render. (Object#dup
  # doesn't dispatch for preamble-class instances — VM gap; explicit
  # defs sidestep it.)
  def dup
    t = self.class.new(@sec, @nsec)
    t.instance_variable_set(:@local, @local)
    t
  end
  alias_method :clone, :dup

  def localtime(*)
    return self if @local
    t = self.class.new(@sec, @nsec)
    t.instance_variable_set(:@local, true)
    t
  end
  alias_method :getlocal, :localtime

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
  # Day of the year, 1..366 (reuses the strftime %j helper).
  def yday
    d = decompose
    __strftime_day_of_year(d[:year], d[:month], d[:day])
  end
  # Day-of-week predicates (wday: Sunday == 0 .. Saturday == 6).
  def sunday?;    wday == 0; end
  def monday?;    wday == 1; end
  def tuesday?;   wday == 2; end
  def wednesday?; wday == 3; end
  def thursday?;  wday == 4; end
  def friday?;    wday == 5; end
  def saturday?;  wday == 6; end
  # Fractional seconds: 0 (Integer) for a whole second, else a Rational
  # of nanoseconds — matching CRuby.
  def subsec
    @nsec == 0 ? 0 : Rational(@nsec, 1_000_000_000)
  end
  # `[sec, min, hour, mday, mon, year, wday, yday, isdst, zone]`.
  # Tier 1 is UTC-only with no DST, so isdst is false and zone "UTC".
  def to_a
    d = decompose
    [d[:sec], d[:min], d[:hour], d[:day], d[:month], d[:year], d[:wday], yday, false, zone]
  end

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
  def utc?; !@local; end       # Tier 1 is UTC-only; see localtime
  def gmt?; !@local; end
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
  def utc
    return self unless @local
    t = self.class.new(@sec, @nsec)
    t.instance_variable_set(:@local, false)
    t
  end
  alias_method :getutc, :utc
  alias_method :gmtime, :utc
  alias_method :getgm, :utc

  # CRuby `Time#to_time` returns self (it exists so Date/DateTime/
  # Time share a coercion protocol — Liquid's `to_date` and Jekyll's
  # date filters duck-type on it).
  def to_time; self; end

  # RFC 3339 / ISO 8601 timestamp. CRuby renders the UTC zone as
  # "Z" (local zones as "+HH:MM"); Tier 1 is UTC-only so the zone
  # suffix is always "Z". `fraction_digits` appends ".%N"-style
  # subsecond digits, exactly as CRuby does.
  def xmlschema(fraction_digits = 0)
    d = decompose
    out = sprintf(
      "%04d-%02d-%02dT%02d:%02d:%02d",
      d[:year], d[:month], d[:day], d[:hour], d[:min], d[:sec],
    )
    if fraction_digits > 0
      frac = sprintf("%09d", @nsec)[0, fraction_digits]
      out << "." << frac
    end
    out << (@local ? "+00:00" : "Z")
  end
  alias_method :iso8601, :xmlschema

  # `require "time"` surface (CRuby defines these in lib/time.rb;
  # rubyrs's `require "time"` lands in the lenient-stub path so the
  # methods live here unconditionally). Tier-1 time is UTC-only,
  # which collapses both forms:
  #   - httpdate (RFC 7231 IMF-fixdate) always renders the GMT
  #     shape — CRuby calls getutc first, which is a no-op here.
  #   - rfc2822 renders "-0000" for UTC receivers (RFC 2822's
  #     "UTC, local offset unknown" zone), the only kind Tier 1
  #     produces. CRuby prints "+HHMM" for genuinely-local times;
  #     that branch is unreachable until local zones land.
  def httpdate
    strftime("%a, %d %b %Y %H:%M:%S GMT")
  end

  def rfc2822
    strftime("%a, %d %b %Y %H:%M:%S -0000")
  end
  alias_method :rfc822, :rfc2822

  # Stringification. CRuby's `Time#to_s` default format is local-
  # time `"YYYY-MM-DD HH:MM:SS ±HHMM"`; Tier 1's UTC-only form is
  # `"YYYY-MM-DD HH:MM:SS UTC"`. `inspect` matches `to_s` (CRuby
  # 3.x parity — inspect used to be different but converged).
  #
  # Memoized: Tier-1 Time is immutable (@sec/@nsec/@local never
  # change after initialize — localtime/utc return COPIES), so the
  # rendered form is a per-object constant. Jekyll's `merge_data!`
  # calls `data["date"].to_s` 3-4x per document (merge_date! →
  # parse_date cache key), which made the full decompose+sprintf
  # (~3.3µs vs CRuby's 0.5µs C path) a measured read-phase cost.
  # CRuby returns a FRESH unfrozen string each call (callers may
  # legitimately mutate it), so the memo is handed out as a `dup`
  # — still ~50x cheaper than recomputing. No frozen-receiver
  # guard: rubyrs `freeze` doesn't block ivar writes on user
  # objects (documented freeze gap, SUBSET.md) — if deep freeze
  # is ever modelled, the memo writes here and in `decompose`
  # need a `frozen?` bypass.
  def to_s
    memo = @__to_s
    return memo.dup if memo
    d = decompose
    s = sprintf(
      "%04d-%02d-%02d %02d:%02d:%02d ",
      d[:year], d[:month], d[:day], d[:hour], d[:min], d[:sec],
    )
    s << (@local ? "+0000" : "UTC")
    @__to_s = s
    s.dup
  end
  alias_method :inspect, :to_s

  # English locale constants for `%A` / `%a` / `%B` / `%b`
  # directives. CRuby's strftime is locale-aware via `LC_TIME`;
  # Tier 1 is C-locale-only (ADR 0017 Rule 1: no host locale
  # peek). These arrays match the English / `LC_ALL=C` shape.
  DAY_NAMES = [
    "Sunday", "Monday", "Tuesday", "Wednesday",
    "Thursday", "Friday", "Saturday",
  ]
  DAY_ABBR = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
  MONTH_NAMES = [
    "January", "February", "March", "April",
    "May", "June", "July", "August",
    "September", "October", "November", "December",
  ]
  MONTH_ABBR = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
  ]

  # `strftime(fmt)` — printf-style time formatter. Implements a
  # useful subset of CRuby's directives; unknown directives pass
  # through verbatim (matching CRuby's lenient default rather
  # than the strict-error mode some libraries layer on top).
  #
  # Supported:
  #   - Numerics: %Y %C %y %m %d %e %H %k %I %l %M %S %j %w %u %s
  #   - Sub-second: %N (with width %3N / %6N / %9N), %L
  #   - Names: %A %a %B %b %h %p %P
  #   - Composites: %F %T %X %R %D %x %r %v %c
  #   - Timezone (UTC-only Tier 1): %z (+0000), %:z (+00:00),
  #     %::z (+00:00:00), %Z (UTC)
  #   - Literals: %% %n %t
  #
  # Padding flags before the directive:
  #   - `-` no padding
  #   - `0` zero padding (default for most numerics)
  #   - `_` space padding (default for %e, %k, %l)
  #   - `^` uppercase the directive's output
  #
  # Width: optional digits between flag and directive (e.g.
  # `%5Y` width-5 year, `%3N` truncated-to-milliseconds nsec).
  def strftime(fmt)
    d = decompose
    # Single-directive fast path. Jekyll's UrlDrop renders every
    # date placeholder through a one-directive format (`%Y`, `%m`,
    # `%-d`, `%b`, ...) — once per placeholder per document URL —
    # and the full parser loop below costs ~6µs/call interpreted
    # (measured 30× CRuby's C strftime). The length gate keeps
    # composite formats ("%Y-%m-%d") at zero extra cost; matched
    # singles skip the parser entirely. Semantics are pinned by the
    # existing time_strftime fixture (the arms mirror the loop's
    # zero-pad / no-pad / name-table behaviour exactly).
    if fmt.length < 4
      case fmt
      when "%Y" then return format("%04d", d[:year])
      when "%m" then return format("%02d", d[:month])
      when "%d" then return format("%02d", d[:day])
      when "%H" then return format("%02d", d[:hour])
      when "%M" then return format("%02d", d[:min])
      when "%S" then return format("%02d", d[:sec])
      when "%y" then return format("%02d", d[:year] % 100)
      when "%-d" then return d[:day].to_s
      when "%-m" then return d[:month].to_s
      when "%b", "%h" then return MONTH_ABBR[d[:month] - 1]
      when "%B" then return MONTH_NAMES[d[:month] - 1]
      when "%a" then return DAY_ABBR[d[:wday]]
      when "%A" then return DAY_NAMES[d[:wday]]
      end
    end
    out = String.new
    i = 0
    len = fmt.length
    while i < len
      ch = fmt[i]
      if ch != "%"
        out << ch
        i += 1
        next
      end
      # Past the `%`; parse optional flag.
      i += 1
      flag = nil
      if i < len
        case fmt[i]
        when "-" then flag = :nopad;    i += 1
        when "0" then flag = :zeropad;  i += 1
        when "_" then flag = :spacepad; i += 1
        when "^" then flag = :upper;    i += 1
        end
      end
      # Parse optional width (decimal digits).
      width = 0
      width_str = String.new
      while i < len
        c = fmt[i]
        break unless c >= "0" && c <= "9"
        width_str << c
        i += 1
      end
      width = width_str.to_i unless width_str.empty?
      # Need a directive next.
      if i >= len
        out << "%"
        break
      end
      dir = fmt[i]
      i += 1
      appended = case dir
      when "%" then "%"
      when "n" then "\n"
      when "t" then "\t"
      when "Y" then __strftime_pad_num(d[:year],  4, flag, width)
      when "C" then __strftime_pad_num(d[:year] / 100, 2, flag, width)
      when "y" then __strftime_pad_num(d[:year] % 100, 2, flag, width)
      when "m" then __strftime_pad_num(d[:month], 2, flag, width)
      when "d" then __strftime_pad_num(d[:day],   2, flag, width)
      when "e" then __strftime_pad_num(d[:day],   2, flag || :spacepad, width)
      when "H" then __strftime_pad_num(d[:hour],  2, flag, width)
      when "k" then __strftime_pad_num(d[:hour],  2, flag || :spacepad, width)
      when "I"
        h12 = d[:hour] % 12
        h12 = 12 if h12 == 0
        __strftime_pad_num(h12, 2, flag, width)
      when "l"
        h12 = d[:hour] % 12
        h12 = 12 if h12 == 0
        __strftime_pad_num(h12, 2, flag || :spacepad, width)
      when "M" then __strftime_pad_num(d[:min],   2, flag, width)
      when "S" then __strftime_pad_num(d[:sec],   2, flag, width)
      when "j"
        doy = __strftime_day_of_year(d[:year], d[:month], d[:day])
        __strftime_pad_num(doy, 3, flag, width)
      when "w" then d[:wday].to_s
      when "u" then (d[:wday] == 0 ? 7 : d[:wday]).to_s
      when "s" then @sec.to_s
      when "N"
        # Nanoseconds with optional width (default 9). Width <
        # 9 truncates the right (e.g. `%3N` = milliseconds);
        # width > 9 right-pads with zeros.
        w = width > 0 ? width : 9
        full = sprintf("%09d", @nsec)
        if w >= 9
          full + ("0" * (w - 9))
        else
          full[0, w]
        end
      when "L" then sprintf("%03d", @nsec / 1_000_000)
      when "p" then d[:hour] < 12 ? "AM" : "PM"
      when "P" then d[:hour] < 12 ? "am" : "pm"
      when "A" then DAY_NAMES[d[:wday]]
      when "a" then DAY_ABBR[d[:wday]]
      when "B" then MONTH_NAMES[d[:month] - 1]
      when "b", "h" then MONTH_ABBR[d[:month] - 1]
      when "z" then "+0000"
      when "Z" then "UTC"
      when ":"
        # `%:z` and `%::z` — extended zone forms. Tier 1 is
        # UTC-only so the offset string is fixed.
        if i < len && fmt[i] == "z"
          i += 1
          "+00:00"
        elsif i + 1 < len && fmt[i] == ":" && fmt[i + 1] == "z"
          i += 2
          "+00:00:00"
        else
          # Malformed — pass the `%:` through.
          "%:"
        end
      when "F" then strftime("%Y-%m-%d")
      when "T", "X" then strftime("%H:%M:%S")
      when "R" then strftime("%H:%M")
      when "D", "x" then strftime("%m/%d/%y")
      when "r" then strftime("%I:%M:%S %p")
      when "v" then strftime("%e-%^b-%Y")  # VMS date — uppercases the month abbr
      when "c" then strftime("%a %b %e %H:%M:%S %Y")
      else
        # Unknown directive — CRuby passes the original `%X`
        # through verbatim. We do the same; flags/width on
        # unknown directives are dropped (the `%X` reconstruction
        # ignores them — same as CRuby's behaviour).
        "%" + dir
      end
      appended = appended.upcase if flag == :upper
      out << appended
    end
    out
  end

  private

  # Numeric padding helper for `strftime` directives. `default_width`
  # is the directive's own default; `width` (from the format
  # string) overrides if non-zero. `flag` controls the pad char:
  #   - `:nopad`    — no padding, just the digits + sign
  #   - `:spacepad` — leading spaces
  #   - `:zeropad` / nil — leading zeros (most numerics' default)
  # Sign handling: negative magnitude renders as `-<digits>`, with
  # the `-` inside the padding column (e.g. width=4, n=-5 →
  # `"-005"` for zeropad, `"  -5"` for spacepad).
  def __strftime_pad_num(n, default_width, flag, width)
    w = width > 0 ? width : default_width
    if flag == :nopad
      return n.to_s
    end
    sign = n < 0 ? "-" : ""
    digits = n.abs.to_s
    pad_count = w - sign.length - digits.length
    pad_count = 0 if pad_count < 0
    pad_char = (flag == :spacepad) ? " " : "0"
    sign + (pad_char * pad_count) + digits
  end

  # Day-of-year (1..366) for the given year/month/day. Uses a
  # static `DAYS_IN_MONTH` lookup + a leap-year offset for
  # March-or-later in leap years.
  DAYS_IN_MONTH = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]

  def __strftime_day_of_year(year, month, day)
    leap_offset = (month > 2 && __strftime_leap?(year)) ? 1 : 0
    prior = 0
    m = 1
    while m < month
      prior += DAYS_IN_MONTH[m - 1]
      m += 1
    end
    prior + day + leap_offset
  end

  def __strftime_leap?(year)
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
  end

  # Decompose `@sec` (Unix epoch UTC) into year/month/day/hour/
  # min/sec via Howard Hinnant's civil-from-days algorithm. The
  # standard reference: http://howardhinnant.github.io/date_algorithms.html
  # `days_from_civil` for the inverse direction; we use the
  # `civil_from_days` variant. Handles negative epochs (pre-1970)
  # via the floor-divide normalisation.
  def decompose
    # Memoized (Tier-1 Time immutability, see `to_s`): every field
    # accessor (`year`/`month`/.../`wday`) plus `to_s`/`xmlschema`/
    # `strftime` re-derived the civil fields per call — Liquid date
    # filters call `strftime` once per rendered page, and Jekyll's
    # document sort touches the accessors per comparison. All call
    # sites read the Hash without mutating it (the shared memo is
    # safe); the Hash is not handed out by any CRuby-surface API.
    memo = @__decompose
    return memo if memo
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

    result = {
      year:  y,
      month: m,
      day:   d,
      hour:  hh,
      min:   mm,
      sec:   ss,
      wday:  wday,
    }
    @__decompose = result
    result
  end
end
