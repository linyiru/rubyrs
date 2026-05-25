//! L3-K follow-up: application-defined ext-types via
//! `register_type_internal`. Verifies the general mechanism a
//! caller would use to ship a Time / UUID / Decimal / etc.
//! through msgpack's ext-type slots — the same path msgpack-ruby's
//! own `lib/msgpack/time.rb` uses for Time, applied to a
//! user-defined `Color` class as a stand-in.
//!
//! Why `Color` and not `Time`: Time itself isn't modelled in the
//! rubyrs subset yet (no `Time.now` / `Time.at` / `#nsec`), so
//! the ext-type machinery has to be exercised with a class
//! whose constructor + accessors the subset CAN drive. Once
//! `Time` lands as a subset addition the wire format ABI
//! shipped here doesn't change — the same `register_type_internal`
//! call signature works.
//!
//! What this pins:
//!   1. Two-way Proc registration (`packer:` and `unpacker:`
//!      shape, hand-rolled at `register_type_internal` instead
//!      of the missing pure-Ruby `register_type` wrapper).
//!   2. Custom positive ext-type id (`0x10` = 16, chosen to
//!      avoid collision with msgpack-ruby's own type ranges).
//!   3. Multi-byte payload encoded via `Array#pack("CCC")`
//!      (3 bytes) — verifying the binary-safe String path
//!      survives the cross-call hand-off from the packer Proc
//!      to the msgpack body builder and back.
//!   4. Restored object IS a fresh instance of the original
//!      user class with the right ivars — i.e. the unpacker
//!      Proc invocation arrives back in Ruby land with the
//!      bytes intact and constructs the right object.
//!   5. Ext frames coexist with normal frames in the same
//!      buffer: one Packer writes [Int, Color, String] in
//!      sequence; the Unpacker reads three values in order
//!      with one Color-typed proc invocation in the middle.
//!
//! Wire frame for a single Color(0xAB, 0xCD, 0xEF):
//!   `c7 03 10 ab cd ef`
//!   ext8 / len=3 / type=0x10 / [r, g, b]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_msgpack_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| common::build_cext_bundle("msgpack-cext", "msgpack")).clone()
}

#[test]
fn cext_msgpack_app_ext_round_trip() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_app_ext_driver.rb");

    let script = format!(
        r#"require "{bundle}"

class Color
  attr_reader :r, :g, :b
  def initialize(r, g, b); @r = r; @g = g; @b = b; end
end

# Single-value round-trip.
to_bytes   = proc {{ |c| [c.r, c.g, c.b].pack("CCC") }}
from_bytes = proc {{ |s|
  b = s.unpack("CCC")
  Color.new(b[0], b[1], b[2])
}}

p = MessagePack::Packer.new
p.register_type_internal(0x10, Color, to_bytes)
p.write(Color.new(0xAB, 0xCD, 0xEF))
puts "single-pack=" + p.to_str.bytes.inspect

u = MessagePack::Unpacker.new
u.register_type_internal(0x10, Color, from_bytes)
u.feed(p.to_str)
got = u.read
puts "single-class=" + got.class.name
puts "single-rgb=" + [got.r, got.g, got.b].inspect

# Mixed frames: an Int, a Color (ext-type), a String — all in
# the same buffer. Verifies the ext frame survives being
# adjacent to other frames and the registry doesn't disturb
# unrelated read paths.
p2 = MessagePack::Packer.new
p2.register_type_internal(0x10, Color, to_bytes)
p2.write(42)
p2.write(Color.new(1, 2, 3))
p2.write("done")
puts "mixed-pack=" + p2.to_str.bytes.inspect

u2 = MessagePack::Unpacker.new
u2.register_type_internal(0x10, Color, from_bytes)
u2.feed(p2.to_str)
v1 = u2.read
v2 = u2.read
v3 = u2.read
puts "mixed-1=" + v1.inspect
puts "mixed-2-class=" + v2.class.name
puts "mixed-2-rgb=" + [v2.r, v2.g, v2.b].inspect
puts "mixed-3=" + v3.inspect

# Time-shaped: 8-byte payload, msgpack-style (4-byte sec + 4-byte
# nsec, big-endian). The class is hand-rolled — when Time lands
# as a subset addition the same Proc shape applies unchanged.
class Stamp
  attr_reader :sec, :nsec
  def initialize(sec, nsec); @sec = sec; @nsec = nsec; end
end

stamp_pack   = proc {{ |t| [t.sec, t.nsec].pack("NN") }}
stamp_unpack = proc {{ |s|
  parts = s.unpack("NN")
  Stamp.new(parts[0], parts[1])
}}

p3 = MessagePack::Packer.new
p3.register_type_internal(-1, Stamp, stamp_pack)
p3.write(Stamp.new(0x12345678, 0x9ABCDEF0))
puts "stamp-pack=" + p3.to_str.bytes.inspect

u3 = MessagePack::Unpacker.new
u3.register_type_internal(-1, Stamp, stamp_unpack)
u3.feed(p3.to_str)
st = u3.read
puts "stamp-class=" + st.class.name
puts "stamp-sec=" + st.sec.to_s
puts "stamp-nsec=" + st.nsec.to_s
"#,
        bundle = bundle_no_ext.display(),
    );
    fs::write(&driver, script).expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(), stdout, stderr,
    );

    let want = "\
single-pack=[199, 3, 16, 171, 205, 239]\n\
single-class=Color\n\
single-rgb=[171, 205, 239]\n\
mixed-pack=[42, 199, 3, 16, 1, 2, 3, 164, 100, 111, 110, 101]\n\
mixed-1=42\n\
mixed-2-class=Color\n\
mixed-2-rgb=[1, 2, 3]\n\
mixed-3=\"done\"\n\
stamp-pack=[215, 255, 18, 52, 86, 120, 154, 188, 222, 240]\n\
stamp-class=Stamp\n\
stamp-sec=305419896\n\
stamp-nsec=2596069104\n";
    assert_eq!(
        stdout, want,
        "stdout mismatch:\n--- got ---\n{}\n--- want ---\n{}",
        stdout, want,
    );
}
