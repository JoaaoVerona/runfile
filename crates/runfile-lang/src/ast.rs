//! The `.run` syntax tree.
//!
//! One file is one [`Target`]. Its body is a [`Block`]: a flat statement list
//! whose block-forming statements nest further blocks. Properties are attached
//! to the block they appear in, which is what gives them their scoping.

use crate::span::Span;

/// A whole `.run` file.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
	/// Leading comment block, minus the marker lines. First line is the short form.
	pub description: Option<String>,
	pub body: Block,
}

/// A run of statements plus the properties set within it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block {
	/// In source order, which is what splits them into the two regions below.
	pub properties: Vec<Property>,
	pub statements: Vec<Statement>,
}

impl Block {
	/// How many properties are written above the block's first statement.
	fn declared(&self) -> usize {
		match self.statements.first() {
			Some(first) => self.properties.partition_point(|p| p.span.line < first.span().line),
			None => self.properties.len(),
		}
	}

	/// The declaration region: properties written before the first statement.
	///
	/// These describe the block before it runs and are what `Props::extend`
	/// applies, so they are also the only ones that can say something about the
	/// shape of the whole block.
	pub fn declaration(&self) -> &[Property] {
		&self.properties[..self.declared()]
	}

	/// Properties written below a statement.
	///
	/// One of these is applied where it sits rather than at the top: it can
	/// read a binding above it, and it takes effect from there down. Almost
	/// always empty -- a property is usually written at the top of its block --
	/// which is what lets the walker keep its cheap path.
	pub fn trailing(&self) -> &[Property] {
		&self.properties[self.declared()..]
	}
}

/// `.name = value`, or bare `.name` for `= true`. Dotted names address a
/// namespace, e.g. `.env.FOO`.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
	pub path: Vec<String>,
	pub value: Option<Expr>,
	pub span: Span,
}

/// What a loop asks before each pass.
///
/// One type rather than three statements: `while`, `until` and `loop` differ
/// only in the question, so every walker that handles one handles all three.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopTest {
	/// `while cond`: run again while it holds.
	While(Expr),
	/// `until cond`: run again while it does **not**. The shape a wait loop
	/// wants -- `until $ curl -sf health` reads as what it is.
	Until(Expr),
	/// `loop`: never ask. `break` is how it ends.
	Forever,
}

impl LoopTest {
	/// The condition, for a walker that has no reason to care which way round
	/// the question is put.
	pub fn cond(&self) -> Option<&Expr> {
		match self {
			LoopTest::While(e) | LoopTest::Until(e) => Some(e),
			LoopTest::Forever => None,
		}
	}

	/// The keyword this was written as.
	pub fn keyword(&self) -> &'static str {
		match self {
			LoopTest::While(_) => "while",
			LoopTest::Until(_) => "until",
			LoopTest::Forever => "loop",
		}
	}
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
	/// `let name = expr`, or `let a, b, _ = expr` to unpack a list.
	///
	/// `names` is what the line binds, in order, with `_` standing for a
	/// position that is matched and thrown away. One name is the ordinary
	/// binding and does no unpacking at all -- a list bound to a single name
	/// stays a list.
	Let {
		names: Vec<String>,
		value: Expr,
		span: Span,
	},
	/// `name = expr`, or `a, b = expr` — reassignment, unpacking by the same
	/// rule as `let`.
	Assign {
		names: Vec<String>,
		value: Expr,
		span: Span,
	},
	/// A bare call evaluated for its effect, e.g. `decrypt(a, b)`.
	Call { expr: Expr, span: Span },
	/// `do … end`: a block with no condition.
	///
	/// Properties are block-scoped, and every block form until now also asked
	/// a question. Somewhere to put `.workdir` for two commands should not
	/// require inventing an `if true`.
	Do { body: Block, span: Span },
	/// `if cond … else … end`
	If {
		cond: Expr,
		then: Block,
		otherwise: Option<Block>,
		span: Span,
	},
	/// `retry n [every secs] … [else …] end`
	///
	/// The body is run again while it fails, up to `attempts` times. `else` is
	/// what to do when it never succeeded; without one, the last failure is
	/// the statement's.
	Retry {
		attempts: Expr,
		delay: Option<Expr>,
		body: Block,
		otherwise: Option<Block>,
		span: Span,
	},
	/// `for name in iter … end`, or `for a, b in pairs … end`.
	///
	/// `names` follows the `let` rule: several names unpack each item, and `_`
	/// discards one.
	For {
		names: Vec<String>,
		iter: Expr,
		body: Block,
		span: Span,
	},
	/// `while cond … end`, `until cond … end`, `loop … end`.
	///
	/// Unlike a `for`, what this runs is not known before it starts, which is
	/// why a `.parallel` block refuses one: a fan-out collects its branches
	/// up front, and there is nothing to collect until the body has run.
	Loop { test: LoopTest, body: Block, span: Span },
	/// `break` — leave the innermost loop.
	///
	/// Refused by the parser outside a loop, so an editor underlines it rather
	/// than a run finding out. It travels as a `RunError` for the same reason
	/// `exit()` travels as an `EvalError`: that is the only path back out of a
	/// walk, and every catcher along the way has to let it through.
	Break { span: Span },
	/// `continue` — start the innermost loop's next pass.
	Continue { span: Span },
	/// `match subject … case … default … end`
	Match {
		subject: Expr,
		cases: Vec<MatchCase>,
		default: Option<Block>,
		span: Span,
	},
	/// `run target args…` — dispatched in-process. Writing `$ run target`
	/// instead re-execs the binary, which is a Shell statement, not this.
	Run {
		target: Vec<InterpPart>,
		args: Vec<Vec<InterpPart>>,
		span: Span,
	},
	/// A `$` run or an `exec` block. Both are one process; a `$` run is sugar
	/// for `exec <default shell>` over its contiguous lines, where blank lines
	/// and comments are transparent rather than terminating.
	Exec {
		command: Option<Vec<InterpPart>>,
		body: Vec<Vec<InterpPart>>,
		/// `detach $ npm start`: start it and do not wait.
		///
		/// A marker on the statement rather than a property, because detaching
		/// is a fact about one process. As `.detach` it described a *file*, and
		/// got both halves wrong at once: every top-level command in the file
		/// was detached, so setup-then-serve was unwritable, while a block
		/// cleared it, so the same command inside an `if` quietly waited.
		///
		/// A `detach $` run is its own statement: contiguous `$` lines fold
		/// into one process, and folding on from here would silently detach
		/// whatever was written below. Several lines as one detached process is
		/// what `detach exec sh` is for.
		detach: bool,
		/// Source line of each `body` entry, 1-based.
		///
		/// Kept because the two are not derivable from each other: a `$` run
		/// skips blank and comment lines, and a backslash continuation folds
		/// several source lines into one body entry. Tooling that reports on the
		/// shell text -- shellcheck delegation -- needs to point back at the
		/// line the author actually wrote.
		lines: Vec<usize>,
		span: Span,
	},
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchCase {
	pub label: String,
	pub body: Block,
	pub span: Span,
}

