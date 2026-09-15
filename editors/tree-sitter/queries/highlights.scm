; Keywords
["let" "do" "if" "else" "end" "for" "in" "while" "until" "loop" "break" "continue"
 "match" "case" "default" "run" "exec" "detach" "parallel"
 ; `every` is a token only inside a `retry` header, so naming it here
 ; cannot colour a binding that happens to share the name.
 "retry" "every"] @keyword

(structured_format) @keyword

; Only a function the language has -- see `call_expression` below.
((capture_call name: (identifier) @function)
 (#any-of? @function
  "abs" "append" "base64_decode" "base64_encode" "basename" "capitalize" "ceil" "code_of" "concat"
  "concat_lists" "confirm" "contains" "decrypt" "directory_exists" "dirname" "ends_with" "error"
  "escape" "exit" "extname" "file_exists" "first" "flatten" "floor" "glob" "index_of"
  "is_executable" "is_number" "join" "join_path" "json_encode" "json_format" "json_get" "json_keys"
  "json_query" "json_set" "json_type" "last" "length" "lines" "max" "md5" "min" "now" "number"
  "one_of" "power" "prepend" "print" "printf" "range" "read_file" "regex_capture"
  "regex_capture_all" "regex_matches" "regex_remove" "regex_replace" "remove_all" "remove_prefix"
  "remove_suffix" "repeat" "replace_all" "reverse" "round" "sha256" "sleep" "slice" "sort" "split"
  "starts_with" "stem" "substring" "temp_dir" "temp_file" "to_lower" "to_upper" "trim" "trim_end"
  "trim_start" "try" "unique" "url_decode" "url_encode" "uuid" "without" "write_file" "zip"))

; Where values come from
["ARG" "ENV" "FLAG" "RUN" "ARGS"] @variable.builtin

(comment) @comment

(property "." @punctuation.special)
(property_name) @property

; Shell is marked, and the marker is the point
(shell_line "$" @keyword)
(shell_capture "$" @keyword)
(shell_content) @string.special
(exec_content) @string.special
(line_continuation) @punctuation.special

(let_statement name: (identifier) @variable)
(assignment name: (identifier) @variable)
(for_statement variable: (identifier) @variable)
(identifier) @variable

; A call is coloured as one only when it names a function the language has, so
; `exists(…)` does not look like a call that will work: it keeps the colour of
; the plain word it is. The error itself is `run :lsp`'s, since no capture is
; drawn as one by every editor. The list is the runner's own `FUNCTIONS`, with
; `code_of` and `try`, and a test in `runfile-lang` holds the two together.
((call_expression function: (identifier) @function.call)
 (#any-of? @function.call
  "abs" "append" "base64_decode" "base64_encode" "basename" "capitalize" "ceil" "code_of" "concat"
  "concat_lists" "confirm" "contains" "decrypt" "directory_exists" "dirname" "ends_with" "error"
  "escape" "exit" "extname" "file_exists" "first" "flatten" "floor" "glob" "index_of"
  "is_executable" "is_number" "join" "join_path" "json_encode" "json_format" "json_get" "json_keys"
  "json_query" "json_set" "json_type" "last" "length" "lines" "max" "md5" "min" "now" "number"
  "one_of" "power" "prepend" "print" "printf" "range" "read_file" "regex_capture"
  "regex_capture_all" "regex_matches" "regex_remove" "regex_replace" "remove_all" "remove_prefix"
  "remove_suffix" "repeat" "replace_all" "reverse" "round" "sha256" "sleep" "slice" "sort" "split"
  "starts_with" "stem" "substring" "temp_dir" "temp_file" "to_lower" "to_upper" "trim" "trim_end"
  "trim_start" "try" "unique" "url_decode" "url_encode" "uuid" "without" "write_file" "zip"))

(run_statement target: (target) @function.call)
(dispatch target: (target) @function.call)
(argument) @string.special

(string) @string
(raw_string) @string
(string_content) @string
(escape_sequence) @string.escape
(number) @number
(boolean) @boolean


(interpolation ["{{" "}}"] @punctuation.special)

["?" "||" "&&" "==" "!=" "<" "<=" ">" ">=" "+" "-" "*" "/" "%" "!" "="] @operator

["(" ")" "[" "]"] @punctuation.bracket
["," "."] @punctuation.delimiter
