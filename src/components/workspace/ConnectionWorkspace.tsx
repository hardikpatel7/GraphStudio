import { useState, useEffect, useCallback } from "react";
import { Save, Loader2, Plug, CheckCircle2, XCircle, Eye, EyeOff, Zap, Star, Trash2, Copy } from "lucide-react";
import { api } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";

interface ConnectionWorkspaceProps {
  connectionId: string;
}

const TYPE_COLORS: Record<string, string> = {
  postgres: "bg-blue-900/50 text-blue-400",
  pg: "bg-blue-900/50 text-blue-400",
  clickhouse: "bg-yellow-900/50 text-yellow-300",
  bigquery: "bg-emerald-900/50 text-emerald-400",
  duckdb: "bg-amber-900/50 text-amber-400",
};

const PG_FIELDS = [
  { key: "host", label: "Host", placeholder: "localhost" },
  { key: "port", label: "Port", placeholder: "5432", type: "number" },
  { key: "database", label: "Database", placeholder: "mydb" },
  { key: "user", label: "User", placeholder: "postgres" },
  { key: "password", label: "Password", placeholder: "********", secret: true },
  { key: "schema", label: "Schema", placeholder: "public" },
];

const CH_FIELDS = [
  { key: "host", label: "Host", placeholder: "clickhouse.internal" },
  { key: "port", label: "Port", placeholder: "8123", type: "number" },
  { key: "username", label: "Username", placeholder: "default" },
  { key: "password", label: "Password", placeholder: "********", secret: true },
  { key: "ssl", label: "SSL", placeholder: "false", type: "boolean" },
  { key: "query_timeout_seconds", label: "Query timeout (s)", placeholder: "30", type: "number" },
  { key: "allow_write_access", label: "Allow write access", placeholder: "false", type: "boolean" },
  { key: "default_database", label: "Default database (hint)", placeholder: "arhaus_dev" },
];

