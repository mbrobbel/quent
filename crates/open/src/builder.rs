// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use crate::{
    config::{RustEntrypoint, ViewerConfig, relative_source_path},
    error::{OpenError, Result},
};

pub struct BuiltViewer {
    pub binary: PathBuf,
    pub ui_dist: Option<PathBuf>,
}

pub struct ViewerBuilder {
    build_root: PathBuf,
    workspace_root: PathBuf,
}

impl ViewerBuilder {
    pub fn new(build_root: PathBuf, workspace_root: PathBuf) -> Self {
        Self {
            build_root,
            workspace_root,
        }
    }

    pub fn build(&self, viewer: &ViewerConfig) -> Result<BuiltViewer> {
        let cache_key = cache_key(viewer);
        let build_dir = self.build_root.join(cache_key);
        let source_dir = build_dir.join("source");
        let wrapper_dir = build_dir.join("wrapper");

        fs::create_dir_all(&build_dir)?;
        self.checkout(&viewer.source.git, &viewer.source.git_ref, &source_dir)?;

        // Build the viewer wrapper first so it can export the engine's UI bindings.
        self.write_wrapper(viewer, &source_dir, &wrapper_dir)?;
        eprintln!(
            "quent-open: building viewer '{}' (first build compiles dependencies and may take several minutes)",
            viewer.name
        );
        run_command(
            "cargo",
            &[
                "build".to_string(),
                "--manifest-path".to_string(),
                wrapper_dir.join("Cargo.toml").display().to_string(),
            ],
            Some(&wrapper_dir),
        )?;
        let binary = wrapper_dir
            .join("target")
            .join("debug")
            .join(executable_name("quent-open-wrapper"));

        let ui_dist = if let Some(ui) = &viewer.ui {
            // Build the frontend from a dedicated UI source when configured,
            // otherwise from the viewer's analyzer-source checkout.
            let ui_root = match &ui.git {
                Some(git) => {
                    let ui_ref = ui.git_ref.as_deref().ok_or_else(|| {
                        OpenError::Config(
                            "viewers.ui.ref is required when viewers.ui.git is set".to_string(),
                        )
                    })?;
                    let ui_source_dir = build_dir.join("ui-source");
                    self.checkout(git, ui_ref, &ui_source_dir)?;
                    ui_source_dir
                }
                None => source_dir.clone(),
            };
            // Inject the engine's generated TypeScript bindings so the frontend
            // renders this engine's entities/timelines.
            if let Some(bindings_dir) = &ui.bindings_dir {
                let target = ui_root.join(bindings_dir);
                fs::create_dir_all(&target)?;
                eprintln!("quent-open: exporting UI bindings to {}", target.display());
                run_command(
                    &binary.display().to_string(),
                    &[
                        "--export-ui-bindings".to_string(),
                        target.display().to_string(),
                    ],
                    None,
                )?;
            }
            eprintln!("quent-open: building UI ({})", ui.build_command);
            run_shell(&ui.build_command, &ui_root.join(&ui.build_dir))?;
            Some(ui_root.join(&ui.dist_dir))
        } else {
            None
        };

        eprintln!("quent-open: viewer built, starting...");
        Ok(BuiltViewer { binary, ui_dist })
    }

    fn checkout(&self, git: &str, git_ref: &str, dir: &Path) -> Result<()> {
        if !dir.exists() {
            eprintln!("quent-open: cloning source {git}");
            run_command(
                "git",
                &[
                    "clone".to_string(),
                    git.to_string(),
                    dir.display().to_string(),
                ],
                None,
            )?;
        }
        // Fetch the requested ref and hard-reset to the fetched commit. A plain
        // `git checkout <branch>` does NOT fast-forward an existing local branch,
        // so a cached clone would otherwise stay pinned to its original commit and
        // silently build a stale source forever.
        eprintln!("quent-open: fetching {git_ref} from {git}");
        run_command(
            "git",
            &[
                "fetch".to_string(),
                "--tags".to_string(),
                "origin".to_string(),
                git_ref.to_string(),
            ],
            Some(dir),
        )?;
        eprintln!("quent-open: checking out {git_ref}");
        run_command(
            "git",
            &[
                "checkout".to_string(),
                "--force".to_string(),
                "--detach".to_string(),
                "FETCH_HEAD".to_string(),
            ],
            Some(dir),
        )
    }

