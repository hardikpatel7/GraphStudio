import { useEffect, useState } from "react";
import { X, Download, Upload, ChevronDown, ChevronRight } from "lucide-react";
import { api } from "@/api/client";

// Connections are deliberately excluded — they hold PG passwords +
// internal hostnames that trip WAF rules at the edge (403). Recreate
// connections per tenant.
const KIND_LABELS: Record<string, string> = {
  dataviews:      "DataViews",
  pipelines:      "Pipelines",
  sources:        "Sources",
  dimensions:     "Dimensions",
  filter_configs: "Filter Configs",
  saved_queries:  "Saved Queries",
};
// Render order — matches user's mental model (input → derived → consumed).
const KIND_ORDER = [
  "sources", "pipelines", "dataviews",
  "dimensions", "filter_configs", "saved_queries",
];

type Inv = Record<string, { id: string; display_name: string }[]>;

/** Bundle export/import modal. Picker UI for selecting objects across
 * kinds; export downloads one JSON, import takes one back.
 */
export function BundleModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [inv, setInv] = useState<Inv>({});
  const [selected, setSelected] = useState<Record<string, Set<string>>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [importMode, setImportMode] = useState<"new" | "replace">("new");

  useEffect(() => {
    if (!open) return;
    setBusy(true);
    setError(null);
    api.getBundleInventory()
      .then((d) => {
        setInv(d || {});
        // Default-collapse everything; selecting nothing is the default.
        setExpanded(KIND_ORDER.reduce((acc, k) => ({ ...acc, [k]: false }), {}));
        setSelected({});
      })
      .catch((e: any) => setError(e?.message || "failed to load inventory"))
      .finally(() => setBusy(false));
  }, [open]);

  const totalSelected = Object.values(selected).reduce((n, s) => n + s.size, 0);

  const toggleKind = (kind: string) => {
    setExpanded((prev) => ({ ...prev, [kind]: !prev[kind] }));
  };
  const toggleItem = (kind: string, id: string) => {
    setSelected((prev) => {
      const next = { ...prev };
      const set = new Set(next[kind] || []);
      if (set.has(id)) set.delete(id); else set.add(id);
      next[kind] = set;
      return next;
    });
  };
  const selectAllInKind = (kind: string, checked: boolean) => {
    setSelected((prev) => ({
      ...prev,
      [kind]: checked ? new Set((inv[kind] || []).map((it) => it.id)) : new Set(),
    }));
  };

  const handleExport = async () => {
    if (totalSelected === 0) return;
    setBusy(true);
    setError(null);
    try {
      const kindsBody: Record<string, string[]> = {};
      for (const [k, set] of Object.entries(selected)) {
        if (set.size > 0) kindsBody[k] = Array.from(set);
      }
      // Direct fetch so we can stream the response into a blob/download.
      const res = await fetch(api.exportBundleUrl(), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ kinds: kindsBody }),
      });
      if (!res.ok) {
        const e = await res.json().catch(() => ({ error: res.statusText }));
        throw new Error(e.error || res.statusText);
      }
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      // Use the filename the server suggested.
      const cd = res.headers.get("Content-Disposition") || "";
      const m = cd.match(/filename="?([^"]+)"?/);
      a.download = m ? m[1] : "bundle.json";
      document.body.appendChild(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
    } catch (e: any) {
      setError(e?.message || "export failed");
    } finally {
      setBusy(false);
    }
  };

  const handleImport = async (file: File) => {
    setBusy(true);
    setError(null);
    try {
      const text = await file.text();
      const data = JSON.parse(text);
      const result = await api.importBundle(data, importMode);
      const entries = result?.by_kind ? (Object.entries(result.by_kind) as Array<[string, any]>) : [];
      const summary = entries.length > 0
        ? entries.map(([k, v]) => `${KIND_LABELS[k] || k}: +${v.inserted}${v.replaced ? ` ↻${v.replaced}` : ""}`).join("  ·  ")
        : "imported";
      alert(`Bundle imported (${importMode}):\n${summary}`);
      // Refresh inventory after import so the user can see new ids.
      const fresh = await api.getBundleInventory();
      setInv(fresh || {});
      setSelected({});
    } catch (e: any) {
      setError(e?.message || "import failed");
    } finally {
      setBusy(false);
    }
  };

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        className="w-[640px] max-h-[80vh] flex flex-col bg-gray-900 border border-gray-700 rounded shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-4 py-2 border-b border-gray-800">
          <div className="text-sm font-semibold text-gray-200">Bundle export / import</div>
          <button onClick={onClose} className="text-gray-500 hover:text-gray-300" aria-label="Close">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-4 py-3">
          {error && (
            <div className="mb-3 px-3 py-2 rounded bg-red-950/50 border border-red-900 text-xs text-red-300">{error}</div>
          )}

          {KIND_ORDER.map((kind) => {
            const items = inv[kind] || [];
            const set = selected[kind] || new Set<string>();
            const allSelected = items.length > 0 && set.size === items.length;
            const isOpen = expanded[kind];
            return (
              <div key={kind} className="mb-2 border border-gray-800 rounded">
                <div className="flex items-center gap-2 px-3 py-1.5 bg-gray-850/40">
                  <button
                    onClick={() => toggleKind(kind)}
                    className="text-gray-400 hover:text-gray-200"
                    title={isOpen ? "Collapse" : "Expand"}
                  >
                    {isOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                  </button>
                  <span className="text-sm text-gray-200 font-medium">{KIND_LABELS[kind] || kind}</span>
                  <span className="text-[10px] text-gray-500">{items.length}</span>
                  <div className="ml-auto flex items-center gap-2">
                    <span className="text-[11px] text-blue-300">{set.size > 0 ? `${set.size} selected` : ""}</span>
                    <label className="text-[11px] text-gray-400 flex items-center gap-1 cursor-pointer">
                      <input
                        type="checkbox"
                        checked={allSelected}
                        onChange={(e) => selectAllInKind(kind, e.target.checked)}
                        disabled={items.length === 0}
                      />
                      All
                    </label>
                  </div>
                </div>
                {isOpen && items.length > 0 && (
                  <div className="px-3 py-1.5 max-h-48 overflow-y-auto">
                    {items.map((it) => (
                      <label key={it.id} className="flex items-center gap-2 py-0.5 text-xs text-gray-300 cursor-pointer hover:text-gray-100">
                        <input
                          type="checkbox"
                          checked={set.has(it.id)}
                          onChange={() => toggleItem(kind, it.id)}
                        />
                        <span className="truncate">{it.display_name}</span>
                        <span className="ml-auto text-[10px] text-gray-500 font-mono truncate">{it.id}</span>
                      </label>
                    ))}
                  </div>
                )}
                {isOpen && items.length === 0 && (
                  <div className="px-3 py-2 text-[11px] text-gray-600 italic">empty</div>
                )}
              </div>
            );
          })}
        </div>

        <div className="flex items-center gap-2 px-4 py-2 border-t border-gray-800">
          <button
            onClick={handleExport}
            disabled={busy || totalSelected === 0}
            className="flex items-center gap-1.5 px-3 py-1 text-xs text-blue-300 border border-blue-800 rounded bg-blue-950/40 hover:bg-blue-900/40 disabled:opacity-40"
            title={totalSelected === 0 ? "Select at least one object" : "Download a JSON containing the selected objects"}
          >
            <Download size={12} />
            Export selected ({totalSelected})
          </button>

          <div className="ml-auto flex items-center gap-2">
            <select
              value={importMode}
              onChange={(e) => setImportMode(e.target.value as "new" | "replace")}
              className="text-[11px] bg-gray-800 border border-gray-700 rounded px-1.5 py-0.5 text-gray-200"
              title='"new" auto-suffixes clashing ids; "replace" overwrites'
            >
              <option value="new">Mode: new</option>
              <option value="replace">Mode: replace</option>
            </select>
            <label
              className="flex items-center gap-1.5 px-3 py-1 text-xs text-amber-300 border border-amber-800 rounded bg-amber-950/40 hover:bg-amber-900/40 cursor-pointer"
              title="Upload a previously-exported bundle JSON"
            >
              <Upload size={12} />
              Import bundle…
              <input
                type="file"
                accept="application/json,.json"
                className="hidden"
                disabled={busy}
                onChange={(e) => {
                  const f = e.target.files?.[0];
                  e.target.value = "";
                  if (f) handleImport(f);
                }}
              />
            </label>
          </div>
        </div>
      </div>
    </div>
  );
}