/// A piece of text handed to a shell: literal runs interleaved with `{{ … }}`.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
	Literal(String),
	Expr(Expr),
}

/// The body of a structured block, and which format it is in.
///
/// Its `Debug` is the block's *tokens* rather than its text, so a fingerprint
/// says what the block contains and not how it is laid out. That is the same
/// reason spans are stripped from one: reformatting a file is not an edit to
/// it, and both the prepare gate and the formatter's own check compare
/// fingerprints. The format is part of it, so changing `json` to something
/// else still counts.
#[derive(Clone, PartialEq)]
pub struct StructuredBody {
	pub format: crate::Structured,
	pub lines: Vec<Vec<InterpPart>>,
}

impl std::fmt::Debug for StructuredBody {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{:?}(", self.format)?;
		for parts in &self.lines {
			for p in parts {
				match p {
					// A fragment that will not tokenise is written out as it
					// stands: layout then counts, which is the safe way to be
					// wrong.
					InterpPart::Literal(text) => match self.format.canonical(text) {
						Some(c) => write!(f, "{c}")?,
						None => write!(f, "{text}\u{1}")?,
					},
					InterpPart::Expr(e) => write!(f, "{e:?}\u{1}")?,
				}
			}
		}
		write!(f, ")")
	}
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
	Number(f64, Span),
	Bool(bool, Span),
	/// A string literal, already split into literal and interpolated parts.
	Str(Vec<InterpPart>, Span),
	List(Vec<Expr>, Span),
	/// A `let` binding or loop variable.
	Ident(String, Span),
	/// `ARG.x`, `ENV.X`, `FLAG.x`, `RUN.x`, or bare `ARGS`.
	Source {
		kind: SourceKind,
		key: Option<String>,
		span: Span,
	},
	Unary {
		op: UnaryOp,
		rhs: Box<Expr>,
		span: Span,
	},
	Binary {
		op: BinaryOp,
		lhs: Box<Expr>,
		rhs: Box<Expr>,
		span: Span,
	},
	/// `a ? b` — take `a`, or `b` if `a` does not resolve.
	Chain {
		lhs: Box<Expr>,
		rhs: Box<Expr>,
		span: Span,
	},
	Index {
		base: Box<Expr>,
		index: Box<Expr>,
		span: Span,
	},
	Call {
		name: String,
		args: Vec<Expr>,
		span: Span,
	},
	/// `json … end`: a block of structured text, as one value of that format.
	///
	/// Each entry is one line, so an interpolation inside it is an ordinary
	/// expression the parser has already read.
	Structured {
		body: StructuredBody,
		span: Span,
	},
	/// `$ cmd` or `exec cmd … end` in value position: run it, take stdout with
	/// one trailing newline stripped. Replaces the old `capture()` function --
	/// `$` and `exec` are now the only way to invoke anything external.
	Capture {
		command: Option<Vec<InterpPart>>,
		body: Vec<Vec<InterpPart>>,
		span: Span,
	},
	/// `run <target> [args]` in value position, which only `code_of` may hold.
	///
	/// A dispatched target writes to the terminal like any other, so the only
	/// value it has to give back is its status -- and that is exactly what
	/// `code_of` asks for. Spelled the same as the statement, because it is the
	/// same dispatch: `code_of(run test)` is `run test`, scored.
	Dispatch {
		target: Vec<InterpPart>,
		args: Vec<Vec<InterpPart>>,
		span: Span,
	},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
	Arg,
	Env,
	Flag,
	Run,
	/// Bare `ARGS` — every positional argument.
	Args,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
	Not,
	Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
	Add,
	Sub,
	Mul,
	Div,
	Rem,
	Eq,
	Ne,
	Lt,
	Le,
	Gt,
	Ge,
	And,
	Or,
}

