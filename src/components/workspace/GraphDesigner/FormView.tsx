import { useCallback, useEffect, useMemo, useState } from "react";
import { Loader2, AlertCircle } from "lucide-react";
import type { GraphSpec, HierarchySpec, MetricSpec } from "./types";
import { Tree } from "./Tree";
import { SourcesInspector } from "./SourcesInspector";
import { HierarchyInspector } from "./HierarchyInspector";
import { LevelInspector } from "./LevelInspector";
import { RelationInspector } from "./RelationInspector";
import { MetricInspector } from "./MetricInspector";

// ─── Wire shapes from /parse and /serialize ──────────────────────────────

interface ParseResponse {
  ok: boolean;
  spec?: GraphSpec;
  error?: string;
}

interface SerializeResponse {
  ok: boolean;
  toml_text?: string;
  error?: string;
}

/// Selected item in the tree. `type` discriminates which inspector
/// to render in the center pane. Used as a union so future inspector
/// types only need to extend the discriminator.
export type Selection =
  | { type: "source"; alias: string }
  | { type: "hierarchy"; name: string }
  | { type: "level"; hierarchy: string; levelId: string }
  | { type: "relation"; index: number }
  | { type: "metric"; sourceAlias: string; name: string }
  | null;

interface Props {
  /// The current TOML text owned by GraphDesigner. FormView parses
  /// it on mount + on external changes (e.g., Reload).
  tomlText: string;
  /// Push a serialized TOML back up to GraphDesigner so its `toml`
  /// state stays in sync. Triggers schema-sketch re-render in the
  /// right pane (which parses parent's toml).
  onTomlChange: (newToml: string) => void;
}

/// Orchestrates the form-based GraphDesigner. Owns the parsed
/// `spec` mutable state; mutations re-serialize via
/// `POST /api/graphs/serialize` and push back up so the parent's
/// `toml` state (and downstream consumers like SchemaSketch) sees
/// the change immediately.
///
/// The serialize round-trip is server-side because:
///   1. `toml::to_string` lives on the Rust struct (single source
///      of truth for ordering, optional-field-skip rules, etc.)
///   2. Frontend doesn't need a TOML library
///   3. Sub-100ms round-trip on localhost — fine for edit-time
// Tree pane width bounds. Defaults wider than the original 240 so
// the longer Relations labels (`from.alias → to.alias`) and full
// metric names fit by default. User can drag-resize from here.
const TREE_MIN_PX = 180;
const TREE_MAX_PX = 560;
const TREE_DEFAULT_PX = 300;

