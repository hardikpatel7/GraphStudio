import { useEffect, useMemo, useState } from "react";
import { Loader2, AlertCircle } from "lucide-react";

// ─── Wire shapes (mirror server/src/graph/spec) ─────────────────────────

interface LevelSpec {
  column: string;
  key?: string | null;
  split?: string | null;
  unnest?: boolean | null;
}

interface HierarchySpec {
  source: string;
  // serde flattens `[hierarchy.<name>.<level_id>]` sub-tables into
  // the same object as `source`, so non-`source` keys are level
  // entries (an IndexMap server-side). We normalize via filtering
  // out the known top-level field below.
  [k: string]: any;
}

interface AttachesAt {
  Single?: string;
  Composite?: string[];
}

interface SourceSpec {
  alias: string;
  table: string;
  attaches_at?: AttachesAt | string | string[] | null;
  filter?: string | null;
}

interface RelationSide {
  alias: string;
  columns: string[];
  cardinality: "1" | "*";
}

interface RelationSpec {
  from: RelationSide;
  to: RelationSide;
}

interface GraphSpec {
  id: string;
  display_name: string;
  sources: SourceSpec[];
  relation?: RelationSpec[];
  hierarchy: Record<string, HierarchySpec>;
  metrics?: Record<string, Record<string, any>>;
}

interface ParseResponse {
  ok: boolean;
  spec?: GraphSpec;
  error?: string;
}

// ─── Layout primitives ─────────────────────────────────────────────────────

const HIERARCHY_BOX_WIDTH = 160;
const LEVEL_HEIGHT = 24;
const HIERARCHY_GAP = 56;
const TOP_PAD = 32;
const LEFT_PAD = 24;
const LABEL_HEIGHT = 22;

interface LaidOutLevel {
  id: string;
  column: string;
  y: number;
}

interface LaidOutHierarchy {
  name: string;
  source: string;
  x: number;
  width: number;
  height: number;
  levels: LaidOutLevel[];
}

/// Normalize the attach value. Spec's `AttachesAt` is an untagged
/// serde enum so the JSON either comes back as a bare string, an
/// array, or — depending on the serializer round-trip — as an object
/// with `Single` / `Composite` discriminators. Handle all three.
function normalizeAttachKinds(a: SourceSpec["attaches_at"]): string[] {
  if (a == null) return [];
  if (typeof a === "string") return [a];
  if (Array.isArray(a)) return a;
  if (typeof a === "object") {
    if (typeof a.Single === "string") return [a.Single];
    if (Array.isArray(a.Composite)) return a.Composite;
  }
  return [];
}

/// Extract the (level_id, LevelSpec) pairs from a HierarchySpec.
/// `source` is the only known non-level field; everything else is a
/// nested sub-table.
function levelsOf(h: HierarchySpec): Array<{ id: string; level: LevelSpec }> {
  return Object.entries(h)
    .filter(([k]) => k !== "source")
    .map(([id, value]) => ({
      id,
      level: (value as LevelSpec) ?? { column: "" },
    }));
}

/// Lay hierarchies out horizontally, levels stacked vertically.
/// First hierarchy is the primary (Decision 31); render it first.
function layoutHierarchies(spec: GraphSpec): {
  hierarchies: LaidOutHierarchy[];
  width: number;
  height: number;
} {
  const entries = Object.entries(spec.hierarchy ?? {});
  let maxLevels = 0;
  const out: LaidOutHierarchy[] = entries.map(([name, h], i) => {
    const levels = levelsOf(h);
    if (levels.length > maxLevels) maxLevels = levels.length;
    const x = LEFT_PAD + i * (HIERARCHY_BOX_WIDTH + HIERARCHY_GAP);
    const y0 = TOP_PAD + LABEL_HEIGHT;
    const laid: LaidOutLevel[] = levels.map((l, li) => ({
      id: l.id,
      column: l.level.column,
      y: y0 + li * LEVEL_HEIGHT,
    }));
    return {
      name,
      source: h.source,
      x,
      width: HIERARCHY_BOX_WIDTH,
      height: LABEL_HEIGHT + levels.length * LEVEL_HEIGHT,
      levels: laid,
    };
  });
  const totalWidth =
    LEFT_PAD * 2 + entries.length * (HIERARCHY_BOX_WIDTH + HIERARCHY_GAP);
  const totalHeight =
    TOP_PAD + LABEL_HEIGHT + maxLevels * LEVEL_HEIGHT + TOP_PAD;
  return { hierarchies: out, width: totalWidth, height: totalHeight };
}

