// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Runtime support for generated application-specific Quent viewers.

use std::{
    collections::HashSet,
    marker::PhantomData,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    Extension, Json, Router,
    http::{StatusCode, Uri, header},
    response::IntoResponse,
    routing::get,
};
use quent_events::Event;
use quent_exporter::{
    ImporterOptions, MsgpackImporterOptions, NdjsonImporterOptions, PostcardImporterOptions,
    create_importer_from_bytes,
};
use quent_query_engine_analyzer::ui::UiAnalyzer;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{
    analyzer_service_router_with_assets,
    error::{ServerError, ServerResult},
};

#[derive(Clone, Debug)]
pub enum UiAssets {
    /// Serve the bundled Quent UI when the server crate is compiled with the
    /// `ui` feature. Without that feature this serves API routes only.
    Default,
    /// Serve assets from an application-specific UI dist directory.
    Directory(PathBuf),
    /// Serve API routes only.
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewTarget {
    BenchmarkRun {
        run_id: String,
        query_log_id: Option<String>,
    },
    QueryLog {
        query_log_id: String,
    },
    /// Artifacts loaded directly from local files, without the benchmark API.
    Local {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ViewLaunchContext {
    pub target: ViewTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_route: Option<String>,
    #[serde(default)]
    pub api_data: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactDescriptor {
    pub id: String,
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactManifest {
    pub context: ViewLaunchContext,
    pub artifacts: Vec<ArtifactDescriptor>,
}

pub trait QuentViewer {
    type Analyzer: UiAnalyzer + Send + Sync + 'static;

    fn ui_assets() -> UiAssets {
        UiAssets::Default
    }

    fn startup_route(context: &ViewLaunchContext, _engine_ids: &[Uuid]) -> String {
        context
            .startup_route
            .clone()
            .unwrap_or_else(|| "/profile".to_string())
    }

    /// Export this engine's UI model as TypeScript bindings into `out_dir`.
    ///
    /// An engine "exposes its UI" simply by deriving [`ts_rs::TS`] on its analyzer's
    /// associated UI types (`EntityRef`, timeline params). The frontend is then built
    /// against these generated bindings, so it renders the engine's entities and
    /// timelines without a hand-maintained per-engine frontend fork.
    fn export_ui_bindings(out_dir: &Path) -> Result<(), ts_rs::ExportError>
    where
        <Self::Analyzer as UiAnalyzer>::EntityRef: ts_rs::TS,
        <Self::Analyzer as UiAnalyzer>::TimelineGlobalParams: ts_rs::TS,
        <Self::Analyzer as UiAnalyzer>::TimelineParams: ts_rs::TS,
    {
        use quent_query_engine_ui::QueryBundle;
        use quent_ui::timeline::{
            request::{BulkTimelineRequest, SingleTimelineRequest},
            response::{BulkTimelinesResponse, SingleTimelineResponse},
        };
        use ts_rs::TS;

        QueryBundle::<<Self::Analyzer as UiAnalyzer>::EntityRef>::export_all_to(out_dir)?;
        SingleTimelineRequest::<
            <Self::Analyzer as UiAnalyzer>::TimelineGlobalParams,
            <Self::Analyzer as UiAnalyzer>::TimelineParams,
        >::export_all_to(out_dir)?;
        SingleTimelineResponse::export_all_to(out_dir)?;
        BulkTimelineRequest::<
            <Self::Analyzer as UiAnalyzer>::TimelineGlobalParams,
            <Self::Analyzer as UiAnalyzer>::TimelineParams,
        >::export_all_to(out_dir)?;
        BulkTimelinesResponse::export_all_to(out_dir)?;
        Ok(())
    }
}

pub struct DefaultQuentViewer<A>(PhantomData<A>);

impl<A> QuentViewer for DefaultQuentViewer<A>
where
    A: UiAnalyzer + Send + Sync + 'static,
{
    type Analyzer = A;
}

#[derive(Debug)]
struct ViewerMainArgs {
    manifest_url: Option<String>,
    listen: String,
    ui_dist: Option<PathBuf>,
    cors: Option<String>,
    export_ui_bindings: Option<PathBuf>,
}

impl ViewerMainArgs {
    fn parse() -> ServerResult<Self> {
        let mut manifest_url = None;
        let mut listen = "127.0.0.1:0".to_string();
        let mut ui_dist = None;
        let mut cors = None;
        let mut export_ui_bindings = None;

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--artifact-manifest-url" => {
                    manifest_url = args.next();
                }
                "--export-ui-bindings" => {
                    export_ui_bindings = Some(PathBuf::from(args.next().ok_or_else(|| {
                        ServerError::Artifact("--export-ui-bindings requires a value".to_string())
                    })?));
                }
                "--listen" => {
                    listen = args.next().ok_or_else(|| {
                        ServerError::Artifact("--listen requires a value".to_string())
                    })?;
                }
                "--ui-dist" => {
                    ui_dist = Some(PathBuf::from(args.next().ok_or_else(|| {
                        ServerError::Artifact("--ui-dist requires a value".to_string())
                    })?));
                }
                "--cors" => {
                    cors = args.next();
                }
                "--print-url" => {}
                other => {
                    return Err(ServerError::Artifact(format!(
                        "unknown viewer wrapper argument: {other}"
                    )));
                }
            }
        }

        Ok(Self {
            manifest_url,
            listen,
            ui_dist,
            cors,
            export_ui_bindings,
        })
    }
}

#[derive(Clone)]
struct LoadedArtifact {
    filename: String,
    format: ArtifactFormat,
    engine_id: Uuid,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ArtifactFormat {
    Ndjson,
    Msgpack,
    Postcard,
}

impl ArtifactFormat {
    fn from_descriptor(artifact: &ArtifactDescriptor) -> Option<Self> {
        artifact
            .format
            .as_deref()
            .and_then(Self::from_str)
            .or_else(|| {
                Path::new(&artifact.filename)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(Self::from_str)
            })
    }

    fn from_str(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "ndjson" | "jsonl" => Some(Self::Ndjson),
            "msgpack" | "mpack" => Some(Self::Msgpack),
            "postcard" => Some(Self::Postcard),
            _ => None,
        }
    }

    fn priority(self) -> usize {
        match self {
            Self::Ndjson => 0,
            Self::Msgpack => 1,
            Self::Postcard => 2,
        }
    }

    fn importer_options(self, filename: &str) -> ImporterOptions {
        let path = PathBuf::from(filename);
        match self {
            Self::Ndjson => ImporterOptions::Ndjson(NdjsonImporterOptions { path }),
            Self::Msgpack => ImporterOptions::Msgpack(MsgpackImporterOptions { path }),
            Self::Postcard => ImporterOptions::Postcard(PostcardImporterOptions { path }),
        }
    }
}

pub async fn viewer_main<V>() -> Result<(), Box<dyn std::error::Error>>
where
    V: QuentViewer,
    <V::Analyzer as UiAnalyzer>::Event: DeserializeOwned + 'static,
    <V::Analyzer as UiAnalyzer>::EntityRef: serde::Serialize,
    <V::Analyzer as UiAnalyzer>::TimelineGlobalParams:
        Send + Sync + Clone + serde::Serialize + std::hash::Hash + Eq + 'static,
    <V::Analyzer as UiAnalyzer>::TimelineParams:
        Send + Sync + Clone + serde::Serialize + std::hash::Hash + Eq + 'static,
    for<'de> <V::Analyzer as UiAnalyzer>::TimelineGlobalParams: serde::Deserialize<'de>,
    for<'de> <V::Analyzer as UiAnalyzer>::TimelineParams: serde::Deserialize<'de>,
{
    let args = ViewerMainArgs::parse()?;
    let manifest_url = args
        .manifest_url
        .ok_or_else(|| ServerError::Artifact("--artifact-manifest-url is required".to_string()))?;
    let client = reqwest::Client::new();
    let manifest = load_manifest(&client, &manifest_url).await?;
    let artifacts = Arc::new(load_artifacts(&client, &manifest).await?);
    let engine_ids = list_engine_ids(&artifacts);
    let context = manifest.context;

    let lister_artifacts = Arc::clone(&artifacts);
    let lister = move || Ok(list_engine_ids(&lister_artifacts));

    let importer_artifacts = Arc::clone(&artifacts);
    let importer = move |engine_id| {
        let artifact = importer_artifacts
            .iter()
            .filter(|artifact| artifact.engine_id == engine_id)
            .max_by_key(|artifact| artifact.format.priority())
            .ok_or_else(|| {
                ServerError::Artifact(format!("no artifact found for engine id {engine_id}"))
            })?;
        let kind = artifact.format.importer_options(&artifact.filename);
        Ok(Box::new(create_importer_from_bytes::<
            <V::Analyzer as UiAnalyzer>::Event,
        >(&kind, artifact.bytes.clone())?)
            as Box<
                dyn Iterator<Item = Event<<V::Analyzer as UiAnalyzer>::Event>>,
            >)
    };

    let ui_assets = args
        .ui_dist
        .map(UiAssets::Directory)
        .unwrap_or_else(V::ui_assets);
    let mut router = analyzer_service_router_with_assets::<V::Analyzer>(
        Box::new(importer),
        Box::new(lister),
        args.cors,
        ui_assets,
    )?;
    router = router.merge(launch_context_router(context.clone()));

    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    let address = listener.local_addr()?;
    let route = V::startup_route(&context, &engine_ids);
    let url = format!("http://{address}{}", normalize_route(&route));
    println!("QUENT_VIEWER_URL={url}");

    axum::serve(listener, router.into_make_service()).await?;
    Ok(())
}

/// Entry point for generated viewer wrappers.
///
/// Handles `--export-ui-bindings <dir>` (emit the engine's TypeScript bindings and
/// exit) before delegating to [`viewer_main`]. Unlike [`viewer_main`], this requires
/// the engine's UI types to implement [`ts_rs::TS`].
pub async fn run<V>() -> Result<(), Box<dyn std::error::Error>>
where
    V: QuentViewer,
    <V::Analyzer as UiAnalyzer>::Event: DeserializeOwned + 'static,
    <V::Analyzer as UiAnalyzer>::EntityRef: serde::Serialize + ts_rs::TS,
    <V::Analyzer as UiAnalyzer>::TimelineGlobalParams:
        Send + Sync + Clone + serde::Serialize + std::hash::Hash + Eq + ts_rs::TS + 'static,
    <V::Analyzer as UiAnalyzer>::TimelineParams:
        Send + Sync + Clone + serde::Serialize + std::hash::Hash + Eq + ts_rs::TS + 'static,
    for<'de> <V::Analyzer as UiAnalyzer>::TimelineGlobalParams: serde::Deserialize<'de>,
    for<'de> <V::Analyzer as UiAnalyzer>::TimelineParams: serde::Deserialize<'de>,
{
    if let Some(dir) = ViewerMainArgs::parse()?.export_ui_bindings {
        V::export_ui_bindings(&dir)?;
        return Ok(());
    }
    viewer_main::<V>().await
}

pub fn launch_context_router(context: ViewLaunchContext) -> Router {
    Router::new()
        .route("/api/quent-open/context", get(launch_context))
        .layer(Extension(Arc::new(context)))
}

async fn launch_context(
    Extension(context): Extension<Arc<ViewLaunchContext>>,
) -> Json<ViewLaunchContext> {
    Json((*context).clone())
}

pub async fn serve_static_dir(
    uri: Uri,
    Extension(root): Extension<Arc<PathBuf>>,
) -> impl IntoResponse {
    let Some(relative_path) = static_relative_path(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let candidate = root.join(&relative_path);
    let path = if candidate.is_file() {
        candidate
    } else {
        root.join("index.html")
    };

    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                bytes,
            )
                .into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn static_relative_path(path: &str) -> Option<PathBuf> {
    let path = path.trim_start_matches('/');
    if path.is_empty() || !path.contains('.') {
        return Some(PathBuf::from("index.html"));
    }

    let mut relative = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(segment) => relative.push(segment),
            _ => return None,
        }
    }
    Some(relative)
}

async fn load_manifest(client: &reqwest::Client, url: &str) -> ServerResult<ArtifactManifest> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| ServerError::Artifact(format!("failed to fetch artifact manifest: {e}")))?
        .error_for_status()
        .map_err(|e| ServerError::Artifact(format!("artifact manifest request failed: {e}")))?
        .json::<ArtifactManifest>()
        .await
        .map_err(|e| ServerError::Artifact(format!("invalid artifact manifest: {e}")))
}

