import { useEffect, useState } from "react";
import { AlertCircle, Plus, X } from "lucide-react";
import type { SourceSpec } from "./types";
import { attachKindsToWire, normalizeAttachKinds } from "./types";
import { Combobox } from "./Combobox";

interface Props {
  source: SourceSpec;
  /// All source aliases in the current spec — used for uniqueness
  /// checks. Includes the editing source's current alias (we exclude
  /// it when checking).
  allAliases: string[];
  /// Every kind name declared by any hierarchy's level list. Drives
  /// the `attaches_at` chip picker so the user can only pick valid
  /// kinds instead of free-text typing.
  knownKinds: string[];
  /// Tables + views discovered in the tenant DuckDB. Powers the
  /// table-field autocomplete. Empty list → Combobox falls back to
  /// free-text mode (still works, just no suggestions).
  duckdbRelations: { value: string; hint?: string }[];
  /// Called with the updated source after a successful blur. Parent
  /// (FormView) re-serializes the whole spec and propagates back to
  /// the GraphDesigner toml state.
  onChange: (updated: SourceSpec) => void;
}

type AttachMode = "none" | "single" | "composite";

/// Per-field validation result. Each field flags either an error
/// (red, blocks the form from committing) or a warning (amber,
/// informational). Combined into the inspector's footer.
interface FieldError {
  field: string;
  message: string;
  severity: "error" | "warning";
}

/// Inspector form for one `SourceSpec`. The form is fully controlled
/// — local draft state owns the field values; `onChange` fires on
/// blur (and on chip-picker changes that don't have a "blur" event).
/// Empty / unchanged values revert silently.
export function SourcesInspector({ source, allAliases, knownKinds, duckdbRelations, onChange }: Props) {
  // Local draft. Synced to `source` when the parent passes a new
  // object (e.g., after a delete + re-select cycle).
  const [alias, setAlias] = useState(source.alias);
  const [table, setTable] = useState(source.table);
  const [filter, setFilter] = useState(source.filter ?? "");
  const [attachMode, setAttachMode] = useState<AttachMode>(() => {
    const kinds = normalizeAttachKinds(source.attaches_at);
    if (kinds.length === 0) return "none";
    if (kinds.length === 1) return "single";
    return "composite";
  });
  const [attachKinds, setAttachKinds] = useState<string[]>(() =>
    normalizeAttachKinds(source.attaches_at),
  );

  // Re-sync when the parent swaps the source we're editing (delete
  // + auto-select another, or external reload).
  useEffect(() => {
    setAlias(source.alias);
    setTable(source.table);
    setFilter(source.filter ?? "");
    const kinds = normalizeAttachKinds(source.attaches_at);
    setAttachKinds(kinds);
    setAttachMode(
      kinds.length === 0 ? "none" : kinds.length === 1 ? "single" : "composite",
    );
  }, [source]);

  // Validation — recomputed on every render. Cheap since the spec
  // is small (handful of sources, a few kinds). Pushed up to the
  // inspector footer.
  const errors: FieldError[] = [];
  if (!alias.trim()) {
    errors.push({ field: "alias", message: "alias is required", severity: "error" });
  } else if (
    allAliases.filter((a) => a === alias).length > 1 ||
    (alias !== source.alias && allAliases.includes(alias))
  ) {
    errors.push({
      field: "alias",
      message: `alias "${alias}" is already in use`,
      severity: "error",
    });
  }
  if (!table.trim()) {
    errors.push({ field: "table", message: "table is required", severity: "error" });
  }
  for (const k of attachKinds) {
    if (!knownKinds.includes(k)) {
      errors.push({
        field: "attaches_at",
        message: `kind "${k}" isn't declared by any hierarchy`,
        severity: "warning",
      });
    }
  }
  if (attachMode === "composite" && attachKinds.length < 2) {
    errors.push({
      field: "attaches_at",
      message: "composite attach needs at least 2 kinds",
      severity: "warning",
    });
  }

  /// Build the updated SourceSpec and push it to the parent. Called
  /// from individual on-blur handlers so each field commits
  /// independently. Strips empty filter to keep the TOML clean.
  const commit = (overrides: Partial<SourceSpec> = {}) => {
    const next: SourceSpec = {
      alias: alias.trim(),
      table: table.trim(),
      attaches_at:
        attachMode === "none" ? undefined : attachKindsToWire(attachKinds),
      filter: filter.trim() === "" ? undefined : filter,
      ...overrides,
    };
    onChange(next);
  };

  return (
    <div className="p-4 space-y-4">
      <div>
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
          Editing source
        </div>
        <h2 className="text-base font-medium text-gray-100 font-mono">{source.alias}</h2>
      </div>

      {/* Alias */}
      <Field
        label="Alias"
        hint="Stable handle referenced by hierarchies, relations, and metric blocks."
        error={errors.find((e) => e.field === "alias")}
      >
        <input
          type="text"
          value={alias}
          onChange={(e) => setAlias(e.target.value)}
          onBlur={() => {
            if (alias.trim() && !errors.some((e) => e.field === "alias" && e.severity === "error")) {
              commit();
            }
          }}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      {/* Table */}
      <Field
        label="DuckDB table"
        hint="Type to filter the tenant's tables + views. Free text is allowed — pick from the list or type a name that isn't materialized yet."
        error={errors.find((e) => e.field === "table")}
      >
        <Combobox
          value={table}
          onChange={setTable}
          onCommit={(next) => {
            if (next.trim()) commit({ table: next.trim() });
          }}
          options={duckdbRelations}
          placeholder="e.g. asv2_ph_master"
          allowFreeText
        />
      </Field>

      {/* Attaches at */}
      <Field
        label="Attaches at"
        hint="Single kind = metric/spine source. Composite (2+ kinds) = composite-attach metric (one primary + auxiliary). None = bridge source."
        error={errors.find((e) => e.field === "attaches_at")}
      >
        <div className="flex items-center gap-1.5 mb-2">
          {(["none", "single", "composite"] as AttachMode[]).map((m) => (
            <button
              key={m}
              onClick={() => {
                setAttachMode(m);
                if (m === "none") {
                  setAttachKinds([]);
                  commit({ attaches_at: undefined });
                } else if (m === "single") {
                  // Keep at most one kind; if currently composite,
                  // drop to just the first.
                  const trimmed = attachKinds.slice(0, 1);
                  setAttachKinds(trimmed);
                  commit({ attaches_at: attachKindsToWire(trimmed) });
                } else {
                  // composite — leave list intact; commit later when
                  // user adds another chip via the picker.
                  if (attachKinds.length > 0) {
                    commit({ attaches_at: attachKindsToWire(attachKinds) });
                  }
                }
              }}
              className={
                "text-[11px] px-2 py-1 rounded border " +
                (attachMode === m
                  ? "bg-blue-900/40 border-blue-700 text-blue-200"
                  : "bg-gray-900 border-gray-700 text-gray-300 hover:bg-gray-800")
              }
            >
              {m}
            </button>
          ))}
        </div>
        {attachMode !== "none" && (
          <AttachKindPicker
            kinds={attachKinds}
            knownKinds={knownKinds}
            maxKinds={attachMode === "single" ? 1 : 8}
            onChange={(next) => {
              setAttachKinds(next);
              commit({ attaches_at: attachKindsToWire(next) });
            }}
          />
        )}
      </Field>

      {/* Filter */}
      <Field
        label="SQL filter"
        hint="Optional WHERE clause body (no WHERE keyword). Free-form SQL. Empty = no filter."
      >
        <input
          type="text"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onBlur={() => commit()}
          placeholder="active = true AND is_deleted = false"
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      {/* Footer: warnings (errors block the form via the underlying
          field's error border; this surface lists the non-blocking
          warnings so the user can see them at a glance). */}
      {errors.some((e) => e.severity === "warning") && (
        <div className="rounded border border-amber-900/60 bg-amber-950/20 p-2.5 text-[11px] text-amber-300 space-y-1">
          {errors
            .filter((e) => e.severity === "warning")
            .map((e, i) => (
              <div key={i} className="flex items-start gap-1.5">
                <AlertCircle size={11} className="mt-0.5 shrink-0" />
                <span>
                  <span className="font-mono text-amber-200/70">{e.field}:</span> {e.message}
                </span>
              </div>
            ))}
        </div>
      )}
    </div>
  );
}

