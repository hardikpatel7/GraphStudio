//! Tonic gRPC wrapper around `graph::legacy::ArticleGraph`.
//!
//! Reads the live snapshot from `AppState.article_graph` (an
//! `ArcSwapOption`); each RPC clones the inner Arc and traverses
//! lock-free. Returns `FAILED_PRECONDITION` when the graph hasn't
//! been built yet.
//!
//! The service surfaces three RPCs:
//!   - `MatchProduct` — product_code/article → ProductHierarchy.
//!   - `ResolveRcl` — per-product RCL trace via `article_graph::resolver`.
//!     Backs the SmartStudio "RCL Explorer" tab.
//!   - `AggregateAt` — pre-aggregated metrics at any node, O(1).

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::AppState;
use crate::graph::legacy::{
    ArticleGraph, MetricKind, NodeId, NodeKind as GraphNodeKind, StrId, explain_constraints,
    explain_dc_policy,
};

/// Generated proto bindings.
pub mod proto {
    tonic::include_proto!("article_graph");
}

use proto::{
    AggregateAtRequest, AggregateAtResponse, Aggregates, ConstraintRow as ProtoConstraintRow,
    ConstraintsExplain as ProtoConstraintsExplain, DcPolicy as ProtoDcPolicy,
    DcPolicyExplain as ProtoDcPolicyExplain, MatchProductRequest, MatchProductResponse,
    NodeKind as ProtoNodeKind, ProductHierarchy, PsmExplain as ProtoPsmExplain,
    ResolveRclRequest, ResolveRclResponse, RuleKind as ProtoRuleKind,
    article_graph_service_server::ArticleGraphService,
};

pub use proto::article_graph_service_server::ArticleGraphServiceServer;

#[derive(Clone)]
pub struct ArticleGraphGrpcService {
    state: Arc<AppState>,
}

impl ArticleGraphGrpcService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Snapshot the current graph or fail with FAILED_PRECONDITION.
    fn snapshot(&self) -> Result<Arc<ArticleGraph>, Status> {
        self.state.legacy_graph.load_full().ok_or_else(|| {
            Status::failed_precondition(
                "article_graph not built yet — run pipeline pl_build_article_graph first",
            )
        })
    }

    /// Build a `ProductHierarchy` proto from a graph node id. The id
    /// must be an Article node — its parents form the l5..l0 chain.
    /// Channel comes from the cross-index. `product_code` is filled in
    /// from the caller-provided value; if the caller passed `article`,
    /// we pick the first product_code child (or empty string).
    fn hierarchy_from_article(
        graph: &ArticleGraph,
        article_id: NodeId,
        product_code_override: Option<&str>,
    ) -> ProductHierarchy {
        let article_node = graph.node(article_id);
        let article = graph.get_str(article_node.name).to_string();

        // Walk parents to gather hierarchy levels.
        let mut chain: Vec<(GraphNodeKind, &str)> = Vec::new();
        let mut cur = article_node.parent;
        while !cur.is_none() {
            let n = graph.node(cur);
            if matches!(n.kind, GraphNodeKind::Root) {
                break;
            }
            chain.push((n.kind, graph.get_str(n.name)));
            cur = n.parent;
        }
        let level = |k: GraphNodeKind| -> String {
            chain
                .iter()
                .find(|(kk, _)| *kk == k)
                .map(|(_, n)| (*n).to_string())
                .unwrap_or_default()
        };

        // brand: O(1) via cross_indices.article_to_brand.
        let brand = graph
            .cross_indices
            .article_to_brand
            .get(&article_id)
            .map(|b| graph.get_str(*b).to_string())
            .unwrap_or_default();

        let channel = graph
            .cross_indices
            .article_to_channel
            .get(&article_id)
            .map(|c| graph.get_str(*c).to_string())
            .unwrap_or_default();

        let product_code = match product_code_override {
            Some(s) => s.to_string(),
            None => article_node
                .children
                .first()
                .map(|&c| graph.get_str(graph.node(c).name).to_string())
                .unwrap_or_default(),
        };

        ProductHierarchy {
            product_code,
            article,
            l0_name: level(GraphNodeKind::L0),
            l1_name: level(GraphNodeKind::L1),
            l2_name: level(GraphNodeKind::L2),
            l3_name: level(GraphNodeKind::L3),
            l4_name: level(GraphNodeKind::L4),
            l5_name: level(GraphNodeKind::L5),
            brand,
            channel,
        }
    }

    /// Resolve a product_code or article key to (article_node_id,
    /// product_code_override). `product_code` ↑ to its parent article;
    /// `article` looks up the article node directly and we pick a
    /// representative product_code child.
    fn resolve_key(
        graph: &ArticleGraph,
        product_code: Option<&str>,
        article: Option<&str>,
    ) -> Option<(NodeId, Option<String>)> {
        if let Some(pc) = product_code {
            // string_pool reverse lookup. Build is over so there's no
            // string_index — scan the by_kind index for ProductCode.
            let pc_id = find_str(graph, pc)?;
            let pc_node_id = graph.find(GraphNodeKind::ProductCode, pc_id)?;
            let pc_node = graph.node(pc_node_id);
            let article_id = pc_node.parent;
            return Some((article_id, Some(pc.to_string())));
        }
        if let Some(article) = article {
            let art_id = find_str(graph, article)?;
            let article_id = graph.find(GraphNodeKind::Article, art_id)?;
            return Some((article_id, None));
        }
        None
    }
}

