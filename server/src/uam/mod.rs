//! User Access Management (UAM) — per-user entitled set resolution.
//!
//! Source of truth: `global.user_access_hierarchy_mapping` in PG.
//! Each `(user_code, acl_code)` row carries a `filters: jsonb` column
//! whose schema matches our [`crate::cross_filter::FilterPayload.filters`]
//! exactly. Empty `filters` = unrestricted access for that pair.
//!
//! Phase A (this module): cold-load every row at boot, resolve each
//! non-empty filter set against the live `ArticleGraph` snapshot, and
//! park the result in an `ArcSwap<HashMap<(user_code, acl_code),
//! Arc<EntitledSet>>>`. Lookup is `O(1)` HashMap access.
//!
//! Phase B (separate): subscribe to PG NOTIFY for
//! `user_access_hierarchy_mapping_changed` +
//! `product_attributes_filter_changed`, recompute affected users
//! incrementally, and ArcSwap to publish. Until then, the cache is
//! warmed once at boot and on explicit refresh.

pub mod store;

pub use store::{UamStore, UamLookupKey};
