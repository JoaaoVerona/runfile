fn main() {
	let root = std::env::args().nth(1).expect("usage: list <dir>");
	match runfile_discovery::discover(std::path::Path::new(&root), None) {
		Ok(c) => {
			println!("{} targets", c.targets.len());
			for (n, t) in &c.targets {
				println!("  {:<36} {:?}", n, t.origin);
			}
		}
		Err(e) => println!("error: {e}"),
	}
}
