import { useEffect, useState } from "react";
import type { LevelSpec } from "./types";

interface Props {
  hierarchy: string;
  levelId: string;
  level: LevelSpec;
  /// Every level id already in use under this hierarchy. Includes
  /// the current level (excluded when checking uniqueness on rename).
  siblingIds: string[];
  /// Persist rename (key swap in the hierarchy map). `prev` = current
  /// id; `next` = the new id. Parent rebuilds the hierarchy object
  /// preserving insertion order — order matters (top-to-leaf).
  onRename: (prev: string, next: string) => void;
  onChange: (updated: LevelSpec) => void;
}

/// Inspector for a single hierarchy level. Levels are positional in
/// the hierarchy (top-to-leaf by declaration order); the inspector
/// doesn't reorder — that's a tree-level operation. Renaming the
/// level id swaps a map key, which the parent handles to preserve
/// order.
export function LevelInspector({
  hierarchy,
  levelId,
  level,
  siblingIds,
  onRename,
  onChange,
}: Props) {
  const [id, setId] = useState(levelId);
  const [column, setColumn] = useState(level.column ?? "");
  const [keyCol, setKeyCol] = useState(level.key ?? "");
  const [split, setSplit] = useState(level.split ?? "");
  const [unnest, setUnnest] = useState(Boolean(level.unnest));

  useEffect(() => {
    setId(levelId);
    setColumn(level.column ?? "");
    setKeyCol(level.key ?? "");
    setSplit(level.split ?? "");
    setUnnest(Boolean(level.unnest));
  }, [hierarchy, levelId, level]);

  const idError =
    !id.trim()
      ? "level id is required"
      : id !== levelId && siblingIds.includes(id)
      ? `level "${id}" already exists in ${hierarchy}`
      : null;

  // Commit the LevelSpec only (rename is handled separately via
  // onRename — the parent re-keys the hierarchy map atomically).
  const commitSpec = (overrides: Partial<LevelSpec> = {}) => {
    onChange({
      column: column.trim(),
      key: keyCol.trim() === "" ? undefined : keyCol.trim(),
      split: split.trim() === "" ? undefined : split,
      unnest: unnest ? true : undefined,
      ...overrides,
    });
  };

  return (
    <div className="p-4 space-y-4">
      <div>
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
          Editing level — {hierarchy}.{levelId}
        </div>
        <h2 className="text-base font-medium text-gray-100 font-mono">{levelId}</h2>
      </div>

      <Field
        label="Level id"
        hint="Renaming changes the TOML key for this level. Order is preserved."
        error={idError}
      >
        <input
          type="text"
          value={id}
          onChange={(e) => setId(e.target.value)}
          onBlur={() => {
            const trimmed = id.trim();
            if (!trimmed || trimmed === levelId || siblingIds.includes(trimmed)) {
              setId(levelId);
              return;
            }
            onRename(levelId, trimmed);
          }}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Column"
        hint="Column on the hierarchy source that supplies this level's value."
      >
        <input
          type="text"
          value={column}
          onChange={(e) => setColumn(e.target.value)}
          onBlur={() => commitSpec()}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Key (optional)"
        hint="Stable identifier column when the display value isn't unique. Defaults to `column`."
      >
        <input
          type="text"
          value={keyCol}
          onChange={(e) => setKeyCol(e.target.value)}
          onBlur={() => commitSpec()}
          placeholder="(defaults to column)"
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Split (optional)"
        hint='Splits a delimited string into multiple values (e.g. "," or "|"). Mutually exclusive with `unnest`.'
      >
        <input
          type="text"
          value={split}
          onChange={(e) => setSplit(e.target.value)}
          onBlur={() => commitSpec()}
          placeholder="e.g. ,"
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Unnest"
        hint="When the column is a native LIST<...>, unnest into one row per element. Mutually exclusive with `split`."
      >
        <label className="flex items-center gap-2 text-xs text-gray-300 cursor-pointer">
          <input
            type="checkbox"
            checked={unnest}
            onChange={(e) => {
              setUnnest(e.target.checked);
              commitSpec({ unnest: e.target.checked ? true : undefined });
            }}
            className="accent-blue-500"
          />
          unnest the column as a list
        </label>
      </Field>

      {split && unnest && (
        <div className="rounded border border-amber-900/60 bg-amber-950/20 p-2.5 text-[11px] text-amber-300">
          `split` and `unnest` are mutually exclusive. The server picks `unnest` when both are set.
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
