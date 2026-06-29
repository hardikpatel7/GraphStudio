//! Tonic service for V4 article_selection reads. Phase 3 of misty-hinton.
//!
//! Wraps the in-memory `ArticleSelectionStore`. Two RPCs:
//!   - `GetList`        — filter + sort + paginate, returns rows + meta
//!   - `GetFilterValues`— distinct values of one column, after applying filters
//!
//! Both RPCs serialize each `ArticleSelectionRow` to JSON (`data_json`)
//! matching the V4 service shape — adding a column doesn't require a proto bump.
//!
//! Refresh / CreateDuckDb / SwapDuckDb (the V4 write-side RPCs) intentionally
//! aren't here — those flows live on smartstudio's HTTP/SSE pipeline path.

use std::cmp::Ordering;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::article_selection::{ArticleSelectionRow, ArticleSelectionStore};

pub mod proto {
    tonic::include_proto!("article_selection");
}

pub use proto::article_selection_service_server::ArticleSelectionServiceServer;

use proto::{
    ArticleSelectionFilter, ArticleSelectionListRequest, ArticleSelectionListResponse,
    ArticleSelectionRow as ProtoRow, FilterValuesRequest, FilterValuesResponse, Meta, SortField,
    article_selection_service_server::ArticleSelectionService,
};

#[derive(Clone)]
pub struct ArticleSelectionGrpcService {
    store: Arc<ArticleSelectionStore>,
}

impl ArticleSelectionGrpcService {
    pub fn new(store: Arc<ArticleSelectionStore>) -> Self {
        Self { store }
    }
}

#[tonic::async_trait]
impl ArticleSelectionService for ArticleSelectionGrpcService {
    async fn get_list(
        &self,
        req: Request<ArticleSelectionListRequest>,
    ) -> Result<Response<ArticleSelectionListResponse>, Status> {
        let req = req.into_inner();
        let snapshot = self.store.snapshot();
        let total_unfiltered = snapshot.len() as i64;

        // Filter.
        let mut filtered: Vec<&ArticleSelectionRow> = snapshot
            .iter()
            .filter(|row| matches_filters(row, &req.filters) && matches_search(row, &req.search))
            .collect();

        // Sort. Multiple sort keys are applied in order (last key is most specific).
        if !req.sort.is_empty() {
            apply_sort(&mut filtered, &req.sort);
        }

        let total = filtered.len() as i64;
        let limit = if req.limit > 0 { req.limit } else { total };
        let offset = req.offset.max(0);
        let start = offset.min(total) as usize;
        let end = ((offset + limit).min(total)) as usize;

        let rows: Vec<ProtoRow> = filtered[start..end]
            .iter()
            .map(|row| row_to_proto(row))
            .collect();

        let total_pages = if limit > 0 { (total + limit - 1) / limit } else { 1 };
        let page = if limit > 0 { offset / limit + 1 } else { 1 };

        tracing::debug!(
            total_unfiltered, total_filtered = total, returned = rows.len(),
            "[article_selection_grpc] GetList"
        );

        Ok(Response::new(ArticleSelectionListResponse {
            rows,
            total,
            meta: Some(Meta {
                limit, offset, total, page, total_pages,
            }),
        }))
    }

    async fn get_filter_values(
        &self,
        req: Request<FilterValuesRequest>,
    ) -> Result<Response<FilterValuesResponse>, Status> {
        let req = req.into_inner();
        let snapshot = self.store.snapshot();

        // Apply filters EXCEPT for the column we're computing distinct values
        // for (matches the V4 cross-filter semantics — choosing a value in
        // one column shouldn't hide other available values for that same column).
        let column = req.column.clone();
        let cross_filters: Vec<ArticleSelectionFilter> = req
            .filters
            .into_iter()
            .filter(|f| f.attribute_name != column)
            .collect();

        let mut seen = std::collections::BTreeSet::new();
        for row in snapshot.iter() {
            if !matches_filters(row, &cross_filters) {
                continue;
            }
            if let Some(v) = column_value(row, &column) {
                if !v.is_empty() {
                    seen.insert(v);
                }
            }
        }

        let data: Vec<String> = seen.into_iter().collect();
        let count = data.len() as i32;
        Ok(Response::new(FilterValuesResponse { data, count }))
    }
}

// ─── Filter helpers ──────────────────────────────────────────────────────────

fn matches_filters(row: &ArticleSelectionRow, filters: &[ArticleSelectionFilter]) -> bool {
    filters.iter().all(|f| matches_one_filter(row, f))
}

