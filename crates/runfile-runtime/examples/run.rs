//! End-to-end: discover, resolve, run.
//!   cargo run -p runfile-runtime --example run -- <dir> <target> [args...]
use std::path::Path;

fn main() {
	let mut a = std::env::args().skip(1);
	let dir = a.next().expect("usage: run <dir> <target> [args...]");
	let target = a.next().expect("usage: run <dir> <target> [args...]");
	let args: Vec<String> = a.collect();

	let cat = match runfile_discovery::discover(Path::new(&dir), None) {
		Ok(c) => c,
		Err(e) => {
			eprintln!("error: {e}");
			std::process::exit(1);
		}
	};
	let mut host = runfile_runtime::dispatch::Host::new(&cat);
	host.assume_yes = true;
	match host.run(&target, &args) {
		Ok(()) => {
			let trace = host.trace.lock().expect("trace");
			for line in trace.iter() {
				let shown: String = line.replace('\n', "\n       ");
				println!("[ran] {shown}");
			}
		}
		Err(e) => {
			eprintln!("error: {e}");
			std::process::exit(1);
		}
	}
}
