//! Parse every `.run` file under the given roots. The converted corpus is the
//! acceptance test for the grammar.
use std::{env, fs, path::{Path, PathBuf}};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(rd) = fs::read_dir(dir) else { return };
	for e in rd.flatten() {
		let p = e.path();
		if p.is_dir() {
			let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
			if name != "node_modules" && name != ".git" {
				collect(&p, out);
			}
		} else if p.extension().is_some_and(|x| x == "run") {
			out.push(p);
		}
	}
}

fn main() {
	let mut files = Vec::new();
	for root in env::args().skip(1) {
		collect(Path::new(&root), &mut files);
	}
	files.sort();
	let (mut ok, mut fail) = (0, 0);
	for f in &files {
		let src = fs::read_to_string(f).unwrap();
		match runfile_lang::parse(&src) {
			Ok(_) => ok += 1,
			Err(e) => {
				fail += 1;
				println!("FAIL {}\n     {e}", f.display());
			}
		}
	}
	println!("parsed {ok}/{} files", ok + fail);
	if fail > 0 {
		std::process::exit(1);
	}
}
