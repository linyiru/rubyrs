# Tier 3 (ADR 0019 Part E) — pure-Ruby Date / DateTime on top of the
# injected Time. Date arithmetic is integer Julian-Day-Number maths
# (Fliegel–Van Flandern), so no VM change. `Date.today` / `DateTime.now`
# read the clock through Time.now.
#
# Scope: civil dates + the common surface (accessors, comparison, day/
# month arithmetic, strftime, iso8601/to_s, to_time, parse). Documented
# divergences from CRuby: Julian-calendar reform (`Date::ITALY` start
# date) is ignored — proleptic Gregorian throughout; no commercial
# (`cwyear`/`cweek`) or ordinal constructors; `parse` handles ISO-8601
# and a few common forms, not CRuby's full heuristic grammar;
# sub-second precision (`sec_fraction`) is dropped.

class Date
  include Comparable

  MONTHNAMES = [nil, 'January', 'February', 'March', 'April', 'May',
                'June', 'July', 'August', 'September', 'October',
                'November', 'December'].freeze
  ABBR_MONTHNAMES = [nil, 'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
                     'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'].freeze
  DAYNAMES = %w[Sunday Monday Tuesday Wednesday Thursday Friday Saturday].freeze
  ABBR_DAYNAMES = %w[Sun Mon Tue Wed Thu Fri Sat].freeze

  # Julian Day Number for a proleptic-Gregorian civil date.
  def self.civil_to_jd(y, m, d)
    a = (14 - m) / 12
    yy = y + 4800 - a
    mm = m + 12 * a - 3
    d + (153 * mm + 2) / 5 + 365 * yy + yy / 4 - yy / 100 + yy / 400 - 32045
  end

  # Inverse: JDN → [year, month, day].
  def self.jd_to_civil(jd)
    a = jd + 32044
    b = (4 * a + 3) / 146097
    c = a - (146097 * b) / 4
    dd = (4 * c + 3) / 1461
    e = c - (1461 * dd) / 4
    m = (5 * e + 2) / 153
    day = e - (153 * m + 2) / 5 + 1
    month = m + 3 - 12 * (m / 10)
    year = 100 * b + dd - 4800 + m / 10
    [year, month, day]
  end

  def self.civil(year = -4712, month = 1, day = 1)
    new(year, month, day)
  end

  class << self
    alias new! civil
  end

  def self.jd(jd = 0)
    y, m, d = jd_to_civil(jd)
    obj = allocate
    obj.send(:init_civil, y, m, d)
    obj
  end

  def self.today
    t = Time.now
    new(t.year, t.month, t.day)
  end

  def self.valid_civil?(y, m, d)
    return false unless m.between?(1, 12)
    return false if d < 1
    d <= days_in_month(y, m)
  end
  class << self; alias valid_date? valid_civil?; end

  def self.leap?(y)
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
  end

  def self.days_in_month(y, m)
    return 29 if m == 2 && leap?(y)
    [nil, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m]
  end

  def self.parse(str, *)
    s = str.to_s.strip
    if s =~ /\A(\d{4})-(\d{2})-(\d{2})/
      new(Regexp.last_match(1).to_i, Regexp.last_match(2).to_i, Regexp.last_match(3).to_i)
    elsif s =~ %r{\A(\d{1,2})/(\d{1,2})/(\d{4})}
      new(Regexp.last_match(3).to_i, Regexp.last_match(1).to_i, Regexp.last_match(2).to_i)
    elsif s =~ /\A(\d{4})(\d{2})(\d{2})\z/
      new(Regexp.last_match(1).to_i, Regexp.last_match(2).to_i, Regexp.last_match(3).to_i)
    else
      raise ArgumentError, "invalid date: #{str.inspect}"
    end
  end

  def self.strptime(str, fmt = '%F')
    h = _strptime(str, fmt)
    unless h && h[:year] && h[:mon] && h[:mday]
      raise ArgumentError, "invalid strptime format - #{fmt.inspect}"
    end
    new(h[:year], h[:mon], h[:mday])
  end

  # Shared strptime parser (Date + DateTime). Returns a Hash with the
  # components it could read (`:year :mon :mday :hour :min :sec`), or
  # nil on a literal mismatch. Implements the common conversion
  # specifiers; unknown ones are skipped best-effort.
  def self._strptime(str, fmt)
    res = {}
    s = str.to_s
    si = 0
    fi = 0
    while fi < fmt.length
      c = fmt[fi]
      if c == '%'
        fi += 1
        conv = fmt[fi]
        case conv
        when 'Y' then n = _strptime_int(s, si, 4); return nil unless n; res[:year] = n[0]; si = n[1]
        when 'y' then n = _strptime_int(s, si, 2); return nil unless n; res[:year] = 2000 + n[0]; si = n[1]
        when 'm' then n = _strptime_int(s, si, 2); return nil unless n; res[:mon] = n[0]; si = n[1]
        when 'd', 'e' then n = _strptime_int(s, si, 2); return nil unless n; res[:mday] = n[0]; si = n[1]
        when 'H' then n = _strptime_int(s, si, 2); return nil unless n; res[:hour] = n[0]; si = n[1]
        when 'M' then n = _strptime_int(s, si, 2); return nil unless n; res[:min] = n[0]; si = n[1]
        when 'S' then n = _strptime_int(s, si, 2); return nil unless n; res[:sec] = n[0]; si = n[1]
        when '%' then return nil unless s[si] == '%'; si += 1
        else
          # Unsupported specifier — skip it without consuming input.
        end
        fi += 1
      elsif c == ' '
        si += 1 while si < s.length && s[si] == ' '
        fi += 1
      else
        return nil unless s[si] == c
        si += 1
        fi += 1
      end
    end
    res
  end

  # Read up to `maxlen` digits (skipping leading blanks, allowing a
  # leading sign) from `s` at `si`. Returns [value, next_index] or nil.
  def self._strptime_int(s, si, maxlen)
    si += 1 while si < s.length && s[si] == ' '
    start = si
    si += 1 if si < s.length && (s[si] == '-' || s[si] == '+')
    digits = 0
    while si < s.length && s[si] =~ /\d/ && digits < maxlen
      si += 1
      digits += 1
    end
    return nil if digits.zero?
    [s[start...si].to_i, si]
  end

  def initialize(year = -4712, month = 1, day = 1)
    init_civil(year, month, day)
  end

  def init_civil(year, month, day)
    unless self.class.valid_civil?(year, month, day)
      raise ArgumentError, 'invalid date'
    end
    @year = year
    @month = month
    @day = day
    @jd = Date.civil_to_jd(year, month, day)
    self
  end
  protected :init_civil

  attr_reader :year
  def month; @month; end
  alias mon month
  def day; @day; end
  alias mday day
  def jd; @jd; end

  # Lilian Date (days since 1582-10-15) and Modified Julian Day
  # (days since 1858-11-17). Plain JDN offsets.
  def ld; @jd - 2299160; end
  def mjd; @jd - 2400001; end

  # 0 = Sunday … 6 = Saturday. JDN 0 is a Monday, so (jd + 1) % 7.
  def wday; (@jd + 1) % 7; end
  def sunday?;    wday == 0; end
  def monday?;    wday == 1; end
  def tuesday?;   wday == 2; end
  def wednesday?; wday == 3; end
  def thursday?;  wday == 4; end
  def friday?;    wday == 5; end
  def saturday?;  wday == 6; end

  def yday
    @jd - Date.civil_to_jd(@year, 1, 1) + 1
  end

  def leap?; Date.leap?(@year); end

  # ISO-8601 commercial date. `cwday` is Monday=1..Sunday=7; the
  # commercial week (`cweek`) and year (`cwyear`) belong to whichever
  # calendar year holds the Thursday of this week (so week 1 is the
  # week containing the first Thursday).
  def cwday; wday.zero? ? 7 : wday; end

  def cwyear
    thursday = @jd - cwday + 4
    Date.jd_to_civil(thursday)[0]
  end

  def cweek
    thursday = @jd - cwday + 4
    y = Date.jd_to_civil(thursday)[0]
    (thursday - Date.civil_to_jd(y, 1, 1)) / 7 + 1
  end

  # Day arithmetic via the JDN; month/year arithmetic clamps the day.
  def +(n)
    raise TypeError, 'expected numeric' unless n.is_a?(Numeric)
    Date.jd(@jd + n.to_i)
  end

  def -(other)
    if other.is_a?(Date)
      # CRuby returns the day difference as a Rational.
      Rational(@jd - other.jd, 1)
    elsif other.is_a?(Numeric)
      Date.jd(@jd - other.to_i)
    else
      raise TypeError, 'expected numeric or date'
    end
  end

  def >>(months)
    total = (@year * 12 + (@month - 1)) + months
    y = total / 12
    m = total % 12 + 1
    d = [@day, Date.days_in_month(y, m)].min
    Date.new(y, m, d)
  end

  def <<(months); self >> (-months); end
  def next_day(n = 1); self + n; end
  def prev_day(n = 1); self - n; end
  def next_month(n = 1); self >> n; end
  def prev_month(n = 1); self << n; end
  def next_year(n = 1); self >> (n * 12); end
  def prev_year(n = 1); self << (n * 12); end
  def succ; self + 1; end
  alias next succ

  # Iterate dates from self toward `limit` by `step` days (negative
  # step counts down). No block → an Enumerator. `upto` / `downto`
  # are the unit-step shorthands.
  def step(limit, step = 1)
    return enum_for(:step, limit, step) unless block_given?
    raise ArgumentError, "step can't be 0" if step.zero?
    d = self
    if step > 0
      while d <= limit
        yield d
        d += step
      end
    else
      while d >= limit
        yield d
        d += step
      end
    end
    self
  end

  def upto(max, &block); step(max, 1, &block); end
  def downto(min, &block); step(min, -1, &block); end

  # Comparable across Date and DateTime via a single UTC-seconds key:
  # a plain Date sits at UTC midnight (time + offset zero).
  def cmp_key; @jd * 86400; end
  protected :cmp_key

  def <=>(other)
    return nil unless other.respond_to?(:cmp_key, true)
    cmp_key <=> other.send(:cmp_key)
  end

  def ==(other)
    other.is_a?(Date) && (self <=> other) == 0
  end
  def eql?(other); self.class == other.class && (self <=> other) == 0; end
  def hash; cmp_key.hash; end

  def to_date; self; end
  def to_datetime; DateTime.new(@year, @month, @day, 0, 0, 0); end

  def to_time
    Time.utc(@year, @month, @day, 0, 0, 0)
  end

  def to_s; format('%04d-%02d-%02d', @year, @month, @day); end
  alias iso8601 to_s
  # CRuby renders the internal `(jd, day-fraction-seconds,
  # nanoseconds), offset-seconds, gregorian-start-jd` tuple. A plain
  # Date has no time-of-day (0s/0n), zero offset, and the default
  # ITALY reform start (2299161j).
  def inspect; "#<Date: #{to_s} ((#{@jd}j,0s,0n),+0s,2299161j)>"; end

  def strftime(fmt = '%F')
    Date._strftime(fmt, @year, @month, @day, 0, 0, 0, wday, yday, nil)
  end

  # Shared strftime engine (Date + DateTime). `off` is the UTC offset in
  # seconds, or nil for a date-only value.
  def self._strftime(fmt, y, mo, d, h, mi, s, wday, yday, off)
    fmt.gsub(/%[-_0^#]?\d*[A-Za-z%]/) do |spec|
      flag = spec[/%([-_0^#])/, 1]
      conv = spec[-1]
      raw =
        case conv
        when 'Y' then format('%04d', y)
        when 'y' then format('%02d', y % 100)
        when 'C' then format('%02d', y / 100)
        when 'm' then format('%02d', mo)
        when 'd' then format('%02d', d)
        when 'e' then format('%2d', d)
        when 'j' then format('%03d', yday)
        when 'H' then format('%02d', h)
        when 'I' then format('%02d', (h % 12).zero? ? 12 : h % 12)
        when 'M' then format('%02d', mi)
        when 'S' then format('%02d', s)
        when 'p' then h < 12 ? 'AM' : 'PM'
        when 'P' then h < 12 ? 'am' : 'pm'
        when 'A' then DAYNAMES[wday]
        when 'a' then ABBR_DAYNAMES[wday]
        when 'B' then MONTHNAMES[mo]
        when 'b', 'h' then ABBR_MONTHNAMES[mo]
        when 'w' then wday.to_s
        when 'u' then (wday.zero? ? 7 : wday).to_s
        when 'z' then off.nil? ? '' : format('%+03d%02d', off / 3600, (off.abs % 3600) / 60)
        when 'F' then format('%04d-%02d-%02d', y, mo, d)
        when 'T', 'X' then format('%02d:%02d:%02d', h, mi, s)
        when 'R' then format('%02d:%02d', h, mi)
        when 'D' then format('%02d/%02d/%02d', mo, d, y % 100)
        when '%' then '%'
        else spec
        end
      case flag
      when '-' then raw.sub(/\A0+(?=\d)/, '')
      when '_' then raw.sub(/\A0+(?=\d)/) { ' ' * Regexp.last_match(0).length }
      else raw
      end
    end
  end
end

class DateTime < Date
  def self.now
    t = Time.now
    off = t.respond_to?(:utc_offset) ? t.utc_offset : 0
    new(t.year, t.month, t.day, t.hour, t.min, t.sec, offset_to_string(off))
  end

  def self.offset_to_string(secs)
    sign = secs < 0 ? '-' : '+'
    a = secs.abs
    format('%s%02d:%02d', sign, a / 3600, (a % 3600) / 60)
  end

  def self.parse(str, *)
    s = str.to_s.strip
    if s =~ /\A(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2}):(\d{2})(?:\.\d+)?\s*([+-]\d{2}:?\d{2}|Z)?/
      m = Regexp.last_match
      off = m[7]
      off = '+00:00' if off.nil? || off == 'Z'
      new(m[1].to_i, m[2].to_i, m[3].to_i, m[4].to_i, m[5].to_i, m[6].to_i, off)
    else
      d = super(s)
      new(d.year, d.month, d.day, 0, 0, 0)
    end
  end

  # `iso8601` only accepts the ISO-8601 form — `parse` already handles
  # it, so delegate.
  def self.iso8601(str = '-4712-01-01T00:00:00+00:00', *)
    parse(str)
  end

  def self.strptime(str, fmt = '%FT%T%z')
    h = _strptime(str, fmt)
    unless h && h[:year] && h[:mon] && h[:mday]
      raise ArgumentError, "invalid strptime format - #{fmt.inspect}"
    end
    off = h[:offset] ? offset_to_string(h[:offset]) : '+00:00'
    new(h[:year], h[:mon], h[:mday], h[:hour] || 0, h[:min] || 0, h[:sec] || 0, off)
  end

  def initialize(year = -4712, month = 1, day = 1, hour = 0, min = 0, sec = 0, offset = '+00:00')
    init_civil(year, month, day)
    @hour = hour
    @min = min
    @sec = sec
    @offset_secs = DateTime.parse_offset(offset)
  end

  def self.parse_offset(offset)
    case offset
    when Numeric then (offset * 86400).round   # Rational day fraction
    when nil, '', 'Z' then 0
    when /\A([+-])(\d{2}):?(\d{2})\z/
      s = Regexp.last_match(1) == '-' ? -1 : 1
      s * (Regexp.last_match(2).to_i * 3600 + Regexp.last_match(3).to_i * 60)
    else 0
    end
  end

  def hour; @hour; end
  def min; @min; end
  alias minute min
  def sec; @sec; end
  alias second sec
  def offset_secs; @offset_secs; end

  # offset as a day-fraction Rational (CRuby's `#offset`); `zone` is
  # the "+HH:MM" string; sub-second precision isn't modelled, so
  # `sec_fraction` is always 0.
  def offset; Rational(@offset_secs, 86400); end
  def zone; DateTime.offset_to_string(@offset_secs); end
  def sec_fraction; Rational(0, 1); end

  # Day arithmetic that preserves the time-of-day and offset (Date#+/-
  # would drop them). `n` is a number of days (fractional allowed; the
  # sub-second remainder is dropped, matching the rest of this file).
  def +(n)
    raise TypeError, 'expected numeric' unless n.is_a?(Numeric)
    total = @jd * 86400 + @hour * 3600 + @min * 60 + @sec + (n * 86400)
    total = total.floor
    day = total / 86400
    rem = total % 86400
    y, m, d = Date.jd_to_civil(day)
    DateTime.new(y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60,
                 DateTime.offset_to_string(@offset_secs))
  end

  def -(other)
    if other.is_a?(Date)
      Rational(cmp_key - other.send(:cmp_key), 86400)
    elsif other.is_a?(Numeric)
      self + (-other)
    else
      raise TypeError, 'expected numeric or date'
    end
  end

  # Same instant, re-expressed in a different offset.
  def new_offset(offset = 0)
    target = DateTime.parse_offset(offset)
    utc = @jd * 86400 + @hour * 3600 + @min * 60 + @sec - @offset_secs
    local = utc + target
    day = local / 86400
    rem = local % 86400
    y, m, d = Date.jd_to_civil(day)
    DateTime.new(y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60,
                 DateTime.offset_to_string(target))
  end

  def to_time
    # Build the UTC instant then keep it (rubyrs Time is UTC-based).
    Time.utc(@year, @month, @day, @hour, @min, @sec) - @offset_secs
  rescue StandardError
    Time.utc(@year, @month, @day, @hour, @min, @sec)
  end

  def to_datetime; self; end
  def to_date; Date.new(@year, @month, @day); end

  def to_s
    format('%04d-%02d-%02dT%02d:%02d:%02d%s', @year, @month, @day,
           @hour, @min, @sec, DateTime.offset_to_string(@offset_secs))
  end
  alias iso8601 to_s
  # CRuby's internal tuple: (jd, day-fraction seconds, nanoseconds),
  # offset seconds, gregorian-start jd. The jd / seconds are the UTC
  # instant (local wall clock minus the offset), so they roll across
  # midnight independently of the local `#jd`. Sub-second is 0n here.
  def inspect
    total = @jd * 86400 + @hour * 3600 + @min * 60 + @sec - @offset_secs
    format('#<DateTime: %s ((%dj,%ds,0n),%+ds,2299161j)>',
           to_s, total / 86400, total % 86400, @offset_secs)
  end

  def strftime(fmt = '%FT%T%:z')
    f = fmt.gsub('%:z', DateTime.offset_to_string(@offset_secs))
    Date._strftime(f, @year, @month, @day, @hour, @min, @sec, wday, yday, @offset_secs)
  end

  # UTC-seconds comparison key: local wall-clock seconds minus the
  # offset, so two DateTimes at the same instant in different zones
  # compare equal (and a plain Date sits at UTC midnight).
  def cmp_key
    @jd * 86400 + (@hour * 3600 + @min * 60 + @sec) - @offset_secs
  end
  protected :cmp_key
end
