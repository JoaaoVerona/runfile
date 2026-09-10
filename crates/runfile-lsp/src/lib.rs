//! A language server for `.run` files.
//!
//! The analysis is the real parser, so an editor and the runner can never
//! disagree about whether a file is valid.

pub mod analysis;
pub mod rpc;
pub mod server;
pub mod shell;

/// Speak the protocol on stdin/stdout until the client says goodbye.
///
/// This is the whole body of `run :lsp`, and it lives here rather than behind
/// a `main` of its own because there is no second binary to hold one. A
/// `runfile-lsp` shipped beside `run` was a second copy of this parser, free
/// to be older than the runner next to it -- which it was three times, and
/// each time an editor underlined valid files in red while the runner ran
/// them. One file cannot skew against itself.
pub fn serve() -> std::io::Result<()> {
	let mut input = std::io::BufReader::new(std::io::stdin().lock());
	let mut output = std::io::stdout().lock();
	server::Server::new().serve(&mut input, &mut output)
}
