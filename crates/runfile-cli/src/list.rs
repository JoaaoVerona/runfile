//! `run :list`, and the message for a name that does not resolve.

use runfile_discovery::{Catalog, Origin, Target};

/// What listing needs from a target file: the first line of its leading
/// comment block, and whether it opted out of being listed.
///
/// One read and one parse per target -- descriptions and `.hide` used to cost
/// two of each.
struct Facts {
	description: String,
	hidden: bool,
}

fn facts(t: &Target) -> Facts {
	let Ok(src) = std::fs::read_to_string(&t.path) else {
		return Facts {
			description: String::new(),
			hidden: false,
		};
	};
	let Ok(ast) = runfile_lang::parse(&src) else {
		return Facts {
			description: String::new(),
			hidden: false,
		};
	};
	Facts {
		description: ast
			.description
			.as_deref()
			.and_then(|d| d.lines().next())
			.unwrap_or_default()
			.to_string(),
		// `.hide` is the only way to hide a target; the old `_` filename prefix
		// carries no meaning.
		hidden: ast
			.body
			.properties
			.iter()
			.any(|p| p.path.first().is_some_and(|h| h == "hide")),
	}
}

/// One name per line, for shell completion. Hidden targets are omitted, the
/// same as the human listing.
pub fn print_names(cat: &Catalog) {
	for t in cat.targets.values() {
		if !facts(t).hidden {
			println!("{}", t.name);
		}
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
	for origin in [Origin::Local, Origin::Included, Origin::Global] {
		let group: Vec<&(&Target, Facts)> = shown.iter().filter(|(t, _)| t.origin == origin).collect();
		if group.is_empty() {
			continue;
		}
		if origin != Origin::Local {
			println!("\n{}:", label(origin));
		}
		for (t, f) in group {
			if f.description.is_empty() {
				println!("  {}", t.name);
			} else {
				println!("  {:<width$}  {}", t.name, f.description);
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
