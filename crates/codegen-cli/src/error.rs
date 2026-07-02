// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Error type for the codegen CLI.

use std::path::PathBuf;
use std::process::ExitStatus;

/// Errors produced while parsing a manifest, scaffolding, or building artifacts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The manifest file could not be read.
    #[error("failed to read manifest `{path}`: {source}")]
    ReadManifest {
        /// Manifest path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The manifest file could not be parsed as TOML.
    #[error("failed to parse manifest `{path}`: {source}")]
    ParseManifest {
        /// Manifest path.
        path: PathBuf,
        /// Underlying TOML error.
        #[source]
        source: toml::de::Error,
    },

    /// A filesystem operation failed.
    #[error("I/O error at `{path}`: {source}")]
    Io {
        /// Path involved in the operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The manifest is syntactically valid but semantically invalid.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// A build tool could not be spawned.
    #[error("failed to run `{program}`: {source}. Is it installed and on PATH?")]
    Spawn {
        /// The program that failed to spawn.
        program: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A build tool exited unsuccessfully.
    #[error("`{program}` exited with {status}")]
    Command {
        /// The program that failed.
        program: String,
        /// The exit status.
        status: ExitStatus,
    },

    /// A requested target is not implemented yet.
    #[error("target `{0}` is not supported yet (this phase implements `python`)")]
    UnsupportedTarget(String),
}

/// Result alias for CLI operations.
pub type Result<T> = std::result::Result<T, Error>;
