import { useEffect, useMemo, useRef, useState } from "react";
import {
  Filter,
  Plus,
  Play,
  X,
  Loader2,
  Copy,
  ChevronDown,
  ChevronRight,
  Search,
  ShieldCheck,
} from "lucide-react";

/// Cross-Filter Explorer — interactive UI over POST /api/cross-filter.
///
/// The cross-filter resolver lives entirely in the V8 in-memory
/// ArticleGraph: `apply_filters` walks article NodeIds against the
/// hierarchy spine + brand/channel cross-indices, then `project_distinct`
/// returns one distinct list per requested attribute. The explorer is
/// a thin UI for that — pick attributes, build filters, fire, see the
/// per-attribute distinct response. Click any returned value to chain
/// it back into the filter set (intersection-walk the catalog).
///
/// No new endpoints — this component is read-only and only POSTs the
/// existing wire shape.

const FILTERABLE_ATTRIBUTES = [
  "l0_name",
  "l1_name",
  "l2_name",
  "l3_name",
  "l4_name",
  "l5_name",
  "brand",
  "channel",
  "article",
];
// Attributes the resolver knows how to project distincts for. Same set
// as FILTERABLE_ATTRIBUTES today; kept separate in case the resolver
// ever supports projecting a different set than it filters on.
const PROJECTABLE_ATTRIBUTES = FILTERABLE_ATTRIBUTES;

const OPERATORS = ["in", "eq", "ne", "not_in"] as const;
type Operator = (typeof OPERATORS)[number];

interface FilterRow {
  /// Stable id so React doesn't lose focus when the user reorders.
  uid: number;
  attribute_name: string;
  operator: Operator;
  values: string[];
}

interface CrossFilterResponse {
  count: number;
  status: boolean;
  data: Record<string, string[]>;
  message: string;
}

let nextUid = 1;
const newRow = (): FilterRow => ({
  uid: nextUid++,
  attribute_name: FILTERABLE_ATTRIBUTES[1] ?? "l1_name",
  operator: "in",
  values: [],
});

