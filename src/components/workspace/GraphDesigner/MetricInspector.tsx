import { useEffect, useState } from "react";
import type { MetricSpec } from "./types";
import { Combobox } from "./Combobox";

interface Props {
  sourceAlias: string;
  metricName: string;
  metric: MetricSpec;
  /// Metric names already in use under this source alias. Used for
  /// uniqueness checks on rename.
  siblingNames: string[];
  /// Every source alias declared in the spec — feeds the source
  /// combobox if the user wants to reassign the metric.
  sourceAliases: string[];
  /// Rename inside the same source.
  onRename: (prev: string, next: string) => void;
  /// Move to a different source alias (renames the parent map key).
  /// The parent (FormView) handles the cross-map relocation.
  onMoveSource: (fromAlias: string, toAlias: string, metricName: string) => void;
  onChange: (updated: MetricSpec) => void;
  onDelete: () => void;
}

/// Inspector for one metric under a source. Edits:
///   - metric name (renames the map key)
///   - source alias (moves the metric to another `[metrics.<alias>]`
///     section)
///   - column (raw column on the source, mutually exclusive with `expr`)
///   - rollup operator
///   - expr (SQL fragment, mutually exclusive with `column`)
///
/// `column` vs `expr` is a server-side oneof — the form keeps both
/// inputs but warns when both are populated.
const ROLLUPS = [
  "sum",
  "min",
  "max",
  "count",
  "count_distinct",
  "set",
  "list",
  "avg",
  "any",
  "all",
];

export function MetricInspector({
  sourceAlias,
  metricName,
  metric,
  siblingNames,
  sourceAliases,
  onRename,
  onMoveSource,
  onChange,
  onDelete,
}: Props) {
  const [name, setName] = useState(metricName);
  const [alias, setAlias] = useState(sourceAlias);
  const [column, setColumn] = useState(metric.column ?? "");
  const [rollup, setRollup] = useState(metric.rollup);
  const [expr, setExpr] = useState(metric.expr ?? "");

  useEffect(() => {
    setName(metricName);
    setAlias(sourceAlias);
    setColumn(metric.column ?? "");
    setRollup(metric.rollup);
    setExpr(metric.expr ?? "");
  }, [sourceAlias, metricName, metric]);

  const nameError =
    !name.trim()
      ? "metric name is required"
      : name !== metricName && siblingNames.includes(name)
      ? `metric "${name}" already exists under ${sourceAlias}`
      : null;

  const bothColAndExpr = column.trim() !== "" && expr.trim() !== "";

  const commitSpec = (overrides: Partial<MetricSpec> = {}) => {
    onChange({
      column: column.trim() === "" ? undefined : column.trim(),
      rollup,
      expr: expr.trim() === "" ? undefined : expr,
      ...overrides,
    });
  };

  return (
    <div className="p-4 space-y-4">
      <div className="flex items-start justify-between">
        <div>
          <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
            Editing metric — {sourceAlias}.{metricName}
          </div>
          <h2 className="text-base font-medium text-gray-100 font-mono">{metricName}</h2>
        </div>
        <button
          onClick={onDelete}
          className="text-[11px] text-gray-500 hover:text-red-400 px-2 py-1"
          title="Delete this metric"
        >
          Delete
        </button>
      </div>

      <Field
        label="Metric name"
        hint="The TOML key — must be unique within this source's metrics."
        error={nameError}
      >
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            const trimmed = name.trim();
            if (!trimmed || trimmed === metricName || siblingNames.includes(trimmed)) {
              setName(metricName);
              return;
            }
            onRename(metricName, trimmed);
          }}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Source alias"
        hint="The source this metric attaches to. Reassigning moves the metric under another `[metrics.<alias>]` section."
      >
        <Combobox
          value={alias}
          onChange={setAlias}
          onCommit={(next) => {
            const trimmed = next.trim();
            if (trimmed && trimmed !== sourceAlias) {
              onMoveSource(sourceAlias, trimmed, metricName);
            }
          }}
          options={sourceAliases.map((a) => ({ value: a }))}
          placeholder="pick a source alias"
          allowFreeText
        />
      </Field>

      <Field
        label="Rollup"
        hint="Fires at attach-time (multiple rows → same node) AND at parent rollup. Must be associative."
      >
        <div className="flex flex-wrap gap-1">
          {ROLLUPS.map((r) => (
            <button
              key={r}
              onClick={() => {
                setRollup(r);
                commitSpec({ rollup: r });
              }}
              className={
                "text-[11px] font-mono px-2 py-0.5 rounded border " +
                (rollup === r
                  ? "bg-blue-900/40 border-blue-700 text-blue-200"
                  : "bg-gray-900 border-gray-700 text-gray-400 hover:bg-gray-800")
              }
            >
              {r}
            </button>
          ))}
        </div>
      </Field>

      <Field
        label="Column"
        hint="Raw column on the source. Mutually exclusive with `expr`."
      >
        <input
          type="text"
          value={column}
          onChange={(e) => setColumn(e.target.value)}
          onBlur={() => commitSpec()}
          placeholder="e.g. ttl_inv"
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Expr (optional)"
        hint="SQL fragment evaluated per row at attach time. Mutually exclusive with `column`."
      >
        <textarea
          value={expr}
          onChange={(e) => setExpr(e.target.value)}
          onBlur={() => commitSpec()}
          placeholder="e.g. CASE WHEN status = 'A' THEN 1 ELSE 0 END"
          rows={3}
          spellCheck={false}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono leading-snug resize-y focus:outline-none focus:border-blue-500"
        />
      </Field>

      {bothColAndExpr && (
        <div className="rounded border border-amber-900/60 bg-amber-950/20 p-2.5 text-[11px] text-amber-300">
          Both `column` and `expr` are set — server picks `expr` and ignores `column`.
          Clear one to be explicit.
        </div>
      )}
    </div>
  );
}

function Field({
  label,
  hint,
  error,
  children,
}: {
  label: string;
  hint?: string;
  error?: string | null;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wider text-gray-400 mb-1">
        {label}
      </div>
      <div className={error ? "ring-1 ring-red-700 rounded" : ""}>{children}</div>
      {hint && !error && (
        <div className="text-[10px] text-gray-600 mt-1">{hint}</div>
      )}
      {error && <div className="text-[10px] text-red-400 mt-1">{error}</div>}
    </div>
  );
}
