// A small syntax highlighter for the code screen. No dependency and no network: a hand-written
// tokenizer that knows comments, strings, numbers, keywords and, for markup, tags and attributes.
// It is a colouring aid, not a parser: an unknown construct stays uncoloured.
//
// Limits are the ones of Android (CodeLimits.kt): a file over 50 Ko or 2 000 lines is shown
// without colours, and the screen says so.

export type TokenKind = "comment" | "string" | "number" | "keyword" | "literal" | "tag" | "attr" | "fn";
export interface Token {
  start: number;
  end: number;
  kind: TokenKind;
}

export const HIGHLIGHT_MAX_BYTES = 50 * 1024;
export const HIGHLIGHT_MAX_LINES = 2000;

interface Lang {
  line: string[];
  block: [string, string][];
  quotes: string[];
  /** Quotes whose string may run over several lines (template literals, triple quotes). */
  multiline: string[];
  triple?: string[];
  keywords: Set<string>;
  literals: Set<string>;
  /** Keywords are case-insensitive (SQL). */
  ci?: boolean;
  /** `#rrggbb` colours and `@rules` (CSS). */
  css?: boolean;
  /** A string followed by `:` is a key (JSON, YAML flow). */
  keys?: boolean;
  markup?: boolean;
  /** Markdown: headings and fences. */
  markdown?: boolean;
}

const words = (s: string) => new Set(s.split(/\s+/).filter(Boolean));

const C_LIKE = {
  line: ["//"],
  block: [["/*", "*/"]] as [string, string][],
  quotes: ['"', "'"],
  multiline: [] as string[],
};

const LANGS: Record<string, Lang> = {
  javascript: {
    ...C_LIKE,
    quotes: ['"', "'", "`"],
    multiline: ["`"],
    keywords: words(
      "async await break case catch class const continue debugger default delete do else export extends finally for from function if import in instanceof let new of return static super switch this throw try typeof var void while with yield as get set type interface enum implements namespace declare readonly abstract private protected public keyof satisfies",
    ),
    literals: words("true false null undefined NaN Infinity"),
  },
  java: {
    ...C_LIKE,
    keywords: words(
      "abstract assert boolean break byte case catch char class const continue default do double else enum extends final finally float for goto if implements import instanceof int interface long native new package private protected public return short static strictfp super switch synchronized this throw throws transient try var void volatile while record sealed permits",
    ),
    literals: words("true false null"),
  },
  c: {
    ...C_LIKE,
    keywords: words(
      "auto break case char const continue default do double else enum extern float for goto if inline int long register restrict return short signed sizeof static struct switch typedef union unsigned void volatile while class namespace template typename public private protected virtual override new delete using bool nullptr constexpr",
    ),
    literals: words("true false NULL nullptr"),
  },
  go: {
    ...C_LIKE,
    quotes: ['"', "'", "`"],
    multiline: ["`"],
    keywords: words(
      "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
    ),
    literals: words("true false nil iota"),
  },
  rust: {
    ...C_LIKE,
    keywords: words(
      "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while",
    ),
    literals: words("true false None Some Ok Err"),
  },
  php: {
    line: ["//", "#"],
    block: [["/*", "*/"]],
    quotes: ['"', "'"],
    multiline: [],
    keywords: words(
      "abstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile extends final finally fn for foreach function global if implements include include_once instanceof interface isset list match namespace new or print private protected public readonly require require_once return static switch throw trait try unset use var while xor yield",
    ),
    literals: words("true false null TRUE FALSE NULL"),
  },
  swiftlike: {
    ...C_LIKE,
    keywords: words(
      "as break case catch class continue default defer do else enum extension fallthrough fileprivate final for func guard if import in init inout internal is let open operator private protocol public repeat rethrows return self Self static struct subscript super switch throw throws try typealias var where while fun val object when override abstract data sealed suspend lateinit by companion interface package using namespace void int string bool float double long char new delegate event readonly async await partial record base",
    ),
    literals: words("true false nil null"),
  },
  python: {
    line: ["#"],
    block: [],
    quotes: ['"', "'"],
    multiline: [],
    triple: ['"""', "'''"],
    keywords: words(
      "and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case",
    ),
    literals: words("True False None self"),
  },
  ruby: {
    line: ["#"],
    block: [["=begin", "=end"]],
    quotes: ['"', "'"],
    multiline: [],
    keywords: words(
      "alias and begin break case class def defined do else elsif end ensure for if in module next not or redo rescue retry return super then undef unless until when while yield require include extend attr_accessor attr_reader attr_writer",
    ),
    literals: words("true false nil self"),
  },
  shell: {
    line: ["#"],
    block: [],
    quotes: ['"', "'"],
    multiline: [],
    keywords: words(
      "if then else elif fi for while until do done case esac in function select time return exit export local readonly unset shift source alias echo cd set eval exec trap test param begin process end foreach switch try catch finally",
    ),
    literals: words("true false"),
  },
  yaml: {
    line: ["#"],
    block: [],
    quotes: ['"', "'"],
    multiline: [],
    keys: true,
    keywords: new Set(),
    literals: words("true false null yes no on off True False"),
  },
  toml: {
    line: ["#"],
    block: [],
    quotes: ['"', "'"],
    multiline: [],
    triple: ['"""', "'''"],
    keywords: new Set(),
    literals: words("true false"),
  },
  json: {
    line: [],
    block: [],
    quotes: ['"'],
    multiline: [],
    keys: true,
    keywords: new Set(),
    literals: words("true false null"),
  },
  css: {
    line: [],
    block: [["/*", "*/"]],
    quotes: ['"', "'"],
    multiline: [],
    css: true,
    keywords: new Set(),
    literals: new Set(),
  },
  sql: {
    line: ["--"],
    block: [["/*", "*/"]],
    quotes: ["'", '"'],
    multiline: [],
    ci: true,
    keywords: words(
      "select from where and or not in is like between join inner left right outer full cross on as group by order having limit offset insert into values update set delete create alter drop table index view database primary key foreign references unique default check constraint union all distinct case when then else end exists asc desc with returning begin commit rollback",
    ),
    literals: words("null true false"),
  },
  markup: {
    line: [],
    block: [["<!--", "-->"]],
    quotes: ['"', "'"],
    multiline: [],
    markup: true,
    keywords: new Set(),
    literals: new Set(),
  },
  markdown: {
    line: [],
    block: [],
    quotes: [],
    multiline: [],
    markdown: true,
    keywords: new Set(),
    literals: new Set(),
  },
};