export function CrossFilterWorkspace() {
  // ── Attribute toggles (top strip) ──
  // Each toggled attribute has both a filter dropdown row AND a card in
  // the response. The two are kept in sync: toggle ON adds a row and a
  // card; toggle OFF removes both. "Add filter" lets users add an
  // input-only row for an attribute they don't want a response card for.
  const INITIAL_ATTRS = ["l1_name", "brand"];
  const [pickedAttrs, setPickedAttrs] = useState<Set<string>>(
    () => new Set(INITIAL_ATTRS),
  );
  // ── Filter rows ── seeded from the toggled attrs.
  const [rows, setRows] = useState<FilterRow[]>(() =>
    INITIAL_ATTRS.map((attr) => ({
      uid: nextUid++,
      attribute_name: attr,
      operator: "in",
      values: [],
    })),
  );
  // ── UAM toggle ──
  const [uamOn, setUamOn] = useState(false);
  const [userCode, setUserCode] = useState("");
  const [aclCode, setAclCode] = useState("");
  // ── Run + response ──
  const [loading, setLoading] = useState(false);
  const [response, setResponse] = useState<CrossFilterResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [durationMs, setDurationMs] = useState<number | null>(null);
  const [showRaw, setShowRaw] = useState(false);
  const [copyMsg, setCopyMsg] = useState<string | null>(null);

  // Auto-scroll the response when it arrives.
  const responseRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (response && responseRef.current) {
      responseRef.current.scrollIntoView({ behavior: "smooth", block: "nearest" });
    }
  }, [response]);

  // ── Helpers ──
  const buildPayload = useMemo(() => {
    return () => {
      const filters = rows
        .filter((r) => r.values.length > 0)
        .map((r) => ({
          attribute_name: r.attribute_name,
          operator: r.operator,
          values: r.values,
        }));
      const attributes = Array.from(pickedAttrs).map((name) => ({
        attribute_name: name,
        // The resolver doesn't actually use these on the simple path;
        // wire-faithful values for the cross_filter::model::Attribute
        // shape so the request validates.
        dimension: "product",
        filter_type: "non-cascaded",
      }));
      const body: any = {
        attributes,
        filters,
        is_urm_filter: uamOn,
      };
      if (uamOn) {
        const u = parseInt(userCode || "0", 10);
        const a = parseInt(aclCode || "0", 10);
        if (Number.isFinite(u)) body.user_code = u;
        if (Number.isFinite(a)) body.acl_code = a;
      }
      return body;
    };
  }, [rows, pickedAttrs, uamOn, userCode, aclCode]);

  const run = async () => {
    setLoading(true);
    setError(null);
    setResponse(null);
    setDurationMs(null);
    const t0 = performance.now();
    try {
      const r = await fetch("/api/cross-filter", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(buildPayload()),
      });
      const text = await r.text();
      let parsed: any = null;
      try {
        parsed = JSON.parse(text);
      } catch {
        throw new Error(`non-JSON response (${r.status}): ${text.slice(0, 200)}`);
      }
      if (!r.ok) {
        throw new Error(parsed?.error ?? text);
      }
      setResponse(parsed as CrossFilterResponse);
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setDurationMs(performance.now() - t0);
      setLoading(false);
    }
  };

  const copyCurl = async () => {
    const body = JSON.stringify(buildPayload(), null, 2);
    const cmd = `curl -X POST http://localhost:3002/api/cross-filter \\\n  -H 'Content-Type: application/json' \\\n  -d '${body.replace(/'/g, "'\\''")}'`;
    try {
      await navigator.clipboard.writeText(cmd);
      setCopyMsg("Copied");
      setTimeout(() => setCopyMsg(null), 1500);
    } catch {
      setCopyMsg("Copy failed");
      setTimeout(() => setCopyMsg(null), 1500);
    }
  };

  /// Click handler for response values: extend an existing IN filter
  /// for the attribute, or append a new row when no filter for this
  /// attribute exists yet.
  const addFilterValue = (attrName: string, value: string) => {
    setRows((prev) => {
      const idx = prev.findIndex(
        (r) => r.attribute_name === attrName && r.operator === "in",
      );
      if (idx >= 0) {
        const next = prev.slice();
        const r = next[idx];
        if (r.values.includes(value)) return prev;
        next[idx] = { ...r, values: [...r.values, value] };
        return next;
      }
      return [
        ...prev,
        {
          uid: nextUid++,
          attribute_name: attrName,
          operator: "in",
          values: [value],
        },
      ];
    });
  };

  const toggleAttribute = (name: string) => {
    setPickedAttrs((prev) => {
      const next = new Set(prev);
      if (next.has(name)) {
        next.delete(name);
        // Untoggling also removes the row for that attribute. Any
        // values the user typed in get dropped — accepting that as the
        // simpler model (the user can re-toggle and refill via
        // cascading distincts in seconds).
        setRows((rs) => rs.filter((r) => r.attribute_name !== name));
      } else {
        next.add(name);
        // Toggling on adds a row IF no row currently exists for this
        // attribute — preserves any pre-existing manual row from
        // "Add filter" rather than duplicating.
        setRows((rs) => {
          if (rs.some((r) => r.attribute_name === name)) return rs;
          return [
            ...rs,
            {
              uid: nextUid++,
              attribute_name: name,
              operator: "in" as const,
              values: [],
            },
          ];
        });
      }
      return next;
    });
  };

  const updateRow = (uid: number, patch: Partial<FilterRow>) => {
    setRows((prev) => prev.map((r) => (r.uid === uid ? { ...r, ...patch } : r)));
  };
  const removeRow = (uid: number) => {
    setRows((prev) => {
      const target = prev.find((r) => r.uid === uid);
      // Removing a row also untoggles its attribute — keeps the top
      // strip consistent with the visible filter rows.
      if (target) {
        setPickedAttrs((p) => {
          if (!p.has(target.attribute_name)) return p;
          const next = new Set(p);
          next.delete(target.attribute_name);
          return next;
        });
      }
      return prev.filter((r) => r.uid !== uid);
    });
  };
  const addRowValue = (uid: number, value: string) => {
    if (!value) return;
    updateRow(uid, {});
    setRows((prev) =>
      prev.map((r) =>
        r.uid === uid && !r.values.includes(value)
          ? { ...r, values: [...r.values, value] }
          : r,
      ),
    );
  };
  const removeRowValue = (uid: number, value: string) => {
    setRows((prev) =>
      prev.map((r) =>
        r.uid === uid ? { ...r, values: r.values.filter((v) => v !== value) } : r,
      ),
    );
  };

  return (
    <div className="h-full flex flex-col bg-gray-950 text-gray-200">
      {/* Header strip */}
      <div className="px-4 py-3 border-b border-gray-800 flex items-center gap-3">
        <Search size={14} className="text-blue-400" />
        <h1 className="text-sm font-medium text-gray-100">Cross Filter Explorer</h1>
        <span className="text-[10px] text-gray-500">
          POST /api/cross-filter · in-memory legacy graph
        </span>
        <div className="ml-auto flex items-center gap-2">
          <button
            onClick={copyCurl}
            className="flex items-center gap-1 text-[11px] px-2 py-1 rounded bg-gray-900 border border-gray-800 text-gray-300 hover:bg-gray-800"
            title="Copy a curl command for the request body"
          >
            <Copy size={11} /> Copy curl
          </button>
          {copyMsg && <span className="text-[10px] text-green-400">{copyMsg}</span>}
          <button
            onClick={run}
            disabled={loading || pickedAttrs.size === 0}
            className="flex items-center gap-1 text-xs px-3 py-1 rounded bg-blue-600 text-white hover:bg-blue-500 disabled:opacity-40"
            title={pickedAttrs.size === 0 ? "Pick at least one attribute" : "Send the request"}
          >
            {loading ? <Loader2 size={12} className="animate-spin" /> : <Play size={12} />}
            Run
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-auto px-4 py-4 space-y-4">
        {/* ── Attributes ── */}
        <Section
          title="Attributes"
          subtitle="What distinct values should the resolver return?"
        >
          <div className="flex flex-wrap gap-1.5">
            {PROJECTABLE_ATTRIBUTES.map((name) => {
              const on = pickedAttrs.has(name);
              return (
                <button
                  key={name}
                  onClick={() => toggleAttribute(name)}
                  className={`text-[11px] font-mono px-2 py-1 rounded border transition-colors ${
                    on
                      ? "border-blue-500 bg-blue-900/60 text-blue-100"
                      : "border-gray-800 bg-gray-900 text-gray-400 hover:border-gray-700 hover:text-gray-200"
                  }`}
                >
                  {name}
                </button>
              );
            })}
          </div>
        </Section>

        {/* ── Filters ── */}
        <Section
          title="Filters"
          subtitle="AND across rows; IN-set within a single row's values."
          right={
            <button
              onClick={() => setRows((prev) => [...prev, newRow()])}
              className="flex items-center gap-1 text-[11px] text-blue-400 hover:underline"
            >
              <Plus size={11} /> Add filter
            </button>
          }
        >
          <div className="space-y-1.5">
            {rows.length === 0 && (
              <div className="text-[11px] text-gray-500">
                No filters — the response will return the full distinct sets across the graph.
              </div>
            )}
            {rows.map((r) => (
              <FilterRowEditor
                key={r.uid}
                row={r}
                otherRows={rows.filter((x) => x.uid !== r.uid)}
                uamPayload={{
                  is_urm_filter: uamOn,
                  user_code: uamOn ? parseInt(userCode || "0", 10) : undefined,
                  acl_code: uamOn ? parseInt(aclCode || "0", 10) : undefined,
                }}
                onChange={(patch) => updateRow(r.uid, patch)}
                onAddValue={(v) => addRowValue(r.uid, v)}
                onRemoveValue={(v) => removeRowValue(r.uid, v)}
                onRemove={() => removeRow(r.uid)}
              />
            ))}
          </div>
        </Section>

        {/* ── UAM ── */}
        <Section
          title="UAM"
          subtitle="Optional. When enabled, intersects the candidate set with the user's entitled articles."
        >
          <div className="flex items-center gap-3 text-xs">
            <label className="flex items-center gap-1.5 cursor-pointer">
              <input
                type="checkbox"
                checked={uamOn}
                onChange={(e) => setUamOn(e.target.checked)}
                className="rounded border-gray-700 bg-gray-900 text-blue-500"
              />
              <ShieldCheck size={11} className={uamOn ? "text-blue-400" : "text-gray-600"} />
              Enforce
            </label>
            <label className="flex items-center gap-1.5">
              <span className="text-[10px] uppercase text-gray-500">user_code</span>
              <input
                type="number"
                value={userCode}
                onChange={(e) => setUserCode(e.target.value)}
                disabled={!uamOn}
                className="w-24 px-1.5 py-0.5 text-xs rounded bg-gray-950 border border-gray-800 text-gray-200 disabled:opacity-40"
              />
            </label>
            <label className="flex items-center gap-1.5">
              <span className="text-[10px] uppercase text-gray-500">acl_code</span>
              <input
                type="number"
                value={aclCode}
                onChange={(e) => setAclCode(e.target.value)}
                disabled={!uamOn}
                className="w-24 px-1.5 py-0.5 text-xs rounded bg-gray-950 border border-gray-800 text-gray-200 disabled:opacity-40"
              />
            </label>
          </div>
        </Section>

        {/* ── Status / response ── */}
        <div ref={responseRef}>
          {error && (
            <div className="rounded border border-red-900/60 bg-red-950/30 p-3 text-xs text-red-300 mb-3">
              {error}
            </div>
          )}
          {response && (
            <Section
              title="Response"
              subtitle={`status: ${response.status ? "ok" : "fail"} · count ${response.count} · ${
                durationMs != null ? `${Math.round(durationMs)} ms` : ""
              }`}
              right={
                <button
                  onClick={() => setShowRaw((v) => !v)}
                  className="flex items-center gap-1 text-[11px] text-gray-400 hover:text-gray-200"
                >
                  {showRaw ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                  Raw JSON
                </button>
              }
            >
              <ResponseCards
                data={response.data}
                onValueClick={addFilterValue}
                pickedAttrs={pickedAttrs}
              />
              {showRaw && (
                <pre className="mt-3 text-[11px] font-mono text-gray-300 bg-black/40 rounded p-2 max-h-64 overflow-auto whitespace-pre-wrap break-words">
                  {JSON.stringify(response, null, 2)}
                </pre>
              )}
            </Section>
          )}
        </div>
      </div>
    </div>
  );
}

