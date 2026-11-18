cmake_minimum_required(VERSION 3.22.1)
project(tomi_highlight C CXX)

# libtomi_highlight.so: tree-sitter runtime + the PHP grammar that Enriched Markdown does not bundle, and the code that
# runs their highlight queries (highlight_core.cpp). The sources of the runtime and of the grammars are NOT in this repository: the
# Gradle task `fetchGrammarSources` clones them at a pinned commit (see app/build.gradle.kts) into TOMI_GRAMMARS_DIR.
if(NOT DEFINED TOMI_GRAMMARS_DIR)
    message(FATAL_ERROR "TOMI_GRAMMARS_DIR is not set (run through Gradle, or see tools/grammar-check/README.md)")
endif()
set(TS_RUNTIME "${TOMI_GRAMMARS_DIR}/tree-sitter")
set(TS_PHP "${TOMI_GRAMMARS_DIR}/tree-sitter-php")

# Highlight queries -> a generated header (raw string literals).
function(embed_query var file)
    file(READ "${CMAKE_CURRENT_SOURCE_DIR}/queries/${file}" content)
    set(${var} "static const char ${var}[] = R\"TSQUERY(${content})TSQUERY\";\n" PARENT_SCOPE)
endfunction()
embed_query(kPhpQuery php.scm)
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/generated/queries_gen.h" "${kPhpQuery}")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/queries/php.scm")

# Size over speed for the generated parsers (even in a debug build: -O0 -g on a 7 MB parser.c is several times bigger).
set(GRAMMAR_FLAGS -Os -g0 -fvisibility=hidden -w)

add_library(ts_runtime STATIC "${TS_RUNTIME}/lib/src/lib.c")
target_include_directories(ts_runtime PUBLIC "${TS_RUNTIME}/lib/include" PRIVATE "${TS_RUNTIME}/lib/src")
target_compile_definitions(ts_runtime PRIVATE _POSIX_C_SOURCE=200112L _DEFAULT_SOURCE)
target_compile_options(ts_runtime PRIVATE ${GRAMMAR_FLAGS})

add_library(ts_php STATIC "${TS_PHP}/php/src/parser.c" "${TS_PHP}/php/src/scanner.c")
target_include_directories(ts_php PRIVATE "${TS_PHP}/php/src")
target_compile_options(ts_php PRIVATE ${GRAMMAR_FLAGS})

add_library(tomi_highlight SHARED highlight_core.cpp jni_bridge.cpp)
target_include_directories(tomi_highlight PRIVATE "${CMAKE_CURRENT_BINARY_DIR}/generated")
target_compile_options(tomi_highlight PRIVATE -Os -g0 -fvisibility=hidden)
target_link_libraries(tomi_highlight PRIVATE ts_runtime ts_php log)
# Do not re-export the static libraries (Enriched Markdown's own library embeds another tree-sitter runtime), 16 KB page alignment
# for Android 15+ devices, no unused code.
target_link_options(tomi_highlight PRIVATE -Wl,--exclude-libs,ALL -Wl,--gc-sections -Wl,-z,max-page-size=16384)
