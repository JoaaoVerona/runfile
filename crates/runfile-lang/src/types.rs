//! What a value can be, as far as the text of a runfile says -- and what every
//! function takes and answers with.
//!
//! The language has four types and no coercion, so a value of the wrong one is
//! refused wherever it arrives: `ARG.port + 1` fails every time it runs, since
//! an argument is always a string. Nothing here runs anything. An expression's
//! type is worked out from what it is made of -- a literal, a source, an
//! operator, a call -- and **a name holds whatever any line binds it to,
//! anywhere**. That is wider than what any one path through a file ever sees,
//! on purpose: a check that reports only what is disjoint from *every* type a
//! value can have cannot report a value that is sometimes right.
//!
//! Flow is ignored rather than followed for the same reason. A failure can leave
//! a `let` undone -- under `.ignore-errors`, in a `retry` that gave up, in a loop
//! that went round again -- so a flow-sensitive answer has to model every place
//! a run can carry on after one, and a place it missed is a report about a value
//! the name can in fact hold. The whole-file union has nothing to miss. What it
//! gives up is the rebinding `let n = ARG.n` then `n = number(n)`, which is the
//! file that already remembered to convert.

use crate::ast::{BinaryOp, Block, Expr, SourceKind, Statement, Target, UnaryOp};
use crate::eval::{ARCH_NAMES, OS_NAMES, RUN_KEYS};
use std::collections::{BTreeMap, BTreeSet};

/// A set of the four types, as bits.
pub(crate) type Kinds = u8;
pub(crate) const STR: Kinds = 1;
pub(crate) const NUM: Kinds = 2;
pub(crate) const BOOL: Kinds = 4;
pub(crate) const LIST: Kinds = 8;
pub(crate) const ANY: Kinds = STR | NUM | BOOL | LIST;

/// What a value can be: its kinds, and -- when one of them is a list -- what
/// the items of that list can be. One level deep; deeper is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ty {
	pub kinds: Kinds,
	pub items: Kinds,
}

impl Ty {
	pub const ANY: Ty = Ty { kinds: ANY, items: ANY };
	/// No value at all: what a call that never returns answers with. Nothing
	/// is ever reported about one.
	pub const NONE: Ty = Ty { kinds: 0, items: 0 };

	pub const fn of(kinds: Kinds) -> Ty {
		Ty {
			kinds,
			items: if kinds & LIST != 0 { ANY } else { 0 },
		}
	}

	pub const fn list(items: Kinds) -> Ty {
		Ty { kinds: LIST, items }
	}

	pub fn union(self, other: Ty) -> Ty {
		let items = |t: Ty| if t.kinds & LIST != 0 { t.items } else { 0 };
		Ty {
			kinds: self.kinds | other.kinds,
			items: items(self) | items(other),
		}
	}

	/// What one item can be, when this can be a list.
	pub fn item(self) -> Ty {
		match self.kinds & LIST {
			0 => Ty::NONE,
			_ => Ty::of(self.items),
		}
	}

	/// Whether this can only be something `accepted` is not: a value exists, and
	/// none of what it can be is accepted.
	pub fn refused(self, accepted: Kinds) -> bool {
		self.kinds != 0 && self.kinds & accepted == 0
	}

	/// Whether this is known to be a number and nothing else -- what a value
	/// reaching a shell arithmetic position has to be, so that bash does not
	/// evaluate it as an expression and run a `$( )` inside it.
	pub fn is_number(self) -> bool {
		self.refused(STR | BOOL | LIST)
	}
}

/// The kinds, as a sentence reads them: `a string`, `a string or a number`.
pub(crate) fn named(kinds: Kinds) -> String {
	let names: Vec<&str> = [(STR, "a string"), (NUM, "a number"), (BOOL, "a bool"), (LIST, "a list")]
		.iter()
		.filter(|(k, _)| kinds & k != 0)
		.map(|(_, n)| *n)
		.collect();
	match names.as_slice() {
		[] => "nothing".into(),
		[one] => (*one).into(),
		[init @ .., last] => format!("{} or {last}", init.join(", ")),
	}
}

/// What a call answers with, given what its arguments are.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Returns {
	Is(Ty),
	/// `exit` and `error`: nothing, ever.
	Never,
	/// `one_of`: the value it was asked about.
	Subject,
	/// `first` and `last`: an item, or `""` for an empty list.
	Item,
	/// `sort`, `slice` and the rest: some of the list's own items.
	Same,
	/// `append` and `prepend`: the list's items and the values added.
	Added,
	/// `concat_lists`: the items of every list.
	Joined,
	/// `flatten`: the items, one level down.
	Flattened,
	/// `try`: the value, or `""` once a failure is caught.
	Caught,
}

