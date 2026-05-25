/* bcrypt_ext.c — Level 1 spike: bcrypt-shape C extension.
 *
 * Mirrors the shape of the bcrypt-ruby gem's `ext/mri/bcrypt_ext.c`,
 * but with a deterministic STUB crypto routine in place of openwall's
 * crypt_blowfish. The point of this Level 1 spike is to validate the
 * rubyrs cext FFI plumbing — non-zero arity, String args from Ruby
 * into C (via RSTRING_PTR / RSTRING_LEN), String return from C back
 * into Ruby — not to ship real bcrypt.
 *
 * To upgrade to actual bcrypt-the-gem:
 *   1. Drop openwall's `crypt_blowfish.c`, `crypt_gensalt.c`, and
 *      `wrapper.c` into this directory (public domain / BSD-ish).
 *   2. Replace `stub_bcrypt` below with a call to `crypt_rn()`.
 *   3. Compile the extra .c files in `build.sh`.
 * That swap is purely mechanical — every architectural question is
 * answered by the fact that THIS file compiles, dlopens, and round
 * trips.
 *
 * Ruby surface:
 *   require "bcrypt_ext"
 *   bcrypt_hash(password_str, salt_str) #=> "$2a$10$" + deterministic-22 + deterministic-31
 */

#include "rubyrs.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

/* STUB. Not crypto. Replace with crypt_rn() to ship real bcrypt.
 *
 * Mixes password and salt bytes into a 53-byte buffer that looks
 * structurally like a bcrypt $2a$ hash so callers can assert on
 * specific deterministic outputs without us pretending to compute
 * a real one. */
static void stub_bcrypt(const char *pw, long pw_len,
                        const char *salt, long salt_len,
                        char *out_53)
{
    /* bcrypt hash digit-and-letter alphabet, used for the trailing 31
     * char "hash" portion. Real bcrypt uses 6-bit groups from the
     * Blowfish ciphertext; we just pick deterministically from
     * (password ⊕ salt) byte mixes. */
    static const char ALPHABET[] =
        "./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    /* The 22-char "salt" portion: take the first up-to-22 bytes of the
     * provided salt and project them into ALPHABET deterministically. */
    for (int i = 0; i < 22; i++) {
        uint8_t b = (salt_len > 0) ? (uint8_t)salt[i % salt_len] : 0;
        out_53[i] = ALPHABET[b % 64];
    }

    /* The 31-char "hash" portion: mix every password byte with every
     * salt byte to get a deterministic dependency on both inputs. */
    for (int i = 0; i < 31; i++) {
        uint32_t acc = (uint32_t)i * 2654435761u; /* Knuth multiplicative */
        for (long p = 0; p < pw_len; p++) {
            acc ^= ((uint32_t)(uint8_t)pw[p]) << ((p + i) & 24);
            acc = (acc << 1) | (acc >> 31);
        }
        for (long s = 0; s < salt_len; s++) {
            acc ^= ((uint32_t)(uint8_t)salt[s]) << ((s + i) & 24);
            acc = (acc << 1) | (acc >> 31);
        }
        out_53[22 + i] = ALPHABET[acc % 64];
    }
}

/* bcrypt_hash(password: String, salt: String) -> String
 *
 * Builds "$2a$10$" + 53-char body and returns it as a Ruby String.
 * Output is deterministic in (password, salt). */
static VALUE bcrypt_hash(VALUE self, VALUE password, VALUE salt) {
    (void)self;

    const char *pw_ptr = RSTRING_PTR(password);
    long        pw_len = RSTRING_LEN(password);
    const char *sa_ptr = RSTRING_PTR(salt);
    long        sa_len = RSTRING_LEN(salt);

    /* "$2a$10$" + 22 salt + 31 hash = 60 bytes, no NUL. */
    char buf[60];
    memcpy(buf, "$2a$10$", 7);
    stub_bcrypt(pw_ptr, pw_len, sa_ptr, sa_len, buf + 7);

    return rb_str_new(buf, 60);
}

void Init_bcrypt_ext(void) {
    rb_define_global_function("bcrypt_hash", RUBY_METHOD_FUNC(bcrypt_hash), 2);
}
