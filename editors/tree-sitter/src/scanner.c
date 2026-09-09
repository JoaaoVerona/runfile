// External scanner for the three rules an EBNF cannot express, plus newlines.
//
//   1. `exec` closes on an `end` at the opener's indentation, and only there:
//      a deeper `end` is body text. The scanner records the indentation of the
//      line each `exec` sits on and stops the body before a line that is
//      exactly that indentation followed by `end`.
//   2. A `$` or `exec` body stops at `{{`, so an interpolation inside it is a
//      node the grammar parses as an expression rather than opaque text.
//   3. A `run` argument is one whitespace-delimited word; a `{{ … }}` inside it
//      is kept whole, quotes and nested braces included.
//
// Newlines are external too, only so that a file without a trailing one still
// ends its last line.

#include "tree_sitter/parser.h"

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

enum TokenType {
	NEWLINE,
	EXEC_KEYWORD,
	CAPTURE_EXEC_KEYWORD,
	EXEC_CONTENT,
	RUN_WORD,
	DISPATCH_WORD,
	STRUCTURED_KEYWORD,
};

#define MAX_INDENT 64

typedef struct {
	// Indentation of the line being scanned, recorded at column 0. A capture
	// `exec` sits mid-line, so this is how its block learns where it opened.
	uint8_t line_indent_len;
	char line_indent[MAX_INDENT];
	// Indentation of the open `exec` block's opener line.
	uint8_t exec_indent_len;
	char exec_indent[MAX_INDENT];
	// A zero-width newline is handed out once at end of file; a second one
	// would loop forever.
	bool eof_newline_emitted;
} Scanner;

static inline void advance(TSLexer *lexer) { lexer->advance(lexer, false); }
static inline void skip(TSLexer *lexer) { lexer->advance(lexer, true); }
static inline bool at_eol(TSLexer *lexer) {
	return lexer->lookahead == '\n' || lexer->lookahead == '\r' || lexer->eof(lexer);
}
static inline bool is_blank(int32_t c) { return c == ' ' || c == '\t'; }

// Consume the indentation at the current position into `buf`, as whitespace
// that precedes the token rather than part of it.
static uint8_t read_indent(TSLexer *lexer, char *buf) {
	uint8_t n = 0;
	while (is_blank(lexer->lookahead)) {
		if (n < MAX_INDENT) buf[n] = (char)lexer->lookahead;
		n++;
		skip(lexer);
	}
	return n > MAX_INDENT ? MAX_INDENT : n;
}

// `exec` followed by a blank: the keyword, not an identifier that starts with
// those letters. Leaves the lexer just past the word.
static bool read_word(TSLexer *lexer, const char *kw, bool blank_after) {
	for (int i = 0; kw[i]; i++) {
		if (lexer->lookahead != kw[i]) return false;
		advance(lexer);
	}
	return blank_after ? is_blank(lexer->lookahead) : !is_blank(lexer->lookahead);
}

static bool read_exec_word(TSLexer *lexer) {
	return read_word(lexer, "exec", true);
}

// `json` alone on the rest of the line: the whole word, with nothing after it
// but the newline. One more name here is all a second format needs.
static bool read_structured_word(TSLexer *lexer) {
	const char *formats[] = {"json", NULL};
	for (int f = 0; formats[f]; f++) {
		if (read_word(lexer, formats[f], false)) return true;
	}
	return false;
}

static bool scan_newline(Scanner *s, TSLexer *lexer) {
	bool matched = false;
	if (lexer->lookahead == '\r') {
		advance(lexer);
		if (lexer->lookahead == '\n') advance(lexer);
		matched = true;
	} else if (lexer->lookahead == '\n') {
		advance(lexer);
		matched = true;
	}
	if (matched) {
		lexer->mark_end(lexer);
		// Look past the token at the next line's indentation, so a capture
		// `exec` later on that line knows where it opened. Only a successful
		// token carries scanner state forward -- what a false return records
		// is discarded -- which is why this lives here rather than in the
		// column-0 check that says no to every line that is not an `exec`.
		s->line_indent_len = 0;
		while (is_blank(lexer->lookahead)) {
			if (s->line_indent_len < MAX_INDENT) s->line_indent[s->line_indent_len] = (char)lexer->lookahead;
			s->line_indent_len++;
			advance(lexer);
		}
		if (s->line_indent_len > MAX_INDENT) s->line_indent_len = MAX_INDENT;
		lexer->result_symbol = NEWLINE;
		s->eof_newline_emitted = false;
		return true;
	}
	if (lexer->eof(lexer) && !s->eof_newline_emitted) {
		s->eof_newline_emitted = true;
		lexer->result_symbol = NEWLINE;
		return true;
	}
	return false;
}