/// What a function takes and what it answers with.
#[derive(Debug)]
pub(crate) struct Signature {
	pub name: &'static str,
	pub min: usize,
	/// `None` when there is no most.
	pub max: Option<usize>,
	/// What each argument may be. Past the end, the last entry repeats.
	pub params: &'static [Kinds],
	pub returns: Returns,
}

const fn sig(
	name: &'static str,
	min: usize,
	max: Option<usize>,
	params: &'static [Kinds],
	returns: Returns,
) -> Signature {
	Signature {
		name,
		min,
		max,
		params,
		returns,
	}
}

const S: Kinds = STR;
const N: Kinds = NUM;
const L: Kinds = LIST;
const A: Kinds = ANY;
const ONE: Option<usize> = Some(1);
const TWO: Option<usize> = Some(2);
const THREE: Option<usize> = Some(3);
const TEXT: Returns = Returns::Is(Ty::of(STR));
const NUMBER: Returns = Returns::Is(Ty::of(NUM));
const TRUTH: Returns = Returns::Is(Ty::of(BOOL));

/// Every function a call reaches, sorted by name.
///
/// Read off the dispatcher by hand, and held to it by tests that call every
/// function with every number of arguments and every type in every position:
/// a type listed here as refused has to fail when it runs, since a mistake in
/// this direction is a report about a call that works.
pub(crate) const SIGNATURES: &[Signature] = &[
	sig("abs", 1, ONE, &[N], NUMBER),
	sig("append", 2, None, &[L, A], Returns::Added),
	sig("base64_decode", 1, ONE, &[S], TEXT),
	sig("base64_encode", 1, ONE, &[S], TEXT),
	sig("basename", 1, ONE, &[S], TEXT),
	sig("capitalize", 1, ONE, &[S], TEXT),
	sig("ceil", 1, ONE, &[N], NUMBER),
	sig("code_of", 1, ONE, &[A], NUMBER),
	sig("concat", 1, None, &[A], TEXT),
	sig("concat_lists", 1, None, &[L], Returns::Joined),
	sig("confirm", 1, ONE, &[S], TRUTH),
	sig("contains", 2, TWO, &[S | L, A], TRUTH),
	sig("decrypt", 2, TWO, &[S, S], TEXT),
	sig("directory_exists", 1, ONE, &[S], TRUTH),
	sig("dirname", 1, ONE, &[S], TEXT),
	sig("ends_with", 2, TWO, &[S, S], TRUTH),
	sig("error", 1, ONE, &[S], Returns::Never),
	sig("escape", 1, ONE, &[S], TEXT),
	sig("exit", 0, ONE, &[N], Returns::Never),
	sig("extname", 1, ONE, &[S], TEXT),
	sig("file_exists", 1, ONE, &[S], TRUTH),
	sig("first", 1, ONE, &[L], Returns::Item),
	sig("flatten", 1, ONE, &[L], Returns::Flattened),
	sig("floor", 1, ONE, &[N], NUMBER),
	sig("glob", 1, ONE, &[S], Returns::Is(Ty::list(STR))),
	sig("index_of", 2, TWO, &[L, A], NUMBER),
	sig("is_executable", 1, ONE, &[S], TRUTH),
	sig("is_number", 1, ONE, &[A], TRUTH),
	sig("join", 1, None, &[S, A], TEXT),
	sig("join_path", 1, None, &[S], TEXT),
	sig("json_encode", 1, ONE, &[A], TEXT),
	sig("json_format", 1, TWO, &[S, S], TEXT),
	sig("json_get", 2, TWO, &[S, S], Returns::Is(Ty::of(STR | NUM | BOOL))),
	sig("json_keys", 1, TWO, &[S, S], Returns::Is(Ty::list(STR | NUM))),
	sig("json_query", 2, TWO, &[S, S], Returns::Is(Ty::list(STR | NUM | BOOL))),
	sig("json_set", 3, THREE, &[S, S, A], TEXT),
	sig("json_type", 1, TWO, &[S, S], TEXT),
	sig("last", 1, ONE, &[L], Returns::Item),
	sig("length", 1, ONE, &[S | L], NUMBER),
	sig("lines", 1, ONE, &[S], Returns::Is(Ty::list(STR))),
	sig("max", 1, None, &[N], NUMBER),
	sig("md5", 1, ONE, &[S], TEXT),
	sig("min", 1, None, &[N], NUMBER),
	sig("now", 0, ONE, &[S], TEXT),
	sig("number", 1, ONE, &[N | S], NUMBER),
	sig("one_of", 2, None, &[A], Returns::Subject),
	sig("power", 2, TWO, &[N, N], NUMBER),
	sig("prepend", 2, None, &[L, A], Returns::Added),
	sig("print", 1, None, &[A], TEXT),
	sig("printf", 1, None, &[S, A], TEXT),
	sig("range", 1, TWO, &[N, N], Returns::Is(Ty::list(NUM))),
	sig("read_file", 1, ONE, &[S], TEXT),
	sig("regex_capture", 3, THREE, &[S, S, N], TEXT),
	sig("regex_capture_all", 3, THREE, &[S, S, N], Returns::Is(Ty::list(STR))),
	sig("regex_matches", 2, TWO, &[S, S], TRUTH),
	sig("regex_remove", 2, TWO, &[S, S], TEXT),
	sig("regex_replace", 3, THREE, &[S, S, S], TEXT),
	sig("remove_all", 2, TWO, &[S, S], TEXT),
	sig("remove_prefix", 2, TWO, &[S, S], TEXT),
	sig("remove_suffix", 2, TWO, &[S, S], TEXT),
	sig("repeat", 2, TWO, &[S, N], TEXT),
	sig("replace_all", 3, THREE, &[S, S, S], TEXT),
	sig("reverse", 1, ONE, &[L], Returns::Same),
	sig("round", 1, ONE, &[N], NUMBER),
	sig("sha256", 1, ONE, &[S], TEXT),
	sig("sleep", 1, ONE, &[N], TEXT),
	sig("slice", 2, THREE, &[L, N, N], Returns::Same),
	sig("sort", 1, ONE, &[L], Returns::Same),
	sig("split", 2, TWO, &[S, S], Returns::Is(Ty::list(STR))),
	sig("starts_with", 2, TWO, &[S, S], TRUTH),
	sig("stem", 1, ONE, &[S], TEXT),
	sig("substring", 2, THREE, &[S, N, N], TEXT),
	sig("temp_dir", 0, Some(0), &[], TEXT),
	sig("temp_file", 0, TWO, &[S, S], TEXT),
	sig("to_lower", 1, ONE, &[S], TEXT),
	sig("to_upper", 1, ONE, &[S], TEXT),
	sig("trim", 1, ONE, &[S], TEXT),
	sig("trim_end", 1, ONE, &[S], TEXT),
	sig("trim_start", 1, ONE, &[S], TEXT),
	sig("try", 1, ONE, &[A], Returns::Caught),
	sig("unique", 1, ONE, &[L], Returns::Same),
	sig("url_decode", 1, ONE, &[S], TEXT),
	sig("url_encode", 1, ONE, &[S], TEXT),
	sig("uuid", 0, Some(0), &[], TEXT),
	sig("without", 2, None, &[L, A], Returns::Same),
	sig("write_file", 2, TWO, &[S, S], TEXT),
	sig("zip", 2, TWO, &[L, L], Returns::Is(Ty::list(LIST))),
];

