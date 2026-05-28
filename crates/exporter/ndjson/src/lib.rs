// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Exporter dumping events as newline-delimited JSON objects into a file.
use std::{
    io::{BufRead, BufReader, Cursor},
    marker::PhantomData,
    path::PathBuf,
};

use quent_events::Event;
use quent_exporter_types::{Exporter, ExporterError, ExporterResult, Importer, ImporterResult};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncWriteExt, BufWriter},
    sync::Mutex,
};
use tracing::{debug, error};
use uuid::Uuid;

/// Options for the ndjson exporter.
///
/// Writes events as newline-delimited JSON (one JSON object per line per file).
/// Human-readable, useful for debugging and manual inspection. Produces one
/// file per instrumentation context in `output_dir`.
#[derive(Debug, Clone)]
pub struct NdjsonExporterOptions {
    pub output_dir: PathBuf,
}

#[derive(Debug)]
pub struct NdjsonExporter {
    writer: Mutex<BufWriter<File>>,
}

impl NdjsonExporter {
    pub async fn try_new(
        application_id: Uuid,
        options: NdjsonExporterOptions,
    ) -> ExporterResult<Self> {
        tokio::fs::create_dir_all(&options.output_dir).await?;
        let path = options
            .output_dir
            .join(format!("{}.ndjson", application_id));
        debug!("exporting to \"{}\"", path.display());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }
}

#[async_trait::async_trait]
impl<T> Exporter<T> for NdjsonExporter
where
    T: Serialize + Send + 'static,
{
    async fn push(&self, event: Event<T>) -> ExporterResult<()> {
        let line = format!(
            "{}\n",
            serde_json::to_string(&event).map_err(|e| ExporterError::Serde(format!("{e:?}")))?
        );
        let mut lock = self.writer.lock().await;
        lock.write_all(line.as_bytes()).await?;
        Ok(())
    }

    async fn force_flush(&self) -> ExporterResult<()> {
        match self.writer.lock().await.flush().await {
            Ok(_) => Ok(()),
            Err(e) => {
                let err = format!("unable to flush ndjson exporter: {e}");
                error!("{err}");
                Err(ExporterError::Flush(err))
            }
        }
    }
}

#[derive(Debug, Clone)]
/// Options for the ndjson importer. Reads events from the file at `path`.
pub struct NdjsonImporterOptions {
    pub path: PathBuf,
}

pub struct NdjsonImporter<T, R = BufReader<std::fs::File>> {
    reader: R,
    _phantom: PhantomData<T>,
}

impl<T> NdjsonImporter<T, BufReader<std::fs::File>> {
    pub fn try_new(options: &NdjsonImporterOptions) -> ImporterResult<Self> {
        let file = std::fs::File::open(&options.path)?;
        Ok(Self::from_reader(BufReader::new(file)))
    }
}

impl<T, R> NdjsonImporter<T, R> {
    pub fn from_reader(reader: R) -> Self
    where
        R: BufRead,
    {
        Self {
            reader,
            _phantom: Default::default(),
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> NdjsonImporter<T, Cursor<Vec<u8>>> {
        NdjsonImporter::from_reader(Cursor::new(bytes))
    }
}

impl<T, R> Importer<T> for NdjsonImporter<T, R>
where
    T: for<'de> Deserialize<'de>,
    R: BufRead,
{
}

impl<T, R> Iterator for NdjsonImporter<T, R>
where
    T: for<'de> Deserialize<'de>,
    R: BufRead,
{
    type Item = Event<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => {
                let trimmed = line.trim_end();
                match serde_json::from_str::<Event<T>>(trimmed) {
                    Ok(event) => Some(event),
                    Err(e) => {
                        error!("failed to parse ndjson line: {e}");
                        None
                    }
                }
            }
            Err(e) => {
                error!("failed to read ndjson: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use quent_events::Event;
    use uuid::Uuid;

    use super::NdjsonImporter;

    #[test]
    fn imports_from_bytes() {
        let event = Event::new(Uuid::nil(), 42, "payload".to_string());
        let bytes = format!("{}\n", serde_json::to_string(&event).unwrap()).into_bytes();

        let mut importer = NdjsonImporter::<String>::from_bytes(bytes);
        let imported = importer.next().unwrap();

        assert_eq!(imported.id, event.id);
        assert_eq!(imported.timestamp, event.timestamp);
        assert_eq!(imported.data, event.data);
        assert!(importer.next().is_none());
    }
}