const BY_EXTENSION: Record<string, keyof typeof LANGS> = {
  js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "javascript",
  ts: "javascript", mts: "javascript", cts: "javascript", tsx: "javascript",
  java: "java", c: "c", h: "c", cpp: "c", cc: "c", cxx: "c", hpp: "c", hh: "c", cs: "swiftlike",
  go: "go", rs: "rust", php: "php", phtml: "php",
  kt: "swiftlike", kts: "swiftlike", swift: "swiftlike", dart: "swiftlike", scala: "swiftlike", gradle: "swiftlike",
  py: "python", rb: "ruby",
  sh: "shell", bash: "shell", zsh: "shell", ps1: "shell",
  yaml: "yaml", yml: "yaml", toml: "toml", ini: "toml",
  json: "json", css: "css", sql: "sql",
  html: "markup", htm: "markup", xml: "markup", svelte: "markup", vue: "markup",
  md: "markdown", markdown: "markdown",
};

/** The grammar of a file extension, or null when there is none (the file is shown without colours). */
export function languageOf(extension: string | null): string | null {
  return extension ? (BY_EXTENSION[extension] ?? null) : null;
}

const NUMBER = /0[xX][0-9a-fA-F_]+|0[bB][01_]+|\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?[a-zA-Z%]{0,3}/y;
const WORD = /[A-Za-z_$][\w$]*/y;
const CSS_WORD = /[A-Za-z_-][\w-]*/y;
const HEX_COLOR = /#[0-9a-fA-F]{3,8}\b/y;
const AT_RULE = /@[A-Za-z-]+/y;
const TAG = /<\/?[A-Za-z][\w:.-]*/y;
const ATTR = /[A-Za-z_:@#][\w:.@#-]*/y;

function matchAt(re: RegExp, text: string, at: number): string | null {
  re.lastIndex = at;
  const m = re.exec(text);
  return m ? m[0] : null;
}

/** Index just after the string that starts at `i` with the quote `q`. */
function scanString(text: string, i: number, q: string, multiline: boolean): number {
  let j = i + 1;
  const n = text.length;
  while (j < n) {
    const c = text[j];
    if (c === "\\") j += 2;
    else if (c === q) return j + 1;
    else if (c === "\n" && !multiline) return j;
    else j++;
  }
  return n;
}

/** Colours `text` with the grammar `lang`. Tokens are in order and never overlap. */
export function highlight(text: string, lang: string): Token[] {
  const def = LANGS[lang];
  if (!def) return [];
  if (def.markdown) return markdownTokens(text);
  const out: Token[] = [];
  const n = text.length;
  let i = 0;
  let inTag = false;

  const push = (start: number, end: number, kind: TokenKind) => {
    if (end > start) out.push({ start, end, kind });
  };

  scan: while (i < n) {
    const c = text[i];

    for (const lc of def.line) {
      if (text.startsWith(lc, i)) {
        const nl = text.indexOf("\n", i);
        const end = nl < 0 ? n : nl;
        push(i, end, "comment");
        i = end;
        continue scan;
      }
    }
    for (const [open, close] of def.block) {
      if (text.startsWith(open, i)) {
        const at = text.indexOf(close, i + open.length);
        const end = at < 0 ? n : at + close.length;
        push(i, end, "comment");
        i = end;
        continue scan;
      }
    }
    for (const t of def.triple ?? []) {
      if (text.startsWith(t, i)) {
        const at = text.indexOf(t, i + 3);
        const end = at < 0 ? n : at + 3;
        push(i, end, "string");
        i = end;
        continue scan;
      }
    }

    if (def.markup) {
      if (!inTag) {
        const tag = matchAt(TAG, text, i);
        if (tag) {
          push(i, i + tag.length, "tag");
          i += tag.length;
          inTag = true;
          continue;
        }
        i++;
        continue;
      }
      if (c === ">" || (c === "/" && text[i + 1] === ">")) {
        const end = c === ">" ? i + 1 : i + 2;
        push(i, end, "tag");
        i = end;
        inTag = false;
        continue;
      }
      if (c === '"' || c === "'") {
        const end = scanString(text, i, c, true);
        push(i, end, "string");
        i = end;
        continue;
      }
      const attr = matchAt(ATTR, text, i);
      if (attr) {
        push(i, i + attr.length, "attr");
        i += attr.length;
        continue;
      }
      i++;
      continue;
    }

    if (def.quotes.includes(c)) {
      const end = scanString(text, i, c, def.multiline.includes(c));
      let kind: TokenKind = "string";
      if (def.keys) {
        let k = end;
        while (k < n && (text[k] === " " || text[k] === "\t")) k++;
        if (text[k] === ":") kind = "attr";
      }
      push(i, end, kind);
      i = end;
      continue;
    }

    if (def.css) {
      const color = c === "#" ? matchAt(HEX_COLOR, text, i) : null;
      if (color) {
        push(i, i + color.length, "number");
        i += color.length;
        continue;
      }
      const rule = c === "@" ? matchAt(AT_RULE, text, i) : null;
      if (rule) {
        push(i, i + rule.length, "keyword");
        i += rule.length;
        continue;
      }
    }

    if (c >= "0" && c <= "9") {
      const num = matchAt(NUMBER, text, i);
      if (num) {
        push(i, i + num.length, "number");
        i += num.length;
        continue;
      }
    }

    const word = matchAt(def.css ? CSS_WORD : WORD, text, i);
    if (word) {
      const key = def.ci ? word.toLowerCase() : word;
      if (def.keywords.has(key)) push(i, i + word.length, "keyword");
      else if (def.literals.has(word)) push(i, i + word.length, "literal");
      else {
        let k = i + word.length;
        while (k < n && text[k] === " ") k++;
        if (text[k] === "(" && !def.css) push(i, i + word.length, "fn");
        // YAML / TOML keys: a word followed by a colon or an equals sign at the start of a line.
        else if (def.keys && text[k] === ":") push(i, i + word.length, "attr");
      }
      i += word.length;
      continue;
    }
    i++;
  }
  return out;
}

/** Headings, fences and inline code of a Markdown file. */
function markdownTokens(text: string): Token[] {
  const out: Token[] = [];
  let pos = 0;
  let fenced = false;
  for (const line of text.split("\n")) {
    const end = pos + line.length;
    if (/^\s*(```|~~~)/.test(line)) {
      out.push({ start: pos, end, kind: "string" });
      fenced = !fenced;
    } else if (fenced) {
      out.push({ start: pos, end, kind: "string" });
    } else if (/^#{1,6}\s/.test(line)) {
      out.push({ start: pos, end, kind: "keyword" });
    } else {
      const re = /`[^`\n]+`/g;
      let m: RegExpExecArray | null;
      while ((m = re.exec(line))) {
        out.push({ start: pos + m.index, end: pos + m.index + m[0].length, kind: "string" });
      }
    }
    pos = end + 1;
  }
  return out;
}
