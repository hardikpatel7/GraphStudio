//! Tonic gRPC wrapper around `rcl::RuleStore`.
//!
//! - Bootstrapped from the default PG `data_source` row in SQLite.
//! - Drives a [`PgListenSource`] (LISTEN/NOTIFY); falls back to
//!   [`PgPollSource`] if the migration triggers haven't been applied.
//! - Exposes 3 unary resolves + a server-stream `Subscribe` that emits a
//!   fresh [`RuleSetSnapshot`] every time the rules change.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rcl::{
    PgListenSource, PgPollSource, ProductHierarchy as RclProductHierarchy, PsmInput as RclPsmInput,
    RuleStore, resolve_constraints, resolve_dc_policy, resolve_psm,
};
use tokio_stream::wrappers::WatchStream;
use tonic::{Request, Response, Status};

// Generated Tonic types from proto/rcl.proto.
pub mod proto {
    tonic::include_proto!("rcl");
}

use proto::{
    ConstraintEntry, ConstraintRow, ConstraintRowList, DcPolicy as ProtoDcPolicy, DcPolicyEntry,
    ProductHierarchy as ProtoProductHierarchy, PsmPair, PsmResolved as ProtoPsmResolved,
    RclRule as ProtoRclRule, ResolveConstraintsRequest, ResolveConstraintsResponse,
    ResolveDcPolicyRequest, ResolveDcPolicyResponse, ResolvePsmRequest, ResolvePsmResponse,
    RuleSetSnapshot, SubscribeRequest, rcl_service_server::RclService,
};

pub use proto::rcl_service_server::RclServiceServer;

// ── Service ────────────────────────────────────────────────────────────────

/// Tonic handler. Wraps an `Arc<RuleStore>`; all RPCs grab a `snapshot()`
/// (cheap Arc clone) and resolve against it.
#[derive(Clone)]
pub struct RclGrpcService {
    store: Arc<RuleStore>,
}

impl RclGrpcService {
    pub fn new(store: Arc<RuleStore>) -> Self {
        Self { store }
    }
}

#[tonic::async_trait]
impl RclService for RclGrpcService {
    async fn resolve_dc_policy(
        &self,
        req: Request<ResolveDcPolicyRequest>,
    ) -> Result<Response<ResolveDcPolicyResponse>, Status> {
        let req = req.into_inner();
        let snap = self.store.snapshot();

        // Borrow strings from the proto request to avoid clones.
        let products: Vec<RclProductHierarchy<'_>> =
            req.products.iter().map(proto_to_hierarchy).collect();
        let resolved = resolve_dc_policy(&snap, &products);

        let resolved_proto = resolved
            .into_iter()
            .map(|(pc, p)| (pc, dc_policy_to_proto(p)))
            .collect();
        Ok(Response::new(ResolveDcPolicyResponse {
            resolved: resolved_proto,
            version: snap.version,
        }))
    }

    async fn resolve_constraints(
        &self,
        req: Request<ResolveConstraintsRequest>,
    ) -> Result<Response<ResolveConstraintsResponse>, Status> {
        let req = req.into_inner();
        let snap = self.store.snapshot();

        let products: Vec<RclProductHierarchy<'_>> =
            req.products.iter().map(proto_to_hierarchy).collect();
        let resolved = resolve_constraints(&snap, &products);

        let resolved_proto = resolved
            .into_iter()
            .map(|(pc, rows)| {
                (
                    pc,
                    ConstraintRowList {
                        rows: rows.iter().map(constraint_to_proto).collect(),
                    },
                )
            })
            .collect();
        Ok(Response::new(ResolveConstraintsResponse {
            resolved: resolved_proto,
            version: snap.version,
        }))
    }

    async fn resolve_psm(
        &self,
        req: Request<ResolvePsmRequest>,
    ) -> Result<Response<ResolvePsmResponse>, Status> {
        let req = req.into_inner();
        let snap = self.store.snapshot();

        let inputs: Vec<RclPsmInput<'_>> = req
            .pairs
            .iter()
            .filter_map(|p| {
                let h = p.hierarchy.as_ref()?;
                Some(RclPsmInput {
                    hierarchy: proto_to_hierarchy(h),
                    store_code: &p.store_code,
                    psa_code: &p.psa_code,
                })
            })
            .collect();

        let resolved = resolve_psm(&snap, &inputs);
        let resolved_proto = resolved
            .into_iter()
            .map(|r| ProtoPsmResolved {
                product_code: r.product_code,
                store_code: r.store_code,
                rcl_code: r.rcl_code,
                is_active: r.is_active,
            })
            .collect();
        Ok(Response::new(ResolvePsmResponse {
            resolved: resolved_proto,
            version: snap.version,
        }))
    }

    type SubscribeStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<RuleSetSnapshot, Status>> + Send>>;

