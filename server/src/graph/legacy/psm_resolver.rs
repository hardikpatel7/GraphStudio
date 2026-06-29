//! Per-product PSM resolver — on-the-fly variant.
//!
//! Mirrors V7's `read_psm_eligible_stores_duckdb` resolution chain but
//! without the per-product md5 precompute that dominates memory on
//! `develop/dev`. Instead of `HashMap<product_code, HashMap<rcl_code,
//! dim_md5>>` (~700 MB on Bealls), we parse each rule's
//! `rcl_dimension` JSON once at build time and build a per-rcl-code
//! index keyed by the dimension field tuple. Resolution per product
//! is then ~3 HashMap lookups (one per priority) totaling ~300 ns —
//! actually faster than the md5 path because we skip md5 computation
//! at lookup AND avoid the per-product hash table indirection.
//!
//! Rule dimensions arrive as JSON like `{"l0_name":"30-bls",
//! "l1_name":"3510-LADIES FOOTWEAR"}`. The keys define the schema
//! (which fields participate in matching for that rule). Multiple
//! schemas can coexist within a single rcl_code; we keep one bucket
//! per distinct schema to support that.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PsmExplain {
    pub rcl_code: String,
    pub rule_code: String,
}

/// One bucket per distinct rcl_dimension schema within a rcl_code.
/// `schema_fields` is sorted lexicographically so we can build keys
/// deterministically from a product's hierarchy.
#[derive(Debug, Default)]
pub struct RclBucket {
    pub schema_fields: Vec<String>,
    /// (values for `schema_fields` in the same order) → rule_code.
    pub by_tuple: HashMap<Vec<String>, String>,
}

#[derive(Debug, Default)]
pub struct RclIndex {
    pub buckets: Vec<RclBucket>,
}

#[derive(Debug, Default)]
pub struct PsmResolver {
    /// `(rcl_code, priority)` from `rcl_master where module_code=101`,
    /// pre-sorted by priority ASC.
    pub priorities: Vec<(String, i32)>,
    /// rcl_code → parsed rule index. Built once at graph construction
    /// from the raw `rcl_dimension::text` rows the duckdb reader
    /// returns.
    pub by_rcl: HashMap<String, RclIndex>,
}

impl PsmResolver {
    /// Build from priorities + raw rule rows `(rcl_code, rule_code,
    /// dim_json_text)`. Parses each dim_json once, groups rules by
    /// their schema (set of dim keys), and packs each schema into a
    /// HashMap keyed by the value tuple.
    pub fn build(
        priorities: Vec<(String, i32)>,
        raw_rules: Vec<(String, String, String)>,
    ) -> Self {
        let mut by_rcl: HashMap<String, RclIndex> = HashMap::new();
        for (rcl_code, rule_code, dim_json) in raw_rules {
            let parsed: HashMap<String, String> =
                match serde_json::from_str::<HashMap<String, serde_json::Value>>(&dim_json) {
                    Ok(m) => m
                        .into_iter()
                        .filter_map(|(k, v)| {
                            // Coerce the JSON value to a string. PG
                            // sends most fields as strings; numbers
                            // and bools we stringify.
                            let s = match v {
                                serde_json::Value::String(s) => Some(s),
                                serde_json::Value::Number(n) => Some(n.to_string()),
                                serde_json::Value::Bool(b) => Some(b.to_string()),
                                _ => None,
                            };
                            s.map(|s| (k, s))
                        })
                        .collect(),
                    Err(_) => continue, // bad row, skip
                };
            if parsed.is_empty() {
                continue;
            }
            let mut schema_fields: Vec<String> = parsed.keys().cloned().collect();
            schema_fields.sort();
            let values: Vec<String> = schema_fields
                .iter()
                .map(|k| parsed.get(k).cloned().unwrap_or_default())
                .collect();
            let idx = by_rcl.entry(rcl_code).or_default();
            // Find or create a bucket with matching schema.
            let bucket_pos = idx
                .buckets
                .iter()
                .position(|b| b.schema_fields == schema_fields);
            match bucket_pos {
                Some(i) => {
                    idx.buckets[i].by_tuple.insert(values, rule_code);
                }
                None => {
                    let mut by_tuple = HashMap::new();
                    by_tuple.insert(values, rule_code);
                    idx.buckets.push(RclBucket {
                        schema_fields,
                        by_tuple,
                    });
                }
            }
        }
        Self { priorities, by_rcl }
    }

