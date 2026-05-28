// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! A default UI analyzer for telemetry that only uses the standard query-engine
//! model components.

use std::{collections::HashMap as StdHashMap, marker::PhantomData};

use quent_analyzer::{
    AnalyzerError, AnalyzerResult, Entity, Span,
    resource::collection::{ResourceCollection, derive_resource_group_types},
};
use quent_events::Event;
use quent_query_engine_model::{QueryEngineEvent, engine::EngineEvent};
use quent_query_engine_ui::{QueryBundle, QueryEntities};
use quent_time::to_secs;
use quent_ui::{
    ResourceGroupNode, ResourceTree,
    quantity::QuantitySpec,
    timeline::{
        request::{BulkTimelineRequest, SingleTimelineRequest},
        response::{BulkTimelinesResponse, SingleTimelineResponse},
    },
};
use rustc_hash::FxHashSet as HashSet;
use uuid::Uuid;

use crate::{
    QueryEngineModel,
    model::{InMemoryQueryEngineModel, InMemoryQueryEngineModelBuilder},
    ui::UiAnalyzer,
};

/// Converts an application event enum into the standard query-engine event enum.
///
/// Generated or application-specific wrappers can implement this for local event
/// adapter types when their model embeds the standard query-engine components.
pub trait IntoQueryEngineEvent {
    fn into_query_engine_event(self) -> QueryEngineEvent;
}

impl IntoQueryEngineEvent for QueryEngineEvent {
    fn into_query_engine_event(self) -> QueryEngineEvent {
        self
    }
}

pub struct QueryEngineUiAnalyzer<E> {
    model: InMemoryQueryEngineModel,
    _event: PhantomData<E>,
}

impl<E> UiAnalyzer for QueryEngineUiAnalyzer<E>
where
    E: IntoQueryEngineEvent + Send + Sync + 'static,
{
    type Event = E;
    type EntityRef = Uuid;
    type TimelineGlobalParams = ();
    type TimelineParams = ();

    fn try_new(engine_id: Uuid, events: impl Iterator<Item = Event<E>>) -> AnalyzerResult<Self> {
        let mut builder = InMemoryQueryEngineModelBuilder::try_new(engine_id)?;
        for event in events {
            builder.try_push(Event::new(
                event.id,
                event.timestamp,
                event.data.into_query_engine_event(),
            ))?;
        }
        Ok(Self {
            model: builder.try_build()?,
            _event: PhantomData,
        })
    }

    fn extract_engine(
        engine_id: Uuid,
        events: impl Iterator<Item = Event<E>>,
    ) -> AnalyzerResult<quent_query_engine_ui::Engine> {
        for event in events {
            if let QueryEngineEvent::Engine(EngineEvent::Init(init)) =
                event.data.into_query_engine_event()
            {
                return Ok(quent_query_engine_ui::Engine {
                    id: engine_id,
                    start_time_unix_ns: Some(event.timestamp),
                    duration_s: None,
                    instance_name: init.instance_name,
                    implementation: Some(
                        quent_query_engine_ui::EngineImplementationAttributes::from(
                            &init.implementation,
                        ),
                    ),
                });
            }
        }
        Ok(quent_query_engine_ui::Engine::new(engine_id))
    }

    fn query_bundle(&self, query_id: Uuid) -> AnalyzerResult<QueryBundle<Uuid>> {
        let view = self.model.query_view(query_id)?;
        let query = self.model.query(query_id)?;
        let start_time_unix_ns = view.query_epoch(query_id)?;
        let duration_s = to_secs(query.span()?.duration());
        let epoch = view.query_epoch(query_id)?;

        let engine = view.engine()?.to_ui()?;
        let query_group_id = query.query_group_id().ok_or_else(|| {
            AnalyzerError::IncompleteEntity(format!("query {query_id} has no query_group_id"))
        })?;
        let query_group = view.query_group(query_group_id)?.to_ui();
        let query = query.to_ui()?;
        let workers = view.workers().map(|w| (w.id(), w.to_ui(epoch))).collect();
        let plans = view.plans().map(|p| (p.id(), p.to_ui())).collect();
        let operators = view.operators().map(|o| (o.id(), o.to_ui(epoch))).collect();
        let ports = view.ports().map(|p| (p.id(), p.to_ui(epoch))).collect();
        let unique_operator_names = view
            .operators()
            .filter_map(|operator| operator.operator_type_name().map(ToOwned::to_owned))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let resource_groups = self
            .model
            .resource_groups()
            .map(|group| (group.id(), group.into()))
            .collect();
        let resource_group_types = derive_resource_group_types(&self.model)?
            .iter()
            .map(|(name, group)| (name.clone(), group.into()))
            .collect();

        let resource_tree = ResourceTree::ResourceGroup(ResourceGroupNode {
            id: self.model.engine.id(),
            children: Vec::new(),
        });

        Ok(QueryBundle {
            query_id,
            entities: QueryEntities {
                engine,
                query_group,
                query,
                workers,
                plans,
                operators,
                ports,
                resource_types: StdHashMap::new(),
                resources: StdHashMap::new(),
                resource_groups,
                resource_group_types,
                fsm_types: StdHashMap::new(),
            },
            plan_tree: view.plan_tree(query_id)?.to_ui(),
            resource_tree,
            unique_operator_names,
            quantity_specs: [
                ("capacity_bytes".into(), QuantitySpec::bytes()),
                ("unit".into(), QuantitySpec::unit()),
            ]
            .into(),
            start_time_unix_ns,
            duration_s,
        })
    }

    fn query_engine_model(&self) -> &impl QueryEngineModel {
        &self.model
    }

    fn single_resource_timeline(
        &self,
        _request: SingleTimelineRequest<(), ()>,
    ) -> AnalyzerResult<SingleTimelineResponse> {
        Err(AnalyzerError::InvalidArgument(
            "query-engine-only analyzer does not provide resource timelines".to_string(),
        ))
    }

    fn bulk_resource_timeline(
        &self,
        request: BulkTimelineRequest<(), ()>,
    ) -> AnalyzerResult<BulkTimelinesResponse> {
        Ok(BulkTimelinesResponse {
            entries: request
                .entries
                .into_keys()
                .map(|key| {
                    (
                        key,
                        quent_ui::timeline::response::BulkTimelinesResponseEntry::Error {
                            message:
                                "query-engine-only analyzer does not provide resource timelines"
                                    .to_string(),
                        },
                    )
                })
                .collect(),
        })
    }
}
