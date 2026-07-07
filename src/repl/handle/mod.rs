// src/repl/handle/mod.rs
//! All input/event/command dispatch logic, split by concern.

pub mod command;
pub mod event;
pub mod git;
pub mod insert;
pub mod key;
pub mod merge;
pub mod normal;
pub mod popup;
pub mod search;
pub mod visual;