pub(crate) fn signature(name: &str) -> Option<&'static Signature> {
	SIGNATURES
		.binary_search_by(|s| s.name.cmp(name))
		.ok()
		.map(|k| &SIGNATURES[k])
}

impl Signature {
	/// Whether it takes `n` arguments.
	pub fn takes(&self, n: usize) -> bool {
		n >= self.min && self.max.is_none_or(|max| n <= max)
	}

	/// How many arguments it takes, as a sentence says it.
	pub fn arity(&self) -> String {
		let plural = |n: usize| if n == 1 { "argument" } else { "arguments" };
		match (self.min, self.max) {
			(0, Some(0)) => "no arguments".into(),
			(a, Some(b)) if a == b => format!("{a} {}", plural(a)),
			(a, Some(b)) if b == a + 1 => format!("{a} or {b} arguments"),
			(a, Some(b)) => format!("{a} to {b} arguments"),
			(a, None) => format!("at least {a} {}", plural(a)),
		}
	}

	/// What argument `at` may be, given what every argument is.
	///
	/// `contains` is the one whose answer depends on another argument: a list
	/// is searched for any value, and a string only for a string.
	pub fn accepts(&self, at: usize, args: &[Ty]) -> Kinds {
		if self.name == "contains" && at == 1 {
			let subject = args.first().copied().unwrap_or(Ty::ANY);
			return if subject.kinds == 0 || subject.kinds & LIST != 0 {
				ANY
			} else {
				STR
			};
		}
		self.params.get(at).or(self.params.last()).copied().unwrap_or(ANY)
	}

