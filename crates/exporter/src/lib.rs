// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Umbrella crate providing unified exporter/importer creation.

use std::{
    io::{BufRead, BufReader, Cursor},
    sync::Arc,
};

use quent_exporter_types::{Exporter, ExporterError, ExporterResult, Importer, ImporterResult};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(not(any(
    feature = "ndjson",
    feature = "msgpack",
    feature = "postcard",
    feature = "collector"
)))]
compile_error!("at least one exporter feature must be enabled");

#[cfg(feature = "collector")]
pub use quent_exporter_collector::CollectorExporterOptions;
#[cfg(feature = "msgpack")]
pub use quent_exporter_msgpack::{MsgpackExporterOptions, MsgpackImporterOptions};
#[cfg(feature = "ndjson")]
pub use quent_exporter_ndjson::{NdjsonExporterOptions, NdjsonImporterOptions};
#[cfg(feature = "postcard")]
pub use quent_exporter_postcard::{PostcardExporterOptions, PostcardImporterOptions};

/// Selects an exporter and its options.
#[derive(Debug, Clone)]
pub enum ExporterOptions {
    #[cfg(feature = "ndjson")]
    Ndjson(NdjsonExporterOptions),
    #[cfg(feature = "msgpack")]
    Msgpack(MsgpackExporterOptions),
    #[cfg(feature = "postcard")]
    Postcard(PostcardExporterOptions),
    #[cfg(feature = "collector")]
    Collector(CollectorExporterOptions),
}

/// Selects an importer and its options.
#[derive(Debug, Clone)]
pub enum ImporterOptions {
    #[cfg(feature = "ndjson")]
    Ndjson(NdjsonImporterOptions),
    #[cfg(feature = "msgpack")]
    Msgpack(MsgpackImporterOptions),
    #[cfg(feature = "postcard")]
    Postcard(PostcardImporterOptions),
}

/// A concrete importer selected from [`ImporterOptions`].
///
/// This avoids type-erasing the importer behind `Box<dyn Importer<_>>` while
/// still allowing the format to be selected at runtime.
pub enum ImporterVariant<T, R = BufReader<std::fs::File>> {
    #[cfg(feature = "ndjson")]
    Ndjson(quent_exporter_ndjson::NdjsonImporter<T, R>),
    #[cfg(feature = "msgpack")]
    Msgpack(quent_exporter_msgpack::MsgpackImporter<T, R>),
    #[cfg(feature = "postcard")]
    Postcard(quent_exporter_postcard::PostcardImporter<T, R>),
}

impl<T, R> Iterator for ImporterVariant<T, R>
where
    T: for<'de> Deserialize<'de>,
    R: BufRead,
{
    type Item = quent_events::Event<T>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            #[cfg(feature = "ndjson")]
            Self::Ndjson(importer) => importer.next(),
            #[cfg(feature = "msgpack")]
            Self::Msgpack(importer) => importer.next(),
            #[cfg(feature = "postcard")]
            Self::Postcard(importer) => importer.next(),
        }
    }
}

impl<T, R> Importer<T> for ImporterVariant<T, R>
where
    T: for<'de> Deserialize<'de>,
    R: BufRead,
{
}

/// Construct an importer from [`ImporterOptions`].
pub fn create_importer<T>(kind: &ImporterOptions) -> ImporterResult<ImporterVariant<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match kind {
        #[cfg(feature = "ndjson")]
        ImporterOptions::Ndjson(options) => Ok(ImporterVariant::Ndjson(
            quent_exporter_ndjson::NdjsonImporter::try_new(options)?,
        )),
        #[cfg(feature = "msgpack")]
        ImporterOptions::Msgpack(options) => Ok(ImporterVariant::Msgpack(
            quent_exporter_msgpack::MsgpackImporter::try_new(options)?,
        )),
        #[cfg(feature = "postcard")]
        ImporterOptions::Postcard(options) => Ok(ImporterVariant::Postcard(
            quent_exporter_postcard::PostcardImporter::try_new(options)?,
        )),
    }
}