    fn write_wrapper(
        &self,
        viewer: &ViewerConfig,
        source_dir: &Path,
        wrapper_dir: &Path,
    ) -> Result<()> {
        let package = viewer.source.package.clone().unwrap_or_else(|| {
            package_name_from_git(&viewer.source.git).unwrap_or_else(|| viewer.name.clone())
        });
        let source_path = source_dir.join(relative_source_path(&viewer.source.path));
        let server_path = self.workspace_root.join("domains/query_engine/server");
        let analyzer_path = self.workspace_root.join("domains/query_engine/analyzer");
        let model_path = self.workspace_root.join("domains/query_engine/model");
        let ui_model_path = self.workspace_root.join("domains/query_engine/ui");
        let analyzer_core_path = self.workspace_root.join("crates/analyzer");
        let attributes_path = self.workspace_root.join("crates/attributes");
        let events_path = self.workspace_root.join("crates/events");
        let exporter_path = self.workspace_root.join("crates/exporter");
        let exporter_types_path = self.workspace_root.join("crates/exporter/types");
        let instrumentation_path = self.workspace_root.join("crates/instrumentation");
        let model_core_path = self.workspace_root.join("crates/model");
        let model_macros_path = self.workspace_root.join("crates/model-macros");
        let stdlib_path = self.workspace_root.join("crates/stdlib");
        let time_path = self.workspace_root.join("crates/time");
        let ui_path = self.workspace_root.join("crates/ui");
        let server_features = if viewer.ui.is_some() {
            String::new()
        } else {
            ", features = [\"ui\"]".to_string()
        };

        fs::create_dir_all(wrapper_dir.join("src"))?;
        fs::write(
            wrapper_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "quent-open-wrapper"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
serde = {{ version = "1.0.228", features = ["derive"] }}
tokio = {{ version = "1.48.0", features = ["macros", "rt-multi-thread"] }}
quent-query-engine-server = {{ path = {}{} }}
quent-query-engine-analyzer = {{ path = {} }}
quent-query-engine-model = {{ path = {} }}
quent-query-engine-ui = {{ path = {} }}
quent-analyzer = {{ path = {} }}
quent-events = {{ path = {} }}
quent-ui = {{ path = {} }}
{} = {{ package = {}, path = {} }}

[patch."https://github.com/rapidsai/quent.git"]
quent-analyzer = {{ path = {} }}
quent-attributes = {{ path = {} }}
quent-events = {{ path = {} }}
quent-exporter = {{ path = {} }}
quent-exporter-types = {{ path = {} }}
quent-instrumentation = {{ path = {} }}
quent-model = {{ path = {} }}
quent-model-macros = {{ path = {} }}
quent-query-engine-analyzer = {{ path = {} }}
quent-query-engine-model = {{ path = {} }}
quent-query-engine-ui = {{ path = {} }}
quent-stdlib = {{ path = {} }}
quent-time = {{ path = {} }}
quent-ui = {{ path = {} }}
"#,
                toml_string(&server_path.display().to_string()),
                server_features,
                toml_string(&analyzer_path.display().to_string()),
                toml_string(&model_path.display().to_string()),
                toml_string(&ui_model_path.display().to_string()),
                toml_string(&analyzer_core_path.display().to_string()),
                toml_string(&events_path.display().to_string()),
                toml_string(&ui_path.display().to_string()),
                package,
                toml_string(&package),
                toml_string(&source_path.display().to_string()),
                toml_string(&analyzer_core_path.display().to_string()),
                toml_string(&attributes_path.display().to_string()),
                toml_string(&events_path.display().to_string()),
                toml_string(&exporter_path.display().to_string()),
                toml_string(&exporter_types_path.display().to_string()),
                toml_string(&instrumentation_path.display().to_string()),
                toml_string(&model_core_path.display().to_string()),
                toml_string(&model_macros_path.display().to_string()),
                toml_string(&analyzer_path.display().to_string()),
                toml_string(&model_path.display().to_string()),
                toml_string(&ui_model_path.display().to_string()),
                toml_string(&stdlib_path.display().to_string()),
                toml_string(&time_path.display().to_string()),
                toml_string(&ui_path.display().to_string()),
            ),
        )?;