/// Small section wrapper — title + subtitle on top, content below.
function Section({
  title,
  subtitle,
  right,
  children,
}: {
  title: string;
  subtitle?: string;
  right?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section>
      <div className="flex items-baseline gap-2 mb-2">
        <h2 className="text-[10px] uppercase tracking-wider font-semibold text-gray-400">
          {title}
        </h2>
        {subtitle && (
          <span className="text-[11px] text-gray-500">{subtitle}</span>
        )}
        {right && <div className="ml-auto">{right}</div>}
      </div>
      {children}
    </section>
  );
}

function FilterRowEditor({
  row,
  otherRows,
  uamPayload,
  onChange,
  onAddValue,
  onRemoveValue,
  onRemove,
}: {
  row: FilterRow;
  // Other filter rows in the form. Used as the cascade context when
  // fetching distincts for THIS row — same pattern as the dataview's
  // cascading filters: "what values are available given the other
  // selections".
  otherRows: FilterRow[];
  // UAM payload from the parent so the dropdown options respect the
  // user's entitled article set when enforcement is on.
  uamPayload: { is_urm_filter: boolean; user_code?: number; acl_code?: number };
  onChange: (patch: Partial<FilterRow>) => void;
  onAddValue: (v: string) => void;
  onRemoveValue: (v: string) => void;
  onRemove: () => void;
}) {
  return (
    <div className="grid grid-cols-[140px_70px_minmax(0,1fr)_28px] gap-2 items-start px-2 py-1.5 rounded border border-gray-800 bg-gray-900/40">
      <select
        value={row.attribute_name}
        onChange={(e) => onChange({ attribute_name: e.target.value, values: [] })}
        className="text-[11px] font-mono px-1.5 py-1 rounded bg-gray-950 border border-gray-800 text-gray-200 focus:outline-none focus:border-blue-500"
        title="Attribute to filter on. Changing it clears the value chips."
      >
        {FILTERABLE_ATTRIBUTES.map((a) => (
          <option key={a} value={a}>
            {a}
          </option>
        ))}
      </select>
      <select
        value={row.operator}
        onChange={(e) => onChange({ operator: e.target.value as Operator })}
        className="text-[11px] px-1.5 py-1 rounded bg-gray-950 border border-gray-800 text-gray-200 focus:outline-none focus:border-blue-500"
      >
        {OPERATORS.map((op) => (
          <option key={op} value={op}>
            {op}
          </option>
        ))}
      </select>
      <ValueDropdown
        attribute={row.attribute_name}
        selected={row.values}
        otherRows={otherRows}
        uamPayload={uamPayload}
        onAdd={onAddValue}
        onRemove={onRemoveValue}
      />
      <button
        onClick={onRemove}
        className="w-7 h-7 flex items-center justify-center rounded text-gray-500 hover:text-red-400 hover:bg-red-950/40"
        title="Remove this filter row"
      >
        <X size={12} />
      </button>
    </div>
  );
}

