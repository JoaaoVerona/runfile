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

	// Newlines are not extras: they end lines.
	extras: () => [/[ \t]+/],

	externals: ($) => [
		$._newline,
		$._exec_keyword,
		$._capture_exec_keyword,
		$.exec_content,
		$.run_word,
	],

	word: ($) => $.identifier,

	// A list may span lines, and a newline between an element and the closing
	// bracket can belong to either the separator or the closer. Both readings
	// produce the same tree, since newlines are hidden, so GLR is allowed to
	// pick either.
	conflicts: ($) => [[$.list]],

	rules: {
		source_file: ($) => repeat($._line),

		_line: ($) =>
			choice(
				$._newline,
				$.comment,
				$.property,
				$.shell_line,
				$.exec_block,
				$.let_statement,
				$.assignment,
				$.if_statement,
				$.for_statement,
				$.match_statement,
				$.run_statement,
				$.expression_statement,
			),

		// A whole line, never trailing: the lexer rejects `#` inside an expression.
		comment: ($) => seq(/#[^\r\n]*/, $._newline),

		// ---- properties

		property: ($) =>
			seq(".", field("name", $.property_name), optional(seq("=", field("value", $._expression))), $._newline),

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
				$._newline,
				optional(field("body", $.exec_body)),
				"end",
				$._newline,
			),

		command: ($) => repeat1(choice($.shell_content, alias($._lone_brace, $.shell_content), $.interpolation, $.line_continuation)),

		// Body text comes from the scanner in runs that stop at `{{`, so an
		// interpolation inside a body is a node of its own.
		exec_body: ($) => repeat1(choice($.exec_content, $.interpolation)),

		// ---- statements

		let_statement: ($) => seq("let", field("name", $.identifier), "=", field("value", choice($.capture, $._expression)), $._newline),

		assignment: ($) => seq(field("name", $.identifier), "=", field("value", choice($.capture, $._expression)), $._newline),

		// `$` or `exec` in value position: what the command prints.
		capture: ($) => choice($.shell_capture, $.exec_capture),
		shell_capture: ($) => seq("$", optional($.shell_text)),
		exec_capture: ($) =>
			seq(alias($._capture_exec_keyword, "exec"), field("command", $.command), $._newline, optional(field("body", $.exec_body)), "end"),

		if_statement: ($) =>
			seq(
				"if",
				field("condition", $._expression),
				$._newline,
				repeat($._line),
				optional(seq("else", $._newline, repeat($._line))),
				"end",
				$._newline,
			),

		for_statement: ($) =>
			seq("for", field("variable", $.identifier), "in", field("iterable", $._expression), $._newline, repeat($._line), "end", $._newline),

		match_statement: ($) =>
			seq(
				"match",
				field("subject", $._expression),
				$._newline,
				repeat(choice($._newline, $.comment)),
				repeat($.match_case),
				optional($.match_default),
				"end",
				$._newline,
			),

		// A label is a string, always quoted: it is compared against a value, and
		// `RUN.os` is a string like any other.
		match_case: ($) => seq("case", field("label", $.string), $._newline, repeat($._line)),
		match_default: ($) => seq("default", $._newline, repeat($._line)),

		run_statement: ($) => seq("run", field("target", alias($.run_word, $.target)), repeat(alias($.run_word, $.argument)), $._newline),

		expression_statement: ($) => seq($._expression, $._newline),

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
				repeat($._newline),
				optional(
					seq(
						$._expression,
						repeat(seq(repeat($._newline), ",", repeat($._newline), $._expression)),
						optional(seq(repeat($._newline), ",")),
					),
				),
				repeat($._newline),
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