/// O(strings) reverse lookup. The graph drops its `string_index` after
/// build (memory savings), so we scan `string_pool` for the value.
/// Used only by the gRPC request path — not on hot loops. For the
/// 272 K-string Bealls dataset this is ~150 µs in release.
fn find_str(graph: &ArticleGraph, needle: &str) -> Option<StrId> {
    graph
        .string_pool
        .iter()
        .position(|s| s.as_ref() == needle)
        .map(|i| StrId(i as u32))
}

#[tonic::async_trait]
impl ArticleGraphService for ArticleGraphGrpcService {
    async fn match_product(
        &self,
        req: Request<MatchProductRequest>,
    ) -> Result<Response<MatchProductResponse>, Status> {
        let req = req.into_inner();
        let graph = self.snapshot()?;
        let (pc, article) = match req.key {
            Some(proto::match_product_request::Key::ProductCode(pc)) => (Some(pc), None),
            Some(proto::match_product_request::Key::Article(art)) => (None, Some(art)),
            None => return Err(Status::invalid_argument("missing key (product_code or article)")),
        };
        let hierarchy = Self::resolve_key(&graph, pc.as_deref(), article.as_deref()).map(
            |(article_id, pc_override)| {
                Self::hierarchy_from_article(&graph, article_id, pc_override.as_deref())
            },
        );
        Ok(Response::new(MatchProductResponse { hierarchy }))
    }