async fn load_artifacts(
    client: &reqwest::Client,
    manifest: &ArtifactManifest,
) -> ServerResult<Vec<LoadedArtifact>> {
    let mut loaded = Vec::new();
    for artifact in &manifest.artifacts {
        let format = ArtifactFormat::from_descriptor(artifact).ok_or_else(|| {
            ServerError::Artifact(format!(
                "unsupported artifact format for {}",
                artifact.filename
            ))
        })?;
        let engine_id = Path::new(&artifact.filename)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| Uuid::parse_str(stem).ok())
            .ok_or_else(|| {
                ServerError::Artifact(format!(
                    "artifact filename does not start with an engine UUID: {}",
                    artifact.filename
                ))
            })?;
        let bytes = client
            .get(&artifact.url)
            .send()
            .await
            .map_err(|e| {
                ServerError::Artifact(format!("failed to fetch artifact {}: {e}", artifact.id))
            })?
            .error_for_status()
            .map_err(|e| {
                ServerError::Artifact(format!("artifact {} request failed: {e}", artifact.id))
            })?
            .bytes()
            .await
            .map_err(|e| {
                ServerError::Artifact(format!(
                    "failed to read artifact {} bytes: {e}",
                    artifact.id
                ))
            })?
            .to_vec();
        loaded.push(LoadedArtifact {
            filename: artifact.filename.clone(),
            format,
            engine_id,
            bytes,
        });
    }
    Ok(loaded)
}

fn list_engine_ids(artifacts: &[LoadedArtifact]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for artifact in artifacts {
        if seen.insert(artifact.engine_id) {
            ids.push(artifact.engine_id);
        }
    }
    ids
}

fn normalize_route(route: &str) -> String {
    if route.starts_with('/') {
        route.to_string()
    } else {
        format!("/{route}")
    }
}
