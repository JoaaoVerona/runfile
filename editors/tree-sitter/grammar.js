// Tree-sitter grammar for `.run` files.
//
// Mirrors GRAMMAR.ebnf at the repository root. Newlines are significant, so
// they are tokens rather than extras, and every line form ends in one. The
// three rules an EBNF cannot express live in src/scanner.c: a `$` or `exec`
// body stops at `{{`, an `exec` block closes on an `end` at the opener's
// indentation, and a `run` argument is one whitespace-delimited word with its
// interpolations kept whole.

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// One word of an `exec` command line. A `#` may sit *inside* a word -- `a#b`,
// a URL fragment -- but may not begin one, which is where a comment begins.
// The runner reads this line itself, splitting it into words and spawning the
// program directly, so a `#` left in would arrive as an argument nobody meant;
// a `$` line is handed to a shell whole, and its `#` is the shell's to apply.
const COMMAND_WORD = String.raw`(?:[^ \t{#\\\r\n]|\\[^\r\n])(?:[^ \t{\\\r\n]|\\[^\r\n])*`;

// A line's last value, with the end of that line. A capture runs to the end of
// the line it is on -- everything after the `$` is the command's, `#` included
// -- so a line that ends in one ends there, with nothing of ours after it.
// `code_of($ x)` is the same shape and the same rule: the `)` has to be the
// last character of the line, which is why neither takes a comment.
const endsLine = ($, name, shell, own) =>
	choice(seq(field(name, shell), $._newline), seq(field(name, own), $._eol));

const PREC = {
	chain: 1,
	or: 2,
	and: 3,
	compare: 4,
	add: 5,
	multiply: 6,
	unary: 7,
	postfix: 8,
};

