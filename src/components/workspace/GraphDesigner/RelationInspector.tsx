import { useEffect, useState } from "react";
import { X, Plus } from "lucide-react";
import type { RelationSpec, RelationSide } from "./types";
import { Combobox } from "./Combobox";

interface Props {
  index: number;
  relation: RelationSpec;
  sourceAliases: string[];
  onChange: (updated: RelationSpec) => void;
  onDelete: () => void;
}

/// Inspector for a single relation. A relation declares a join
/// between two sources with cardinalities — both columns lists must
/// have the same length and pair positionally.
///
/// Cardinality `1` means at most one matching row per key on that
/// side; `*` means many. `*:*` is illegal — use a bridge source
/// (no `attaches_at`) with two `*:1` relations.
export function RelationInspector({
  index,
  relation,
  sourceAliases,
  onChange,
  onDelete,
}: Props) {
  const [from, setFrom] = useState<RelationSide>(relation.from);
  const [to, setTo] = useState<RelationSide>(relation.to);

  useEffect(() => {
    setFrom(relation.from);
    setTo(relation.to);
  }, [relation]);

  const colsMatch = from.columns.length === to.columns.length;
  const starStar = from.cardinality === "*" && to.cardinality === "*";

  const commit = (nextFrom: RelationSide, nextTo: RelationSide) => {
    onChange({ from: nextFrom, to: nextTo });
  };

  return (
    <div className="p-4 space-y-4">
      <div className="flex items-start justify-between">
        <div>
          <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
            Editing relation #{index}
          </div>
          <h2 className="text-base font-medium text-gray-100 font-mono">
            {from.alias || "?"} <span className="text-gray-500">→</span>{" "}
            {to.alias || "?"}
          </h2>
        </div>
        <button
          onClick={onDelete}
          className="text-[11px] text-gray-500 hover:text-red-400 px-2 py-1"
          title="Delete this relation"
        >
          Delete
        </button>
      </div>

      <RelationSideEditor
        label="From"
        side={from}
        sourceAliases={sourceAliases}
        onChange={(next) => {
          setFrom(next);
          commit(next, to);
        }}
      />
      <RelationSideEditor
        label="To"
        side={to}
        sourceAliases={sourceAliases}
        onChange={(next) => {
          setTo(next);
          commit(from, next);
        }}
      />

      {!colsMatch && (
        <div className="rounded border border-red-900/60 bg-red-950/20 p-2.5 text-[11px] text-red-300">
          column count mismatch: `from` has {from.columns.length}, `to` has{" "}
          {to.columns.length}. Pair positionally — both sides need the same count.
        </div>
      )}
      {starStar && (
        <div className="rounded border border-red-900/60 bg-red-950/20 p-2.5 text-[11px] text-red-300">
          `*:*` cardinality is rejected. Use a bridge source (no `attaches_at`) with
          two `*:1` relations instead.
        </div>
      )}
    </div>
  );
}

function RelationSideEditor({
  label,
  side,
  sourceAliases,
  onChange,
}: {
  label: string;
  side: RelationSide;
  sourceAliases: string[];
  onChange: (next: RelationSide) => void;
}) {
  const [alias, setAlias] = useState(side.alias);
  const [draftCol, setDraftCol] = useState("");
  useEffect(() => {
    setAlias(side.alias);
  }, [side.alias]);

  const setCols = (next: string[]) => onChange({ ...side, columns: next });
  const setCard = (c: "1" | "*") => onChange({ ...side, cardinality: c });

  return (
    <div className="rounded border border-gray-800 p-3 space-y-3 bg-gray-950/60">
      <div className="text-[10px] uppercase tracking-wider text-gray-500">{label}</div>

      <Field label="Source alias">
        <Combobox
          value={alias}
          onChange={setAlias}
          onCommit={(next) => {
            const trimmed = next.trim();
            if (trimmed && trimmed !== side.alias) {
              onChange({ ...side, alias: trimmed });
            }
          }}
          options={sourceAliases.map((a) => ({ value: a }))}
          placeholder="pick a source alias"
          allowFreeText
        />
      </Field>

      <Field label="Cardinality">
        <div className="flex items-center gap-1.5">
          {(["1", "*"] as const).map((c) => (
            <button
              key={c}
              onClick={() => setCard(c)}
              className={
                "text-[11px] font-mono px-3 py-1 rounded border " +
                (side.cardinality === c
                  ? "bg-blue-900/40 border-blue-700 text-blue-200"
                  : "bg-gray-900 border-gray-700 text-gray-300 hover:bg-gray-800")
              }
            >
              {c}
            </button>
          ))}
          <span className="text-[10px] text-gray-600 ml-2">
            {side.cardinality === "1"
              ? "at most one row per key"
              : "many rows per key"}
          </span>
        </div>
      </Field>

      <Field
        label="Columns"
        hint="Column names on this side. Positional pairing with the other side's list."
      >
        {side.columns.length > 0 && (
          <div className="flex flex-wrap gap-1 mb-1.5">
            {side.columns.map((c, i) => (
              <span
                key={`${c}-${i}`}
                className="inline-flex items-center gap-1 text-[11px] px-1.5 py-0.5 rounded bg-gray-800 border border-gray-700 text-gray-200 font-mono"
              >
                {c}
                <button
                  onClick={() => setCols(side.columns.filter((_, idx) => idx !== i))}
                  className="text-gray-400 hover:text-red-400"
                  title="Remove column"
                >
                  <X size={10} />
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="flex items-center gap-1.5">
          <input
            type="text"
            value={draftCol}
            onChange={(e) => setDraftCol(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && draftCol.trim()) {
                e.preventDefault();
                setCols([...side.columns, draftCol.trim()]);
                setDraftCol("");
              }
            }}
            placeholder="column name (Enter to add)"
            className="flex-1 text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1 font-mono focus:outline-none focus:border-blue-500"
          />
          <button
            onClick={() => {
              if (draftCol.trim()) {
                setCols([...side.columns, draftCol.trim()]);
                setDraftCol("");
              }
            }}
            disabled={!draftCol.trim()}
            className="flex items-center gap-1 text-[11px] px-2 py-1 rounded bg-blue-600 hover:bg-blue-500 text-white disabled:opacity-50"
          >
            <Plus size={11} />
            Add
          </button>
        </div>
      </Field>
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wider text-gray-400 mb-1">
        {label}
      </div>
      {children}
      {hint && <div className="text-[10px] text-gray-600 mt-1">{hint}</div>}
    </div>
  );
}
