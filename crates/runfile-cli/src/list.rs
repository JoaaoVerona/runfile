//! `run :list`, and the message for a name that does not resolve.

use runfile_discovery::{Catalog, Origin, Target};

/// Descriptions come from each file's leading comment block, so listing reads
/// every target -- cheap, since they are small.
fn describe(t: &Target) -> String {
	let Ok(src) = std::fs::read_to_string(&t.path) else {
		return String::new();
	};
	runfile_lang::parse(&src)
		.ok()
		.and_then(|x| x.description)
		.map(|d| d.lines().next().unwrap_or("").to_string())
		.unwrap_or_default()
}

pub fn print(cat: &Catalog) {
	let shown: Vec<&Target> = cat.targets.values().filter(|t| !hidden(t)).collect();
	if shown.is_empty() {
		println!("no targets");
		return;
	}
	let width = shown.iter().map(|t| t.name.len()).max().unwrap_or(0);
	for origin in [Origin::Local, Origin::Included, Origin::Global] {
		let group: Vec<&&Target> = shown.iter().filter(|t| t.origin == origin).collect();
		if group.is_empty() {
			continue;
		}
		if origin != Origin::Local {
			println!("\n{}:", label(origin));
		}
		for t in group {
			let d = describe(t);
			if d.is_empty() {
				println!("  {}", t.name);
			} else {
				println!("  {:<width$}  {d}", t.name);
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

/// `.hide` is the only way to hide a target; the old `_` filename prefix
/// carries no meaning.
fn hidden(t: &Target) -> bool {
	let Ok(src) = std::fs::read_to_string(&t.path) else {
		return false;
	};
	runfile_lang::parse(&src)
		.map(|x| {
			x.body
				.properties
				.iter()
				.any(|p| p.path.first().is_some_and(|h| h == "hide"))
		})
		.unwrap_or(false)
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
