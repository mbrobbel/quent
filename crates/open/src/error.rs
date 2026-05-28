// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("api error: {0}")]
    Api(String),
    #[error("build error: {0}")]
    Build(String),
    #[error("no configured viewer matched the benchmark data")]
    NoViewer,
    #[error("process failed: {program} {args:?}: {stderr}")]
    Process {
        program: String,
        args: Vec<String>,
        stderr: String,
    },
    #[error("unable to find workspace root from {0}")]
    WorkspaceRoot(PathBuf),
}

pub type Result<T> = std::result::Result<T, OpenError>;
