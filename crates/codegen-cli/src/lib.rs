// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `quent-codegen`: scaffold and build codegen artifacts (Python/C++/Rust) from
//! a quent model described by a `quent.toml` manifest.
//!
//! The tool never links the user's model directly. It generates small, detached
//! bridge/instrumentation crates whose `build.rs` invokes `quent-codegen`'s
//! emitters at cargo build time (the pattern used by the in-tree examples), then
//! drives `maturin`/`cargo`/`cmake` and collects the artifacts.

pub mod build;
pub mod config;
pub mod error;
pub mod names;
pub mod paths;
pub mod scaffold;
pub mod templates;

pub use error::{Error, Result};
