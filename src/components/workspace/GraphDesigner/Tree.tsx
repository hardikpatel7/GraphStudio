import { useState } from "react";
import { ChevronRight, ChevronDown, Plus, Trash2, Database, Layers, ArrowRight, Sigma } from "lucide-react";
import type { GraphSpec, SourceSpec, HierarchySpec, MetricSpec, RelationSpec } from "./types";
import { levelsOf, normalizeAttachKinds } from "./types";
import type { Selection } from "./FormView";

interface Props {
  spec: GraphSpec;
  selection: Selection;
  onSelect: (s: Selection) => void;
  onSpecChange: (next: GraphSpec) => void;
}

/// Tree of every editable element under the graph. Section groups —
/// Sources / Hierarchies / Relations / Metrics — each collapsible.
/// `+` adds a new item with sensible defaults; trash removes.
///
/// All four section types are editable: clicking a row opens the
/// matching inspector (SourcesInspector, HierarchyInspector,
/// LevelInspector, RelationInspector, MetricInspector).
export function Tree({ spec, selection, onSelect, onSpecChange }: Props) {
  const [openSources, setOpenSources] = useState(true);
  const [openHierarchies, setOpenHierarchies] = useState(true);
  const [openRelations, setOpenRelations] = useState(false);
  const [openMetrics, setOpenMetrics] = useState(false);
  // Per-hierarchy expand state — defaults to expanded so the user
  // sees the full structure on first load. Closed states persist
  // in component state across re-renders of the same FormView.
  const [hierarchyOpen, setHierarchyOpen] = useState<Record<string, boolean>>({});

  const isSourceSelected = (alias: string) =>
    selection?.type === "source" && selection.alias === alias;
  const isHierarchySelected = (name: string) =>
    selection?.type === "hierarchy" && selection.name === name;
  const isLevelSelected = (h: string, lvl: string) =>
    selection?.type === "level" && selection.hierarchy === h && selection.levelId === lvl;
  const isRelationSelected = (i: number) =>
    selection?.type === "relation" && selection.index === i;
  const isMetricSelected = (src: string, name: string) =>
    selection?.type === "metric" &&
    selection.sourceAlias === src &&
    selection.name === name;

  // ── Add handlers ──────────────────────────────────────────────────

  const addSource = () => {
    // Generate a non-colliding alias. `src_N` where N is one more
    // than the current count + uniqueness loop.
    const existing = new Set((spec.sources ?? []).map((s) => s.alias));
    let n = (spec.sources?.length ?? 0) + 1;
    let alias = `src_${n}`;
    while (existing.has(alias)) {
      n += 1;
      alias = `src_${n}`;
    }
    const next: GraphSpec = {
      ...spec,
      sources: [...(spec.sources ?? []), { alias, table: "" }],
    };
    onSpecChange(next);
    onSelect({ type: "source", alias });
  };

  const deleteSource = (alias: string) => {
    const next: GraphSpec = {
      ...spec,
      sources: (spec.sources ?? []).filter((s) => s.alias !== alias),
    };
    onSpecChange(next);
    if (selection?.type === "source" && selection.alias === alias) {
      onSelect(null);
    }
  };

  const addHierarchy = () => {
    const existing = new Set(Object.keys(spec.hierarchy ?? {}));
    let n = existing.size + 1;
    let name = `dim_${n}`;
    while (existing.has(name)) {
      n += 1;
      name = `dim_${n}`;
    }
    const next: GraphSpec = {
      ...spec,
      hierarchy: {
        ...(spec.hierarchy ?? {}),
        [name]: { source: spec.sources?.[0]?.alias ?? "" } as HierarchySpec,
      },
    };
    onSpecChange(next);
    onSelect({ type: "hierarchy", name });
  };

  const deleteHierarchy = (name: string) => {
    const next: Record<string, HierarchySpec> = { ...(spec.hierarchy ?? {}) };
    delete next[name];
    onSpecChange({ ...spec, hierarchy: next });
    if (
      (selection?.type === "hierarchy" && selection.name === name) ||
      (selection?.type === "level" && selection.hierarchy === name)
    ) {
      onSelect(null);
    }
  };

  const addRelation = () => {
    // Default both sides to the first source alias if available;
    // user reassigns immediately in the inspector. `*-1` is the
    // common case (bridge / dim join), so we seed with that.
    const a = spec.sources?.[0]?.alias ?? "";
    const b = spec.sources?.[1]?.alias ?? a;
    const newRel: RelationSpec = {
      from: { alias: a, columns: [], cardinality: "*" },
      to: { alias: b, columns: [], cardinality: "1" },
    };
    const nextList = [...(spec.relation ?? []), newRel];
    onSpecChange({ ...spec, relation: nextList });
    onSelect({ type: "relation", index: nextList.length - 1 });
  };

  const addMetric = () => {
    // Default to the first source alias that already has an attaches_at
    // (= a metric source); fall back to the first source.
    const attachedSrc =
      (spec.sources ?? []).find(
        (s) => normalizeAttachKinds(s.attaches_at).length > 0,
      )?.alias ?? spec.sources?.[0]?.alias;
    if (!attachedSrc) {
      // No sources at all — bail; can't anchor a metric.
      return;
    }
    const bucket = spec.metrics?.[attachedSrc] ?? {};
    let n = Object.keys(bucket).length + 1;
    let name = `metric_${n}`;
    while (bucket[name]) {
      n += 1;
      name = `metric_${n}`;
    }
    const newMetric: MetricSpec = { rollup: "sum", column: "" };
    const nextBucket = { ...bucket, [name]: newMetric };
    onSpecChange({
      ...spec,
      metrics: { ...(spec.metrics ?? {}), [attachedSrc]: nextBucket },
    });
    onSelect({ type: "metric", sourceAlias: attachedSrc, name });
  };

  // ── Render ────────────────────────────────────────────────────────

  return (
    <div className="h-full overflow-auto bg-gray-950 text-[11px]">
      {/* Sources */}
      <Section
        title="Sources"
        icon={<Database size={11} />}
        count={spec.sources?.length ?? 0}
        open={openSources}
        onToggle={() => setOpenSources((v) => !v)}
        onAdd={addSource}
      >
        {(spec.sources ?? []).map((s) => (
          <RowItem
            key={s.alias}
            label={s.alias}
            sublabel={describeSource(s)}
            active={isSourceSelected(s.alias)}
            onClick={() => onSelect({ type: "source", alias: s.alias })}
            onDelete={() => {
              if (window.confirm(`Delete source "${s.alias}"?`)) deleteSource(s.alias);
            }}
            indent={1}
          />
        ))}
        {(spec.sources ?? []).length === 0 && <EmptyRow text="No sources yet" indent={1} />}
      </Section>

      {/* Hierarchies */}
      <Section
        title="Hierarchies"
        icon={<Layers size={11} />}
        count={Object.keys(spec.hierarchy ?? {}).length}
        open={openHierarchies}
        onToggle={() => setOpenHierarchies((v) => !v)}
        onAdd={addHierarchy}
      >
        {Object.entries(spec.hierarchy ?? {}).map(([name, h]) => {
          const open = hierarchyOpen[name] ?? true;
          const levels = levelsOf(h);
          return (
            <div key={name}>
              <RowItem
                label={name}
                sublabel={`source: ${h.source} · ${levels.length} level${levels.length === 1 ? "" : "s"}`}
                icon={
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      setHierarchyOpen({ ...hierarchyOpen, [name]: !open });
                    }}
                    className="hover:text-gray-200"
                  >
                    {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                  </button>
                }
                active={isHierarchySelected(name)}
                onClick={() => onSelect({ type: "hierarchy", name })}
                onDelete={() => {
                  if (window.confirm(`Delete hierarchy "${name}" and all its levels?`)) {
                    deleteHierarchy(name);
                  }
                }}
                indent={1}
              />
              {open &&
                levels.map((lvl) => (
                  <RowItem
                    key={`${name}.${lvl.id}`}
                    label={lvl.id}
                    sublabel={`column: ${lvl.level.column}`}
                    active={isLevelSelected(name, lvl.id)}
                    onClick={() =>
                      onSelect({ type: "level", hierarchy: name, levelId: lvl.id })
                    }
                    indent={2}
                  />
                ))}
            </div>
          );
        })}
        {Object.keys(spec.hierarchy ?? {}).length === 0 && (
          <EmptyRow text="No hierarchies yet" indent={1} />
        )}
      </Section>

      {/* Relations */}
      <Section
        title="Relations"
        icon={<ArrowRight size={11} />}
        count={spec.relation?.length ?? 0}
        open={openRelations}
        onToggle={() => setOpenRelations((v) => !v)}
        onAdd={addRelation}
      >
        {(spec.relation ?? []).map((r, i) => (
          <RowItem
            key={i}
            label={`${r.from.alias} → ${r.to.alias}`}
            sublabel={`${r.from.cardinality} : ${r.to.cardinality}`}
            active={isRelationSelected(i)}
            onClick={() => onSelect({ type: "relation", index: i })}
            onDelete={() => {
              if (window.confirm(`Delete relation #${i}?`)) {
                const next = (spec.relation ?? []).filter((_, idx) => idx !== i);
                onSpecChange({ ...spec, relation: next });
                if (selection?.type === "relation" && selection.index === i) {
                  onSelect(null);
                }
              }
            }}
            indent={1}
          />
        ))}
        {(spec.relation ?? []).length === 0 && (
          <EmptyRow text="No relations defined" indent={1} />
        )}
      </Section>

      {/* Metrics */}
      <Section
        title="Metrics"
        icon={<Sigma size={11} />}
        count={countMetrics(spec)}
        open={openMetrics}
        onToggle={() => setOpenMetrics((v) => !v)}
        onAdd={addMetric}
        addTooltip={
          (spec.sources?.length ?? 0) === 0
            ? "Add at least one source before adding metrics"
            : undefined
        }
        addDisabled={(spec.sources?.length ?? 0) === 0}
      >
        {Object.entries(spec.metrics ?? {}).map(([srcAlias, mDict]) => (
          <div key={srcAlias}>
            <div className="px-2 py-0.5 text-[10px] text-gray-500 font-mono pl-6">
              {srcAlias}
            </div>
            {Object.keys(mDict).map((mName) => (
              <RowItem
                key={`${srcAlias}.${mName}`}
                label={mName}
                sublabel={`rollup: ${mDict[mName].rollup}`}
                active={isMetricSelected(srcAlias, mName)}
                onClick={() => onSelect({ type: "metric", sourceAlias: srcAlias, name: mName })}
                indent={2}
              />
            ))}
          </div>
        ))}
        {countMetrics(spec) === 0 && <EmptyRow text="No metrics defined" indent={1} />}
      </Section>
    </div>
  );
}

