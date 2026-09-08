; `$` lines run in a shell -- bash where it exists, sh otherwise.
((shell_line (shell_text) @injection.content)
 (#set! injection.language "bash"))

((shell_capture (shell_text) @injection.content)
 (#set! injection.language "bash"))

((code_of (shell_text) @injection.content)
 (#set! injection.language "bash"))

; An `exec` body is the command's stdin; it is shell only when the command is.
((exec_block
   command: (command) @_command
   body: (exec_body) @injection.content)
 (#match? @_command "^(sh|bash|dash|ash|ksh|zsh|brush)\\b")
 (#set! injection.language "bash"))

((exec_capture
   command: (command) @_command
   body: (exec_body) @injection.content)
 (#match? @_command "^(sh|bash|dash|ash|ksh|zsh|brush)\\b")
 (#set! injection.language "bash"))

; A `json` block's body is JSON, so it is highlighted as JSON. The format node
; carries the name, which is what makes a second format need no query of its own.
((structured
   format: (structured_format) @injection.language
   body: (exec_body) @injection.content))