// At column 0 of any statement line. Records the line's indentation whether
// or not it turns out to be an `exec` line, so a capture later on the same
// line knows where it opened.
static bool scan_exec_keyword(Scanner *s, TSLexer *lexer) {
	if (lexer->get_column(lexer) != 0) return false;
	s->line_indent_len = read_indent(lexer, s->line_indent);
	if (!read_exec_word(lexer)) return false;
	lexer->mark_end(lexer);
	s->exec_indent_len = s->line_indent_len;
	memcpy(s->exec_indent, s->line_indent, MAX_INDENT);
	lexer->result_symbol = EXEC_KEYWORD;
	return true;
}

// After `=` in a binding: the same block, opened from the middle of a line.
static bool scan_capture_exec_keyword(Scanner *s, TSLexer *lexer) {
	while (is_blank(lexer->lookahead)) skip(lexer);
	if (!read_exec_word(lexer)) return false;
	lexer->mark_end(lexer);
	s->exec_indent_len = s->line_indent_len;
	memcpy(s->exec_indent, s->line_indent, MAX_INDENT);
	lexer->result_symbol = CAPTURE_EXEC_KEYWORD;
	return true;
}

// `json` after `=` in a binding: the same block shape as a capture `exec`,
// opened from the middle of a line, so it records the same indentation.
static bool scan_structured_keyword(Scanner *s, TSLexer *lexer) {
	while (is_blank(lexer->lookahead)) skip(lexer);
	if (!read_structured_word(lexer)) return false;
	lexer->mark_end(lexer);
	s->exec_indent_len = s->line_indent_len;
	memcpy(s->exec_indent, s->line_indent, MAX_INDENT);
	lexer->result_symbol = STRUCTURED_KEYWORD;
	return true;
}

// Is the line at the current position the one that closes the open block?
// Exactly the opener's indentation then `end`; trailing blanks are allowed
// only when the opener was not indented, matching the parser.
static bool at_terminator(Scanner *s, TSLexer *lexer) {
	for (uint8_t i = 0; i < s->exec_indent_len; i++) {
		if (lexer->lookahead != s->exec_indent[i]) return false;
		advance(lexer);
	}
	const char *kw = "end";
	for (int i = 0; kw[i]; i++) {
		if (lexer->lookahead != kw[i]) return false;
		advance(lexer);
	}
	if (s->exec_indent_len == 0) {
		while (is_blank(lexer->lookahead)) advance(lexer);
	}
	return at_eol(lexer);
}

// Body text up to the next `{{`, the closing `end` line, or end of file. The
// token end is marked before every speculative read, so what was read to
// decide is never accidentally kept.
static bool scan_exec_content(Scanner *s, TSLexer *lexer) {
	bool any = false;
	for (;;) {
		if (lexer->get_column(lexer) == 0) {
			lexer->mark_end(lexer);
			if (at_terminator(s, lexer)) break;
			// Whatever the check read is body text.
			if (lexer->get_column(lexer) > 0) any = true;
		}
		if (lexer->eof(lexer)) {
			lexer->mark_end(lexer);
			break;
		}
		if (lexer->lookahead == '{') {
			lexer->mark_end(lexer);
			advance(lexer);
			if (lexer->lookahead == '{') break;
			any = true;
			lexer->mark_end(lexer);
			continue;
		}
		advance(lexer);
		any = true;
		lexer->mark_end(lexer);
	}
	if (!any) return false;
	lexer->result_symbol = EXEC_CONTENT;
	return true;
}

