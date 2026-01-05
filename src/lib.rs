//! safe-rm: Safe file deletion tool for AI agents
//!
//! This library provides Git-aware access control for file deletion,
//! allowing AI agents to safely delete only clean or ignored files.

pub mod cli;
pub mod error;
pub mod git_checker;
pub mod path_checker;
