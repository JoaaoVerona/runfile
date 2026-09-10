//! `run :list`, and the message for a name that does not resolve.

use runfile_discovery::{Catalog, Origin, Target};

/// What listing needs from a target file: the first line of its leading
/// comment block, and whether its name keeps it out of the listing.
pub(crate) struct Facts {
	pub(crate) description: String,
	pub(crate) hidden: bool,
	/// Whether the target reads its command line, so a generated editor task
	/// knows to offer an argument prompt.
	pub(crate) uses_args: bool,
}

impl Facts {
	fn none() -> Self {
		Facts {
			description: String::new(),
			hidden: false,
			uses_args: false,
		}
	}
}

pub(crate) fn facts(t: &Target) -> Facts {
	let Ok(src) = std::fs::read_to_string(&t.path) else {
		return Facts::none();
	};
	let Ok(ast) = runfile_lang::parse(&src) else {
		return Facts::none();
	};
	Facts {
		uses_args: src.contains("ARGS") || src.contains("ARG."),
		description: ast
			.description
			.as_deref()
			.and_then(|d| d.lines().next())
			.unwrap_or_default()
			.to_string(),
		hidden: runfile_discovery::is_hidden(&t.name),
	}
}

/// The catalog as JSON, for tooling that needs more than names -- the editor
/// extension builds its tree, its tasks and its run buttons from this.
///
/// Serialized by hand: the shape is four string fields, and a serde dependency
/// in the CLI to emit them would not earn its keep.
pub fn print_json(cat: &Catalog) {
	println!("{{");
	println!("  \"formatVersion\": {FORMAT_VERSION},");
	println!("  \"targets\": [");
	let rows: Vec<(&Target, Facts)> = cat
		.targets
		.values()
		.map(|t| (t, facts(t)))
		.filter(|(_, f)| !f.hidden)
		.collect();
	for (i, (t, f)) in rows.iter().enumerate() {
		let comma = if i + 1 == rows.len() { "" } else { "," };
		println!(
			"    {{\"name\": {}, \"description\": {}, \"origin\": {}, \"path\": {}}}{comma}",
			quote(&t.name),
			quote(&f.description),
			quote(label(t.origin)),
			quote(&t.path.to_string_lossy()),
		);
	}
	println!("  ]");
	println!("}}");
}

/// Bumped when the shape changes incompatibly, so an old extension can say so
/// rather than misread a new CLI.
pub const FORMAT_VERSION: u32 = 2;

fn quote(s: &str) -> String {
	let mut out = String::with_capacity(s.len() + 2);
	out.push('"');
	for c in s.chars() {
		match c {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
			c => out.push(c),
		}
	}
	out.push('"');
	out
}

/// One name per line, for shell completion. Hidden targets are omitted, the
/// same as the human listing.
/// Every target a person may type, hidden ones left out.
pub fn names(cat: &Catalog) -> Vec<String> {
	cat.targets
		.values()
		.filter(|t| !facts(t).hidden)
		.map(|t| t.name.clone())
		.collect()
}

pub fn print_names(cat: &Catalog) {
	for n in names(cat) {
		println!("{n}");
	}
}

pub fn print(cat: &Catalog) {
	let shown: Vec<(&Target, Facts)> = cat
		.targets
		.values()
		.map(|t| (t, facts(t)))
		.filter(|(_, f)| !f.hidden)
		.collect();
	if shown.is_empty() {
		println!("no targets");
		return;
	}
	let width = shown.iter().map(|(t, _)| t.name.len()).max().unwrap_or(0);
	let mut first = true;
	for origin in [Origin::Global, Origin::Local, Origin::Included] {
		let group: Vec<&(&Target, Facts)> = shown.iter().filter(|(t, _)| t.origin == origin).collect();
		if group.is_empty() {
			continue;
		}
		// The local group is the unlabelled default only while nothing comes
		// before it: an unheaded run of names below `global:` reads as more
		// global ones. Everything else says which group it is.
		if !first || origin != Origin::Local {
			if !first {
				println!();
			}
			println!("{}:", label(origin));
		}
		first = false;
		for (t, f) in group {
			let described = f.description.clone();
			if described.is_empty() {
				println!("  {}", t.name);
			} else {
				println!("  {:<width$}  {described}", t.name);
			}
		}
	}
}

fn label(o: Origin) -> &'static str {
	match o {
		Origin::Local => "local",
		Origin::Included => "subprojects",
		Origin::Global => "global",
	}
}

pub fn unknown(cat: &Catalog, name: &str) -> String {
	let near: Vec<&str> = cat
		.targets
		.keys()
		.filter(|k| k.contains(name) || name.contains(k.as_str()))
		.map(String::as_str)
		.take(5)
		.collect();
	if near.is_empty() {
		format!("no target named `{name}`; run `run :list` to see them")
	} else {
		format!("no target named `{name}`\n\ndid you mean: {}", near.join(", "))
	}
}
