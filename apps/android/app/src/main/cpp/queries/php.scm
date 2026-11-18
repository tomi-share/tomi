; Highlight query for PHP (grammar: tree-sitter/tree-sitter-php, MIT), derived from its queries/highlights.scm.
;
; Captures are the 14 token types of Enriched Markdown (keyword operator punctuation string number constant comment function type
; variable property tag attribute embedded). A suffix after a dot is ignored.
; On the same text, a LATER pattern wins over an earlier one: general rules first, specific rules after.
; Predicates supported by highlight_core.cpp: #eq? #not-eq? #any-of? #match? #not-match?.

[
  (php_tag)
  (php_end_tag)
] @tag

; Strings come first: a variable interpolated in a string is coloured as a variable by the rules below.
[
  (string)
  (string_content)
  (encapsed_string)
  (heredoc)
  (heredoc_body)
  (nowdoc_body)
] @string

(comment) @comment

(integer) @number
(float) @number
(boolean) @constant
(null) @constant

; Operators and punctuation
[
  "+" "-" "*" "/" "%" "**" "." "="
  "+=" "-=" "*=" "/=" "%=" "**=" ".=" "??="
  "&=" "|=" "^=" "<<=" ">>="
  "==" "===" "!=" "!==" "<>" "<" ">" "<=" ">=" "<=>"
  "&&" "||" "!" "&" "|" "^" "~" "<<" ">>" "??" "|>"
  "++" "--" "->" "?->" "=>" "::" "?" "@" "$"
  "and" "or" "xor"
] @operator

[
  "(" ")" "[" "]" "{" "}" "," ";" ":" "\\" "..." "#["
] @punctuation

; Keywords
[
  "as"
  "break"
  "case"
  "catch"
  "class"
  "clone"
  "const"
  "continue"
  "declare"
  "default"
  "do"
  "echo"
  "else"
  "elseif"
  "enddeclare"
  "endfor"
  "endforeach"
  "endif"
  "endswitch"
  "endwhile"
  "enum"
  "exit"
  "extends"
  "finally"
  "fn"
  "for"
  "foreach"
  "function"
  "global"
  "goto"
  "if"
  "implements"
  "include"
  "include_once"
  "instanceof"
  "insteadof"
  "interface"
  "match"
  "namespace"
  "new"
  "print"
  "require"
  "require_once"
  "return"
  "switch"
  "throw"
  "trait"
  "try"
  "unset"
  "use"
  "while"
  "yield"
  "yield from"
  (abstract_modifier)
  (final_modifier)
  (readonly_modifier)
  (static_modifier)
  (visibility_modifier)
] @keyword

(function_static_declaration "static" @keyword)
(relative_scope) @keyword

; Constants (before the rules for names in a type or call position, which win over it)
((name) @constant
 (#match? @constant "^_?[A-Z][A-Z0-9_]+$"))
((name) @constant
 (#match? @constant "^__[A-Z][A-Z0-9_]+__$"))
(const_declaration (const_element (name) @constant))
(enum_case name: (name) @constant)

; Namespaces and types
(namespace_definition
  name: (namespace_name
    (name) @type))

(namespace_name
  (name) @type)

(namespace_use_clause
  [
    (name) @type
    (qualified_name
      (name) @type)
    alias: (name) @type
  ])

(namespace_use_clause
  type: "function"
  [
    (name) @function
    (qualified_name
      (name) @function)
    alias: (name) @function
  ])

(namespace_use_clause
  type: "const"
  [
    (name) @constant
    (qualified_name
      (name) @constant)
    alias: (name) @constant
  ])

(primitive_type) @type
(cast_type) @type
(named_type [
  (name) @type
  (qualified_name (name) @type)
  (relative_name (name) @type)
])
(named_type (name) @keyword
  (#any-of? @keyword "static" "self"))

(object_creation_expression [
  (name) @type
  (qualified_name (name) @type)
  (relative_name (name) @type)
])

(scoped_call_expression
  scope: [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])

(base_clause [
  (name) @type
  (qualified_name (name) @type)
  (relative_name (name) @type)
])

(class_interface_clause [
  (name) @type
  (qualified_name (name) @type)
  (relative_name (name) @type)
])

(class_declaration name: (name) @type)
(interface_declaration name: (name) @type)
(trait_declaration name: (name) @type)
(enum_declaration name: (name) @type)

; Variables: after the strings, so that "$name" inside a string is a variable.
(variable_name) @variable
((variable_name) @keyword
 (#eq? @keyword "$this"))

; Functions and members
(array_creation_expression "array" @function)
(list_literal "list" @function)

(method_declaration
  name: (name) @function)

(function_call_expression
  function: [
    (qualified_name (name))
    (relative_name (name))
    (name)
  ] @function)

(scoped_call_expression
  name: (name) @function)

(class_constant_access_expression
  . [
    (name) @type
    (qualified_name (name) @type)
    (relative_name (name) @type)
  ])
; Foo::BAR, Level::High: the second name is a constant (or enum case).
(class_constant_access_expression
  (_) . (name) @constant)

(member_call_expression
  name: (name) @function)

(function_definition
  name: (name) @function)

(property_element
  (variable_name) @property)

(member_access_expression
  name: (variable_name (name)) @property)
(member_access_expression
  name: (name) @property)

(attribute
  (name) @attribute)
(attribute
  (qualified_name (name) @attribute))