	pub fn answer(&self, args: &[Ty]) -> Ty {
		let first = args.first().copied().unwrap_or(Ty::ANY);
		let list = |items: Kinds| match first.kinds & LIST {
			0 => Ty::NONE,
			_ => Ty::list(items),
		};
		match self.returns {
			Returns::Is(t) => t,
			Returns::Never => Ty::NONE,
			Returns::Subject => first,
			Returns::Item => first.item().union(Ty::of(STR)),
			Returns::Same => list(first.items),
			Returns::Added => list(args.iter().skip(1).fold(first.items, |k, t| k | t.kinds)),
			Returns::Joined => Ty::list(args.iter().fold(0, |k, t| k | t.item().kinds)),
			Returns::Flattened => list(match first.items & LIST {
				0 => first.items,
				_ => ANY,
			}),
			Returns::Caught => first.union(Ty::of(STR)),
		}
	}
}

/// Where a value came from, when it is one of the few whose every possible
/// value is known: `RUN.os` and `RUN.arch`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Values {
	/// Not worked out yet.
	Bottom,
	Only(BTreeSet<&'static str>),
	Any,
}

impl Values {
	fn join(self, other: Values) -> Values {
		match (self, other) {
			(Values::Bottom, x) | (x, Values::Bottom) => x,
			(Values::Only(mut a), Values::Only(b)) => {
				a.extend(b);
				Values::Only(a)
			}
			_ => Values::Any,
		}
	}
}

