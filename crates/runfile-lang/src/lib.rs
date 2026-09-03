//! The Runfile language: lexer, parser and AST for `.run` files.
//!
//! Grammar reference: `GRAMMAR.ebnf` at the workspace root.
//!
//! A `.run` file is one target. Lines are classified by their first token:
//! `#` comment, `.name` property, `$ ` shell, `exec …`/`end` block, otherwise
//! a statement. Language is the default; shell is explicitly marked.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod span;