        fs::write(wrapper_dir.join("src/main.rs"), wrapper_main_rs(viewer)?)?;
        Ok(())
    }
}

fn wrapper_main_rs(viewer: &ViewerConfig) -> Result<String> {
    Ok(match viewer.rust_entrypoint()? {
        RustEntrypoint::Analyzer(analyzer_type) => format!(
            r#"use quent_query_engine_server::{{
    initialize_tracing,
    viewer::{{run, DefaultQuentViewer}},
}};

type GeneratedViewer = DefaultQuentViewer<{analyzer_type}>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    initialize_tracing("info");
    run::<GeneratedViewer>().await
}}
"#
        ),
        RustEntrypoint::Viewer(viewer_type) => format!(
            r#"use quent_query_engine_server::{{
    initialize_tracing,
    viewer::viewer_main,
}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    initialize_tracing("info");
    viewer_main::<{viewer_type}>().await
}}
"#
        ),
        RustEntrypoint::QueryEngineEvent(event_type) => format!(
            r#"use quent_query_engine_analyzer::basic_ui::{{IntoQueryEngineEvent, QueryEngineUiAnalyzer}};
use quent_query_engine_model::QueryEngineEvent;
use quent_query_engine_server::{{
    initialize_tracing,
    viewer::{{viewer_main, DefaultQuentViewer}},
}};

#[derive(serde::Deserialize)]
struct GeneratedEvent(#[serde(with = "serde_event")] {event_type});

impl IntoQueryEngineEvent for GeneratedEvent {{
    fn into_query_engine_event(self) -> QueryEngineEvent {{
        match self.0 {{
            {event_type}::Engine(event) => QueryEngineEvent::Engine(event),
            {event_type}::Worker(event) => QueryEngineEvent::Worker(event),
            {event_type}::QueryGroup(event) => QueryEngineEvent::QueryGroup(event),
            {event_type}::Query(event) => QueryEngineEvent::Query(event),
            {event_type}::Plan(event) => QueryEngineEvent::Plan(event),
            {event_type}::Operator(event) => QueryEngineEvent::Operator(event),
            {event_type}::Port(event) => QueryEngineEvent::Port(event),
        }}
    }}
}}

mod serde_event {{
    use serde::Deserialize;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<{event_type}, D::Error>
    where
        D: serde::Deserializer<'de>,
    {{
        {event_type}::deserialize(deserializer)
    }}
}}

type GeneratedViewer = DefaultQuentViewer<QueryEngineUiAnalyzer<GeneratedEvent>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    initialize_tracing("info");
    viewer_main::<GeneratedViewer>().await
}}
"#
        ),
    })
}

fn run_shell(command: &str, cwd: &Path) -> Result<()> {
    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/C".to_string(), command.to_string()]);
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-lc".to_string(), command.to_string()]);
    run_command(program, &args, Some(cwd))
}

fn run_command(program: &str, args: &[String], cwd: Option<&Path>) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // Inherit stdio so git/cargo progress streams to the user in real time —
    // these steps can take minutes, and silent capture looks like a hang.
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(OpenError::Process {
            program: program.to_string(),
            args: args.to_vec(),
            stderr: format!("exited with {status}"),
        })
    }
}

fn cache_key(viewer: &ViewerConfig) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("{viewer:?}"));
    to_hex(&hash.finalize())[..16].to_string()
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn package_name_from_git(git: &str) -> Option<String> {
    git.rsplit(['/', ':'])
        .next()
        .map(|name| name.trim_end_matches(".git").to_string())
        .filter(|name| !name.is_empty())
}

fn executable_name(name: &str) -> String {
    #[cfg(windows)]
    {
        format!("{name}.exe")
    }
    #[cfg(not(windows))]
    {
        name.to_string()
    }
}
