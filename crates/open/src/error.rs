// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use thiserror::Error;

/// Result alias for `quent-open` operations.
pub type Result<T> = std::result::Result<T, OpenError>;

/// Errors that can occur while opening Quent artifacts in a viewer.
#[derive(Debug, Error)]
pub enum OpenError {
    /// An I/O error occurred (reading artifacts, spawning the viewer, etc.).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
