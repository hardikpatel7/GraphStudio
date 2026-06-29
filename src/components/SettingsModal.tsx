import { useState, useEffect } from "react";
import { X, ChevronDown, ChevronRight, Settings, FolderOpen, Globe, FileCode, HardDrive } from "lucide-react";
import { useAppsStore } from "@/stores/apps";

interface Props {
  onClose: () => void;
}

/* ── Reusable field components ── */

function SettingsField({ label, desc, children }: { label: string; desc?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-4 py-2.5">
      <div className="w-44 shrink-0 pt-1">
        <div className="text-xs font-medium text-gray-300">{label}</div>
        {desc && <div className="text-[10px] text-gray-500 mt-0.5">{desc}</div>}
      </div>
      <div className="flex-1">{children}</div>
    </div>
  );
}

function SettingsInput({ value, mono, readOnly = true }: {
  value: string; mono?: boolean; readOnly?: boolean;
}) {
  return (
    <input
      type="text"
      value={value}
      readOnly={readOnly}
      className={`w-full px-3 py-1.5 bg-gray-800 border border-gray-700 rounded-lg text-xs text-gray-200 outline-none ${mono ? "font-mono" : ""} ${readOnly ? "text-gray-400 cursor-default" : ""}`}
    />
  );
}

function SettingsGroup({ title, icon, children, defaultOpen }: {
  title: string; icon: React.ReactNode; children: React.ReactNode; defaultOpen?: boolean;
}) {
  const [open, setOpen] = useState(defaultOpen ?? true);
  return (
    <div className="border border-gray-800 rounded-lg overflow-hidden">
      <button
        onClick={() => setOpen(!open)}
        className="w-full flex items-center gap-2 px-4 py-2.5 text-xs font-medium text-gray-300 hover:bg-gray-800/50 transition-colors"
      >
        {open ? <ChevronDown size={12} className="text-gray-500" /> : <ChevronRight size={12} className="text-gray-500" />}
        {icon}
        <span>{title}</span>
      </button>
      {open && <div className="px-4 pb-3 border-t border-gray-800/50">{children}</div>}
    </div>
  );
}

/* ── Main Modal ── */

export function SettingsModal({ onClose }: Props) {
  const { identity, fetchIdentity } = useAppsStore();

  useEffect(() => {
    if (!identity) fetchIdentity();
  }, [identity, fetchIdentity]);

  const tenantId = identity?.id || "(unknown)";
  const displayName = identity?.display_name || "";
  const client = identity?.client || "";
  const env = identity?.environment || "";
  const appType = identity?.app_type || "";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="fixed inset-0 bg-black/50 backdrop-blur-sm" onClick={onClose} />
      <div className="relative bg-gray-900 rounded-xl shadow-2xl border border-gray-700 w-[640px] max-h-[85vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-gray-800 shrink-0">
          <div>
            <h2 className="text-base font-semibold text-gray-100">Settings</h2>
            <span className="text-[10px] text-gray-500 font-mono">{tenantId}</span>
          </div>
          <div className="flex items-center gap-2">
            <button onClick={onClose} className="p-1.5 text-gray-400 hover:text-gray-200 rounded-lg hover:bg-gray-800 transition-colors">
              <X size={18} />
            </button>
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-auto px-5 py-4 space-y-4">
          <p className="text-[11px] text-gray-500 italic">
            Tenant identity is provided by the server config. To change paths, gRPC, or codegen settings,
            edit the TOML config files for this tenant.
          </p>

          {/* Tenant Identity */}
          <SettingsGroup title="Tenant Identity" icon={<Settings size={12} className="text-blue-400" />}>
            <SettingsField label="Display Name" desc="Human-readable tenant name">
              <SettingsInput value={displayName} />
            </SettingsField>
            <SettingsField label="Client" desc="Client name">
              <SettingsInput value={client} />
            </SettingsField>
            <SettingsField label="App Type" desc="Application type">
              <SettingsInput value={appType} />
            </SettingsField>
            <SettingsField label="Environment" desc="Deployment environment">
              <SettingsInput value={env} mono />
            </SettingsField>
            <SettingsField label="Tenant ID" desc="Composed identifier">
              <SettingsInput value={tenantId} mono />
            </SettingsField>
          </SettingsGroup>

          {/* Paths placeholder */}
          <SettingsGroup title="Paths" icon={<FolderOpen size={12} className="text-amber-400" />} defaultOpen={false}>
            <p className="text-[11px] text-gray-500 py-2">
              Filesystem paths are configured in the tenant TOML files.
            </p>
          </SettingsGroup>

          {/* gRPC Service placeholder */}
          <SettingsGroup title="gRPC Service" icon={<Globe size={12} className="text-green-400" />} defaultOpen={false}>
            <p className="text-[11px] text-gray-500 py-2">
              gRPC service config is loaded from the tenant TOML files.
            </p>
          </SettingsGroup>

          {/* Code Generation placeholder */}
          <SettingsGroup title="Code Generation" icon={<FileCode size={12} className="text-purple-400" />} defaultOpen={false}>
            <p className="text-[11px] text-gray-500 py-2">
              Code generation defaults are configured in the language pack and tenant TOML files.
            </p>
          </SettingsGroup>

          {/* GCP placeholder */}
          <SettingsGroup title="GCP / Cloud" icon={<HardDrive size={12} className="text-cyan-400" />} defaultOpen={false}>
            <p className="text-[11px] text-gray-500 py-2">
              GCP project, GCS bucket, and BigQuery dataset are configured in the tenant TOML files.
            </p>
          </SettingsGroup>
        </div>
      </div>
    </div>
  );
}