// Skip a `{{ … }}` whose opener has just been consumed: nested braces count,
// and a string inside is skipped whole so a `}}` in it does not close early.
static void skip_interpolation(TSLexer *lexer) {
	int depth = 1;
	while (depth > 0 && !at_eol(lexer)) {
		int32_t c = lexer->lookahead;
		if (c == '"') {
			advance(lexer);
			while (!at_eol(lexer) && lexer->lookahead != '"') {
				if (lexer->lookahead == '\\') advance(lexer);
				if (!at_eol(lexer)) advance(lexer);
			}
			if (lexer->lookahead == '"') advance(lexer);
		} else if (c == '{') {
			advance(lexer);
			if (lexer->lookahead == '{') {
				advance(lexer);
				depth++;
			}
		} else if (c == '}') {
			advance(lexer);
			if (lexer->lookahead == '}') {
				advance(lexer);
				depth--;
			}
		} else {
			advance(lexer);
		}
	}
}

static bool scan_run_word(TSLexer *lexer) {
	while (is_blank(lexer->lookahead)) skip(lexer);
	if (at_eol(lexer)) return false;
	while (!at_eol(lexer) && !is_blank(lexer->lookahead)) {
		if (lexer->lookahead == '{') {
			advance(lexer);
			if (lexer->lookahead == '{') {
				advance(lexer);
				skip_interpolation(lexer);
			}
			continue;
		}
		advance(lexer);
	}
	lexer->mark_end(lexer);
	lexer->result_symbol = RUN_WORD;
	return true;
}

// A `run` word inside `code_of(…)`. The same word, except that the `)` closing
// the call ends it -- unless a `(` in the word opened it, which is how the
// parser reads one too.
static bool scan_dispatch_word(TSLexer *lexer) {
	while (is_blank(lexer->lookahead)) skip(lexer);
	if (at_eol(lexer) || lexer->lookahead == ')') return false;
	int depth = 0;
	while (!at_eol(lexer) && !is_blank(lexer->lookahead)) {
		if (lexer->lookahead == ')') {
			if (depth == 0) break;
			depth--;
		} else if (lexer->lookahead == '(') {
			depth++;
		} else if (lexer->lookahead == '{') {
			advance(lexer);
			if (lexer->lookahead == '{') {
				advance(lexer);
				skip_interpolation(lexer);
			}
			continue;
		}
		advance(lexer);
	}
	lexer->mark_end(lexer);
	lexer->result_symbol = DISPATCH_WORD;
	return true;
}

void *tree_sitter_runfile_external_scanner_create(void) {
	Scanner *s = calloc(1, sizeof(Scanner));
	return s;
}

void tree_sitter_runfile_external_scanner_destroy(void *payload) { free(payload); }

unsigned tree_sitter_runfile_external_scanner_serialize(void *payload, char *buffer) {
	memcpy(buffer, payload, sizeof(Scanner));
	return sizeof(Scanner);
}

void tree_sitter_runfile_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
	Scanner *s = payload;
	if (length == sizeof(Scanner)) {
		memcpy(s, buffer, sizeof(Scanner));
	} else {
		memset(s, 0, sizeof(Scanner));
	}
}

bool tree_sitter_runfile_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
	Scanner *s = payload;
	// Order matters: the newline check never consumes anything unless it
	// succeeds, so it is safe to run first; the keyword checks skip blanks and
	// are only tried when nothing else could be wanted at that position.
	if (valid[NEWLINE] && scan_newline(s, lexer)) return true;
	if (valid[EXEC_CONTENT]) return scan_exec_content(s, lexer);
	if (valid[RUN_WORD]) return scan_run_word(lexer);
	if (valid[DISPATCH_WORD]) return scan_dispatch_word(lexer);
	if (valid[STRUCTURED_KEYWORD] && scan_structured_keyword(s, lexer)) return true;
	if (valid[CAPTURE_EXEC_KEYWORD]) return scan_capture_exec_keyword(s, lexer);
	if (valid[EXEC_KEYWORD]) return scan_exec_keyword(s, lexer);
	return false;
}
