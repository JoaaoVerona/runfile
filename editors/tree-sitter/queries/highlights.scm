; Keywords
["let" "if" "else" "end" "for" "in" "match" "case" "default" "run" "exec"
 ; `every` is a token only inside a `retry` header, so naming it here
 ; cannot colour a binding that happens to share the name.
 "retry" "every"] @keyword

(code_of "code_of" @function)

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

(call_expression function: (identifier) @function.call)

(run_statement target: (target) @function.call)
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
