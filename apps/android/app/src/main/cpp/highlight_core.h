// Tree-sitter highlighting of the grammars that Enriched Markdown does not bundle (PHP).
// Plain C++ without JNI, so that the same code runs in the Android library and in the host test (tools/grammar-check).
#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>

namespace tomi {

// Same order as com.swmansion.enriched.markdown.codehighlight.SyntaxTokenType: the decorator of the library indexes its enum
// with this number.
enum TokenType : int32_t {
    KEYWORD = 0,
    OPERATOR,
    PUNCTUATION,
    STRING,
    NUMBER,
    CONSTANT,
    COMMENT,
    FUNCTION,
    TYPE,
    VARIABLE,
    PROPERTY,
    TAG,
    ATTRIBUTE,
    EMBEDDED,
    TOKEN_TYPE_COUNT,
};

enum class Language { Php };

// Largest input parsed: same limits as Enriched Markdown (50 KB of UTF-8). A bigger input is not parsed (no colour).
constexpr size_t kMaxBytes = 50 * 1024;

// Colour runs of [code] (UTF-8, [len] bytes). [out] receives flat triplets (start, end, tokenType): start inclusive and end exclusive,
// as UTF-16 code unit offsets (what a Java/Kotlin String uses), in order, never overlapping. Returns false when nothing could be
// produced (input too big, grammar or query failed to load): [out] is then empty. Thread safe.
bool tokenize(Language language, const char* code, size_t len, std::vector<int32_t>& out);

// Error of the highlight query of [language] ("" when it compiled), for the host test. Triggers the lazy loading.
const char* queryError(Language language);

}  // namespace tomi
