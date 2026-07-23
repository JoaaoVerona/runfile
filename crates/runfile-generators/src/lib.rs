mod editorconfig;
mod jetbrains;
mod task_descriptors;
mod vscode;
mod zed;

pub use editorconfig::*;
pub use jetbrains::*;
pub use task_descriptors::*;
pub use vscode::*;
pub use zed::*;

#[cfg(test)]
mod tests;