export function FormView({ tomlText, onTomlChange }: Props) {
  const [spec, setSpec] = useState<GraphSpec | null>(null);
  const [parseError, setParseError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [serializeError, setSerializeError] = useState<string | null>(null);
  const [selection, setSelection] = useState<Selection>(null);

  // Tree-pane width — persisted per-browser so the user's preferred
  // tree width survives reloads. Bounded so it can't grow off-screen
  // or collapse past the point where row labels are unreadable.
  const [treeWidth, setTreeWidth] = useState<number>(() => {
    if (typeof window === "undefined") return TREE_DEFAULT_PX;
    const saved = parseInt(
      window.localStorage.getItem("ss.graph.tree.width") || "",
      10,
    );
    if (Number.isFinite(saved) && saved >= TREE_MIN_PX && saved <= TREE_MAX_PX) {
      return saved;
    }
    return TREE_DEFAULT_PX;
  });
  useEffect(() => {
    window.localStorage.setItem("ss.graph.tree.width", String(treeWidth));
  }, [treeWidth]);

  // Splitter drag — window-level listeners so the drag continues when
  // the cursor leaves the 4px handle. End on mouseup. Body cursor +
  // userSelect are stashed/restored to prevent text-selection during
  // the drag.
  const startResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = treeWidth;
    const onMove = (ev: MouseEvent) => {
      const next = Math.min(
        TREE_MAX_PX,
        Math.max(TREE_MIN_PX, startWidth + (ev.clientX - startX)),
      );
      setTreeWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  };

  // DuckDB relations (tables + views) for the source table combobox.
  // Fetched once when the form view mounts — the list doesn't
  // change while the user is editing a single graph spec.
  const [relations, setRelations] = useState<{ value: string; hint?: string }[]>([]);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const r = await fetch("/api/duckdb/relations");
        if (!r.ok) return;
        const data: { relations: { name: string; kind: string }[] } = await r.json();
        if (cancelled) return;
        setRelations(
          data.relations.map((row) => ({ value: row.name, hint: row.kind })),
        );
      } catch {
        // Silent — combobox falls back to free-text mode when the
        // option list is empty.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Re-parse whenever the parent's TOML changes from an external
  // source (load / reload / TOML-tab edit). We don't re-parse when
  // FormView itself wrote the toml back (we already have the spec
  // locally) — guarded by a `lastWrittenToml` ref.
  const [lastWrittenToml, setLastWrittenToml] = useState<string | null>(null);
  const reparseNeeded = tomlText !== lastWrittenToml;

  useEffect(() => {
    if (!reparseNeeded) return;
    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const r = await fetch("/api/graphs/parse", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ toml_text: tomlText }),
        });
        const data: ParseResponse = await r.json();
        if (cancelled) return;
        if (data.ok && data.spec) {
          setSpec(data.spec);
          setParseError(null);
        } else {
          // Parse failure: clear spec; the form pane will surface a
          // hint that switching to TOML mode is required to fix.
          setSpec(null);
          setParseError(data.error ?? "parse failed");
        }
      } catch (e: any) {
        if (cancelled) return;
        setSpec(null);
        setParseError(e?.message ?? String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tomlText, reparseNeeded]);

  /// Serialize the mutated spec back to TOML and push to parent.
  /// Called by inspectors after every edit + on tree add/delete.
  /// `setLastWrittenToml` prevents the round-trip from re-parsing
  /// our own write.
  const writeBack = useCallback(
    async (next: GraphSpec) => {
      setSpec(next); // local optimistic update
      setSerializeError(null);
      try {
        const r = await fetch("/api/graphs/serialize", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ spec: next }),
        });
        const data: SerializeResponse = await r.json();
        if (!data.ok || !data.toml_text) {
          setSerializeError(data.error ?? "serialize failed");
          return;
        }
        setLastWrittenToml(data.toml_text);
        onTomlChange(data.toml_text);
      } catch (e: any) {
        setSerializeError(e?.message ?? String(e));
      }
    },
    [onTomlChange],
  );

  // Existing source aliases — fed into inspectors for uniqueness
  // checks. Recomputed on every spec change; cheap (handful of items).
  const sourceAliases = useMemo(
    () => (spec?.sources ?? []).map((s) => s.alias),
    [spec],
  );

  // ── Render ──────────────────────────────────────────────────────────

  if (parseError) {
    return (
      <div className="p-3">
        <div className="rounded border border-amber-900/60 bg-amber-950/20 p-2.5 text-[11px] text-amber-300">
          <div className="flex items-start gap-1.5">
            <AlertCircle size={12} className="mt-0.5 shrink-0" />
            <div>
              <div className="font-medium mb-1">TOML doesn't parse — form edit unavailable</div>
              <pre className="font-mono whitespace-pre-wrap break-words text-amber-200/80">
                {parseError}
              </pre>
              <div className="mt-2 text-amber-300/70">
                Switch to the TOML tab to fix the syntax, then return to Form mode.
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  if (loading && !spec) {
    return (
      <div className="flex items-center gap-2 text-xs text-gray-500 px-3 py-3">
        <Loader2 size={12} className="animate-spin" />
        Parsing TOML…
      </div>
    );
  }

  if (!spec) return null;

  // 2-column resizable split: tree | inspector. The right pane
  // (Status / Schema / Explore) lives outside this component on
  // GraphDesigner, so we only render the left two columns here.
  //
  // `gridTemplateColumns` is inline so the drag handle can update
  // it without going through Tailwind. The handle column is a
  // narrow strip with cursor:col-resize; window-level listeners
  // (see startResize) keep the drag alive when the cursor leaves
  // the strip.
  return (
    <div
      className="h-full grid overflow-hidden"
      style={{ gridTemplateColumns: `${treeWidth}px 4px 1fr` }}
    >
      <Tree
        spec={spec}
        selection={selection}
        onSelect={setSelection}
        onSpecChange={writeBack}
      />
      <div
        onMouseDown={startResize}
        onDoubleClick={() => setTreeWidth(TREE_DEFAULT_PX)}
        title="Drag to resize · double-click to reset"
        className="cursor-col-resize bg-transparent hover:bg-blue-500/40 active:bg-blue-500/60 transition-colors border-l border-r border-gray-800"
      />
      <div className="overflow-auto bg-gray-950">
        {serializeError && (
          <div className="m-3 rounded border border-red-900/60 bg-red-950/30 p-2.5 text-[11px] text-red-300">
            serialize failed: <span className="font-mono">{serializeError}</span>
          </div>
        )}
        {renderInspector(spec, selection, sourceAliases, relations, writeBack, setSelection)}
      </div>
    </div>
  );
}

function renderInspector(
  spec: GraphSpec,
  selection: Selection,
  sourceAliases: string[],
  relations: { value: string; hint?: string }[],
  onSpecChange: (next: GraphSpec) => void,
  setSelection: (s: Selection) => void,
): React.ReactNode {
  if (!selection) {
    return (
      <div className="p-6 text-[11px] text-gray-500">
        Select an item from the tree on the left to edit it. Add new
        elements via the <span className="text-gray-300">+</span> buttons in the tree.
      </div>
    );
  }
  switch (selection.type) {
    case "source": {
      const idx = spec.sources.findIndex((s) => s.alias === selection.alias);
      if (idx < 0) return <NotFound kind="source" name={selection.alias} />;
      return (
        <SourcesInspector
          source={spec.sources[idx]}
          allAliases={sourceAliases}
          knownKinds={knownKindsOf(spec)}
          duckdbRelations={relations}
          onChange={(updated) => {
            const next = { ...spec, sources: [...spec.sources] };
            next.sources[idx] = updated;
            onSpecChange(next);
          }}
        />
      );
    }
    case "hierarchy": {
      const h = spec.hierarchy?.[selection.name];
      if (!h) return <NotFound kind="hierarchy" name={selection.name} />;
      const siblingNames = Object.keys(spec.hierarchy ?? {});
      return (
        <HierarchyInspector
          name={selection.name}
          hierarchy={h}
          siblingNames={siblingNames}
          sourceAliases={sourceAliases}
          onRename={(prev, next) => {
            onSpecChange(renameHierarchy(spec, prev, next));
            setSelection({ type: "hierarchy", name: next });
          }}
          onChange={(updated) => {
            const nextH = { ...(spec.hierarchy ?? {}), [selection.name]: updated };
            onSpecChange({ ...spec, hierarchy: nextH });
          }}
          onSelectLevel={(levelId) =>
            setSelection({ type: "level", hierarchy: selection.name, levelId })
          }
        />
      );
    }
    case "level": {
      const h = spec.hierarchy?.[selection.hierarchy];
      const lvl = h?.[selection.levelId];
      if (!h || !lvl) return <NotFound kind="level" name={`${selection.hierarchy}.${selection.levelId}`} />;
      const siblingIds = Object.keys(h).filter((k) => k !== "source");
      return (
        <LevelInspector
          hierarchy={selection.hierarchy}
          levelId={selection.levelId}
          level={lvl}
          siblingIds={siblingIds}
          onRename={(prev, next) => {
            onSpecChange(renameLevel(spec, selection.hierarchy, prev, next));
            setSelection({ type: "level", hierarchy: selection.hierarchy, levelId: next });
          }}
          onChange={(updated) => {
            const nextH = { ...h, [selection.levelId]: updated };
            onSpecChange({
              ...spec,
              hierarchy: { ...(spec.hierarchy ?? {}), [selection.hierarchy]: nextH },
            });
          }}
        />
      );
    }
    case "relation": {
      const rel = spec.relation?.[selection.index];
      if (!rel) return <NotFound kind="relation" name={`#${selection.index}`} />;
      return (
        <RelationInspector
          index={selection.index}
          relation={rel}
          sourceAliases={sourceAliases}
          onChange={(updated) => {
            const next = [...(spec.relation ?? [])];
            next[selection.index] = updated;
            onSpecChange({ ...spec, relation: next });
          }}
          onDelete={() => {
            if (!window.confirm(`Delete relation #${selection.index}?`)) return;
            const next = (spec.relation ?? []).filter((_, i) => i !== selection.index);
            onSpecChange({ ...spec, relation: next });
            setSelection(null);
          }}
        />
      );
    }
    case "metric": {
      const metricsForSrc = spec.metrics?.[selection.sourceAlias];
      const m = metricsForSrc?.[selection.name];
      if (!m) return <NotFound kind="metric" name={`${selection.sourceAlias}.${selection.name}`} />;
      const siblingNames = Object.keys(metricsForSrc ?? {});
      return (
        <MetricInspector
          sourceAlias={selection.sourceAlias}
          metricName={selection.name}
          metric={m}
          siblingNames={siblingNames}
          sourceAliases={sourceAliases}
          onRename={(prev, next) => {
            onSpecChange(renameMetric(spec, selection.sourceAlias, prev, next));
            setSelection({ type: "metric", sourceAlias: selection.sourceAlias, name: next });
          }}
          onMoveSource={(from, to, mName) => {
            onSpecChange(moveMetric(spec, from, to, mName));
            setSelection({ type: "metric", sourceAlias: to, name: mName });
          }}
          onChange={(updated) => {
            const nextForSrc = { ...(metricsForSrc ?? {}), [selection.name]: updated };
            onSpecChange({
              ...spec,
              metrics: { ...(spec.metrics ?? {}), [selection.sourceAlias]: nextForSrc },
            });
          }}
          onDelete={() => {
            if (!window.confirm(`Delete metric "${selection.sourceAlias}.${selection.name}"?`)) return;
            const nextForSrc: Record<string, MetricSpec> = { ...(metricsForSrc ?? {}) };
            delete nextForSrc[selection.name];
            const nextMetrics: GraphSpec["metrics"] = { ...(spec.metrics ?? {}) };
            // Drop the source bucket entirely when it goes empty so
            // serialized TOML doesn't leave a `[metrics.<alias>]` table
            // header pointing at nothing.
            if (Object.keys(nextForSrc).length === 0) {
              delete nextMetrics[selection.sourceAlias];
            } else {
              nextMetrics[selection.sourceAlias] = nextForSrc;
            }
            onSpecChange({ ...spec, metrics: nextMetrics });
            setSelection(null);
          }}
        />
      );
    }
  }
}

function NotFound({ kind, name }: { kind: string; name: string }) {
  return (
    <div className="p-6 text-[11px] text-gray-500">
      {kind} <span className="font-mono">{name}</span> not found (deleted?).
    </div>
  );
}

/// Preserve insertion order when renaming a hierarchy map key. JS
/// object spread re-keys but re-uses old insertion order — so we
/// rebuild the object slot-by-slot, substituting the new name where
/// the old one was.
function renameHierarchy(spec: GraphSpec, prev: string, next: string): GraphSpec {
  const out: Record<string, HierarchySpec> = {};
  for (const [k, v] of Object.entries(spec.hierarchy ?? {})) {
    out[k === prev ? next : k] = v;
  }
  return { ...spec, hierarchy: out };
}

function renameLevel(
  spec: GraphSpec,
  hierarchyName: string,
  prev: string,
  next: string,
): GraphSpec {
  const h = spec.hierarchy?.[hierarchyName];
  if (!h) return spec;
  // Rebuild keeping insertion order: source first (by convention),
  // then the existing level keys with `prev` substituted to `next`.
  const rebuilt: HierarchySpec = { source: h.source };
  for (const [k, v] of Object.entries(h)) {
    if (k === "source") continue;
    rebuilt[k === prev ? next : k] = v;
  }
  return {
    ...spec,
    hierarchy: { ...(spec.hierarchy ?? {}), [hierarchyName]: rebuilt },
  };
}

function renameMetric(
  spec: GraphSpec,
  sourceAlias: string,
  prev: string,
  next: string,
): GraphSpec {
  const forSrc = spec.metrics?.[sourceAlias];
  if (!forSrc) return spec;
  const rebuilt: Record<string, MetricSpec> = {};
  for (const [k, v] of Object.entries(forSrc)) {
    rebuilt[k === prev ? next : k] = v;
  }
  return {
    ...spec,
    metrics: { ...(spec.metrics ?? {}), [sourceAlias]: rebuilt },
  };
}

/// Relocate a metric from one source bucket to another. Source
/// buckets are themselves map entries — collisions are caller-checked
/// (here, we let the user overwrite if they reuse a name).
function moveMetric(
  spec: GraphSpec,
  fromAlias: string,
  toAlias: string,
  metricName: string,
): GraphSpec {
  const fromBucket = { ...(spec.metrics?.[fromAlias] ?? {}) };
  const metric = fromBucket[metricName];
  if (!metric) return spec;
  delete fromBucket[metricName];
  const toBucket = { ...(spec.metrics?.[toAlias] ?? {}), [metricName]: metric };
  const next: GraphSpec["metrics"] = { ...(spec.metrics ?? {}) };
  if (Object.keys(fromBucket).length === 0) {
    delete next[fromAlias];
  } else {
    next[fromAlias] = fromBucket;
  }
  next[toAlias] = toBucket;
  return { ...spec, metrics: next };
}

/// Every kind name declared by any hierarchy's level list. Used by
/// the SourcesInspector to populate the `attaches_at` dropdown so
/// the user can only pick existing kinds (not free-text).
function knownKindsOf(spec: GraphSpec): string[] {
  const kinds: string[] = [];
  for (const [, h] of Object.entries(spec.hierarchy ?? {})) {
    for (const k of Object.keys(h)) {
      if (k !== "source") kinds.push(k);
    }
  }
  return kinds;
}
