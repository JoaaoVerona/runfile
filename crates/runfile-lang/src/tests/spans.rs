//! Where the tree says a name is written.
//!
//! A name that does not resolve is underlined where the tree says it is, and
//! reported at the line the tree gives it. Nothing read that position closely
//! until then, and it was wrong in most of the places a name can sit: an
//! indented line counted from column zero, a trimmed value from before the
//! blank it lost, an interpolation from its `{{`, a string's interpolation from
//! the start of the expression rather than the file, and every name on the
//! second line of a spilled list was placed on the first.

use crate::Span;
use crate::ast::{Block, Expr, InterpPart, Property, SourceKind, Statement};

/// Every name in a tree -- a read, a call, a source -- with the text it has to
/// start with and the span it carries.
fn names(b: &Block, out: &mut Vec<(String, Span)>) {
	let props = |ps: &[Property], out: &mut Vec<(String, Span)>| {
		for p in ps {
			if let Some(v) = &p.value {
				expr(v, out);
			}
		}
	};
	props(&b.properties, out);
	for st in &b.statements {
		match st {
			Statement::Let { value, .. } | Statement::Assign { value, .. } => expr(value, out),
			Statement::Call { expr: e, .. } => expr(e, out),
			Statement::Do { body, .. } => names(body, out),
			Statement::If {
				cond, then, otherwise, ..
			} => {
				expr(cond, out);
				names(then, out);
				if let Some(o) = otherwise {
					names(o, out);
				}
			}
			Statement::Retry {
				attempts,
				delay,
				body,
				otherwise,
				..
			} => {
				expr(attempts, out);
				if let Some(d) = delay {
					expr(d, out);
				}
				names(body, out);
				if let Some(o) = otherwise {
					names(o, out);
				}
			}
			Statement::For { iter, body, .. } => {
				expr(iter, out);
				names(body, out);
			}
			Statement::Loop { test, body, .. } => {
				if let Some(c) = test.cond() {
					expr(c, out);
				}
				names(body, out);
			}
			Statement::Break { .. } | Statement::Continue { .. } => {}
			Statement::Match {
				subject,
				cases,
				default,
				..
			} => {
				expr(subject, out);
				for c in cases {
					names(&c.body, out);
				}
				if let Some(d) = default {
					names(d, out);
				}
			}
			Statement::Run { target, args, .. } => {
				parts(target, out);
				args.iter().for_each(|a| parts(a, out));
			}
			Statement::Exec { command, body, .. } => {
				if let Some(c) = command {
					parts(c, out);
				}
				body.iter().for_each(|l| parts(l, out));
			}
		}
	}
}

fn parts(ps: &[InterpPart], out: &mut Vec<(String, Span)>) {
	for p in ps {
		if let InterpPart::Expr(e) = p {
			expr(e, out);
		}
	}
}

fn expr(e: &Expr, out: &mut Vec<(String, Span)>) {
	match e {
		Expr::Number(..) | Expr::Bool(..) => {}
		Expr::Ident(name, span) => out.push((name.clone(), *span)),
		Expr::Source { kind, span, .. } => {
			let root = match kind {
				SourceKind::Arg => "ARG",
				SourceKind::Env => "ENV",
				SourceKind::Flag => "FLAG",
				SourceKind::Run => "RUN",
				SourceKind::Args => "ARGS",
			};
			out.push((root.to_string(), *span));
		}
		Expr::Str(ps, _) => parts(ps, out),
		Expr::List(items, _) => items.iter().for_each(|i| expr(i, out)),
		Expr::Unary { rhs, .. } => expr(rhs, out),
		Expr::Binary { lhs, rhs, .. } | Expr::Chain { lhs, rhs, .. } => {
			expr(lhs, out);
			expr(rhs, out);
		}
		Expr::Index { base, index, .. } => {
			expr(base, out);
			expr(index, out);
		}
		Expr::Call { name, args, span } => {
			out.push((name.clone(), *span));
			args.iter().for_each(|a| expr(a, out));
		}
		Expr::Structured { body, .. } => body.lines.iter().for_each(|l| parts(l, out)),
		Expr::Capture { command, body, .. } => {
			if let Some(c) = command {
				parts(c, out);
			}
			body.iter().for_each(|l| parts(l, out));
		}
		Expr::Dispatch { target, args, .. } => {
			parts(target, out);
			args.iter().for_each(|a| parts(a, out));
		}
	}
}

/// Every place `src` names something, and whether each one is where the tree
/// says: its span starts at the name, on the line it carries.
fn misplaced(src: &str) -> Vec<String> {
	let tree = crate::parse(src).unwrap_or_else(|e| panic!("{e}\n\n{src}"));
	let mut found = Vec::new();
	names(&tree.body, &mut found);
	found
		.into_iter()
		.filter_map(|(name, span)| {
			let at = src.get(span.start..).unwrap_or_default();
			let line = src[..span.start.min(src.len())].matches('\n').count() + 1;
			(!at.starts_with(&name) || line != span.line).then(|| {
				let shown: String = at.chars().take(name.len() + 8).collect();
				format!(
					"`{name}`: span {}..{} says line {}, and there the source reads {shown:?} on line {line}",
					span.start, span.end, span.line
				)
			})
		})
		.collect()
}

