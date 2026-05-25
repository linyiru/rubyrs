# Vendored crypt_blowfish

These files are copied verbatim from the [bcrypt-ruby gem][1]'s
`ext/mri/` directory, which itself vendors [Openwall's crypt_blowfish][2].

| File | Purpose |
|---|---|
| `crypt_blowfish.c` / `.h` | Solar Designer's bcrypt implementation (the Eksblowfish key schedule + the `$2*$`-format `crypt_rn`). |
| `crypt_gensalt.c` / `.h` | Salt generation. |
| `wrapper.c` | The `crypt_ra` / `crypt_gensalt_ra` wrappers that bcrypt_ext.c calls. |
| `crypt.c` / `.h` | Glue. |
| `ow-crypt.h` | Public header — what `bcrypt_ext.c` `#include`s. |

## License

`crypt_blowfish.c`'s opening comment says it best:

> Written by Solar Designer <solar at openwall.com> in 1998-2014.
> No copyright is claimed, and the software is hereby placed in the
> public domain.

The rest of the files carry the same notice or a 2-clause BSD-equivalent
fallback. Compatible with both MIT and Apache-2.0 (rubyrs's own dual
license).

## Why vendored, not git submodule

This is a spike directory, and the C source compiles unchanged. Pulling
it inline keeps the example self-contained and the build script
non-magical. If we promote bcrypt support out of `examples/` into a real
shipped feature, this becomes a real dependency decision.

[1]: https://github.com/bcrypt-ruby/bcrypt-ruby/tree/master/ext/mri
[2]: https://www.openwall.com/crypt/
