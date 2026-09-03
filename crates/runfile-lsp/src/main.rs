//! `runfile-lsp` -- the language server, speaking LSP over stdin/stdout.

use std::io::{BufReader, stdin, stdout};

fn main() -> std::io::Result<()> {
	let mut input = BufReader::new(stdin().lock());
	let mut output = stdout().lock();
	runfile_lsp::server::Server::new().serve(&mut input, &mut output)
}
