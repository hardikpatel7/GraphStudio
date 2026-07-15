//! Inert scaffold stub of the private `rcl` crate (Retail Constraint Language).
//!
//! This is NOT the real crate. It exists so the public GraphStudio repo
//! compiles without SSH access to `rust-shared-utils` AND without carrying any
//! of `rcl`'s proprietary rule-resolution algorithm. Only the *shapes* (types,
//! fields, signatures) the server references are reproduced here; every body is
//! empty/permissive:
//!
//!   * `resolve_dc_policy` / `resolve_constraints` / `resolve_psm` return empty.
//!   * `RclRule::matches` always returns `false` (no selection logic).
//!   * `RuleStore` seeds a single empty `RuleSet` and never changes it.
//!   * the PG change sources are no-ops.
//!
//! Net effect: RCL resolves to nothing (permissive/empty). No IP crosses over.
//! See CLAUDE.md ("Vendored scaffold stubs").

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// Borrowed product-hierarchy input. Constructed by the server; carried into
/// the resolvers (which ignore it here).
#[derive(Debug, Clone, Copy)]
pub struct ProductHierarchy<'a> {
    pub product_code: &'a str,
    pub l0_name: &'a str,
    pub l1_name: &'a str,
    pub l2_name: &'a str,
    pub l3_name: &'a str,
    pub l4_name: &'a str,
    pub l5_name: &'a str,
    pub brand: &'a str,
}

/// A single rule. The server reads every field and calls `matches`; the stub's
/// `matches` never matches, so no rule is ever selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RclRule {
    pub rcl_code: String,
    pub priority: i32,
    pub specificity: i32,
    pub sel_l0: Option<HashSet<String>>,
    pub sel_l1: Option<HashSet<String>>,
    pub sel_l2: Option<HashSet<String>>,
    pub sel_l3: Option<HashSet<String>>,
    pub sel_l4: Option<HashSet<String>>,
    pub sel_l5: Option<HashSet<String>>,
    pub sel_brand: Option<HashSet<String>>,
    pub sel_article: Option<HashSet<String>>,
}

impl RclRule {
    /// Inert: never matches. The real selection/specificity logic is the
    /// proprietary part and is deliberately absent.
    #[allow(clippy::too_many_arguments)]
    pub fn matches(
        &self,
        _l0: &str,
        _l1: &str,
        _l2: &str,
        _l3: &str,
        _l4: &str,
        _l5: &str,
        _brand: &str,
    ) -> bool {
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcPolicy {
    pub default_store_groups: Vec<String>,
    pub default_product_profile: String,
    pub dc_store_rule: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintRow {
    pub psa_code: String,
    pub aps: f64,
    pub wos: f64,
    pub min_stock: f64,
    pub max_stock: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRuleDim {
    pub rule_code: String,
    pub dimensions: HashMap<String, String>,
}

/// The full rule snapshot. The stub only ever produces an empty one, so every
/// map lookup the server makes returns `None`, yielding empty resolution output.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleSet {
    pub rules: Vec<RclRule>,
    pub policies: HashMap<(String, String), DcPolicy>,
    pub policy_rules: HashMap<String, Vec<PolicyRuleDim>>,
    pub constraints: HashMap<(String, String), Vec<ConstraintRow>>,
    pub constraint_rules: HashMap<String, Vec<PolicyRuleDim>>,
    pub version: u64,
    pub bytes_hash: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PsmInput<'a> {
    pub hierarchy: ProductHierarchy<'a>,
    pub store_code: &'a str,
    pub psa_code: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsmResolved {
    pub product_code: String,
    pub store_code: String,
    pub rcl_code: String,
    pub is_active: bool,
}

/// Inert: no policy resolves. Empty in → empty out.
pub fn resolve_dc_policy<'a, 'r>(
    _rules: &'r RuleSet,
    _products: &[ProductHierarchy<'a>],
) -> HashMap<String, &'r DcPolicy> {
    HashMap::new()
}

/// Inert: no constraints resolve.
pub fn resolve_constraints<'a, 'r>(
    _rules: &'r RuleSet,
    _products: &[ProductHierarchy<'a>],
) -> HashMap<String, &'r [ConstraintRow]> {
    HashMap::new()
}

/// Inert: no product-store mappings resolve.
pub fn resolve_psm<'a>(_rules: &RuleSet, _inputs: &[PsmInput<'a>]) -> Vec<PsmResolved> {
    Vec::new()
}

/// Marker trait for a rule-change source. The stub's sources emit nothing.
pub trait ChangeSource: Send + 'static {}

/// No-op LISTEN/NOTIFY change source.
pub struct PgListenSource;
impl PgListenSource {
    pub fn new(_dsn: impl Into<String>) -> Self {
        PgListenSource
    }
}
impl ChangeSource for PgListenSource {}

/// No-op polling change source.
pub struct PgPollSource;
impl PgPollSource {
    pub fn new(_dsn: impl Into<String>, _interval: Duration) -> Self {
        PgPollSource
    }
}
impl ChangeSource for PgPollSource {}

/// The SQL used by the real store to load rules. The server only constructs
/// this via `Default`; fields are never read here.
#[derive(Debug, Clone, Default)]
pub struct StoreQueries {
    pub rcl_master_sql: String,
    pub dc_policy_sql: String,
    pub constraints_sql: String,
    pub policy_rules_sql: String,
    pub constraint_rules_sql: String,
}

/// Holds a single, never-changing empty `RuleSet`. The `watch::Sender` is kept
/// alive (inside the struct) so `subscribe()` receivers and the pipeline
/// scheduler's `rx.changed().await` don't observe a closed channel.
#[derive(Clone)]
pub struct RuleStore {
    // Retained solely to keep the watch channel open for the process lifetime.
    #[allow(dead_code)]
    tx: Arc<watch::Sender<Arc<RuleSet>>>,
    rx: watch::Receiver<Arc<RuleSet>>,
}

impl RuleStore {
    /// Inert: ignores the DSN / source / queries, seeds one empty `RuleSet`,
    /// and never errors.
    pub async fn start(
        _dsn: String,
        _source: Box<dyn ChangeSource>,
        _queries: StoreQueries,
    ) -> anyhow::Result<Self> {
        let (tx, rx) = watch::channel(Arc::new(RuleSet::default()));
        Ok(Self {
            tx: Arc::new(tx),
            rx,
        })
    }

    pub fn snapshot(&self) -> Arc<RuleSet> {
        self.rx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<RuleSet>> {
        self.rx.clone()
    }

    pub fn version(&self) -> u64 {
        0
    }
}
