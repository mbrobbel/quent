// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `quent-open` opens local Quent benchmark artifacts in an application-specific
//! viewer. See <https://github.com/rapidsai/quent/issues/234>.
//!
//! This is a scaffold: the CLI surface is in place; the import/build/launch
//! pipeline is not yet implemented.

mod error;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::error::Result;

#[derive(Debug, Parser)]
#[command(name = "quent-open")]
#[command(about = "Open local Quent benchmark artifacts in an application-specific viewer")]
struct Cli {
    /// Config file path. Defaults to ./quent-open.toml, then ~/.config/quent/open.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Do not open a browser.
    #[arg(long, global = true)]
    no_browser: bool,

    /// Print the opened viewer URL.
    #[arg(long, global = true)]
    print_url: bool,

    /// Force a specific viewer by name from the config (skips automatic matching).
    #[arg(long, global = true)]
    viewer: Option<String>,

    #[command(subcommand)]
    command: OpenCommand,
}

#[derive(Debug, Subcommand)]
enum OpenCommand {
    /// Analyze local Quent artifact files directly.
    Local {
        /// Artifact files and/or directories to analyze. Files must be named
        /// `<engine-uuid>.<ext>` with an ndjson, msgpack, or postcard extension.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        OpenCommand::Local { paths } => run_local(&cli, paths).await,
    }
}

/// Open local artifact files in a viewer.
///
/// TODO(#234): load config, import the local artifacts, select and build a
/// matching viewer, serve the artifacts, then launch the viewer and open a
/// browser.
async fn run_local(_cli: &Cli, paths: &[PathBuf]) -> Result<()> {
    for path in paths {
        println!("would open: {}", path.display());
    }
    Ok(())
}
