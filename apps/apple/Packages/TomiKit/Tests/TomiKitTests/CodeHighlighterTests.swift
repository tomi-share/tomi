import XCTest
@testable import TomiKit

/// The colours of the code screen: what each language marks, and what holds for any text (order, bounds).
final class CodeHighlighterTests: XCTestCase {
    /// The pieces coloured, as "text:kind", in order.
    private func marks(_ text: String, _ language: String) -> [String] {
        let units = Array(text.utf16)
        return CodeHighlighter.tokens(text, language: language).map {
            "\(String(decoding: units[$0.start ..< $0.end], as: UTF16.self)):\($0.kind)"
        }
    }

    // MARK: Languages

    func testTheLanguageComesFromTheExtension() {
        XCTAssertEqual(CodeHighlighter.language(forFileName: "main.rs"), "rust")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "App.TSX"), "tsx")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "deploy.sh"), "bash")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "index.htm"), "html")
        XCTAssertNil(CodeHighlighter.language(forFileName: "Makefile"))
        XCTAssertNil(CodeHighlighter.language(forFileName: "photo.png"))
    }

    func testFilesAndroidLeavesUncolouredAreColouredHere() {
        XCTAssertEqual(CodeHighlighter.language(forFileName: "Main.kt"), "swiftlike")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "View.swift"), "swiftlike")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "app.rb"), "ruby")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "tool.cpp"), "c")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "schema.sql"), "sql")
        XCTAssertEqual(CodeHighlighter.language(forFileName: "Cargo.toml"), "toml")
        XCTAssertNil(CodeHighlighter.language(forFileName: "init.lua"), "a code file, shown without colours")
    }

    func testEveryGrammarOfTheCodeScreenHasItsColours() {
        // The names `CodeLanguages` gives must all be known here: none silently stays uncoloured.
        for name in ["a.rs", "a.js", "a.ts", "a.tsx", "a.css", "a.html", "a.json", "a.py", "a.java", "a.sh", "a.go", "a.c", "a.yaml", "a.md", "a.php"] {
            XCTAssertNotNil(CodeHighlighter.language(forFileName: name), name)
        }
        XCTAssertEqual(CodeHighlighter.tokens("let a = 1", language: "klingon"), [])
    }

    // MARK: What is marked

    func testKeywordsLiteralsNumbersAndCalls() {
        XCTAssertEqual(
            marks("const total = add(1, 0x1F) ?? null;", "javascript"),
            ["const:keyword", "add:function", "1:number", "0x1F:number", "null:literal"]
        )
    }

    func testCommentsToTheEndOfTheLineAndBlocks() {
        XCTAssertEqual(marks("a // if x\nb", "javascript"), ["// if x:comment"])
        XCTAssertEqual(marks("a /* if\n x */ return", "javascript"), ["/* if\n x */:comment", "return:keyword"])
        XCTAssertEqual(marks("/* never closed", "c"), ["/* never closed:comment"])
        XCTAssertEqual(marks("x = 1 -- note", "sql"), ["1:number", "-- note:comment"])
    }

    func testStringsStopAtTheLineEndUnlessTheLanguageLetsThemRun() {
        XCTAssertEqual(marks("a = \"if\nreturn", "javascript"), ["\"if:string", "return:keyword"])
        XCTAssertEqual(marks("a = `if\nreturn`", "javascript"), ["`if\nreturn`:string"])
        XCTAssertEqual(marks("s = \"a\\\"b\" if", "python"), ["\"a\\\"b\":string", "if:keyword"])
        XCTAssertEqual(marks("\"\"\"doc\nif\"\"\" pass", "python"), ["\"\"\"doc\nif\"\"\":string", "pass:keyword"])
    }

    func testNumbers() {
        XCTAssertEqual(marks("1_000 3.14 1e-9 0b1010 10px 50%", "css"), ["1_000:number", "3.14:number", "1e-9:number", "0b1010:number", "10px:number", "50%:number"])
        XCTAssertEqual(marks("abc123", "javascript"), [], "digits inside a name are not a number")
    }

    func testSQLKeywordsWhateverTheirCase() {
        XCTAssertEqual(marks("SELECT name From t", "sql"), ["SELECT:keyword", "From:keyword"])
    }

    func testJSONAndYAMLKeys() {
        XCTAssertEqual(marks("{\"name\": \"tomi\", \"n\": 2, \"ok\": true}", "json"), [
            "\"name\":property", "\"tomi\":string", "\"n\":property", "2:number", "\"ok\":property", "true:literal",
        ])
        XCTAssertEqual(marks("name: tomi\nport: 8080 # http", "yaml"), ["name:property", "port:property", "8080:number", "# http:comment"])
    }

    func testAYAMLValueKeepsItsApostropheAndItsAnchor() {
        XCTAssertEqual(marks("title: it's fine\nurl: http://a/#top", "yaml"), ["title:property", "url:property", "http:property"])
        XCTAssertEqual(marks("title: 'quoted'", "yaml"), ["title:property", "'quoted':string"])
    }

    func testAShellHashInsideAWordIsNotAComment() {
        XCTAssertEqual(marks("echo $# # count", "bash"), ["echo:keyword", "# count:comment"])
        XCTAssertEqual(marks("#!/bin/sh\nif true; then", "bash"), ["#!/bin/sh:comment", "if:keyword", "true:literal", "then:keyword"])
    }

    func testARustLifetimeIsNotAString() {
        XCTAssertEqual(marks("fn get<'a>(x: &'a str) -> char { 'x' }", "rust"), ["fn:keyword", "'x':string"])
        XCTAssertEqual(marks("let c = '\\n'; let d = '😀';", "rust"), ["let:keyword", "'\\n':string", "let:keyword", "'😀':string"])
    }

    func testTOMLKeys() {
        XCTAssertEqual(marks("name = \"tomi\"\nlto = true # speed", "toml"), ["name:property", "\"tomi\":string", "lto:property", "true:literal", "# speed:comment"])
    }

    func testCSS() {
        XCTAssertEqual(marks("@media x { a { color: #fff; margin: 0 } }", "css"), ["@media:keyword", "#fff:number", "0:number"])
        XCTAssertEqual(marks("#header { }", "css"), [], "an id is not a colour")
    }

    func testMarkup() {
        XCTAssertEqual(marks("<a href=\"x\" hidden>if</a><!-- c -->", "html"), [
            "<a:tag", "href:attribute", "\"x\":string", "hidden:attribute", ">:tag", "</a:tag", ">:tag", "<!-- c -->:comment",
        ])
        XCTAssertEqual(marks("a < b", "html"), [])
    }

    func testMarkdown() {
        XCTAssertEqual(marks("# Title\ntext `code` here\n```\nlet a\n```\n#nospace", "markdown"), [
            "# Title:keyword", "`code`:string", "```:string", "let a:string", "```:string",
        ])
    }

    // MARK: Whatever the text

    func testTokensAreInOrderNeverOverlapAndStayInsideTheText() {
        let samples = [
            "", "\n", "'", "\"", "`", "/*", "<!--", "<", "</", "#", "\\", "'\\", "\"\\", "0x", "1e", "1.", "@", "```", "~~~\n",
            "fn main() { let s = \"é😀\"; // ok\n}", "a: 'b\nc: \"d", "<a b='c", "SELECT 'x", "x = '''\n", "=begin\nnever",
            String(repeating: "(", count: 300), "😀😀😀 if 😀", "if\u{0}then", "#!/bin/sh\n\techo \"$1\"",
        ]
        let languages = [
            "javascript", "typescript", "tsx", "java", "c", "go", "rust", "php", "python", "bash", "yaml", "json", "css", "html",
            "markdown", "swiftlike", "ruby", "sql", "toml",
        ]
        for language in languages {
            for sample in samples {
                let length = sample.utf16.count
                var cursor = 0
                for token in CodeHighlighter.tokens(sample, language: language) {
                    XCTAssertGreaterThanOrEqual(token.start, cursor, "\(language): \(sample.debugDescription)")
                    XCTAssertGreaterThan(token.end, token.start, "\(language): \(sample.debugDescription)")
                    XCTAssertLessThanOrEqual(token.end, length, "\(language): \(sample.debugDescription)")
                    cursor = token.end
                }
            }
        }
    }

    func testRunsAreCutAtLineEnds() {
        let text = "a /* one\ntwo */ if"
        let doc = CodeDocument(text)
        let runs = CodeLines.bucketRuns(doc, tokens: CodeHighlighter.tokens(text, language: "c"))
        XCTAssertEqual(runs[0], [LineRun(startCol: 2, endCol: 8, kind: .comment)])
        XCTAssertEqual(runs[1], [LineRun(startCol: 0, endCol: 6, kind: .comment), LineRun(startCol: 7, endCol: 9, kind: .keyword)])
        XCTAssertNil(CodeLines.bucketRuns(CodeDocument("plain\ntext"), tokens: [])[0])
    }

    func testAFileIsColouredOrSaysWhyNot() {
        let short = CodeDocument("let a = 1\nreturn a")
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.js", doc: short).noticeKey, nil)
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.js", doc: short).runs[1], [LineRun(startCol: 0, endCol: 6, kind: .keyword)])
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.lua", doc: short), .noLanguage)
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.lua", doc: short).noticeKey, "code_no_colours_language")
        let long = CodeDocument(String(repeating: "a\n", count: CodeLimits.highlightMaxLines + 1))
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.js", doc: long), .tooLong)
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.js", doc: long).noticeKey, "code_no_colours_size")
        XCTAssertTrue(CodeHighlighter.colours(fileName: "a.js", doc: long).runs.isEmpty)
        // The language first: a long file without a language says so, not that it is too long (Android).
        XCTAssertEqual(CodeHighlighter.colours(fileName: "a.lua", doc: long), .noLanguage)
    }

    func testALongFileIsNotColoured() {
        // The rule is `CodeLimits.canHighlight` (50 KB or 2,000 lines), the same as Android and Windows.
        XCTAssertTrue(CodeLimits.canHighlight(text: "a\nb", lineCount: 2))
        XCTAssertFalse(CodeLimits.canHighlight(text: "a", lineCount: CodeLimits.highlightMaxLines + 1))
        XCTAssertFalse(CodeLimits.canHighlight(text: String(repeating: "a", count: CodeLimits.highlightMaxBytes + 1), lineCount: 1))
    }
}
