// Host test of highlight_core.cpp (same code as the Android library). Usage: grammar-check <php> <file> [--tokens]
// Prints the query status, then the file with every coloured run as [type|text]; --tokens lists the runs one per line.
#include <cstdio>
#include <fstream>
#include <iostream>
#include <iterator>
#include <string>
#include <vector>

#include "../../app/src/main/cpp/highlight_core.h"

static const char* kNames[] = {"keyword", "operator", "punctuation", "string", "number",   "constant", "comment",
                               "function", "type",    "variable",    "property", "tag",    "attribute", "embedded"};

static std::u16string toUtf16(const std::string& s) {
    std::u16string out;
    for (size_t i = 0; i < s.size();) {
        unsigned char b = static_cast<unsigned char>(s[i]);
        uint32_t cp;
        int n;
        if (b < 0x80) { cp = b; n = 1; }
        else if ((b & 0xE0) == 0xC0) { cp = b & 0x1F; n = 2; }
        else if ((b & 0xF0) == 0xE0) { cp = b & 0x0F; n = 3; }
        else { cp = b & 0x07; n = 4; }
        for (int k = 1; k < n && i + k < s.size(); k++) cp = (cp << 6) | (static_cast<unsigned char>(s[i + k]) & 0x3F);
        i += n;
        if (cp >= 0x10000) { cp -= 0x10000; out.push_back(static_cast<char16_t>(0xD800 + (cp >> 10))); out.push_back(static_cast<char16_t>(0xDC00 + (cp & 0x3FF))); }
        else out.push_back(static_cast<char16_t>(cp));
    }
    return out;
}

static std::string toUtf8(const std::u16string& s) {
    std::string out;
    for (size_t i = 0; i < s.size(); i++) {
        uint32_t cp = s[i];
        if (cp >= 0xD800 && cp < 0xDC00 && i + 1 < s.size()) { cp = 0x10000 + ((cp - 0xD800) << 10) + (s[++i] - 0xDC00); }
        if (cp < 0x80) out.push_back(static_cast<char>(cp));
        else if (cp < 0x800) { out.push_back(static_cast<char>(0xC0 | (cp >> 6))); out.push_back(static_cast<char>(0x80 | (cp & 0x3F))); }
        else if (cp < 0x10000) { out.push_back(static_cast<char>(0xE0 | (cp >> 12))); out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 0x3F))); out.push_back(static_cast<char>(0x80 | (cp & 0x3F))); }
        else { out.push_back(static_cast<char>(0xF0 | (cp >> 18))); out.push_back(static_cast<char>(0x80 | ((cp >> 12) & 0x3F))); out.push_back(static_cast<char>(0x80 | ((cp >> 6) & 0x3F))); out.push_back(static_cast<char>(0x80 | (cp & 0x3F))); }
    }
    return out;
}

int main(int argc, char** argv) {
    if (argc < 3) { std::fprintf(stderr, "usage: grammar-check <php> <file> [--tokens]\n"); return 2; }
    const bool list = argc > 3 && std::string(argv[3]) == "--tokens";
    const tomi::Language language = tomi::Language::Php;

    std::ifstream in(argv[2], std::ios::binary);
    std::string code((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());

    const std::string error = tomi::queryError(language);
    if (!error.empty()) { std::printf("QUERY ERROR: %s\n", error.c_str()); return 1; }

    std::vector<int32_t> tokens;
    if (!tomi::tokenize(language, code.data(), code.size(), tokens)) { std::printf("NO TOKENS (failed or too big)\n"); return 1; }

    const std::u16string text = toUtf16(code);
    std::string rendered;
    size_t cursor = 0;
    for (size_t i = 0; i + 2 < tokens.size(); i += 3) {
        size_t s = static_cast<size_t>(tokens[i]), e = static_cast<size_t>(tokens[i + 1]);
        if (s < cursor || e > text.size() || e < s) { std::printf("BAD RUN %zu: %zu..%zu (cursor %zu)\n", i / 3, s, e, cursor); return 1; }
        const std::string slice = toUtf8(text.substr(s, e - s));
        if (list) std::printf("%-12s %5zu..%-5zu %s\n", kNames[tokens[i + 2]], s, e, slice.c_str());
        rendered += toUtf8(text.substr(cursor, s - cursor));
        rendered += std::string("[") + kNames[tokens[i + 2]] + "|" + slice + "]";
        cursor = e;
    }
    rendered += toUtf8(text.substr(cursor));
    if (!list) std::printf("%s\n", rendered.c_str());
    std::printf("-- %zu runs\n", tokens.size() / 3);
    return 0;
}
