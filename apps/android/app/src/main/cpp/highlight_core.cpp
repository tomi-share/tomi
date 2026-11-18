#include "highlight_core.h"

#include <tree_sitter/api.h>

#include <algorithm>
#include <cstring>
#include <mutex>
#include <regex>
#include <map>
#include <string>

#include "queries_gen.h"

#ifdef __ANDROID__
#include <android/log.h>
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, "TomiHighlight", __VA_ARGS__)
#else
#include <cstdio>
#define LOGE(...) (std::fprintf(stderr, __VA_ARGS__), std::fprintf(stderr, "\n"))
#endif

extern "C" {
const TSLanguage* tree_sitter_php(void);
}

namespace tomi {
namespace {

struct Grammar {
    const TSLanguage* language = nullptr;
    TSQuery* query = nullptr;
    std::string error;
    // Capture id of the query -> TokenType, or -1 for a capture that is not a colour (ignored).
    std::vector<int32_t> captureTypes;
    // PHP only: the grammar starts in HTML mode, so a snippet without an opening tag is parsed after "<?php\n".
    bool needsPhpTag = false;
};

// Capture name (the part before the first '.') -> token type. The queries use the 14 names of the library directly.
int32_t typeOfCapture(const std::string& name) {
    static const struct {
        const char* name;
        TokenType type;
    } kNames[] = {
        {"keyword", KEYWORD},   {"operator", OPERATOR}, {"punctuation", PUNCTUATION}, {"string", STRING},
        {"number", NUMBER},     {"constant", CONSTANT}, {"comment", COMMENT},         {"function", FUNCTION},
        {"type", TYPE},         {"variable", VARIABLE}, {"property", PROPERTY},       {"tag", TAG},
        {"attribute", ATTRIBUTE}, {"embedded", EMBEDDED},
    };
    const std::string base = name.substr(0, name.find('.'));
    for (const auto& entry : kNames) {
        if (base == entry.name) return entry.type;
    }
    return -1;
}

void load(Grammar& grammar, const TSLanguage* language, const char* querySource, bool needsPhpTag) {
    grammar.language = language;
    grammar.needsPhpTag = needsPhpTag;
    uint32_t errorOffset = 0;
    TSQueryError errorType = TSQueryErrorNone;
    grammar.query = ts_query_new(language, querySource, static_cast<uint32_t>(std::strlen(querySource)), &errorOffset, &errorType);
    if (grammar.query == nullptr) {
        // Line and column of the error, to find it in the .scm file.
        uint32_t line = 1, col = 0;
        for (uint32_t i = 0; i < errorOffset && querySource[i] != '\0'; i++) {
            if (querySource[i] == '\n') {
                line++;
                col = 0;
            } else {
                col++;
            }
        }
        grammar.error = "query error " + std::to_string(static_cast<int>(errorType)) + " at line " + std::to_string(line) + " column " +
                        std::to_string(col);
        LOGE("%s", grammar.error.c_str());
        return;
    }
    const uint32_t count = ts_query_capture_count(grammar.query);
    grammar.captureTypes.resize(count, -1);
    for (uint32_t i = 0; i < count; i++) {
        uint32_t length = 0;
        const char* name = ts_query_capture_name_for_id(grammar.query, i, &length);
        grammar.captureTypes[i] = typeOfCapture(std::string(name, length));
    }
}

Grammar& grammarFor(Language language) {
    static Grammar php;
    static std::once_flag phpOnce;
    (void)language;  // PHP is the only grammar.
    std::call_once(phpOnce, [] { load(php, tree_sitter_php(), kPhpQuery, true); });
    return php;
}

std::string stepString(const TSQuery* query, const TSQueryPredicateStep& step) {
    uint32_t length = 0;
    const char* value = ts_query_string_value_for_id(query, step.value_id, &length);
    return std::string(value, length);
}

// Longest text (in bytes) given to std::regex by #match? / #not-match?. The subjects are identifiers and short tokens (constants,
// class names, variables): 4096 bytes is far beyond any real one. Longer subjects are not matched and their segment stays uncoloured.
constexpr size_t kMaxRegexSubjectBytes = 4096;

// The library evaluates no predicate itself. Supported: #eq? #not-eq? #any-of? #match? #not-match? (the first argument is a capture
// of the match). Any other predicate is ignored (true).
// Compiled regexes of one tokenize() call, by pattern: building a std::regex per match is the main cost otherwise.
using RegexCache = std::map<std::string, std::regex>;

bool predicatesHold(const TSQuery* query, const TSQueryMatch& match, const char* code, size_t len, RegexCache& regexes) {
    uint32_t stepCount = 0;
    const TSQueryPredicateStep* steps = ts_query_predicates_for_pattern(query, match.pattern_index, &stepCount);
    if (stepCount == 0) return true;

    auto captureText = [&](uint32_t captureId) -> std::string {
        for (uint16_t i = 0; i < match.capture_count; i++) {
            if (match.captures[i].index == captureId) {
                const uint32_t start = ts_node_start_byte(match.captures[i].node);
                const uint32_t end = ts_node_end_byte(match.captures[i].node);
                if (start <= end && end <= len) return std::string(code + start, end - start);
            }
        }
        return std::string();
    };

    uint32_t i = 0;
    while (i < stepCount) {
        uint32_t end = i;
        while (end < stepCount && steps[end].type != TSQueryPredicateStepTypeDone) end++;
        if (end > i && steps[i].type == TSQueryPredicateStepTypeString) {
            const std::string name = stepString(query, steps[i]);
            // arguments: steps (i, end)
            std::vector<std::string> args;
            std::vector<bool> isCapture;
            for (uint32_t k = i + 1; k < end; k++) {
                if (steps[k].type == TSQueryPredicateStepTypeCapture) {
                    args.push_back(captureText(steps[k].value_id));
                    isCapture.push_back(true);
                } else {
                    args.push_back(stepString(query, steps[k]));
                    isCapture.push_back(false);
                }
            }
            if ((name == "eq?" || name == "not-eq?") && args.size() == 2) {
                const bool equal = args[0] == args[1];
                if (equal != (name == "eq?")) return false;
            } else if (name == "any-of?" && args.size() >= 2) {
                bool found = false;
                for (size_t k = 1; k < args.size(); k++) found = found || args[0] == args[k];
                if (!found) return false;
            } else if ((name == "match?" || name == "not-match?") && args.size() == 2) {
                // Bound the subject BEFORE std::regex: the libc++ regex is a backtracking engine, so a pathological segment (a huge
                // "identifier" such as a 40 KB token without separator) could run for a very long time on the calling thread (ANR).
                // Over the limit the regex is not run at all and the whole match is dropped, for match? and not-match? alike:
                // that segment simply gets no colour (see kMaxRegexSubjectBytes).
                if (args[0].size() > kMaxRegexSubjectBytes) return false;
                bool matches = false;
                try {
                    auto it = regexes.find(args[1]);
                    if (it == regexes.end()) it = regexes.emplace(args[1], std::regex(args[1])).first;
                    matches = std::regex_search(args[0], it->second);
                } catch (const std::regex_error&) {
                    matches = false;
                }
                if (matches != (name == "match?")) return false;
            }
        }
        i = end + 1;
    }
    return true;
}

struct Capture {
    uint32_t pattern;
    uint32_t start;
    uint32_t end;
    int32_t type;
};

}  // namespace

const char* queryError(Language language) { return grammarFor(language).error.c_str(); }

bool tokenize(Language language, const char* code, size_t len, std::vector<int32_t>& out) {
    out.clear();
    if (len == 0 || len > kMaxBytes) return false;
    Grammar& grammar = grammarFor(language);
    if (grammar.query == nullptr || grammar.language == nullptr) return false;

    // PHP: without an opening tag the grammar reads the whole snippet as HTML text. Add one, and shift the offsets back.
    std::string prefixed;
    const char* source = code;
    size_t sourceLen = len;
    uint32_t shift = 0;
    static const char kOpenTag[] = {'<', '?'};
    if (grammar.needsPhpTag && std::search(code, code + len, kOpenTag, kOpenTag + 2) == code + len) {
        prefixed = "<?php\n";
        shift = static_cast<uint32_t>(prefixed.size());
        prefixed.append(code, len);
        source = prefixed.data();
        sourceLen = prefixed.size();
    }

    TSParser* parser = ts_parser_new();
    if (!ts_parser_set_language(parser, grammar.language)) {
        ts_parser_delete(parser);
        return false;
    }
    TSTree* tree = ts_parser_parse_string(parser, nullptr, source, static_cast<uint32_t>(sourceLen));
    if (tree == nullptr) {
        ts_parser_delete(parser);
        return false;
    }

    std::vector<Capture> captures;
    RegexCache regexes;
    TSQueryCursor* cursor = ts_query_cursor_new();
    ts_query_cursor_exec(cursor, grammar.query, ts_tree_root_node(tree));
    TSQueryMatch match;
    while (ts_query_cursor_next_match(cursor, &match)) {
        if (!predicatesHold(grammar.query, match, source, sourceLen, regexes)) continue;
        for (uint16_t i = 0; i < match.capture_count; i++) {
            const int32_t type = grammar.captureTypes[match.captures[i].index];
            if (type < 0) continue;
            const uint32_t start = ts_node_start_byte(match.captures[i].node);
            const uint32_t end = ts_node_end_byte(match.captures[i].node);
            if (end <= start || end <= shift) continue;
            captures.push_back({match.pattern_index, start > shift ? start - shift : 0, end - shift, type});
        }
    }
    ts_query_cursor_delete(cursor);
    ts_tree_delete(tree);
    ts_parser_delete(parser);

    // A later pattern of the query wins over an earlier one on the same text (the queries put the general rules first).
    std::stable_sort(captures.begin(), captures.end(), [](const Capture& a, const Capture& b) { return a.pattern < b.pattern; });
    std::vector<int8_t> painted(len, -1);
    for (const Capture& capture : captures) {
        const size_t end = std::min<size_t>(capture.end, len);
        for (size_t i = capture.start; i < end; i++) painted[i] = static_cast<int8_t>(capture.type);
    }

    // UTF-8 byte offset -> UTF-16 offset.
    std::vector<int32_t> utf16(len + 1);
    int32_t index = 0;
    for (size_t i = 0; i < len; i++) {
        utf16[i] = index;
        const unsigned char b = static_cast<unsigned char>(code[i]);
        if ((b & 0xC0) != 0x80) index += (b & 0xF8) == 0xF0 ? 2 : 1;
    }
    utf16[len] = index;

    size_t i = 0;
    while (i < len) {
        if (painted[i] < 0) {
            i++;
            continue;
        }
        size_t j = i + 1;
        while (j < len && painted[j] == painted[i]) j++;
        out.push_back(utf16[i]);
        out.push_back(utf16[j]);
        out.push_back(painted[i]);
        i = j;
    }
    return true;
}

}  // namespace tomi
