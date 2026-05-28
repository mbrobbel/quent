// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

mod artifact_service;
mod builder;
mod config;
mod error;

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use artifact_service::{
    ArtifactService, Asset, DownloadedArtifact, artifact_format, ensure_supported_artifacts,
};
use clap::{Parser, Subcommand};
use quent_query_engine_server::viewer::{ViewLaunchContext, ViewTarget};
use serde_json::json;

use crate::{
    builder::ViewerBuilder,
    config::{Config, ViewerConfig},
    error::{OpenError, Result},
};

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

#[derive(Debug)]
struct ResolvedTarget {
    assets: Vec<Asset>,
    context: ViewLaunchContext,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.clone())?;
    let OpenCommand::Local { paths } = &cli.command;
    run_local(&config, &cli, paths).await
}

async fn run_local(config: &Config, cli: &Cli, paths: &[PathBuf]) -> Result<()> {
    let artifacts = load_local_artifacts(paths)?;
    ensure_supported_artifacts(&artifacts)?;

    let resolved = resolve_local_target(&artifacts);
    let viewer = select_viewer(config, cli, &resolved)?;

    launch(config, cli, viewer, resolved.context, artifacts).await
}

fn select_viewer<'a>(
    config: &'a Config,
    cli: &Cli,
    resolved: &ResolvedTarget,
) -> Result<&'a ViewerConfig> {
    match &cli.viewer {
        Some(name) => config.viewer_by_name(name),
        None => config.select_viewer(None, None, &resolved.assets),
    }
}

async fn launch(
    config: &Config,
    cli: &Cli,
    viewer: &ViewerConfig,
    mut context: ViewLaunchContext,
    artifacts: Vec<DownloadedArtifact>,
) -> Result<()> {
    context.viewer_name = Some(viewer.name.clone());
    let artifact_service = ArtifactService::start(artifacts, context).await?;

    let builder = ViewerBuilder::new(config.cache.build_dir.clone(), workspace_root()?);
    let built_viewer = builder.build(viewer)?;

    run_viewer(
        &built_viewer.binary,
        &artifact_service.manifest_url,
        built_viewer.ui_dist.as_deref(),
        cli.no_browser,
        cli.print_url,
    )
}

fn resolve_local_target(artifacts: &[DownloadedArtifact]) -> ResolvedTarget {
    let assets: Vec<Asset> = artifacts
        .iter()
        .map(|artifact| artifact.asset.clone())
        .collect();
    let files = assets
        .iter()
        .map(|asset| asset.original_filename.clone())
        .collect();
    let context = ViewLaunchContext {
        target: ViewTarget::Local { files },
        api_base_url: None,
        viewer_name: None,
        query_name: None,
        startup_route: Some("/profile".to_string()),
        api_data: json!({ "assets": assets.clone() }),
    };
    ResolvedTarget { assets, context }
}

fn load_local_artifacts(paths: &[PathBuf]) -> Result<Vec<DownloadedArtifact>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut entries = fs::read_dir(path)?
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|entry| entry.is_file() && is_supported_artifact(entry))
                .collect::<Vec<_>>();
            entries.sort();
            files.append(&mut entries);
        } else if path.is_file() {
            files.push(path.clone());
        } else {
            return Err(OpenError::Config(format!(
                "local artifact path does not exist: {}",
                path.display()
            )));
        }
    }

    files
        .into_iter()
        .enumerate()
        .map(|(index, path)| {
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    OpenError::Config(format!("invalid artifact filename: {}", path.display()))
                })?
                .to_string();
            let format = artifact_format(&filename).ok_or_else(|| {
                OpenError::Config(format!(
                    "unsupported artifact file '{}' (expected an ndjson, msgpack, or postcard extension)",
                    path.display()
                ))
            })?;
            let bytes = fs::read(&path)?;
            Ok(DownloadedArtifact {
                asset: Asset {
                    id: index as u64 + 1,
                    original_filename: filename,
                    media_type: media_type_for(format).to_string(),
                },
                bytes,
                format,
            })
        })
        .collect()
}

fn is_supported_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(artifact_format)
        .is_some()
}

fn media_type_for(format: &str) -> &'static str {
    match format {
        "ndjson" => "application/x-ndjson",
        "msgpack" => "application/vnd.msgpack",
        _ => "application/octet-stream",
    }
}

fn run_viewer(
    binary: &Path,
    manifest_url: &str,
    ui_dist: Option<&Path>,
    no_browser: bool,
    print_url: bool,
) -> Result<()> {
    let mut command = ProcessCommand::new(binary);
    command
        .arg("--artifact-manifest-url")
        .arg(manifest_url)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .arg("--print-url")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(ui_dist) = ui_dist {
        command.arg("--ui-dist").arg(ui_dist);
    }

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| OpenError::Build("viewer stdout was not captured".to_string()))?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let url = loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err(OpenError::Build(
                "viewer exited before printing QUENT_VIEWER_URL".to_string(),
            ));
        }
        if let Some(url) = line.trim_end().strip_prefix("QUENT_VIEWER_URL=") {
            break url.to_string();
        }
        print!("{line}");
    };

    std::thread::spawn(move || {
        for line in reader.lines().map_while(std::result::Result::ok) {
            println!("{line}");
        }
    });

    if print_url || no_browser {
        println!("{url}");
    }
    if !no_browser {
        open_browser(&url)?;
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(OpenError::Build(format!(
            "viewer exited with status {status}"
        )))
    }
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = ProcessCommand::new("open");
        command.arg(url);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = ProcessCommand::new("xdg-open");
        command.arg(url);
        command
    };

    command.spawn()?;
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or(OpenError::WorkspaceRoot(manifest_dir))
}
