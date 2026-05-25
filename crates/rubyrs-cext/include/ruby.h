/* ruby.h — drop-in header for CRuby-targeted C extensions.
 *
 * CRuby C extensions universally write `#include <ruby.h>`. Provide
 * this name as a thin alias for `rubyrs.h` so existing extension
 * source compiles unchanged.
 *
 * The compatibility level is whatever rubyrs-cext currently exposes
 * — see rubyrs.h for the full list. Extensions that reach for an
 * API symbol we haven't implemented will fail to link at the C ext
 * build step, with a clean missing-symbol error. */

#ifndef RUBY_H
#define RUBY_H

#include "rubyrs.h"

#endif /* RUBY_H */
