//! Importing superpowers plan files into the plan (spec §12.1). Files are only read.

pub mod parse;

pub use parse::{ParsedPlan, ParsedStep, ParsedTask, parse};

#[cfg(test)]
mod tests;