    async fn resolve_rcl(
        &self,
        req: Request<ResolveRclRequest>,
    ) -> Result<Response<ResolveRclResponse>, Status> {
        let req = req.into_inner();
        let graph = self.snapshot()?;
        let (pc, article) = match req.key {
            Some(proto::resolve_rcl_request::Key::ProductCode(pc)) => (Some(pc), None),
            Some(proto::resolve_rcl_request::Key::Article(art)) => (None, Some(art)),
            None => return Err(Status::invalid_argument("missing key (product_code or article)")),
        };
        let Some((article_id, pc_override)) =
            Self::resolve_key(&graph, pc.as_deref(), article.as_deref())
        else {
            return Ok(Response::new(ResolveRclResponse {
                hierarchy: None,
                dc_policy: None,
                constraints: None,
                psm: None,
                ruleset_version: 0,
            }));
        };
        let hierarchy =
            Self::hierarchy_from_article(&graph, article_id, pc_override.as_deref());

        // Acquire the live RuleSet for resolution.
        let ruleset = {
            let guard = self.state.rcl_store.read().await;
            let store = guard
                .as_ref()
                .ok_or_else(|| {
                    Status::failed_precondition(
                        "RCL service not running — enable [rcl] in environment.toml",
                    )
                })?
                .clone();
            store.snapshot()
        };

        // Pick which kinds to resolve. Empty list = all three.
        let want_dc = req.kinds.is_empty()
            || req.kinds.contains(&(ProtoRuleKind::DcPolicy as i32));
        let want_constraints = req.kinds.is_empty()
            || req.kinds.contains(&(ProtoRuleKind::Constraints as i32));
        let want_psm =
            req.kinds.is_empty() || req.kinds.contains(&(ProtoRuleKind::Psm as i32));

        // Build the rcl::ProductHierarchy borrow from the proto we
        // just constructed (lifetime is tied to `hierarchy`).
        let rcl_input = rcl::ProductHierarchy {
            product_code: &hierarchy.product_code,
            l0_name: &hierarchy.l0_name,
            l1_name: &hierarchy.l1_name,
            l2_name: &hierarchy.l2_name,
            l3_name: &hierarchy.l3_name,
            l4_name: &hierarchy.l4_name,
            l5_name: &hierarchy.l5_name,
            brand: &hierarchy.brand,
        };

        let dc_policy = if want_dc {
            explain_dc_policy(&ruleset, &rcl_input).map(|e| ProtoDcPolicyExplain {
                rcl_code: e.rcl_code,
                rule_code: e.rule_code,
                policy: Some(ProtoDcPolicy {
                    default_store_groups: e.policy.default_store_groups.clone(),
                    default_product_profile: e.policy.default_product_profile.clone(),
                    dc_store_rule: e.policy.dc_store_rule.clone(),
                }),
            })
        } else {
            None
        };

        let constraints = if want_constraints {
            explain_constraints(&ruleset, &rcl_input).map(|e| ProtoConstraintsExplain {
                rcl_code: e.rcl_code,
                rule_code: e.rule_code,
                rows: e
                    .rows
                    .iter()
                    .map(|c| ProtoConstraintRow {
                        psa_code: c.psa_code.clone(),
                        aps: c.aps,
                        wos: c.wos,
                        min_stock: c.min_stock,
                        max_stock: c.max_stock,
                    })
                    .collect(),
            })
        } else {
            None
        };

        // On-the-fly PSM resolution. The resolver walks each priority's
        // bucket index and projects the product's hierarchy fields
        // into the bucket's schema for the lookup. No md5 round-trip
        // and no per-product hash table on the graph.
        let psm = if want_psm {
            graph
                .psm
                .explain(|field: &str| -> String {
                    match field {
                        "l0_name" => hierarchy.l0_name.clone(),
                        "l1_name" => hierarchy.l1_name.clone(),
                        "l2_name" => hierarchy.l2_name.clone(),
                        "l3_name" => hierarchy.l3_name.clone(),
                        "l4_name" => hierarchy.l4_name.clone(),
                        "l5_name" => hierarchy.l5_name.clone(),
                        "brand" => hierarchy.brand.clone(),
                        "channel" => hierarchy.channel.clone(),
                        "article" => hierarchy.article.clone(),
                        "product_code" => hierarchy.product_code.clone(),
                        _ => String::new(),
                    }
                })
                .map(|e| ProtoPsmExplain {
                    rcl_code: e.rcl_code,
                    rule_code: e.rule_code,
                })
        } else {
            None
        };

        Ok(Response::new(ResolveRclResponse {
            hierarchy: Some(hierarchy),
            dc_policy,
            constraints,
            psm,
            ruleset_version: ruleset.version,
        }))
    }

    async fn aggregate_at(
        &self,
        req: Request<AggregateAtRequest>,
    ) -> Result<Response<AggregateAtResponse>, Status> {
        let req = req.into_inner();
        let graph = self.snapshot()?;
        let kind = match ProtoNodeKind::try_from(req.kind) {
            Ok(k) => k,
            Err(_) => return Err(Status::invalid_argument("invalid node kind")),
        };
        let graph_kind = match kind {
            ProtoNodeKind::L0 => GraphNodeKind::L0,
            ProtoNodeKind::L1 => GraphNodeKind::L1,
            ProtoNodeKind::L2 => GraphNodeKind::L2,
            ProtoNodeKind::L3 => GraphNodeKind::L3,
            ProtoNodeKind::L4 => GraphNodeKind::L4,
            ProtoNodeKind::L5 => GraphNodeKind::L5,
            ProtoNodeKind::Article => GraphNodeKind::Article,
            ProtoNodeKind::ProductCode => GraphNodeKind::ProductCode,
            ProtoNodeKind::Channel => GraphNodeKind::Channel,
            ProtoNodeKind::StoreCode => GraphNodeKind::StoreCode,
            ProtoNodeKind::Unspecified => {
                return Err(Status::invalid_argument("kind must be specified"));
            }
        };
        let aggregates = find_str(&graph, &req.name)
            .and_then(|name_id| graph.find(graph_kind, name_id))
            .map(|node_id| {
                let m = &graph.node(node_id).metrics;
                Aggregates {
                    oh: m[MetricKind::Oh.idx()] as i64,
                    oo: m[MetricKind::Oo.idx()] as i64,
                    it: m[MetricKind::It.idx()] as i64,
                    reserve_quantity: m[MetricKind::ReserveQuantity.idx()] as i64,
                    allocated_units: m[MetricKind::AllocatedUnits.idx()] as i64,
                    lw_units: m[MetricKind::LwUnits.idx()] as i64,
                    lw_revenue: m[MetricKind::LwRevenue.idx()] as i64,
                    lw_margin: m[MetricKind::LwMargin.idx()] as i64,
                }
            });
        Ok(Response::new(AggregateAtResponse {
            aggregates,
            graph_version: graph.graph_version,
        }))
    }
}