/// Construct an importer from [`ImporterOptions`] using an in-memory byte buffer.
///
/// The path in `kind` is used only to choose the importer variant; bytes are not
/// read from disk.
pub fn create_importer_from_bytes<T>(
    kind: &ImporterOptions,
    bytes: Vec<u8>,
) -> ImporterResult<ImporterVariant<T, Cursor<Vec<u8>>>>
where
    T: for<'de> Deserialize<'de>,
{
    match kind {
        #[cfg(feature = "ndjson")]
        ImporterOptions::Ndjson(_) => Ok(ImporterVariant::Ndjson(
            quent_exporter_ndjson::NdjsonImporter::<T, Cursor<Vec<u8>>>::from_bytes(bytes),
        )),
        #[cfg(feature = "msgpack")]
        ImporterOptions::Msgpack(_) => Ok(ImporterVariant::Msgpack(
            quent_exporter_msgpack::MsgpackImporter::<T, Cursor<Vec<u8>>>::from_bytes(bytes),
        )),
        #[cfg(feature = "postcard")]
        ImporterOptions::Postcard(_) => Ok(ImporterVariant::Postcard(
            quent_exporter_postcard::PostcardImporter::<T, Cursor<Vec<u8>>>::from_bytes(bytes),
        )),
    }
}

/// Construct an importer from [`ImporterOptions`] using a caller-provided reader.
///
/// The path in `kind` is used only to choose the importer variant; bytes are not
/// read from disk.
pub fn create_importer_from_reader<T, R>(
    kind: &ImporterOptions,
    reader: R,
) -> ImporterResult<ImporterVariant<T, R>>
where
    T: for<'de> Deserialize<'de>,
    R: BufRead,
{
    match kind {
        #[cfg(feature = "ndjson")]
        ImporterOptions::Ndjson(_) => Ok(ImporterVariant::Ndjson(
            quent_exporter_ndjson::NdjsonImporter::from_reader(reader),
        )),
        #[cfg(feature = "msgpack")]
        ImporterOptions::Msgpack(_) => Ok(ImporterVariant::Msgpack(
            quent_exporter_msgpack::MsgpackImporter::from_reader(reader),
        )),
        #[cfg(feature = "postcard")]
        ImporterOptions::Postcard(_) => Ok(ImporterVariant::Postcard(
            quent_exporter_postcard::PostcardImporter::from_reader(reader),
        )),
    }
}

/// Construct an exporter from [`ExporterOptions`].
pub async fn create_exporter<T>(
    kind: ExporterOptions,
    application_id: Uuid,
) -> ExporterResult<Arc<dyn Exporter<T>>>
where
    T: Serialize + Send + 'static,
{
    match kind {
        #[cfg(feature = "ndjson")]
        ExporterOptions::Ndjson(options) => Ok(Arc::new(
            quent_exporter_ndjson::NdjsonExporter::try_new(application_id, options).await?,
        ) as Arc<dyn Exporter<T>>),
        #[cfg(feature = "msgpack")]
        ExporterOptions::Msgpack(options) => Ok(Arc::new(
            quent_exporter_msgpack::MsgpackExporter::try_new(application_id, options).await?,
        ) as Arc<dyn Exporter<T>>),
        #[cfg(feature = "postcard")]
        ExporterOptions::Postcard(options) => Ok(Arc::new(
            quent_exporter_postcard::PostcardExporter::try_new(application_id, options).await?,
        ) as Arc<dyn Exporter<T>>),
        #[cfg(feature = "collector")]
        ExporterOptions::Collector(options) => Ok(Arc::new(
            quent_exporter_collector::CollectorExporter::try_new(application_id, options)
                .await
                .map_err(|e| ExporterError::Collector(e.to_string()))?,
        ) as Arc<dyn Exporter<T>>),
    }
}
