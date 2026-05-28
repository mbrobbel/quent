// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Utilities for server implementations

use crate::{
    analyzer_cache::AnalyzerCache, state::ServiceState, timeline_cache::TimelineCache,
    viewer::UiAssets,
};
use axum::{Extension, Router as AxumRouter, routing::get};
use quent_collector::server::{CollectorService, CollectorServiceOptions};
use quent_collector_proto::collector_server::CollectorServer;
use quent_query_engine_analyzer::ui::UiAnalyzer;
use std::{hash::Hash, sync::Arc};

use serde::{Deserialize, Serialize};
use tonic::transport::{Server as GrpcServer, server::Router};
use tower_http::cors::CorsLayer;

pub mod analyzer_cache;
pub mod error;
mod state;
mod timeline_cache;
mod ui;
pub mod viewer;

pub fn initialize_tracing(log_level: &str) {
    use tracing_subscriber::{
        EnvFilter,
        fmt::{self, format::FmtSpan},
        layer::SubscriberExt,
        registry,
        util::SubscriberInitExt,
    };
    registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(format!("{log_level},h2=off,tonic=off"))),
        )
        .with(
            fmt::layer()
                .with_target(true)
                .with_span_events(FmtSpan::CLOSE)
                .with_writer(std::io::stderr),
        )
        .init();
}

pub fn collector_service<E>(
    options: CollectorServiceOptions,
) -> Result<Router, Box<dyn std::error::Error>>
where
    E: Serialize + Send + Sync + 'static,
    for<'de> E: Deserialize<'de>,
{
    let collector = CollectorService::<E>::new(options);
    Ok(GrpcServer::builder().add_service(CollectorServer::new(collector)))
}

pub fn analyzer_service_router<A>(
    importer: Box<analyzer_cache::ImporterFn<A>>,
    lister: Box<analyzer_cache::ListerFn>,
    cors: Option<String>,
) -> Result<AxumRouter, Box<dyn std::error::Error>>
where
    A: UiAnalyzer + Send + Sync + 'static,
    <A as UiAnalyzer>::EntityRef: serde::Serialize,
    <A as UiAnalyzer>::TimelineGlobalParams:
        Send + Sync + Clone + serde::Serialize + Hash + Eq + 'static,
    <A as UiAnalyzer>::TimelineParams: Send + Sync + Clone + serde::Serialize + Hash + Eq + 'static,
    for<'de> <A as UiAnalyzer>::TimelineGlobalParams: serde::Deserialize<'de>,
    for<'de> <A as UiAnalyzer>::TimelineParams: serde::Deserialize<'de>,
{
    analyzer_service_router_with_assets::<A>(importer, lister, cors, UiAssets::Default)
}

pub fn analyzer_service_router_with_assets<A>(
    importer: Box<analyzer_cache::ImporterFn<A>>,
    lister: Box<analyzer_cache::ListerFn>,
    cors: Option<String>,
    ui_assets: UiAssets,
) -> Result<AxumRouter, Box<dyn std::error::Error>>
where
    A: UiAnalyzer + Send + Sync + 'static,
    <A as UiAnalyzer>::EntityRef: serde::Serialize,
    <A as UiAnalyzer>::TimelineGlobalParams:
        Send + Sync + Clone + serde::Serialize + Hash + Eq + 'static,
    <A as UiAnalyzer>::TimelineParams: Send + Sync + Clone + serde::Serialize + Hash + Eq + 'static,
    for<'de> <A as UiAnalyzer>::TimelineGlobalParams: serde::Deserialize<'de>,
    for<'de> <A as UiAnalyzer>::TimelineParams: serde::Deserialize<'de>,
{
    let state = ServiceState {
        analyzers: AnalyzerCache::<A>::new(importer, lister),
        timelines: TimelineCache::new(),
    };

    let mut http_routes = axum::Router::new().nest("/api/engines", ui::routes(state));

    #[cfg(feature = "swagger")]
    {
        use utoipa::OpenApi;
        use utoipa_swagger_ui::SwaggerUi;
        let api = ui::ApiDoc::openapi();
        http_routes =
            http_routes.merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api));
    }

    if let Some(cors) = cors {
        let cors = CorsLayer::new()
            .allow_origin(cors.parse::<axum::http::HeaderValue>().unwrap())
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([axum::http::header::CONTENT_TYPE]);
        http_routes = http_routes.layer(cors);
    }

    match ui_assets {
        UiAssets::Default => {
            #[cfg(feature = "ui")]
            {
                http_routes = http_routes.fallback(get(ui::embedded::serve));
            }
        }
        UiAssets::Directory(path) => {
            http_routes = http_routes
                .fallback(get(viewer::serve_static_dir))
                .layer(Extension(Arc::new(path)));
        }
        UiAssets::None => {}
    }

    Ok(http_routes)
}