export function ConnectionWorkspace({ connectionId }: ConnectionWorkspaceProps) {
  const [ds, setDs] = useState<any>(null);
  const [displayName, setDisplayName] = useState("");
  const [dsType, setDsType] = useState("");
  const [config, setConfig] = useState<Record<string, any>>({});
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [showPassword, setShowPassword] = useState(false);
  const [isDefault, setIsDefault] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [cloning, setCloning] = useState(false);
  const select = useWorkspaceStore((s) => s.select);

  const load = useCallback(async () => {
    try {
      setError(null);
      const all = await api.getDataSources();
      const found = all.find((d: any) => d.id === connectionId);
      if (!found) throw new Error("Data source not found");
      setDs(found);
      setDisplayName(found.display_name || "");
      setDsType(found.type || "");
      setIsDefault(Boolean(found.is_default));
      const cfg = typeof found.config === "string" ? JSON.parse(found.config) : found.config || {};
      setConfig(cfg);
    } catch (e: any) {
      setError(e.message || "Failed to load connection");
    }
  }, [connectionId]);

  useEffect(() => {
    setDs(null);
    setSaveMsg(null);
    setTestResult(null);
    load();
  }, [load]);

  const handleSave = async () => {
    setSaving(true);
    setSaveMsg(null);
    try {
      await api.updateDataSource(connectionId, {
        display_name: displayName,
        type: dsType,
        config,
        is_default: isDefault,
      });
      setSaveMsg("Saved");
      setTimeout(() => setSaveMsg(null), 2000);
    } catch (e: any) {
      setSaveMsg("Error: " + (e.message || "Save failed"));
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      // Persist the form first so the test reads what's currently typed, not the
      // previously-saved config. (The /test endpoint reads from the DB.)
      await api.updateDataSource(connectionId, {
        display_name: displayName,
        type: dsType,
        config,
        is_default: isDefault,
      });
      const result = await api.testDataSource(connectionId);
      setTestResult({ ok: true, message: result.message || "Connection successful" });
    } catch (e: any) {
      setTestResult({ ok: false, message: e.message || "Connection failed" });
    } finally {
      setTesting(false);
    }
  };

  const handleClone = async () => {
    // Default to "<source>-copy"; collisions on the server return a 500
    // (UNIQUE constraint), at which point the prompt loops to retry.
    const suggested = `${connectionId}-copy`;
    const newId = window.prompt(
      `Clone "${displayName || connectionId}" into a new connection.\n\nNew connection id:`,
      suggested,
    );
    if (!newId || !newId.trim()) return;
    setCloning(true);
    setSaveMsg(null);
    try {
      // If the form has unsaved edits, persist them first so the clone picks
      // up the latest values (the backend reads from the source DB row, not
      // from the form). The user's edits could otherwise be lost.
      await api.updateDataSource(connectionId, {
        display_name: displayName,
        type: dsType,
        config,
        is_default: isDefault,
      });
      const created = await api.cloneDataSource(connectionId, { id: newId.trim() });
      window.dispatchEvent(new CustomEvent("connections-changed"));
      // Navigate the workspace to the new connection so the user can edit
      // the fields in place.
      select({ type: "connection", id: created.id });
    } catch (e: any) {
      setSaveMsg("Error: " + (e?.message || "Clone failed"));
    } finally {
      setCloning(false);
    }
  };

  const handleDelete = async () => {
    const label = displayName || connectionId;
    if (!window.confirm(`Delete connection "${label}"? This cannot be undone.`)) return;
    setDeleting(true);
    setSaveMsg(null);
    try {
      await api.deleteDataSource(connectionId);
      // Clear workspace selection — falls back to the empty state.
      select(null);
      // Notify the sidebar to refetch its connections list.
      window.dispatchEvent(new CustomEvent("connections-changed"));
    } catch (e: any) {
      setSaveMsg("Error: " + (e.message || "Delete failed"));
      setDeleting(false);
    }
  };

  const updateConfig = (key: string, value: any) => {
    setConfig((prev) => ({ ...prev, [key]: value }));
  };

  const getFields = () => {
    if (dsType === "postgres" || dsType === "pg") return PG_FIELDS;
    if (dsType === "clickhouse") return CH_FIELDS;
    // For other types, derive fields from config keys
    return Object.keys(config).map((key) => ({
      key,
      label: key.charAt(0).toUpperCase() + key.slice(1).replace(/_/g, " "),
      placeholder: "",
      secret: key === "password" || key.includes("secret") || key.includes("token"),
    }));
  };

  if (error) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-center">
          <p className="text-red-400 text-sm">{error}</p>
          <button onClick={load} className="mt-3 text-xs text-blue-400 hover:text-blue-300">Retry</button>
        </div>
      </div>
    );
  }

  if (!ds) {
    return (
      <div className="h-full bg-gray-950 text-gray-100 flex items-center justify-center">
        <div className="text-sm text-gray-500">Loading...</div>
      </div>
    );
  }

  const typeColor = TYPE_COLORS[dsType] || TYPE_COLORS.postgres || "bg-gray-800 text-gray-400";
  const fields = getFields();

  return (
    <div className="h-full bg-gray-950 text-gray-100 flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-5 pt-4 pb-3 shrink-0 border-b border-gray-800">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2 min-w-0">
            <Plug size={18} className="text-green-400 shrink-0" />
            <h1 className="text-lg font-semibold truncate">{displayName || "Connection"}</h1>
            <span className={`text-[10px] px-2 py-0.5 rounded font-medium shrink-0 ${typeColor}`}>
              {dsType || "unknown"}
            </span>
            <button
              onClick={() => setIsDefault((v) => !v)}
              title={isDefault ? `Default for ${dsType}` : `Mark as default for ${dsType}`}
              className={`flex items-center gap-1 text-[10px] px-2 py-0.5 rounded font-medium shrink-0 transition-colors ${
                isDefault
                  ? "bg-yellow-900/40 text-yellow-300 border border-yellow-800"
                  : "bg-gray-800 text-gray-500 border border-gray-700 hover:text-gray-300"
              }`}
            >
              <Star size={10} className={isDefault ? "fill-yellow-300" : ""} />
              {isDefault ? "default" : "make default"}
            </button>
          </div>
          <div className="flex items-center gap-2 shrink-0 ml-4">
            {saveMsg && (
              <span className={`text-xs ${saveMsg.startsWith("Error") ? "text-red-400" : "text-green-400"}`}>
                {saveMsg}
              </span>
            )}
            <button
              onClick={handleSave}
              disabled={saving || deleting || cloning}
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-blue-600 hover:bg-blue-500 disabled:opacity-50 transition-colors"
            >
              {saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
              Save
            </button>
            <button
              onClick={handleClone}
              disabled={saving || deleting || cloning}
              title="Save current edits, then create a new connection with the same config"
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-gray-700 hover:bg-gray-600 disabled:opacity-50 transition-colors"
            >
              {cloning ? <Loader2 size={12} className="animate-spin" /> : <Copy size={12} />}
              Clone
            </button>
            <button
              onClick={handleDelete}
              disabled={saving || deleting || cloning}
              title="Delete this connection"
              className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-red-700 hover:bg-red-600 disabled:opacity-50 transition-colors"
            >
              {deleting ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
              Delete
            </button>
          </div>
        </div>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto px-5 py-4 space-y-6">
        {/* Display Name */}
        <div>
          <label className="block text-xs font-medium text-gray-400 mb-1">Display Name</label>
          <input
            type="text"
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
          />
        </div>

        {/* Config Fields */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-3">Configuration</h2>
          <div className="grid grid-cols-2 gap-4">
            {fields.map((f: any) => (
              <div key={f.key} className={f.key === "host" || f.key === "database" ? "" : ""}>
                <label className="block text-xs font-medium text-gray-400 mb-1">{f.label}</label>
                <div className="relative">
                  {f.type === "boolean" ? (
                    <select
                      value={String(config[f.key] ?? false)}
                      onChange={(e) => updateConfig(f.key, e.target.value === "true")}
                      className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                    >
                      <option value="false">false</option>
                      <option value="true">true</option>
                    </select>
                  ) : (
                    <input
                      type={f.secret && !showPassword ? "password" : f.type || "text"}
                      value={config[f.key] ?? ""}
                      onChange={(e) => updateConfig(f.key, f.type === "number" ? Number(e.target.value) : e.target.value)}
                      className="w-full px-3 py-2 text-sm rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500 pr-8"
                      placeholder={f.placeholder}
                    />
                  )}
                  {f.secret && f.type !== "boolean" && (
                    <button
                      onClick={() => setShowPassword((p) => !p)}
                      className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300"
                    >
                      {showPassword ? <EyeOff size={14} /> : <Eye size={14} />}
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Test Connection */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-3">Test Connection</h2>
          <div className="flex items-center gap-3">
            <button
              onClick={handleTest}
              disabled={testing}
              className="flex items-center gap-1.5 px-4 py-2 text-xs font-medium rounded bg-gray-800 border border-gray-700 hover:bg-gray-700 disabled:opacity-50 transition-colors"
            >
              {testing ? <Loader2 size={13} className="animate-spin" /> : <Zap size={13} />}
              Test Connection
            </button>
            {testResult && (
              <div className={`flex items-center gap-1.5 text-xs ${testResult.ok ? "text-green-400" : "text-red-400"}`}>
                {testResult.ok ? <CheckCircle2 size={14} /> : <XCircle size={14} />}
                <span>{testResult.message}</span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
