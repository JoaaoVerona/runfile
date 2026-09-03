//! Source locations. Every token and AST node carries one so the LSP can map
//! diagnostics back to the file without a second pass.

/// A half-open byte range into the source, plus the 1-based line it starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
	pub start: usize,
	pub end: usize,
	pub line: usize,
}

impl Span {
	pub fn new(start: usize, end: usize, line: usize) -> Self {
		Self { start, end, line }
	}

	/// A span covering both ends, for a node built from several tokens.
	pub fn to(self, other: Span) -> Self {
		Self {
			start: self.start,
			end: other.end,
			line: self.line,
		}
	}
}
