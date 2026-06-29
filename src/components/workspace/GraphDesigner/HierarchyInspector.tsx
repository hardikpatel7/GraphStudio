import { useEffect, useState } from "react";
import { GripVertical, Trash2, Plus } from "lucide-react";
import type { HierarchySpec } from "./types";
import { levelsOf } from "./types";
import { Combobox } from "./Combobox";

interface Props {
  name: string;
  hierarchy: HierarchySpec;
  /// Every hierarchy name already in use (for rename uniqueness).
  /// Includes the current name.
  siblingNames: string[];
  /// Source aliases — drives the source combobox.
  sourceAliases: string[];
  /// Persist a rename of the hierarchy key.
  onRename: (prev: string, next: string) => void;
  /// Persist the new HierarchySpec (source + level reorder/add/delete).
  onChange: (updated: HierarchySpec) => void;
  /// Open the level editor for a given level id.
  onSelectLevel: (levelId: string) => void;
}

/// Inspector for a single hierarchy. Edits:
///   - the hierarchy key (rename)
///   - the spine `source` (which source supplies the columns)
///   - the level list — add / delete / reorder
///
/// Per-level editing (column, key, split, unnest) is handled by
/// LevelInspector, opened by clicking a level row in this inspector
/// or via the Tree.
export function HierarchyInspector({
  name,
  hierarchy,
  siblingNames,
  sourceAliases,
  onRename,
  onChange,
  onSelectLevel,
}: Props) {
  const [draftName, setDraftName] = useState(name);
  const [source, setSource] = useState(hierarchy.source);
  useEffect(() => {
    setDraftName(name);
    setSource(hierarchy.source);
  }, [name, hierarchy]);

  const levels = levelsOf(hierarchy);

  const nameError =
    !draftName.trim()
      ? "hierarchy name is required"
      : draftName !== name && siblingNames.includes(draftName)
      ? `hierarchy "${draftName}" already exists`
      : null;

  /// Rebuild the HierarchySpec preserving level insertion order
  /// after a list mutation. The spec has `source` + arbitrary level
  /// keys at the same depth; we splice the levels array back as the
  /// non-source keys in order.
  const writeLevels = (nextLevels: { id: string; level: any }[]) => {
    const out: HierarchySpec = { source: hierarchy.source };
    for (const lvl of nextLevels) {
      out[lvl.id] = lvl.level;
    }
    onChange(out);
  };

  const addLevel = () => {
    // Generate a unique default id, then open it in the inspector.
    const existing = new Set(levels.map((l) => l.id));
    let n = levels.length + 1;
    let id = `l${n}`;
    while (existing.has(id)) {
      n += 1;
      id = `l${n}`;
    }
    writeLevels([...levels, { id, level: { column: "" } }]);
    onSelectLevel(id);
  };

  const deleteLevel = (id: string) => {
    writeLevels(levels.filter((l) => l.id !== id));
  };

  const moveLevel = (id: string, direction: "up" | "down") => {
    const idx = levels.findIndex((l) => l.id === id);
    if (idx < 0) return;
    const target = direction === "up" ? idx - 1 : idx + 1;
    if (target < 0 || target >= levels.length) return;
    const next = [...levels];
    [next[idx], next[target]] = [next[target], next[idx]];
    writeLevels(next);
  };

  return (
    <div className="p-4 space-y-4">
      <div>
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
          Editing hierarchy
        </div>
        <h2 className="text-base font-medium text-gray-100 font-mono">{name}</h2>
      </div>

      <Field
        label="Hierarchy name"
        hint="The TOML key for this hierarchy (e.g. `product`, `store`). Must match a GraphStudio dimension."
        error={nameError}
      >
        <input
          type="text"
          value={draftName}
          onChange={(e) => setDraftName(e.target.value)}
          onBlur={() => {
            const trimmed = draftName.trim();
            if (!trimmed || trimmed === name || siblingNames.includes(trimmed)) {
              setDraftName(name);
              return;
            }
            onRename(name, trimmed);
          }}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 font-mono focus:outline-none focus:border-blue-500"
        />
      </Field>

      <Field
        label="Source"
        hint="The source alias whose rows provide the hierarchy spine. Must be one source's `attaches_at` chain — usually the master / dimension table."
      >
        <Combobox
          value={source}
          onChange={setSource}
          onCommit={(next) => {
            const trimmed = next.trim();
            if (trimmed && trimmed !== hierarchy.source) {
              onChange({ ...hierarchy, source: trimmed });
            }
          }}
          options={sourceAliases.map((a) => ({ value: a }))}
          placeholder="pick a source alias"
          allowFreeText
        />
      </Field>

      <div>
        <div className="flex items-center justify-between mb-1">
          <div className="text-[10px] uppercase tracking-wider text-gray-400">
            Levels — top-to-leaf
          </div>
          <button
            onClick={addLevel}
            className="flex items-center gap-1 text-[11px] px-2 py-0.5 rounded bg-blue-600 hover:bg-blue-500 text-white"
          >
            <Plus size={11} />
            Add level
          </button>
        </div>
        {levels.length === 0 ? (
          <div className="text-[11px] text-gray-600 italic px-2 py-1.5 border border-dashed border-gray-800 rounded">
            No levels yet. Click "Add level" to create the root.
          </div>
        ) : (
          <ul className="divide-y divide-gray-900 border border-gray-800 rounded overflow-hidden">
            {levels.map((lvl, idx) => (
              <li
                key={lvl.id}
                className="flex items-center gap-2 px-2 py-1.5 bg-gray-950 hover:bg-gray-900 group"
              >
                <GripVertical
                  size={11}
                  className="text-gray-700 group-hover:text-gray-500 shrink-0"
                />
                <button
                  onClick={() => onSelectLevel(lvl.id)}
                  className="flex-1 text-left text-[11px] truncate font-mono text-gray-300 hover:text-gray-100"
                  title={`Edit ${name}.${lvl.id}`}
                >
                  <span className="text-gray-500 mr-1.5">{idx + 1}.</span>
                  {lvl.id}
                  <span className="text-gray-600 ml-2">
                    column: {lvl.level?.column ?? "?"}
                  </span>
                </button>
                <button
                  onClick={() => moveLevel(lvl.id, "up")}
                  disabled={idx === 0}
                  className="text-[10px] px-1 text-gray-500 hover:text-gray-200 disabled:opacity-30 disabled:cursor-not-allowed"
                  title="Move up"
                >
                  ↑
                </button>
                <button
                  onClick={() => moveLevel(lvl.id, "down")}
                  disabled={idx === levels.length - 1}
                  className="text-[10px] px-1 text-gray-500 hover:text-gray-200 disabled:opacity-30 disabled:cursor-not-allowed"
                  title="Move down"
                >
                  ↓
                </button>
                <button
                  onClick={() => {
                    if (window.confirm(`Delete level "${name}.${lvl.id}"?`)) {
                      deleteLevel(lvl.id);
                    }
                  }}
                  className="text-gray-600 hover:text-red-400 px-1"
                  title="Delete level"
                >
                  <Trash2 size={11} />
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
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