// ─── Atoms ─────────────────────────────────────────────────────────────────

function Section({
  title,
  icon,
  count,
  open,
  onToggle,
  onAdd,
  addDisabled,
  addTooltip,
  children,
}: {
  title: string;
  icon: React.ReactNode;
  count: number;
  open: boolean;
  onToggle: () => void;
  onAdd?: () => void;
  addDisabled?: boolean;
  addTooltip?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="border-b border-gray-900">
      <div className="flex items-center px-2 py-1.5 hover:bg-gray-900/40 group">
        <button
          onClick={onToggle}
          className="flex items-center gap-1.5 flex-1 text-left text-gray-300"
        >
          {open ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
          <span className="text-gray-500">{icon}</span>
          <span className="uppercase tracking-wider text-[10px] font-semibold">{title}</span>
          <span className="text-gray-600 text-[10px] font-mono">{count}</span>
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            if (!addDisabled && onAdd) onAdd();
          }}
          disabled={addDisabled}
          title={addTooltip ?? `Add ${title.toLowerCase()}`}
          className="text-gray-600 hover:text-gray-200 disabled:opacity-30 disabled:cursor-not-allowed"
        >
          <Plus size={11} />
        </button>
      </div>
      {open && children}
    </div>
  );
}

function RowItem({
  label,
  sublabel,
  icon,
  active,
  onClick,
  onDelete,
  indent,
}: {
  label: string;
  sublabel?: string;
  icon?: React.ReactNode;
  active?: boolean;
  onClick: () => void;
  onDelete?: () => void;
  indent: number;
}) {
  // Native browser tooltip via `title` — labels/sublabels truncate
  // with CSS but the full text shows on hover. The row-level title
  // composes both so users see the whole node identity even when
  // both columns get clipped.
  const tooltip = sublabel ? `${label} — ${sublabel}` : label;
  return (
    <div
      onClick={onClick}
      title={tooltip}
      className={
        "flex items-baseline gap-1.5 py-0.5 pr-3 cursor-pointer group " +
        (active
          ? "bg-blue-900/40 text-gray-100"
          : "hover:bg-gray-900/60 text-gray-300")
      }
      style={{ paddingLeft: indent * 16 + 8 }}
    >
      {icon}
      <span className="font-mono truncate flex-1" title={label}>
        {label}
      </span>
      {sublabel && (
        <span
          className="text-gray-600 text-[10px] truncate max-w-[140px]"
          title={sublabel}
        >
          {sublabel}
        </span>
      )}
      {onDelete && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
          className="invisible group-hover:visible text-gray-600 hover:text-red-400 px-1"
        >
          <Trash2 size={10} />
        </button>
      )}
    </div>
  );
}

function EmptyRow({ text, indent }: { text: string; indent: number }) {
  return (
    <div
      className="text-[10px] text-gray-600 italic py-0.5 pr-3"
      style={{ paddingLeft: indent * 16 + 8 }}
    >
      {text}
    </div>
  );
}

function describeSource(s: SourceSpec): string {
  const attaches = normalizeAttachKinds(s.attaches_at);
  if (attaches.length > 0) return `→ ${attaches.join(", ")}`;
  return "(bridge)";
}

function countMetrics(spec: GraphSpec): number {
  return Object.values(spec.metrics ?? {}).reduce(
    (n, m) => n + Object.keys(m).length,
    0,
  );
}