/// How a line binds its names.
enum Site<'a> {
	/// `let x = e`: the value whole.
	Whole(&'a Expr),
	/// `let a, b = e` and `for x in e`: an item of the value, `depth` levels in.
	Item(&'a Expr, usize),
}

/// What every name in a file can hold, anywhere in it.
pub struct Names {
	types: BTreeMap<String, Ty>,
	values: BTreeMap<String, Values>,
	/// Whether what the `_shared.run` files above bind is known. When it is not,
	/// a name could hold anything they bound, and so is anything.
	known: bool,
}

impl Names {
	pub fn of(file: &Target, chain: Option<&[Target]>) -> Names {
		let mut sites = Vec::new();
		// A shared file binds with its top-level `let`s and nothing else.
		for shared in chain.unwrap_or_default() {
			for st in &shared.body.statements {
				if let Statement::Let { names, value, .. } = st {
					sites.push((names.as_slice(), site(names, value)));
				}
			}
		}
		collect(&file.body, &mut sites);
		let mut n = Names {
			types: BTreeMap::new(),
			values: BTreeMap::new(),
			known: chain.is_some(),
		};
		for (names, _) in &sites {
			for name in names.iter().filter(|x| *x != "_") {
				n.types.insert(name.clone(), Ty::NONE);
				n.values.insert(name.clone(), Values::Bottom);
			}
		}
		// Each pass can only add to what a name holds, and there is a most it can
		// hold, so this ends.
		loop {
			let mut changed = false;
			for (names, site) in &sites {
				let (t, v) = match site {
					Site::Whole(e) => (n.ty(e), n.run_values(e)),
					Site::Item(e, depth) => ((0..*depth).fold(n.ty(e), |t, _| t.item()), Values::Any),
				};
				for name in names.iter().filter(|x| *x != "_") {
					let held = n.types[name.as_str()];
					if held.union(t) != held {
						n.types.insert(name.clone(), held.union(t));
						changed = true;
					}
					let had = n.values[name.as_str()].clone();
					let now = had.clone().join(v.clone());
					if now != had {
						n.values.insert(name.clone(), now);
						changed = true;
					}
				}
			}
			if !changed {
				return n;
			}
		}
	}

	/// What `e` can be.
	pub fn ty(&self, e: &Expr) -> Ty {
		match e {
			Expr::Number(..) => Ty::of(NUM),
			Expr::Bool(..) => Ty::of(BOOL),
			Expr::Str(..) | Expr::Structured { .. } | Expr::Capture { .. } => Ty::of(STR),
			Expr::List(items, _) => Ty::list(items.iter().fold(0, |k, i| k | self.ty(i).kinds)),
			Expr::Ident(name, _) => match self.known {
				true => self.types.get(name).copied().unwrap_or(Ty::ANY),
				false => Ty::ANY,
			},
			Expr::Source { kind, key, .. } => match kind {
				SourceKind::Arg | SourceKind::Env => Ty::of(STR),
				SourceKind::Flag => Ty::of(BOOL),
				SourceKind::Args => Ty::list(STR),
				SourceKind::Run => match key.as_deref() {
					Some("namespaces") => Ty::list(STR),
					Some(k) if RUN_KEYS.contains(&k) => Ty::of(STR),
					_ => Ty::ANY,
				},
			},
			Expr::Unary { op: UnaryOp::Not, .. } => Ty::of(BOOL),
			Expr::Unary { op: UnaryOp::Neg, .. } => Ty::of(NUM),
			Expr::Binary { op, .. } => match op {
				BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => Ty::of(NUM),
				_ => Ty::of(BOOL),
			},
			Expr::Chain { lhs, rhs, .. } => self.ty(lhs).union(self.ty(rhs)),
			Expr::Index { base, .. } => self.ty(base).item(),
			Expr::Call { name, args, .. } => match signature(name) {
				Some(s) => s.answer(&args.iter().map(|a| self.ty(a)).collect::<Vec<_>>()),
				None => Ty::ANY,
			},
			Expr::Dispatch { .. } => Ty::ANY,
		}
	}

	/// Every value `e` can have, when that is known: `RUN.os` and `RUN.arch`,
	/// read directly, through a `?` between them, or through a name every line
	/// binds to one of those.
	///
	/// A string literal is not followed through a name. `let mode = "dev"`
	/// above `if mode == "prod"` is how a setting someone edits by hand is
	/// written, and a check that called that comparison a mistake would be the
	/// mistake.
	pub fn values(&self, e: &Expr) -> Option<BTreeSet<&'static str>> {
		match self.run_values(e) {
			Values::Only(set) => Some(set),
			_ => None,
		}
	}

	fn run_values(&self, e: &Expr) -> Values {
		match e {
			Expr::Source {
				kind: SourceKind::Run,
				key,
				..
			} => match key.as_deref() {
				Some("os") => Values::Only(OS_NAMES.iter().copied().collect()),
				Some("arch") => Values::Only(ARCH_NAMES.iter().copied().collect()),
				_ => Values::Any,
			},
			Expr::Ident(name, _) if self.known => self.values.get(name).cloned().unwrap_or(Values::Any),
			Expr::Chain { lhs, rhs, .. } => match (self.run_values(lhs), self.run_values(rhs)) {
				(a @ Values::Only(_), b @ Values::Only(_)) => a.join(b),
				(Values::Bottom, x) | (x, Values::Bottom) => x,
				_ => Values::Any,
			},
			_ => Values::Any,
		}
	}
}

fn site<'a>(names: &[String], value: &'a Expr) -> Site<'a> {
	match names.len() {
		1 => Site::Whole(value),
		_ => Site::Item(value, 1),
	}
}

/// Every line in a block that binds a name, however deep.
fn collect<'a>(b: &'a Block, out: &mut Vec<(&'a [String], Site<'a>)>) {
	for st in &b.statements {
		match st {
			Statement::Let { names, value, .. } | Statement::Assign { names, value, .. } => {
				out.push((names.as_slice(), site(names, value)));
			}
			Statement::For { names, iter, body, .. } => {
				out.push((names.as_slice(), Site::Item(iter, if names.len() == 1 { 1 } else { 2 })));
				collect(body, out);
			}
			Statement::Do { body, .. } | Statement::Loop { body, .. } => collect(body, out),
			Statement::If { then, otherwise, .. }
			| Statement::Retry {
				body: then, otherwise, ..
			} => {
				collect(then, out);
				if let Some(o) = otherwise {
					collect(o, out);
				}
			}
			Statement::Match { cases, default, .. } => {
				for body in cases.iter().map(|c| &c.body).chain(default) {
					collect(body, out);
				}
			}
			Statement::Call { .. }
			| Statement::Break { .. }
			| Statement::Continue { .. }
			| Statement::Run { .. }
			| Statement::Exec { .. } => {}
		}
	}
}
