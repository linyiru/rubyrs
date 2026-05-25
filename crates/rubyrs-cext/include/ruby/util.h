/* ruby/util.h — stub header for vendored crypt_blowfish.
 *
 * CRuby's <ruby/util.h> defines things like `ruby_strdup`,
 * `ruby_xmalloc`, etc. — Ruby-flavoured wrappers around their
 * libc equivalents. crypt_blowfish's wrapper.c includes it only
 * for a strdup workaround; nothing else in the vendored code
 * actually pulls a symbol from it.
 *
 * For the spike, expose the libc symbols directly under their
 * `ruby_*` names. This is a one-line-per-symbol stub, NOT a
 * faithful reimplementation. If a real ext relies on Ruby's
 * GC integration via ruby_xmalloc, that gets replaced by a
 * proper impl when we hit it. */

#ifndef RUBY_UTIL_H
#define RUBY_UTIL_H

#include <stdlib.h>
#include <string.h>

#ifndef ruby_strdup
#define ruby_strdup(s) strdup(s)
#endif

#ifndef ruby_xmalloc
#define ruby_xmalloc(n) malloc(n)
#endif

#ifndef ruby_xfree
#define ruby_xfree(p) free(p)
#endif

#endif /* RUBY_UTIL_H */