/// Find the (laid-out level) coordinates of a given attach kind on
/// whichever hierarchy declares it. Returns null when the kind isn't
/// in any hierarchy (validate would catch this — but the spec might
/// be transiently broken while the user types).
function locateKind(
  hierarchies: LaidOutHierarchy[],
  kindName: string,
): { x: number; y: number; hierarchy: string } | null {
  for (const h of hierarchies) {
    const level = h.levels.find((l) => l.id === kindName);
    if (level) {
      return {
        x: h.x + h.width / 2,
        y: level.y + LEVEL_HEIGHT / 2,
        hierarchy: h.name,
      };
    }
  }
  return null;
}

// ─── Component ─────────────────────────────────────────────────────────────

interface Props {
  tomlText: string;
}

/// Debounced TOML→spec render. Parses server-side (one source of
/// truth for the spec shape) so we don't have to ship a TOML parser
/// to the browser.
export function SchemaSketch({ tomlText }: Props) {
  const [spec, setSpec] = useState<GraphSpec | null>(null);
  const [parseError, setParseError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const t = setTimeout(async () => {
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
    }, 400); // debounce — typing-friendly without re-rendering every keystroke
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [tomlText]);

  const layout = useMemo(() => {
    if (!spec) return null;
    return layoutHierarchies(spec);
  }, [spec]);

  // Per-source attach annotations: which kind does each metric
  // source attach to? We use these to draw a callout under the
  // matching level box. Bridge sources are visualized as arrows
  // (see below) instead.
  const metricAnchors = useMemo(() => {
    if (!spec || !layout) return [];
    const out: { source: string; x: number; y: number; metrics: string[] }[] =
      [];
    for (const s of spec.sources ?? []) {
      const kinds = normalizeAttachKinds(s.attaches_at);
      if (kinds.length === 0) continue;
      // We anchor at the first kind in the composite list (primary).
      const loc = locateKind(layout.hierarchies, kinds[0]);
      if (!loc) continue;
      const metrics = Object.keys(spec.metrics?.[s.alias] ?? {});
      if (metrics.length === 0) continue;
      out.push({ source: s.alias, x: loc.x, y: loc.y, metrics });
    }
    return out;
  }, [spec, layout]);

  // Bridge edges: source has no attaches_at + >=2 relations to
  // different hierarchies. The Rust validator already enforces this
  // shape, but for the sketch we just trust the spec — relations
  // whose endpoints we can locate become drawn arrows.
  const bridgeArrows = useMemo(() => {
    if (!spec || !layout) return [];
    const arrows: {
      from: { x: number; y: number };
      to: { x: number; y: number };
      label: string;
    }[] = [];
    const bridgeSources = (spec.sources ?? []).filter(
      (s) => !s.attaches_at,
    );
    for (const bridge of bridgeSources) {
      const rels = (spec.relation ?? []).filter(
        (r) => r.from.alias === bridge.alias || r.to.alias === bridge.alias,
      );
      // For each pair of relations on the same bridge, draw an arc.
      // The "other side" of each relation points at a hierarchy
      // source — we resolve that to a level by matching the relation's
      // to.columns[0] against any level's column or key.
      const endpoints: { x: number; y: number; label: string }[] = [];
      for (const r of rels) {
        const otherAlias =
          r.from.alias === bridge.alias ? r.to.alias : r.from.alias;
        const otherCol =
          r.from.alias === bridge.alias ? r.to.columns[0] : r.from.columns[0];
        // Find hierarchy where source === otherAlias.
        const hLaid = layout.hierarchies.find(
          (h) => spec.hierarchy?.[h.name]?.source === otherAlias,
        );
        if (!hLaid) continue;
        // Match column against any level's column (and `key` when set).
        const hSpec = spec.hierarchy[hLaid.name];
        const match = levelsOf(hSpec).find(
          (l) =>
            l.level.column === otherCol ||
            (l.level.key && l.level.key === otherCol),
        );
        if (!match) continue;
        const lvl = hLaid.levels.find((l) => l.id === match.id);
        if (!lvl) continue;
        endpoints.push({
          x: hLaid.x + hLaid.width / 2,
          y: lvl.y + LEVEL_HEIGHT / 2,
          label: hLaid.name,
        });
      }
      if (endpoints.length >= 2) {
        // Connect the first two (multi-way bridges are rare; we'd
        // need fan-out logic that doesn't pay off until they exist).
        arrows.push({
          from: endpoints[0],
          to: endpoints[1],
          label: bridge.alias,
        });
      }
    }
    return arrows;
  }, [spec, layout]);

  if (!spec && !parseError && loading) {
    return (
      <div className="flex items-center gap-2 text-xs text-gray-500 px-3 py-3">
        <Loader2 size={12} className="animate-spin" />
        Parsing TOML…
      </div>
    );
  }
  if (parseError) {
    return (
      <div className="m-3 rounded border border-amber-900/60 bg-amber-950/20 p-2.5 text-[11px] text-amber-300">
        <div className="flex items-start gap-1.5">
          <AlertCircle size={12} className="mt-0.5 shrink-0" />
          <pre className="font-mono whitespace-pre-wrap break-words">{parseError}</pre>
        </div>
      </div>
    );
  }
  if (!spec || !layout) return null;

  const width = Math.max(layout.width, 480);
  const height = Math.max(layout.height + metricAnchors.length * 16, 200);

  return (
    <div className="p-3">
      <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-2">
        Schema sketch · {Object.keys(spec.hierarchy ?? {}).length} hierarchies ·{" "}
        {(spec.sources ?? []).length} sources ·{" "}
        {Object.values(spec.metrics ?? {}).reduce(
          (n, m) => n + Object.keys(m).length,
          0,
        )}{" "}
        metrics
      </div>

      <svg
        width={width}
        height={height}
        className="block border border-gray-800 rounded bg-gray-900/30"
      >
        {/* Bridge arrows — drawn first so hierarchy boxes overlap them */}
        {bridgeArrows.map((arc, i) => {
          const midX = (arc.from.x + arc.to.x) / 2;
          const midY = Math.min(arc.from.y, arc.to.y) - 24;
          const path = `M ${arc.from.x} ${arc.from.y} Q ${midX} ${midY} ${arc.to.x} ${arc.to.y}`;
          return (
            <g key={i}>
              <path
                d={path}
                stroke="#a78bfa"
                strokeWidth={1.5}
                fill="none"
                strokeDasharray="3 3"
              />
              <text
                x={midX}
                y={midY - 4}
                textAnchor="middle"
                className="fill-purple-300 text-[10px] font-mono"
              >
                {arc.label}
              </text>
            </g>
          );
        })}

        {/* Hierarchies */}
        {layout.hierarchies.map((h) => (
          <g key={h.name}>
            {/* Title bar */}
            <rect
              x={h.x}
              y={TOP_PAD}
              width={h.width}
              height={LABEL_HEIGHT}
              fill="#1e3a8a"
              stroke="#3b82f6"
            />
            <text
              x={h.x + h.width / 2}
              y={TOP_PAD + LABEL_HEIGHT / 2 + 4}
              textAnchor="middle"
              className="fill-blue-100 text-[11px] font-medium"
            >
              {h.name}
            </text>
            {/* Levels */}
            {h.levels.map((l, i) => (
              <g key={l.id}>
                <rect
                  x={h.x}
                  y={l.y}
                  width={h.width}
                  height={LEVEL_HEIGHT}
                  fill={i % 2 === 0 ? "#111827" : "#0f172a"}
                  stroke="#374151"
                />
                <text
                  x={h.x + 6}
                  y={l.y + LEVEL_HEIGHT / 2 + 4}
                  className="fill-gray-200 text-[11px] font-mono"
                >
                  {l.id}
                </text>
                <text
                  x={h.x + h.width - 6}
                  y={l.y + LEVEL_HEIGHT / 2 + 4}
                  textAnchor="end"
                  className="fill-gray-500 text-[10px] font-mono"
                >
                  {l.column}
                </text>
              </g>
            ))}
            {/* Source name beneath the hierarchy */}
            <text
              x={h.x + h.width / 2}
              y={TOP_PAD + h.height + 14}
              textAnchor="middle"
              className="fill-gray-500 text-[10px] font-mono"
            >
              ← {h.source}
            </text>
          </g>
        ))}

        {/* Metric source annotations — small chips next to attach */}
        {metricAnchors.map((m, i) => (
          <g key={i}>
            <circle
              cx={m.x}
              cy={m.y}
              r={3.5}
              fill="#10b981"
              stroke="#064e3b"
            />
            <text
              x={m.x + 8}
              y={m.y + 4}
              className="fill-emerald-300 text-[10px] font-mono"
            >
              {m.source}: {m.metrics.join(", ")}
            </text>
          </g>
        ))}
      </svg>

      {/* Legend */}
      <div className="mt-3 flex flex-wrap items-center gap-3 text-[10px] text-gray-500">
        <span className="flex items-center gap-1">
          <span className="inline-block w-3 h-3 rounded-sm bg-blue-700 border border-blue-500" />
          hierarchy title
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-3 h-3 rounded-sm bg-gray-900 border border-gray-700" />
          level
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-2 h-2 rounded-full bg-emerald-500" />
          metric source attach
        </span>
        <span className="flex items-center gap-1">
          <svg width={20} height={6}>
            <path d="M 0 3 L 20 3" stroke="#a78bfa" strokeWidth={1.5} strokeDasharray="3 3" />
          </svg>
          bridge source
        </span>
      </div>
    </div>
  );
}
