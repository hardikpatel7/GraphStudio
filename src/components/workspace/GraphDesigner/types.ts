// Shared GraphSpec wire types. Mirrors `server/src/graph/spec/`
// — kept in one place so FormView, SchemaSketch, ExplorePane, and
// any future consumers parse the same shape. When the backend
// struct changes, this file is the single point of update.

export interface LevelSpec {
  column: string;
  key?: string | null;
  split?: string | null;
  unnest?: boolean | null;
}

/// `serde(flatten)` server-side means level sub-tables (e.g.
/// `[hierarchy.product.l0]`) appear at the same JSON depth as the
/// `source` field. The form discriminates by the known scalar
/// (`source`) vs. unknown keys (treated as levels).
export interface HierarchySpec {
  source: string;
  [levelId: string]: any;
}

/// AttachesAt comes back as one of:
///   - undefined / null
///   - bare string (single attach)
///   - array of strings (composite attach)
///   - object `{Single: "..."}` / `{Composite: [...]}` after some
///     serde round-trips (untagged enum quirk)
export type AttachesAt = string | string[] | { Single?: string; Composite?: string[] };

export interface SourceSpec {
  alias: string;
  table: string;
  attaches_at?: AttachesAt | null;
  filter?: string | null;
}

export interface RelationSide {
  alias: string;
  columns: string[];
  cardinality: "1" | "*";
}

export interface RelationSpec {
  from: RelationSide;
  to: RelationSide;
}

export interface MetricSpec {
  column?: string | null;
  rollup: string;
  expr?: string | null;
}

export interface GraphSpec {
  id: string;
  display_name: string;
  sources: SourceSpec[];
  relation?: RelationSpec[];
  hierarchy: Record<string, HierarchySpec>;
  metrics?: Record<string, Record<string, MetricSpec>>;
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Normalize `attaches_at` to a flat `string[]`. Empty array = no
/// attach (= bridge source).
export function normalizeAttachKinds(a: SourceSpec["attaches_at"]): string[] {
  if (a == null) return [];
  if (typeof a === "string") return [a];
  if (Array.isArray(a)) return a;
  if (typeof a === "object") {
    if (typeof a.Single === "string") return [a.Single];
    if (Array.isArray(a.Composite)) return a.Composite;
  }
  return [];
}

/// (Level id, level spec) pairs for a HierarchySpec.
export function levelsOf(h: HierarchySpec): { id: string; level: LevelSpec }[] {
  return Object.entries(h)
    .filter(([k]) => k !== "source")
    .map(([id, value]) => ({
      id,
      level: (value as LevelSpec) ?? { column: "" },
    }));
}

/// Convert flat string[] back to the wire-friendly `attaches_at`
/// shape: undefined (bridge) / single string / array. Server's
/// untagged AttachesAt deserializes any of these.
export function attachKindsToWire(kinds: string[]): SourceSpec["attaches_at"] {
  if (kinds.length === 0) return undefined;
  if (kinds.length === 1) return kinds[0];
  return kinds;
}
