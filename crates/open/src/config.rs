// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    artifact_service::Asset,
    error::{OpenError, Result},
};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub viewers: Vec<ViewerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            build_dir: default_build_dir(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewerConfig {
    pub name: String,
    #[serde(default, rename = "match")]
    pub match_rules: MatchRules,
    #[serde(default)]
    pub asset: AssetMatch,
    pub source: SourceConfig,
    pub rust: RustConfig,
    #[serde(default)]
    pub ui: Option<UiConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchRules {
    #[serde(default)]
    pub query_engine: QueryEngineMatch,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub commit_hash: Option<String>,
    #[serde(default)]
    pub extra_info: BTreeMap<String, Value>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct QueryEngineMatch {
    #[serde(default)]
    pub engine_name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetMatch {
    #[serde(default)]
    pub original_filename_regex: Option<String>,
    #[serde(default)]
    pub media_type_regex: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceConfig {
    pub git: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RustConfig {
    #[serde(default)]
    pub analyzer_type: Option<String>,
    #[serde(default)]
    pub viewer_type: Option<String>,
    #[serde(default)]
    pub query_engine_event_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiConfig {
    /// Optional dedicated frontend git source. When set, the UI is checked out
    /// separately from `[viewers.source]`; otherwise the UI builds from the
    /// viewer's analyzer-source checkout.
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// Working directory for `build_command`, relative to the checkout root.
    #[serde(default)]
    pub build_dir: PathBuf,
    /// Shell command that builds the frontend.
    pub build_command: String,
    /// Built frontend `dist` directory, relative to the checkout root.
    pub dist_dir: PathBuf,
    /// Directory (relative to the checkout root) where quent-open writes the
    /// engine's generated TypeScript bindings before building, so the frontend
    /// renders this engine's entities. The frontend imports its engine types from
    /// here (e.g. the path it already hardcodes for ts-rs bindings).
    #[serde(default)]
    pub bindings_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustEntrypoint<'a> {
    Analyzer(&'a str),
    Viewer(&'a str),
    QueryEngineEvent(&'a str),
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let path = match path {
            Some(path) => expand_tilde(path),
            None => default_config_path()?,
        };
        let contents = fs::read_to_string(&path)?;
        let mut config: Config = toml::from_str(&contents)?;
        config.cache.build_dir = expand_tilde(config.cache.build_dir);
        for viewer in &mut config.viewers {
            if let Some(ui) = &mut viewer.ui {
                ui.build_dir = expand_tilde(ui.build_dir.clone());
                ui.dist_dir = expand_tilde(ui.dist_dir.clone());
            }
        }
        Ok(config)
    }

    pub fn viewer_by_name(&self, name: &str) -> Result<&ViewerConfig> {
        self.viewers
            .iter()
            .find(|viewer| viewer.name == name)
            .ok_or_else(|| {
                let available = self
                    .viewers
                    .iter()
                    .map(|viewer| viewer.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                OpenError::Config(format!(
                    "no viewer named '{name}' in config (available: [{available}])"
                ))
            })
    }

    pub fn select_viewer<'a>(
        &'a self,
        run: Option<&Value>,
        query_log: Option<&Value>,
        assets: &[Asset],
    ) -> Result<&'a ViewerConfig> {
        self.viewers
            .iter()
            .find_map(|viewer| match viewer.matches(run, query_log, assets) {
                Ok(true) => Some(Ok(viewer)),
                Ok(false) => None,
                Err(e) => Some(Err(e)),
            })
            .transpose()?
            .ok_or(OpenError::NoViewer)
    }
}

impl ViewerConfig {
    pub fn matches(
        &self,
        run: Option<&Value>,
        query_log: Option<&Value>,
        assets: &[Asset],
    ) -> Result<bool> {
        if !self.match_rules.matches(run, query_log) {
            return Ok(false);
        }
        self.asset.matches(assets)
    }

    pub fn rust_entrypoint(&self) -> Result<RustEntrypoint<'_>> {
        match (
            self.rust.analyzer_type.as_deref(),
            self.rust.viewer_type.as_deref(),
            self.rust.query_engine_event_type.as_deref(),
        ) {
            (Some(analyzer), None, None) => Ok(RustEntrypoint::Analyzer(analyzer)),
            (None, Some(viewer), None) => Ok(RustEntrypoint::Viewer(viewer)),
            (None, None, Some(event_type)) => Ok(RustEntrypoint::QueryEngineEvent(event_type)),
            (None, None, None) => Err(OpenError::Config(format!(
                "viewer '{}' must set rust.analyzer_type, rust.viewer_type, or rust.query_engine_event_type",
                self.name
            ))),
            _ => Err(OpenError::Config(format!(
                "viewer '{}' must set only one of rust.analyzer_type, rust.viewer_type, or rust.query_engine_event_type",
                self.name
            ))),
        }
    }
}

impl MatchRules {
    fn matches(&self, run: Option<&Value>, query_log: Option<&Value>) -> bool {
        if !self.query_engine.matches(run) {
            return false;
        }
        if !matches_query_engine_field(run, "version", self.version.as_deref()) {
            return false;
        }
        if !matches_query_engine_field(run, "commit_hash", self.commit_hash.as_deref()) {
            return false;
        }
        if !matches_labels(run, &self.labels) {
            return false;
        }
        matches_extra_info(run, query_log, &self.extra_info)
    }
}

impl QueryEngineMatch {
    fn matches(&self, run: Option<&Value>) -> bool {
        matches_query_engine_field(run, "engine_name", self.engine_name.as_deref())
            && matches_query_engine_field(run, "version", self.version.as_deref())
            && matches_query_engine_field(run, "commit_hash", self.commit_hash.as_deref())
    }
}

impl AssetMatch {
    fn matches(&self, assets: &[Asset]) -> Result<bool> {
        let filename_regex = self
            .original_filename_regex
            .as_deref()
            .map(Regex::new)
            .transpose()?;
        let media_type_regex = self
            .media_type_regex
            .as_deref()
            .map(Regex::new)
            .transpose()?;
        if filename_regex.is_none() && media_type_regex.is_none() {
            return Ok(true);
        }
        Ok(assets.iter().any(|asset| {
            filename_regex
                .as_ref()
                .is_none_or(|regex| regex.is_match(&asset.original_filename))
                && media_type_regex
                    .as_ref()
                    .is_none_or(|regex| regex.is_match(&asset.media_type))
        }))
    }
}

fn matches_query_engine_field(run: Option<&Value>, field: &str, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    run.and_then(|run| run.get("query_engine"))
        .and_then(|engine| engine.get(field))
        .and_then(Value::as_str)
        .is_some_and(|actual| actual == expected)
}

fn matches_labels(run: Option<&Value>, expected: &[String]) -> bool {
    if expected.is_empty() {
        return true;
    }
    let labels = run
        .and_then(|run| run.get("labels"))
        .and_then(Value::as_array);
    expected.iter().all(|expected| {
        labels.is_some_and(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .any(|actual| actual == expected)
        })
    })
}

fn matches_extra_info(
    run: Option<&Value>,
    query_log: Option<&Value>,
    expected: &BTreeMap<String, Value>,
) -> bool {
    expected.iter().all(|(path, expected)| {
        let run_value = run
            .and_then(|run| run.get("extra_info"))
            .and_then(|extra| lookup_path(extra, path));
        let query_value = query_log
            .and_then(|query_log| query_log.get("extra_info"))
            .and_then(|extra| lookup_path(extra, path));
        run_value
            .or(query_value)
            .is_some_and(|actual| json_value_matches(actual, expected))
    })
}

fn lookup_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.as_object()?.get(segment))
}

fn json_value_matches(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::String(actual), Value::String(expected)) => actual == expected,
        (Value::Number(actual), Value::Number(expected)) => actual == expected,
        (Value::Bool(actual), Value::Bool(expected)) => actual == expected,
        _ => actual == expected,
    }
}

fn default_config_path() -> Result<PathBuf> {
    let local = PathBuf::from("quent-open.toml");
    if local.exists() {
        return Ok(local);
    }
    let home_config = expand_tilde(PathBuf::from("~/.config/quent/open.toml"));
    if home_config.exists() {
        return Ok(home_config);
    }
    Err(OpenError::Config(
        "no config path provided and neither ./quent-open.toml nor ~/.config/quent/open.toml exists"
            .to_string(),
    ))
}

fn default_build_dir() -> PathBuf {
    PathBuf::from("~/.cache/quent/open/builds")
}

pub fn expand_tilde(path: PathBuf) -> PathBuf {
    let Some(path_str) = path.to_str() else {
        return path;
    };
    if path_str == "~" {
        return home_dir().unwrap_or(path);
    }
    path_str
        .strip_prefix("~/")
        .and_then(|rest| home_dir().map(|home| home.join(rest)))
        .unwrap_or(path)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

pub fn relative_source_path(path: &Option<PathBuf>) -> &Path {
    path.as_deref().unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn viewer_match_uses_query_engine_asset_and_extra_info() {
        let viewer: ViewerConfig = toml::from_str(
            r#"
name = "cudf-polars"
match.query_engine.engine_name = "cudf-polars"
match.extra_info.cache_state = "warm"
asset.original_filename_regex = ".*\\.(ndjson|msgpack|postcard)$"

[source]
git = "git@example.com:org/repo.git"
ref = "main"

[rust]
analyzer_type = "app::Analyzer"
"#,
        )
        .unwrap();
        let run = json!({
            "query_engine": {"engine_name": "cudf-polars"},
            "extra_info": {"cache_state": "warm"}
        });
        let assets = vec![Asset {
            id: 1,
            original_filename: "550e8400-e29b-41d4-a716-446655440000.ndjson".to_string(),
            media_type: "application/x-ndjson".to_string(),
        }];

        assert!(viewer.matches(Some(&run), None, &assets).unwrap());
    }
}