    async fn subscribe(
        &self,
        _req: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let rx = self.store.subscribe();
        // WatchStream emits the current value on subscribe and then on every change.
        let watch_stream = WatchStream::new(rx);
        let mapped = async_stream::stream! {
            for await arc in watch_stream {
                yield Ok(ruleset_to_proto(&arc));
            }
        };
        Ok(Response::new(Box::pin(mapped)))
    }
}

// ── Bootstrap ──────────────────────────────────────────────────────────────

/// Build a [`RuleStore`] for `dsn`. Tries [`PgListenSource`] first; if the
/// migration triggers aren't installed, the LISTEN call still succeeds — it
/// just never wakes. Callers that need polling-only behavior can pass
/// `use_polling=true`.
pub async fn build_rule_store(dsn: String, use_polling: bool) -> anyhow::Result<RuleStore> {
    if use_polling {
        let source = PgPollSource::new(dsn.clone(), Duration::from_secs(10));
        RuleStore::start(dsn, Box::new(source), Default::default()).await
    } else {
        let source = PgListenSource::new(dsn.clone());
        RuleStore::start(dsn, Box::new(source), Default::default()).await
    }
}

// ── proto ↔ rcl conversions ────────────────────────────────────────────────

fn proto_to_hierarchy(p: &ProtoProductHierarchy) -> RclProductHierarchy<'_> {
    RclProductHierarchy {
        product_code: &p.product_code,
        l0_name: &p.l0_name,
        l1_name: &p.l1_name,
        l2_name: &p.l2_name,
        l3_name: &p.l3_name,
        l4_name: &p.l4_name,
        l5_name: &p.l5_name,
        brand: &p.brand,
    }
}

fn dc_policy_to_proto(p: &rcl::DcPolicy) -> ProtoDcPolicy {
    ProtoDcPolicy {
        default_store_groups: p.default_store_groups.clone(),
        default_product_profile: p.default_product_profile.clone(),
        dc_store_rule: p.dc_store_rule.clone(),
    }
}

fn constraint_to_proto(c: &rcl::ConstraintRow) -> ConstraintRow {
    ConstraintRow {
        psa_code: c.psa_code.clone(),
        aps: c.aps,
        wos: c.wos,
        min_stock: c.min_stock,
        max_stock: c.max_stock,
    }
}

/// Translate a full [`rcl::RuleSet`] to a protobuf snapshot for streaming.
fn ruleset_to_proto(rs: &rcl::RuleSet) -> RuleSetSnapshot {
    RuleSetSnapshot {
        rules: rs.rules.iter().map(rule_to_proto).collect(),
        // Policies are now keyed by (rcl_code, rule_code). The proto entry is
        // a flat list keyed by rcl_code only (legacy shape). Encode the
        // composite key as `<rcl_code>:<rule_code>` so consumers can still
        // distinguish rule_code variants without a proto schema bump.
        policies: rs
            .policies
            .iter()
            .map(|((rcl_code, rule_code), p)| DcPolicyEntry {
                rcl_code: if rule_code.is_empty() {
                    rcl_code.clone()
                } else {
                    format!("{}:{}", rcl_code, rule_code)
                },
                policy: Some(dc_policy_to_proto(p)),
            })
            .collect(),
        // Constraints are now keyed by (rcl_code, rule_code) — same shape
        // change as policies above. Encode the key as `<rcl_code>:<rule_code>`
        // so the proto stays scalar.
        constraints: rs
            .constraints
            .iter()
            .map(|((rcl_code, rule_code), rows)| ConstraintEntry {
                rcl_code: if rule_code.is_empty() {
                    rcl_code.clone()
                } else {
                    format!("{}:{}", rcl_code, rule_code)
                },
                rows: rows.iter().map(constraint_to_proto).collect(),
            })
            .collect(),
        version: rs.version,
        bytes_hash: rs.bytes_hash,
    }
}

fn rule_to_proto(r: &rcl::RclRule) -> ProtoRclRule {
    // Selectors are now sets of values (was a single string). The proto
    // field stays scalar — emit a comma-joined view for inspection.
    // Consumers wanting structured access should call the gRPC `Resolve*`
    // methods rather than reading the snapshot proto directly.
    let join = |sel: &Option<std::collections::HashSet<String>>| -> String {
        sel.as_ref().map(|s| {
            let mut v: Vec<&String> = s.iter().collect();
            v.sort();
            v.into_iter().cloned().collect::<Vec<_>>().join(",")
        }).unwrap_or_default()
    };
    ProtoRclRule {
        rcl_code: r.rcl_code.clone(),
        priority: r.priority,
        specificity: r.specificity,
        sel_l0: join(&r.sel_l0),
        sel_l1: join(&r.sel_l1),
        sel_l2: join(&r.sel_l2),
        sel_l3: join(&r.sel_l3),
        sel_l4: join(&r.sel_l4),
        sel_l5: join(&r.sel_l5),
        sel_brand: join(&r.sel_brand),
        sel_article: join(&r.sel_article),
    }
}