fn assert_placed(src: &str) {
	let wrong = misplaced(src);
	assert!(wrong.is_empty(), "{}\n\n{src}", wrong.join("\n"));
}

#[test]
fn a_name_on_an_indented_line_is_placed_from_the_indentation() {
	assert_placed("if top\n\tlet a = b\n\tc = d\n\tprint(e)\n\tif f\n\t\tg(h)\n\tend\nend\n");
}

#[test]
fn a_value_is_placed_after_the_blanks_it_was_trimmed_of() {
	assert_placed("let a =    b\nc =  d\nif   e\nend\nwhile  f\nend\nuntil\t g\nend\n");
	assert_placed("retry  n   every   s\n\t$ true\nend\nmatch   m\n\tcase \"x\"\n\t\t$ true\nend\n");
	assert_placed("for  x,  y   in   pairs\n\tprint(x)\nend\nparallel   for z in zs\n\tprint(z)\nend\n");
	assert_placed("if a\n\t$ true\nelse   if   b\n\t$ true\nelse if c\n\t$ true\nend\n");
}

#[test]
fn a_name_inside_an_interpolation_is_placed_from_the_name_not_the_braces() {
	assert_placed("$ echo {{ a }} {{b}} {{   c   }}\n\tlet x = \"{{ d }}-{{ e(f) }}\"\n");
	assert_placed("print(\"one {{ a }}\", \"two {{ b(\"{{ c }}\") }}\")\n");
	assert_placed(".env.X = \"{{ a }}\"\n.workdir = b\n$ true\n");
	assert_placed("run {{ a }}:build --x={{ b }} {{ c }}\nrun  d:e   {{ f }}\n");
}

#[test]
fn a_name_in_a_command_or_a_body_is_placed_where_it_is_written() {
	assert_placed("exec python3 {{ a }}\n\tprint({{ b }})\n\t\t{{ c }}\nend\n");
	assert_placed("let out = $ git log {{ a }}\nlet doc = exec jq {{ b }}\n\t.{{ c }}\nend\n");
	assert_placed("let doc = json\n\t{\"a\": {{ a }}, \"b\": {{ b(c) }}}\nend\n");
	assert_placed("detach $ serve {{ a }}\n  detach exec node {{ b }}\n\t{{ c }}\n  end\n");
}

#[test]
fn a_call_holding_a_capture_places_every_argument() {
	assert_placed("let files = lines($ git ls-files {{ a }})\nfor f in f2(b, c,   $ ls {{ d }})\nend\n");
	assert_placed("let c = code_of(run {{ a }}:test --x={{ b }})\n\tcode_of(  $ make {{ c }})\n");
	assert_placed("if code_of($ test -f {{ a }})\nend\nmatch $ curl {{ b }}\nend\n");
}

#[test]
fn a_name_on_a_later_line_of_a_spilled_list_is_placed_on_that_line() {
	assert_placed("let xs = [\n\ta, # a note {{ not_code }}\n\t# a line of its own\n\tb(c),\n\t[d, \"{{ e }}\"],\n]\n");
	assert_placed("for x in [\n\tone,\n\ttwo,\n]\n\tprint(x)\nend\n");
	assert_placed("\t\tprint([\n\t\t\ta,\n\n\t\t\tb,\n\t\t])\n");
}

#[test]
fn a_crlf_file_places_its_names_as_its_lf_twin_does() {
	let lf = "if a\n\tlet xs = [\n\t\tb, # c\n\t\td,\n\t]\n\t$ echo {{ e }} \\\n\t\t{{ f }}\nend\n";
	assert_placed(lf);
	assert_placed(&lf.replace('\n', "\r\n"));
}

#[test]
fn a_continued_shell_line_places_the_names_on_its_later_lines() {
	assert_placed("$ docker run \\\n\t--name {{ a }} \\\n\t{{ b }}\n$ echo {{ c }}\n");
}

#[test]
fn every_name_in_every_file_in_this_repository_is_where_the_tree_says() {
	let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
	let mut files = Vec::new();
	let mut dirs = vec![root];
	while let Some(d) = dirs.pop() {
		for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
			let p = e.path();
			let name = e.file_name().to_string_lossy().into_owned();
			if p.is_dir() {
				if !matches!(name.as_str(), "target" | "node_modules" | ".git") && !name.starts_with("target-") {
					dirs.push(p);
				}
			} else if p.extension().is_some_and(|x| x == "run") {
				files.push(p);
			}
		}
	}
	assert!(files.len() > 20, "the sweep found almost nothing: {}", files.len());
	let mut wrong = Vec::new();
	for f in files {
		let src = std::fs::read_to_string(&f).unwrap();
		if crate::parse(&src).is_err() {
			continue;
		}
		wrong.extend(misplaced(&src).into_iter().map(|w| format!("{}: {w}", f.display())));
	}
	assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