/// Multi-select dropdown for an attribute's values. On open, fetches
/// distincts via cross-filter using the OTHER filter rows as cascade
/// context. Selected values render as chips; the dropdown shows the
/// full distinct list with checkmarks for already-selected values.
function ValueDropdown({
  attribute,
  selected,
  otherRows,
  uamPayload,
  onAdd,
  onRemove,
}: {
  attribute: string;
  selected: string[];
  otherRows: FilterRow[];
  uamPayload: { is_urm_filter: boolean; user_code?: number; acl_code?: number };
  onAdd: (v: string) => void;
  onRemove: (v: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [values, setValues] = useState<string[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  // Re-fetch when the dropdown opens or the cascade context changes.
  // The fetch hits /api/cross-filter with attributes=[{attr}] and
  // filters=otherRows so the returned set respects the other rows.
  const cascadeKey = useMemo(
    () =>
      JSON.stringify({
        attribute,
        filters: otherRows
          .filter((r) => r.values.length > 0)
          .map((r) => ({ a: r.attribute_name, op: r.operator, v: [...r.values].sort() })),
        uam: uamPayload,
      }),
    [attribute, otherRows, uamPayload],
  );

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    const filters = otherRows
      .filter((r) => r.values.length > 0)
      .map((r) => ({
        attribute_name: r.attribute_name,
        operator: r.operator,
        values: r.values,
      }));
    const body: any = {
      attributes: [{ attribute_name: attribute, dimension: "product", filter_type: "non-cascaded" }],
      filters,
      is_urm_filter: uamPayload.is_urm_filter,
    };
    if (uamPayload.is_urm_filter) {
      if (Number.isFinite(uamPayload.user_code)) body.user_code = uamPayload.user_code;
      if (Number.isFinite(uamPayload.acl_code)) body.acl_code = uamPayload.acl_code;
    }
    fetch("/api/cross-filter", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
      .then(async (r) => {
        const text = await r.text();
        let parsed: any = null;
        try {
          parsed = JSON.parse(text);
        } catch {
          throw new Error(`non-JSON response (${r.status}): ${text.slice(0, 200)}`);
        }
        if (!r.ok) throw new Error(parsed?.error ?? text);
        if (cancelled) return;
        const list: string[] = (parsed?.data?.[attribute] ?? []) as string[];
        setValues(list);
      })
      .catch((e: any) => {
        if (!cancelled) setError(e?.message ?? String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, cascadeKey]);

  const filtered = useMemo(() => {
    if (!values) return [] as string[];
    const q = search.trim().toLowerCase();
    if (!q) return values;
    return values.filter((v) => v.toLowerCase().includes(q));
  }, [values, search]);

  return (
    <div className="relative">
      <div className="flex flex-wrap items-center gap-1">
        {selected.map((v) => (
          <span
            key={v}
            className="inline-flex items-center gap-1 text-[11px] font-mono px-1.5 py-0.5 rounded border border-blue-800/60 bg-blue-950/40 text-blue-200"
          >
            {v}
            <button
              onClick={() => onRemove(v)}
              className="text-blue-400 hover:text-blue-100"
              title="Remove value"
            >
              <X size={10} />
            </button>
          </span>
        ))}
        <button
          onClick={() => setOpen((v) => !v)}
          className="flex items-center gap-1 text-[11px] px-2 py-0.5 rounded border border-gray-800 bg-gray-950 text-gray-300 hover:border-gray-700"
          title="Pick from cross-filter distincts (cascades over the other rows)"
        >
          <Plus size={10} /> {selected.length === 0 ? "Pick values" : "Add value"}
          <ChevronDown size={10} />
        </button>
      </div>
      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute z-50 left-0 mt-1 w-72 rounded border border-gray-700 bg-gray-950 shadow-xl">
            <div className="px-2 py-1.5 border-b border-gray-800 flex items-center gap-2">
              <Search size={11} className="text-gray-500" />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="filter…"
                autoFocus
                className="flex-1 text-xs font-mono px-1 py-0.5 bg-transparent text-gray-200 focus:outline-none"
              />
              <span className="text-[10px] text-gray-500 tabular-nums">
                {selected.length}/{values?.length ?? 0}
              </span>
            </div>
            <div className="max-h-64 overflow-auto">
              {loading && (
                <div className="px-2 py-2 text-xs text-gray-500 flex items-center gap-1.5">
                  <Loader2 size={11} className="animate-spin" /> Loading distincts…
                </div>
              )}
              {error && (
                <div className="px-2 py-2 text-xs text-red-300">{error}</div>
              )}
              {!loading && !error && values && values.length === 0 && (
                <div className="px-2 py-2 text-xs text-gray-500 italic">
                  no values for this attribute under the current filters
                </div>
              )}
              {!loading && !error && filtered.length === 0 && values && values.length > 0 && (
                <div className="px-2 py-2 text-xs text-gray-500 italic">
                  no matches for "{search}"
                </div>
              )}
              {filtered.map((v) => {
                const on = selected.includes(v);
                return (
                  <button
                    key={v}
                    onClick={() => (on ? onRemove(v) : onAdd(v))}
                    className={`block w-full text-left text-[11px] font-mono px-2 py-0.5 ${
                      on
                        ? "text-blue-200 bg-blue-950/40"
                        : "text-gray-300 hover:bg-gray-900"
                    }`}
                    title={`${on ? "Remove" : "Add"} ${v}`}
                  >
                    <span className="inline-block w-3">{on ? "✓" : ""}</span>
                    {v}
                  </button>
                );
              })}
              {filtered.length > 500 && (
                <div className="px-2 py-1 text-[10px] text-gray-500 italic border-t border-gray-800">
                  showing first 500 — refine the search to see more
                </div>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/// Render the response as one card per requested attribute. Each value
/// is a clickable chip that adds it as an IN filter for that attribute.
function ResponseCards({
  data,
  onValueClick,
  pickedAttrs,
}: {
  data: Record<string, string[]>;
  onValueClick: (attr: string, value: string) => void;
  pickedAttrs: Set<string>;
}) {
  // Render in the order the user picked attributes — gives stable
  // positioning regardless of map iteration order.
  const orderedAttrs = Array.from(pickedAttrs).filter((k) =>
    Object.prototype.hasOwnProperty.call(data, k),
  );
  if (orderedAttrs.length === 0) {
    return (
      <div className="text-xs text-gray-500">
        No data returned. Either no candidates matched the filters, or no attributes were
        selected.
      </div>
    );
  }
  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
      {orderedAttrs.map((attr) => {
        const values = data[attr] || [];
        return (
          <div
            key={attr}
            className="rounded border border-gray-800 bg-gray-900/40 overflow-hidden"
          >
            <div className="flex items-center justify-between px-3 py-2 bg-gray-900/60 border-b border-gray-800">
              <span className="font-mono text-[11px] text-gray-200">{attr}</span>
              <span className="text-[10px] text-gray-500">
                {values.length} {values.length === 1 ? "value" : "values"}
              </span>
            </div>
            <div className="max-h-64 overflow-auto p-2 space-y-0.5">
              {values.length === 0 && (
                <div className="text-[11px] text-gray-500 italic px-1">no values</div>
              )}
              {values.slice(0, 200).map((v) => (
                <button
                  key={v}
                  onClick={() => onValueClick(attr, v)}
                  className="block w-full text-left text-[11px] font-mono px-2 py-0.5 rounded text-gray-300 hover:bg-blue-950/40 hover:text-blue-200"
                  title={`Add ${attr} = ${v} to the filter set`}
                >
                  {v}
                </button>
              ))}
              {values.length > 200 && (
                <div className="text-[10px] text-gray-500 italic px-1 pt-1">
                  +{values.length - 200} more (truncated for display)
                </div>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// Avoid unused-import warnings for icons that get conditionally rendered
// (this list also acts as a usage doc for new contributors).
void [Filter];
