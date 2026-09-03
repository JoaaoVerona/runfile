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
	pub properties: Vec<Property>,
	pub statements: Vec<Statement>,
}

/// `.name = value`, or bare `.name` for `= true`. Dotted names address a
/// namespace, e.g. `.env.FOO`.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
	pub path: Vec<String>,
	pub value: Option<Expr>,
	pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
	/// `let name = expr`
	Let { name: String, value: Expr, span: Span },
	/// `name = expr` — reassignment of an existing binding.
	Assign { name: String, value: Expr, span: Span },
	/// A bare call evaluated for its effect, e.g. `decrypt(a, b)`.
	Call { expr: Expr, span: Span },
	/// `if cond … else … end`
	If { cond: Expr, then: Block, otherwise: Option<Block>, span: Span },
	/// `for name in iter … end`
	For { name: String, iter: Expr, body: Block, span: Span },
	/// `match subject … case … default … end`
	Match { subject: Expr, cases: Vec<MatchCase>, default: Option<Block>, span: Span },
	/// `run target args…` — dispatched in-process. Writing `$ run target`
	/// instead re-execs the binary, which is a Shell statement, not this.
	Run { target: Vec<InterpPart>, args: Vec<Vec<InterpPart>>, span: Span },
	/// A `$` run or an `exec` block. Both are one process; a `$` run is sugar
	/// for `exec <default shell>` over its contiguous lines, where blank lines
	/// and comments are transparent rather than terminating.
	Exec { command: Option<Vec<InterpPart>>, body: Vec<Vec<InterpPart>>, span: Span },
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
	Source { kind: SourceKind, key: Option<String>, span: Span },
	Unary { op: UnaryOp, rhs: Box<Expr>, span: Span },
	Binary { op: BinaryOp, lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
	/// `a ? b` — take `a`, or `b` if `a` does not resolve.
	Chain { lhs: Box<Expr>, rhs: Box<Expr>, span: Span },
	Index { base: Box<Expr>, index: Box<Expr>, span: Span },
	Call { name: String, args: Vec<Expr>, span: Span },
	/// `$ cmd` or `exec cmd … end` in value position: run it, take stdout with
	/// one trailing newline stripped. Replaces the old `capture()` function --
	/// `$` and `exec` are now the only way to invoke anything external.
	Capture { command: Option<Vec<InterpPart>>, body: Vec<Vec<InterpPart>>, span: Span },
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

impl Expr {
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
			| Expr::Capture { span: s, .. } => *s,
		}
	}
}