fn matches_one_filter(row: &ArticleSelectionRow, f: &ArticleSelectionFilter) -> bool {
    let val = match column_value(row, &f.attribute_name) {
        Some(v) => v,
        None => return false,
    };
    let op = if f.operator.is_empty() { "in" } else { f.operator.as_str() };
    match op {
        "in" => f.values.iter().any(|v| v == &val),
        "eq" => f.values.first().map(|v| v == &val).unwrap_or(false),
        "contains" => f.values.iter().any(|v| val.contains(v)),
        _ => false,
    }
}

fn matches_search(row: &ArticleSelectionRow, search: &str) -> bool {
    let q = search.trim();
    if q.is_empty() {
        return true;
    }
    let q_lower = q.to_lowercase();
    // Search a small set of obvious string columns. Matches V4 — full
    // free-text indexing is out of scope for Phase 3. ph_code is now i64
    // post Bucket-1 → format it for the substring check.
    let ph_code_str = row.ph_code.to_string();
    [
        ph_code_str.as_str(),
        row.article.as_str(),
        row.product_description.as_str(),
        row.style_color_description.as_str(),
        row.brand.as_str(),
    ]
    .iter()
    .any(|s| s.to_lowercase().contains(&q_lower))
}

fn apply_sort(rows: &mut Vec<&ArticleSelectionRow>, sort: &[SortField]) {
    rows.sort_by(|a, b| {
        for s in sort {
            let ord = compare_column(a, b, &s.column);
            let ord = if s.direction.eq_ignore_ascii_case("desc") { ord.reverse() } else { ord };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
}

fn compare_column(a: &ArticleSelectionRow, b: &ArticleSelectionRow, col: &str) -> Ordering {
    // Numeric columns sort numerically; everything else lexicographically.
    macro_rules! cmp_i64 { ($f:ident) => { a.$f.cmp(&b.$f) } }
    macro_rules! cmp_f64 { ($f:ident) => { a.$f.partial_cmp(&b.$f).unwrap_or(Ordering::Equal) } }
    match col {
        "oh" => cmp_i64!(oh),
        "oo" => cmp_i64!(oo),
        "it" => cmp_i64!(it),
        "reserve_quantity" => cmp_i64!(reserve_quantity),
        "allocated_units" => cmp_i64!(allocated_units),
        "net_available_inventory" => cmp_i64!(net_available_inventory),
        "lw_units" => cmp_i64!(lw_units),
        "lw_margin" => cmp_i64!(lw_margin),
        "lw_revenue" => cmp_i64!(lw_revenue),
        "min_stock" => cmp_i64!(min_stock),
        "max_stock" => cmp_i64!(max_stock),
        "wos" => cmp_i64!(wos),
        "min_woc" => cmp_i64!(min_woc),
        "max_woc" => cmp_i64!(max_woc),
        "price" => cmp_f64!(price),
        "discount" => cmp_f64!(discount),
        "in_stock_perc" => cmp_f64!(in_stock_perc),
        "aps" => cmp_f64!(aps),
        _ => column_value(a, col)
            .unwrap_or_default()
            .cmp(&column_value(b, col).unwrap_or_default()),
    }
}

/// Project one column's value to a string (for filter / search / sort).
/// Returns None if the column name is unknown. Optional columns flatten to
/// `""` here so filter/sort treat null and empty consistently.
fn column_value(row: &ArticleSelectionRow, col: &str) -> Option<String> {
    Some(match col {
        "ph_code" => row.ph_code.to_string(),
        "article" => row.article.clone(),
        "l0_name" => row.l0_name.clone(),
        "l1_name" => row.l1_name.clone(),
        "l2_name" => row.l2_name.clone(),
        "l3_name" => row.l3_name.clone(),
        "l4_name" => row.l4_name.clone(),
        "l5_name" => row.l5_name.clone(),
        "style_color_description" => row.style_color_description.clone(),
        "product_description" => row.product_description.clone(),
        "sizes" => row.sizes.clone(),
        "upc" => row.upc.clone(),
        "product_life_cycle" => row.product_life_cycle.clone(),
        "article_status_tag" => row.article_status_tag.clone(),
        "brand" => row.brand.clone(),
        "channel" => row.channel.clone(),
        "dcs" => row.dcs.clone().unwrap_or_default(),
        "store_groups" => row.store_groups.clone().unwrap_or_default(),
        "allocation_rules" => row.allocation_rules.clone().unwrap_or_default(),
        "oh_map" => row.oh_map.clone().unwrap_or_default(),
        "rq_map" => row.rq_map.clone().unwrap_or_default(),
        "au_map" => row.au_map.clone().unwrap_or_default(),
        "last_allocated" => row.last_allocated.clone().unwrap_or_default(),
        _ => return None,
    })
}

fn row_to_proto(row: &ArticleSelectionRow) -> ProtoRow {
    let json = serde_json::to_string(row).unwrap_or_default();
    ProtoRow { data_json: json }
}