    pub fn is_ready(&self) -> bool {
        !self.priorities.is_empty() && !self.by_rcl.is_empty()
    }

    /// Walk the priority chain. For each priority's rcl_code, walk
    /// each schema bucket: project the product's hierarchy into the
    /// bucket's schema, look up the value tuple in `by_tuple`. First
    /// match wins.
    ///
    /// `get_field` maps a dimension key (`"l0_name"`, `"brand"`, …)
    /// to the product's value as `String`. We allocate per call to
    /// dodge the lifetime gymnastics that would otherwise leak into
    /// every caller — the cost is sub-µs (3 priorities × ~5 fields
    /// × short String) and trivial relative to the HashMap lookups.
    pub fn explain(&self, mut get_field: impl FnMut(&str) -> String) -> Option<PsmExplain> {
        for (rcl_code, _priority) in &self.priorities {
            let Some(idx) = self.by_rcl.get(rcl_code) else {
                continue;
            };
            for bucket in &idx.buckets {
                let key: Vec<String> = bucket
                    .schema_fields
                    .iter()
                    .map(|f| get_field(f))
                    .collect();
                if let Some(rule_code) = bucket.by_tuple.get(&key) {
                    return Some(PsmExplain {
                        rcl_code: rcl_code.clone(),
                        rule_code: rule_code.clone(),
                    });
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<(String, String, String)> {
        vec![
            // rcl_code 65538 — schema {l0_name, l1_name}
            (
                "65538".into(),
                "rule-A".into(),
                r#"{"l0_name":"30-bls","l1_name":"3510-LADIES FOOTWEAR"}"#.into(),
            ),
            // rcl_code 16 — schema {l0_name}
            (
                "16".into(),
                "rule-B".into(),
                r#"{"l0_name":"30-bls"}"#.into(),
            ),
            // rcl_code 16 — different bucket: schema {l0_name, brand}
            (
                "16".into(),
                "rule-C".into(),
                r#"{"l0_name":"30-bls","brand":"VENUS"}"#.into(),
            ),
        ]
    }

    fn sample_product(k: &str) -> String {
        match k {
            "l0_name" => "30-bls".into(),
            "l1_name" => "3510-LADIES FOOTWEAR".into(),
            "brand" => "VENUS".into(),
            _ => String::new(),
        }
    }

    #[test]
    fn higher_priority_wins() {
        let r = PsmResolver::build(
            vec![
                ("65538".into(), 1),
                ("16".into(), 113),
            ],
            rules(),
        );
        let explain = r.explain(sample_product).expect("match");
        assert_eq!(explain.rcl_code, "65538");
        assert_eq!(explain.rule_code, "rule-A");
    }

    #[test]
    fn fallthrough_when_first_priority_misses() {
        let r = PsmResolver::build(
            vec![
                ("33".into(), 1),
                ("16".into(), 113),
            ],
            rules(),
        );
        let explain = r.explain(sample_product).expect("match");
        assert_eq!(explain.rcl_code, "16");
        assert!(matches!(explain.rule_code.as_str(), "rule-B" | "rule-C"));
    }

    #[test]
    fn no_match_returns_none() {
        let r = PsmResolver::build(
            vec![("65538".into(), 1)],
            vec![(
                "65538".into(),
                "rule-X".into(),
                r#"{"l0_name":"OTHER"}"#.into(),
            )],
        );
        let explain = r.explain(|k| match k {
            "l0_name" => "30-bls".into(),
            _ => String::new(),
        });
        assert!(explain.is_none());
    }
}