/// A property value the parser can read straight off the page.
///
/// A flag takes a bool, and a bool literal is the one constant it can be
/// written with. Everything here is a constant that can never be one, so it is
/// reported where it is written rather than quietly read as `false`.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
	Number,
	List,
	/// A string with nothing interpolated into it, carried whole so a message
	/// can say `write `true`, not `"true"``.
	Str(String),
}

impl Statement {
	pub fn span(&self) -> Span {
		match self {
			Statement::Let { span, .. }
			| Statement::Assign { span, .. }
			| Statement::Call { span, .. }
			| Statement::Do { span, .. }
			| Statement::If { span, .. }
			| Statement::Retry { span, .. }
			| Statement::For { span, .. }
			| Statement::Loop { span, .. }
			| Statement::Break { span }
			| Statement::Continue { span }
			| Statement::Match { span, .. }
			| Statement::Run { span, .. }
			| Statement::Exec { span, .. } => *span,
		}
	}
}

impl Expr {
	/// Whether evaluating this reads the environment: an `ENV.X` anywhere in
	/// it, interpolations included.
	///
	/// What decides whether a property's value needs the environment brought up
	/// to date before it is worked out. Rebuilding reads and decrypts every
	/// `.env-file` there is, and the one place a header is evaluated twice --
	/// the probe that looks for `.watch`, on every run -- must not do that for
	/// a header that never looks. A capture or a dispatch counts, since the
	/// process it starts is handed the environment whole.
	pub fn reads_env(&self) -> bool {
		let parts = |ps: &[InterpPart]| {
			ps.iter().any(|p| match p {
				InterpPart::Expr(e) => e.reads_env(),
				InterpPart::Literal(_) => false,
			})
		};
		match self {
			Expr::Source { kind, .. } => *kind == SourceKind::Env,
			Expr::Number(..) | Expr::Bool(..) | Expr::Ident(..) => false,
			Expr::Str(ps, _) => parts(ps),
			Expr::List(items, _) => items.iter().any(Expr::reads_env),
			Expr::Unary { rhs, .. } => rhs.reads_env(),
			Expr::Binary { lhs, rhs, .. } | Expr::Chain { lhs, rhs, .. } => lhs.reads_env() || rhs.reads_env(),
			Expr::Index { base, index, .. } => base.reads_env() || index.reads_env(),
			Expr::Call { args, .. } => args.iter().any(Expr::reads_env),
			Expr::Structured { body, .. } => body.lines.iter().any(|l| parts(l)),
			Expr::Capture { .. } | Expr::Dispatch { .. } => true,
		}
	}

	/// This expression as a constant that is not a bool, if that is what it is.
	///
	/// `None` covers the two things a flag may be written with: a bool literal,
	/// and anything whose value is not known until the run -- a source, a call,
	/// a comparison, a `?` chain, or a string with an interpolation in it.
	/// Nothing is folded: `1 + 1` is left to the run, because a flag written
	/// as arithmetic is not the mistake this is looking for.
	pub fn constant_non_bool(&self) -> Option<Constant> {
		match self {
			Expr::Number(..) => Some(Constant::Number),
			Expr::List(..) => Some(Constant::List),
			Expr::Str(parts, _) => parts
				.iter()
				.map(|p| match p {
					InterpPart::Literal(t) => Some(t.as_str()),
					InterpPart::Expr(_) => None,
				})
				.collect::<Option<Vec<_>>>()
				.map(|lits| Constant::Str(lits.concat())),
			_ => None,
		}
	}

	pub fn span(&self) -> Span {
		match self {
			Expr::Number(_, s)
			| Expr::Bool(_, s)
			| Expr::Str(_, s)
			| Expr::List(_, s)
			| Expr::Ident(_, s)
			| Expr::Source { span: s, .. }
			| Expr::Unary { span: s, .. }
			| Expr::Binary { span: s, .. }
			| Expr::Chain { span: s, .. }
			| Expr::Index { span: s, .. }
			| Expr::Call { span: s, .. }
			| Expr::Structured { span: s, .. }
			| Expr::Capture { span: s, .. }
			| Expr::Dispatch { span: s, .. } => *s,
		}
	}
}