module.exports = grammar({
	name: "runfile",

	// Newlines are not extras: they end lines. Neither is a comment, though it
	// may end any of them: as an extra it would be found *inside* a string and
	// inside a `$` line, whose `#` is the shell's. `_eol` is where one may go,
	// and that is every line this language reads and no line it hands over.
	//
	// It is external all the same, because where a comment starts is a rule
	// about the character *before* the `#` -- one that begins a word -- which
	// a regex here cannot see: by the time one matched, the whitespace in
	// front of it would already have been skipped.
	extras: () => [/[ \t]+/],

	externals: ($) => [
		$._newline,
		$.comment,
		$._exec_keyword,
		$._capture_exec_keyword,
		$.exec_content,
		$.run_word,
		$.dispatch_word,
		$._structured_keyword,
	],

	word: ($) => $.identifier,

	// A list may span lines, and a newline between an element and the closing
	// bracket can belong to either the separator or the closer. Both readings
	// produce the same tree, since newlines are hidden, so GLR is allowed to
	// pick either.
	//
	// `name(a, b, …` is an ordinary argument list until a `$` turns out to
	// follow the last comma, and a capture can only be the last argument, so
	// nothing before it says which of the two is being read. GLR carries both
	// and the one that cannot finish dies where it stands.
	conflicts: ($) => [[$.list], [$.capture_call, $.arguments]],

	rules: {
		source_file: ($) => repeat($._line),

		// The end of a line the language reads: a comment may close it. Not
		// `shell_line`'s, and not an `exec` body's -- there the `#` belongs to
		// whoever is being handed the text.
		_eol: ($) => seq(optional($.comment), $._newline),

		_line: ($) =>
			choice(
				$._eol,
				$.property,
				$.shell_line,
				$.exec_block,
				$.let_statement,
				$.assignment,
				$.do_statement,
				$.if_statement,
				$.retry_statement,
				$.for_statement,
				$.while_statement,
				$.until_statement,
				$.loop_statement,
				$.break_statement,
				$.continue_statement,
				$.match_statement,
				$.run_statement,
				$.expression_statement,
			),

		// ---- properties

		property: ($) =>
			seq(".", field("name", $.property_name), optional(seq("=", field("value", $._expression))), $._eol),

		property_name: ($) => seq($.identifier, repeat(seq(".", $.identifier))),

		// ---- shell

		shell_line: ($) => seq("$", optional($.shell_text), $._newline),

		shell_text: ($) => repeat1(choice($.shell_content, alias($._lone_brace, $.shell_content), $.interpolation, $.line_continuation)),

		shell_content: () => token.immediate(prec(-1, /([^{\\\r\n]|\\[^\r\n])+/)),
		_lone_brace: () => token.immediate("{"),
		line_continuation: () => token.immediate(/\\\r?\n/),

		exec_block: ($) =>
			seq(
				alias($._exec_keyword, "exec"),
				field("command", $.command),
				$._eol,
				optional(field("body", $.exec_body)),
				"end",
				$._eol,
			),

		// An `exec` command is a line of **words**, which is how the runner
		// reads it too -- and why a comment may end this line while a `$`
		// line's `#` is the shell's. Nothing here is `token.immediate`, so a
		// word ends at the blank after it rather than running to the end of
		// the line; an immediate run, which is what a `$` line's text is,
		// would take the ` # note` with it and leave `_eol` nothing to find.
		command: ($) =>
			repeat1(
				choice(
					alias($._command_word, $.shell_content),
					alias($._command_brace, $.shell_content),
					alias($._command_interpolation, $.interpolation),
					alias($._command_continuation, $.line_continuation),
				),
			),
		_command_word: () => token(prec(-1, new RegExp(COMMAND_WORD))),
		_command_brace: () => "{",
		_command_interpolation: ($) => seq("{{", $._expression, "}}"),
		_command_continuation: () => /\\\r?\n/,

		// Body text comes from the scanner in runs that stop at `{{`, so an
		// interpolation inside a body is a node of its own.
		exec_body: ($) => repeat1(choice($.exec_content, $.interpolation)),

		// ---- statements

		// Several names unpack a list positionally; `_` is a position thrown
		// away, and needs no rule of its own because the identifier pattern
		// already admits it.
		let_statement: ($) =>
			seq(
				"let",
				field("name", $.identifier),
				repeat(seq(",", field("name", $.identifier))),
				"=",
				endsLine($, "value", choice($.capture_call, $.shell_capture), choice($.structured, $.exec_capture, $._expression)),
			),

		// A call whose last argument is a `$` run -- `lines($ git ls-files)`,
		// and `code_of($ cmd)`, which is that same shape wearing a name people
		// already know. Its shell text stops at the `)`; a capture otherwise
		// runs to end of line, which is why it can only ever be the *last*
		// argument, and why a command holding a parenthesis has to be a
		// statement of its own.
		//
		// Only `code_of` may hold a `run` dispatch. That is a rule about what
		// a call means rather than what it looks like -- a dispatched target
		// gives back a status and nothing else -- so the runner states it and
		// this grammar does not: an editor colours `lines(run x)` and the
		// parser is what refuses it.
		capture_call: ($) =>
			seq(
				field("name", $.identifier),
				"(",
				repeat(seq($._expression, ",")),
				choice($._capture_argument, $.dispatch),
				")",
			),
		_capture_argument: ($) => seq("$", optional(alias($._capture_text, $.shell_text))),

		// `code_of(run build)`: the same dispatch the statement spells, scored.
		// Only `code_of` takes one -- a dispatched target writes to the
		// terminal like any other, so its status is the only value it has.
		dispatch: ($) =>
			seq("run", field("target", alias($.dispatch_word, $.target)), repeat(alias($.dispatch_word, $.argument))),
		_capture_text: ($) =>
			repeat1(choice(alias($._capture_content, $.shell_content), alias($._lone_brace, $.shell_content), $.interpolation)),
		_capture_content: () => token.immediate(prec(-1, /([^{)\\\r\n]|\\[^\r\n])+/)),

		assignment: ($) =>
			seq(
				field("name", $.identifier),
				repeat(seq(",", field("name", $.identifier))),
				"=",
				endsLine($, "value", choice($.capture_call, $.shell_capture), choice($.structured, $.exec_capture, $._expression)),
			),

		// `json … end`: a block of structured text, as one value of that format.
		// Its body closes on an `end` at the opener's indentation, like every
		// other body here, so the scanner needs nothing new. Adding YAML or
		// TOML later is one more word in `structured_format`.
		structured: ($) =>
			seq(
				field("format", alias($._structured_keyword, $.structured_format)),
				$._eol,
				optional(field("body", $.exec_body)),
				"end",
			),

		// `$` or `exec` in value position: what the command prints. The two are
		// named apart rather than wrapped in one `capture` node, because the
		// end of their line differs: a `$` capture runs to the end of it, `#`
		// included, while an `exec` one closes on an `end` of its own and takes
		// a comment after it like any other line.
		shell_capture: ($) => seq("$", optional($.shell_text)),
		exec_capture: ($) =>
			seq(alias($._capture_exec_keyword, "exec"), field("command", $.command), $._eol, optional(field("body", $.exec_body)), "end"),

		// `do` … `end`: a block with no condition, so a property has somewhere
		// to go without inventing a question.
		do_statement: ($) => seq("do", $._eol, repeat($._line), "end", $._eol),

		if_statement: ($) =>
			seq(
				"if",
				// A `$` run may stand as the condition: it is true when the
				// command succeeds. Never an `exec` -- its `end` would be the
				// one the `if` wants.
				endsLine($, "condition", $.shell_capture, $._expression),
				repeat($._line),
				optional($._else_clause),
				"end",
				$._eol,
			),

		// `else if` continues the chain rather than opening a block of its
		// own, so one `end` closes however many of them there are. Hidden, so
		// the whole chain is one `if_statement` node -- which is what it is.
		_else_clause: ($) =>
			choice(
				seq(
					"else",
					"if",
					endsLine($, "condition", $.shell_capture, $._expression),
					repeat($._line),
					optional($._else_clause),
				),
				seq("else", $._eol, repeat($._line)),
			),

		// `retry n [every s]` … `[else …]` `end`: the body is run again while it
		// fails, up to n times, and `else` is what to do when it never worked.
		retry_statement: ($) =>
			seq(
				"retry",
				field("attempts", $._expression),
				optional(seq("every", field("delay", $._expression))),
				$._eol,
				repeat($._line),
				optional(seq("else", $._eol, repeat($._line))),
				"end",
				$._eol,
			),

		for_statement: ($) =>
			seq(
				"for",
				field("variable", $.identifier),
				repeat(seq(",", field("variable", $.identifier))),
				"in",
				endsLine($, "iterable", $.capture_call, $._expression),
				repeat($._line),
				"end",
				$._eol,
			),

		// `while` and `until` are the same block asking opposite questions.
		// Written as two rules rather than a `choice` of keywords so the node
		// name says which one is on the screen.
		while_statement: ($) =>
			seq("while", endsLine($, "condition", $.shell_capture, $._expression), repeat($._line), "end", $._eol),

		until_statement: ($) =>
			seq("until", endsLine($, "condition", $.shell_capture, $._expression), repeat($._line), "end", $._eol),

		// Takes no condition: `break` is how it ends.
		loop_statement: ($) => seq("loop", $._eol, repeat($._line), "end", $._eol),

		break_statement: ($) => seq("break", $._eol),
		continue_statement: ($) => seq("continue", $._eol),

		match_statement: ($) =>
			seq(
				"match",
				// A `$` run may stand as the subject, and the cases are then
				// exit codes: `case "0"`, `case "1"`.
				endsLine($, "subject", $.shell_capture, $._expression),
				repeat($._eol),
				repeat($.match_case),
				optional($.match_default),
				"end",
				$._eol,
			),

		// A label is a string, always quoted: it is compared against a value, and
		// `RUN.os` is a string like any other.
		match_case: ($) => seq("case", field("label", $.string), $._eol, repeat($._line)),
		match_default: ($) => seq("default", $._eol, repeat($._line)),

		run_statement: ($) => seq("run", field("target", alias($.run_word, $.target)), repeat(alias($.run_word, $.argument)), $._eol),

		expression_statement: ($) => choice(seq($.capture_call, $._newline), seq($._expression, $._eol)),

		// ---- expressions

		_expression: ($) =>
			choice(
				$.chain_expression,
				$.binary_expression,
				$.unary_expression,
				$.call_expression,
				$.index_expression,
				$._primary,
			),

		// `a ? b`: a, or b when a does not resolve. Not a ternary.
		chain_expression: ($) => prec.left(PREC.chain, seq($._expression, "?", $._expression)),

		binary_expression: ($) =>
			choice(
				prec.left(PREC.or, seq($._expression, "||", $._expression)),
				prec.left(PREC.and, seq($._expression, "&&", $._expression)),
				prec.left(PREC.compare, seq($._expression, choice("==", "!=", "<", "<=", ">", ">="), $._expression)),
				prec.left(PREC.add, seq($._expression, choice("+", "-"), $._expression)),
				prec.left(PREC.multiply, seq($._expression, choice("*", "/", "%"), $._expression)),
			),

		unary_expression: ($) => prec(PREC.unary, seq(choice("!", "-"), $._expression)),

		call_expression: ($) => prec(PREC.postfix, seq(field("function", $.identifier), field("arguments", $.arguments))),
		arguments: ($) => seq("(", optional(seq($._expression, repeat(seq(",", $._expression)), optional(","))), ")"),

		index_expression: ($) => prec(PREC.postfix, seq($._expression, "[", $._expression, "]")),

		_primary: ($) => choice($.number, $.string, $.raw_string, $.boolean, $.list, $.source, $.identifier, $.parenthesized_expression),

		parenthesized_expression: ($) => seq("(", $._expression, ")"),

		// May span lines.
		list: ($) =>
			seq(
				"[",
				repeat($._eol),
				optional(
					seq(
						$._expression,
						repeat(seq(repeat($._eol), ",", repeat($._eol), $._expression)),
						optional(seq(repeat($._eol), ",")),
					),
				),
				repeat($._eol),
				"]",
			),

		source: ($) => choice(seq(field("root", choice("ARG", "ENV", "FLAG", "RUN")), ".", field("key", $.identifier)), "ARGS"),

		boolean: () => choice("true", "false"),
		number: () => /[0-9]+(\.[0-9]+)?/,
		identifier: () => /[A-Za-z_][A-Za-z0-9_-]*/,

		// A string skips `{{ … }}`, so a quote inside an interpolation does not
		// terminate it. The parser gets that for free: the interpolation's
		// expression is parsed as an expression, quotes and all.
		string: ($) => seq('"', repeat(choice($.string_content, alias($._string_brace, $.string_content), $.escape_sequence, $.interpolation)), '"'),
		string_content: () => token.immediate(/[^"\\{]+/),
		_string_brace: () => token.immediate("{"),
		escape_sequence: () => token.immediate(/\\["\\ntr{]/),

		// Keeps its backslashes, but `\"` still does not terminate it.
		raw_string: ($) =>
			seq(
				'r"',
				repeat(choice(alias($.raw_content, $.string_content), alias($._raw_escape, $.string_content), alias($._string_brace, $.string_content), $.interpolation)),
				'"',
			),
		raw_content: () => token.immediate(/[^"\\{]+/),
		_raw_escape: () => token.immediate(/\\./),

		interpolation: ($) => seq("{{", $._expression, "}}"),
	},
});
