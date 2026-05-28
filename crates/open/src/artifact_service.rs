// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashMap, path::Path, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use quent_query_engine_server::viewer::{ArtifactDescriptor, ArtifactManifest, ViewLaunchContext};
use serde::{Deserialize, Serialize};

use crate::error::{OpenError, Result};

/// Metadata for a single Quent artifact file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub id: u64,
    pub original_filename: String,
    pub media_type: String,
}

#[derive(Clone)]
pub struct DownloadedArtifact {
    pub asset: Asset,
    pub bytes: Vec<u8>,
    pub format: &'static str,
}

pub struct ArtifactService {
    pub manifest_url: String,
    _handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct ArtifactState {
    manifest: ArtifactManifest,
    artifacts: Arc<HashMap<String, DownloadedArtifact>>,
}

impl ArtifactService {
    pub async fn start(
        artifacts: Vec<DownloadedArtifact>,
        context: ViewLaunchContext,
    ) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let base_url = format!("http://{address}");

        let artifacts_by_id = artifacts
            .into_iter()
            .map(|artifact| (artifact.asset.id.to_string(), artifact))
            .collect::<HashMap<_, _>>();
        let descriptors = artifacts_by_id
            .iter()
            .map(|(id, artifact)| ArtifactDescriptor {
                id: id.clone(),
                filename: artifact.asset.original_filename.clone(),
                media_type: Some(artifact.asset.media_type.clone()),
                format: Some(artifact.format.to_string()),
                url: format!("{base_url}/artifacts/{id}"),
            })
            .collect();

        let state = Arc::new(ArtifactState {
            manifest: ArtifactManifest {
                context,
                artifacts: descriptors,
            },
            artifacts: Arc::new(artifacts_by_id),
        });

        let router = Router::new()
            .route("/manifest", get(manifest))
            .route("/artifacts/{id}", get(artifact))
            .with_state(state);

        let handle = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router.into_make_service()).await {
                eprintln!("quent-open artifact service failed: {error}");
            }
        });

        Ok(Self {
            manifest_url: format!("{base_url}/manifest"),
            _handle: handle,
        })
    }
}

async fn manifest(State(state): State<Arc<ArtifactState>>) -> Json<ArtifactManifest> {
    Json(state.manifest.clone())
}

async fn artifact(
    AxumPath(id): AxumPath<String>,
    State(state): State<Arc<ArtifactState>>,
) -> impl IntoResponse {
    let Some(artifact) = state.artifacts.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, artifact.asset.media_type.clone())],
        artifact.bytes.clone(),
    )
        .into_response()
}

pub fn artifact_format(filename: &str) -> Option<&'static str> {
    match Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("ndjson") | Some("jsonl") => Some("ndjson"),
        Some("msgpack") | Some("mpack") => Some("msgpack"),
        Some("postcard") => Some("postcard"),
        _ => None,
    }
}

pub fn ensure_supported_artifacts(artifacts: &[DownloadedArtifact]) -> Result<()> {
    if artifacts.is_empty() {
        return Err(OpenError::Api(
            "no raw Quent artifacts with ndjson, msgpack, or postcard extensions were found"
                .to_string(),
        ));
    }
    Ok(())
}
