//! History: the branch graph, log search, commit details, diffs and blame. Everything here only
//! reads the repository (through gix); nothing writes.

pub mod diff;
pub mod graph;
pub mod layout;
pub mod search;

#[cfg(test)]
mod tests;
