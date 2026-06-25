// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `quent-open` opens local Quent benchmark artifacts in an application-specific
//! viewer. See <https://github.com/rapidsai/quent/issues/234>.
//!
//! Given a context directory, it reads the `model.qmi` provenance sidecar,
//! generates a small viewer crate pinned to the recorded quent + analyzer commits
//! (see [`wrapper`]), builds and serves it, and opens a browser.
//!
//! Building the viewer fetches the recorded git sources and compiles the embedded
//! UI, which runs `pnpm`/`node` on first build (cached afterwards); these must be
//! available on `PATH`.

mod error;
mod spec;
mod viewer;
mod wrapper;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use quent_build_info::{ArtifactInfo, SIDECAR_FILE_NAME};

use crate::error::{OpenError, Result};
use crate::spec::ViewerSpec;
use crate::viewer::ViewerGroup;

#[derive(Debug, Parser)]
#[command(name = "quent-open")]
#[command(about = "Open local Quent benchmark artifacts in an application-specific viewer")]
struct Cli {
    /// Do not open a browser (a viewer's URL is always printed when it is ready).
    #[arg(long, global = true)]
    no_browser: bool,

    #[command(subcommand)]
    command: OpenCommand,
}

#[derive(Debug, Subcommand)]
enum OpenCommand {
    /// Analyze local Quent artifacts directly.
    Local {
        /// Context directories to analyze. A context directory holds a `model.qmi`
        /// provenance sidecar at its root, plus one per-entity subdirectory per
        /// entity containing that entity's event stream.
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

/// Discover all context directories under `paths` (recursively), group them into
/// one viewer per distinct build spec (same analyzer + pinned commits + format),
/// then build and serve those viewers in parallel. Contexts that can't be opened
/// (no analyzer package, unreadable sidecar) are skipped with a warning rather
/// than aborting.
async fn run_local(cli: &Cli, paths: &[PathBuf]) -> Result<()> {
    let contexts = spec::discover_contexts(paths)?;

    // One group per build spec; contexts sharing a spec share a viewer.
    let mut groups: BTreeMap<String, ViewerGroup> = BTreeMap::new();
    for context in contexts {
        let spec = match read_artifact_info(&context)
            .and_then(|info| ViewerSpec::from_artifact(&context, &info))
        {
            Ok(spec) => spec,
            Err(e) => {
                eprintln!("skipping {}: {e}", context.display());
                continue;
            }
        };
        groups
            .entry(spec.group_key())
            .or_insert_with(|| ViewerGroup {
                spec: spec.clone(),
                contexts: Vec::new(),
            })
            .contexts
            .push(context);
    }

    if groups.is_empty() {
        return Err(OpenError::NoContexts);
    }
    viewer::open_all(groups.into_values().collect(), cli.no_browser).await
}

/// Read the [`ArtifactInfo`] sidecar from the context directory `dir`.
fn read_artifact_info(dir: &Path) -> Result<ArtifactInfo> {
    ArtifactInfo::read_sidecar(dir).map_err(|source| OpenError::Sidecar {
        path: dir.join(SIDECAR_FILE_NAME),
        source,
    })
}
