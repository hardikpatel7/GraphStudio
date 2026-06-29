//! Tonic gRPC wrapper around `crate::cross_filter` + `crate::uam`.
//!
//! Two RPCs — `CrossFilter` and `GetEntitlements`. Logic lives in the
//! pure modules; this file is just a translation layer between proto
//! types and Rust types. Mirrors the existing
//! `services/article_graph_grpc.rs` shape.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::AppState;
use crate::cross_filter::model::{Filter, Operator, Values};
use crate::cross_filter::{apply_filters, project_distinct, EntitledSet};

/// Generated proto bindings.
pub mod proto {
    tonic::include_proto!("cross_filter");
}

use proto::{
    AttributeValues, CrossFilterRequest, CrossFilterResponse, Filter as ProtoFilter,
    GetEntitlementsRequest, GetEntitlementsResponse,
    cross_filter_service_server::CrossFilterService,
};

pub use proto::cross_filter_service_server::CrossFilterServiceServer;

const ENTITLEMENT_SAMPLE_LIMIT: usize = 50;

#[derive(Clone)]
pub struct CrossFilterGrpcService {
    state: Arc<AppState>,
}

impl CrossFilterGrpcService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl CrossFilterService for CrossFilterGrpcService {
    async fn cross_filter(
        &self,
        req: Request<CrossFilterRequest>,
    ) -> Result<Response<CrossFilterResponse>, Status> {
        let req = req.into_inner();
        let graph = self
            .state
            .legacy_graph
            .load_full()
            .ok_or_else(|| Status::failed_precondition("article_graph not built yet"))?;

        let filters: Vec<Filter> = req.filters.iter().map(proto_to_filter).collect();

        // UAM resolution. Same semantics as the HTTP path: required
        // when is_urm_filter=true, optional otherwise.
        let entitled: Option<EntitledSet> = if req.is_urm_filter {
            let user = req.user_code.ok_or_else(|| {
                Status::invalid_argument("user_code is required when is_urm_filter=true")
            })?;
            let acl = req.acl_code.ok_or_else(|| {
                Status::invalid_argument("acl_code is required when is_urm_filter=true")
            })?;
            let entry = self.state.uam.lookup(user, acl).ok_or_else(|| {
                Status::permission_denied(format!(
                    "no UAM entry for user_code={user} acl_code={acl}"
                ))
            })?;
            entry.entitled.clone()
        } else {
            None
        };

        let attr_owned: Vec<String> = req
            .attributes
            .iter()
            .map(|a| a.attribute_name.clone())
            .collect();

        // Run on a blocking task — projection is CPU-bound.
        let graph_b = graph.clone();
        let entitled_b = entitled;
        let result = tokio::task::spawn_blocking(move || -> CrossFilterResponse {
            let candidates = apply_filters(&graph_b, &filters, entitled_b.as_ref());
            let attr_refs: Vec<&str> = attr_owned.iter().map(String::as_str).collect();
            let data = project_distinct(&graph_b, &candidates, &attr_refs);
            let count = data.len() as i32;
            let proto_data: std::collections::HashMap<String, AttributeValues> = data
                .into_iter()
                .map(|(k, v)| (k, AttributeValues { values: v }))
                .collect();
            CrossFilterResponse {
                data: proto_data,
                count,
                status: true,
                message: "Successful".to_string(),
            }
        })
        .await
        .map_err(|e| Status::internal(format!("task join: {e}")))?;

        Ok(Response::new(result))
    }

    async fn get_entitlements(
        &self,
        req: Request<GetEntitlementsRequest>,
    ) -> Result<Response<GetEntitlementsResponse>, Status> {
        let req = req.into_inner();
        let entry = match self.state.uam.lookup(req.user_code, req.acl_code) {
            Some(e) => e,
            None => {
                return Ok(Response::new(GetEntitlementsResponse {
                    found: false,
                    unrestricted: false,
                    article_count: 0,
                    store_count: 0,
                    raw_filter_count: 0,
                    sample_articles: vec![],
                }));
            }
        };

        // Unrestricted = the row had empty filters jsonb.
        let unrestricted = entry.entitled.is_none();

        // For restricted entries, surface counts + a sample of
        // entitled article display names. The sample is intentionally
        // small — full lists go through CrossFilter or the
        // article_graph DataView.
        let (article_count, sample) = if let Some(ent) = &entry.entitled {
            let articles = ent.articles.as_ref();
            let count = articles.map(|s| s.len()).unwrap_or(0) as i32;
            let sample: Vec<String> = if let Some(set) = articles {
                let graph = self.state.legacy_graph.load_full();
                if let Some(g) = graph {
                    set.iter()
                        .take(ENTITLEMENT_SAMPLE_LIMIT)
                        .map(|id| g.get_str(g.node(*id).name).to_string())
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            (count, sample)
        } else {
            (0, Vec::new())
        };

        let store_count = entry
            .entitled
            .as_ref()
            .and_then(|e| e.store_codes.as_ref())
            .map(|s| s.len() as i32)
            .unwrap_or(0);

        Ok(Response::new(GetEntitlementsResponse {
            found: true,
            unrestricted,
            article_count,
            store_count,
            raw_filter_count: entry.raw_filter_count as i32,
            sample_articles: sample,
        }))
    }
}

/// proto::Filter → crate::cross_filter::model::Filter. The proto uses
/// `repeated string values` (always a list); model uses
/// `Values::List(Vec<Value>)` to support both list and singleton
/// shapes from the JSON HTTP path.
fn proto_to_filter(f: &ProtoFilter) -> Filter {
    let op = match f.operator.as_str() {
        "" | "in" => Operator::In,
        "eq" => Operator::Eq,
        "ne" => Operator::Ne,
        "in_eq" => Operator::InEq,
        "not_in" => Operator::NotIn,
        "like" => Operator::Like,
        "ilike" => Operator::ILike,
        _ => Operator::In,
    };
    let values = Values::List(
        f.values
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect(),
    );
    Filter {
        filter_id: None,
        attribute_name: f.attribute_name.clone(),
        dimension: None, // cross-filter resolver doesn't currently use dimension
        values,
        operator: op,
        extra: None,
    }
}