// ─── Atoms ─────────────────────────────────────────────────────────────────

function Field({
  label,
  hint,
  error,
  children,
}: {
  label: string;
  hint?: string;
  error?: FieldError;
  children: React.ReactNode;
}) {
  const isError = error?.severity === "error";
  return (
    <div className={isError ? "" : ""}>
      <div className="text-[10px] uppercase tracking-wider text-gray-400 mb-1">
        {label}
      </div>
      <div className={isError ? "ring-1 ring-red-700 rounded" : ""}>{children}</div>
      {hint && !error && (
        <div className="text-[10px] text-gray-600 mt-1">{hint}</div>
      )}
      {error && (
        <div
          className={
            "text-[10px] mt-1 " +
            (error.severity === "error" ? "text-red-400" : "text-amber-400")
          }
        >
          {error.message}
        </div>
      )}
    </div>
  );
}

function AttachKindPicker({
  kinds,
  knownKinds,
  maxKinds,
  onChange,
}: {
  kinds: string[];
  knownKinds: string[];
  maxKinds: number;
  onChange: (next: string[]) => void;
}) {
  const [draft, setDraft] = useState("");
  const available = knownKinds.filter((k) => !kinds.includes(k));

  const add = (k: string) => {
    if (!k || kinds.includes(k) || kinds.length >= maxKinds) return;
    onChange([...kinds, k]);
    setDraft("");
  };
  const remove = (k: string) => onChange(kinds.filter((x) => x !== k));

  return (
    <div className="space-y-2">
      {kinds.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {kinds.map((k) => (
            <span
              key={k}
              className="inline-flex items-center gap-1 text-[11px] px-1.5 py-0.5 rounded bg-blue-900/40 border border-blue-700 text-blue-200 font-mono"
            >
              {k}
              <button
                onClick={() => remove(k)}
                className="text-blue-300 hover:text-red-400"
                title="Remove"
              >
                <X size={10} />
              </button>
            </span>
          ))}
        </div>
      )}
      {kinds.length < maxKinds && (
        <div className="flex items-center gap-1.5">
          <select
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            className="text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 focus:outline-none focus:border-blue-500 flex-1"
          >
            <option value="">— pick a kind —</option>
            {available.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </select>
          <button
            onClick={() => add(draft)}
            disabled={!draft || kinds.length >= maxKinds}
            className="flex items-center gap-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
          >
            <Plus size={11} />
            Add
          </button>
        </div>
      )}
    </div>
  );
}
