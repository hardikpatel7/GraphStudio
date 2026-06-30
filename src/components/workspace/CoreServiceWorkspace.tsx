import { useState } from "react";
import { Server, Cpu, ArrowRightLeft, Database, Terminal, ChevronRight, Zap } from "lucide-react";
import { RclTrace } from "@/components/RclTrace";

interface CoreServiceWorkspaceProps {
  serviceId: string;
}

interface RpcDef {
  name: string;
  description: string;
  input: string;
  output: string;
}

const RCL_RPCS: RpcDef[] = [
  {
    name: "ResolveStoreEligibility",
    description: "Determines which dark stores are eligible to fulfil orders for a given SKU, based on delivery type, stocking rules, and zone coverage.",
    input: "StoreEligibilityRequest { sku_codes, dark_store_ids, delivery_type, zone_id }",
    output: "StoreEligibilityResponse { eligibility: [{ sku_code, dark_store_id, eligible, reason }] }",
  },
  {
    name: "ResolveReplenishmentRules",
    description: "Resolves replenishment configuration for a SKU × dark store pair — min/max stock levels, reorder quantity, and lead time from the assigned warehouse.",
    input: "ReplenishmentRequest { sku_codes, dark_store_ids, include_inactive, as_of_date }",
    output: "ReplenishmentResponse { rules: [{ sku_code, dark_store_id, min_stock, max_stock, reorder_qty, lead_time_hours }] }",
  },
  {
    name: "ResolveDeliveryConstraints",
    description: "Applies fulfilment constraints: cold-chain requirements, delivery unit rounding, maximum order quantity, and service zone restrictions.",
    input: "DeliveryConstraintRequest { allocations, constraint_set_id, zone_overrides }",
    output: "DeliveryConstraintResponse { constrained: [{ sku_code, dark_store_id, qty, applied_rules }] }",
  },
];

const CROSS_FILTER_DESC = "The cross-filter service coordinates filter state across multiple DataViews within a module. When a user applies a filter in one DataView, the cross-filter service propagates relevant filter conditions to related DataViews, maintaining consistent dimension slicing across the UI. It operates via a pub-sub model where DataViews subscribe to filter channels and receive updates when upstream filters change.";

const SERVICE_CONFIGS: Record<string, { parquetSource: string; cacheStrategy: string }> = {
  "rcl-resolution": {
    parquetSource: "gs://smartstudio-data/{tenant}/rcl/*.parquet",
    cacheStrategy: "LRU with 15-minute TTL, keyed by (product_set_hash, store_set_hash)",
  },
  "cross-filter": {
    parquetSource: "N/A (operates on dimension metadata)",
    cacheStrategy: "In-memory filter graph, rebuilt on dimension config change",
  },
};

export function CoreServiceWorkspace({ serviceId }: CoreServiceWorkspaceProps) {
  const [testEndpoint, setTestEndpoint] = useState("");
  const isRcl = serviceId === "rcl-resolution";
  const isCrossFilter = serviceId === "cross-filter";
  const svcConfig = SERVICE_CONFIGS[serviceId];

  return (
    <div className="h-full bg-gray-950 text-gray-100 flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-5 pt-4 pb-3 shrink-0 border-b border-gray-800">
        <div className="flex items-center gap-2">
          <Server size={18} className="text-indigo-400 shrink-0" />
          <h1 className="text-lg font-semibold">{serviceId}</h1>
          <span className="text-[10px] px-2 py-0.5 rounded bg-indigo-900/50 text-indigo-400 font-medium">
            Core Service
          </span>
        </div>
        <p className="text-xs text-gray-500 mt-1">
          {isRcl && "Configurations service gRPC service for fulfilment configuration evaluation"}
          {isCrossFilter && "Cross-filter coordination service for multi-DataView filter sync"}
          {!isRcl && !isCrossFilter && `Core service: ${serviceId}`}
        </p>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto px-5 py-4 space-y-6">
        {/* RPCs for RCL */}
        {isRcl && (
          <div>
            <h2 className="text-sm font-semibold text-gray-300 mb-3">RPCs</h2>
            <div className="space-y-3">
              {RCL_RPCS.map((rpc) => (
                <div key={rpc.name} className="rounded border border-gray-800 bg-gray-900/30 p-4">
                  <div className="flex items-center gap-2 mb-2">
                    <Cpu size={14} className="text-indigo-400" />
                    <span className="text-sm font-semibold text-gray-200">{rpc.name}</span>
                  </div>
                  <p className="text-xs text-gray-400 mb-3">{rpc.description}</p>
                  <div className="space-y-2">
                    <div className="flex items-start gap-2">
                      <ArrowRightLeft size={12} className="text-green-400 mt-0.5 shrink-0" />
                      <div>
                        <span className="text-[10px] text-gray-500 uppercase tracking-wider">Input</span>
                        <p className="text-xs font-mono text-gray-300 mt-0.5">{rpc.input}</p>
                      </div>
                    </div>
                    <div className="flex items-start gap-2">
                      <ChevronRight size={12} className="text-blue-400 mt-0.5 shrink-0" />
                      <div>
                        <span className="text-[10px] text-gray-500 uppercase tracking-wider">Output</span>
                        <p className="text-xs font-mono text-gray-300 mt-0.5">{rpc.output}</p>
                      </div>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Cross-filter description */}
        {isCrossFilter && (
          <div>
            <h2 className="text-sm font-semibold text-gray-300 mb-3">Service Description</h2>
            <div className="rounded border border-gray-800 bg-gray-900/30 p-4">
              <p className="text-xs text-gray-400 leading-relaxed">{CROSS_FILTER_DESC}</p>
            </div>
          </div>
        )}

        {/* Generic fallback */}
        {!isRcl && !isCrossFilter && (
          <div>
            <h2 className="text-sm font-semibold text-gray-300 mb-3">Service Details</h2>
            <div className="rounded border border-gray-800 bg-gray-900/30 p-4">
              <p className="text-xs text-gray-500 italic">
                This core service is not yet fully configured. Service definitions and RPCs will appear here once implemented.
              </p>
            </div>
          </div>
        )}

        {/* Config Section */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-3">Config</h2>
          <div className="rounded border border-gray-800 bg-gray-900/30 overflow-hidden">
            <div className="flex items-center gap-3 px-4 py-3 border-b border-gray-800">
              <Database size={13} className="text-gray-500" />
              <div>
                <span className="text-[10px] text-gray-500 uppercase tracking-wider block">Parquet Source</span>
                <span className="text-xs font-mono text-gray-300">
                  {svcConfig?.parquetSource || "Not configured"}
                </span>
              </div>
            </div>
            <div className="flex items-center gap-3 px-4 py-3">
              <Cpu size={13} className="text-gray-500" />
              <div>
                <span className="text-[10px] text-gray-500 uppercase tracking-wider block">Cache Strategy</span>
                <span className="text-xs text-gray-300">
                  {svcConfig?.cacheStrategy || "Default"}
                </span>
              </div>
            </div>
          </div>
        </div>

        {/* Fulfillment Config Explorer — only for the rcl-resolution service.
            Configuration trace: pick a SKU, see which
            fulfillment configurations matched and the resolved payload (constraint rows
            with min/max, dark_store_ids, etc.). */}
        {isRcl && <RclExplorerPanel />}

        {/* Test Section */}
        <div>
          <h2 className="text-sm font-semibold text-gray-300 mb-3">Test</h2>
          <div className="rounded border border-gray-800 bg-gray-900/30 p-4">
            <label className="block text-xs font-medium text-gray-400 mb-1">REST Endpoint</label>
            <div className="flex items-center gap-2">
              <div className="flex items-center gap-1 px-2 py-1.5 rounded bg-gray-800 border border-gray-700 text-xs text-gray-500 shrink-0">
                <Terminal size={12} />
                POST
              </div>
              <input
                type="text"
                value={testEndpoint}
                onChange={(e) => setTestEndpoint(e.target.value)}
                className="flex-1 px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500 font-mono"
                placeholder={`http://localhost:50051/${serviceId}/test`}
              />
              <button className="px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-400 hover:bg-gray-700 hover:text-gray-200 transition-colors">
                Send
              </button>
            </div>
            <p className="text-[11px] text-gray-600 mt-2">
              Enter the gRPC-gateway REST endpoint to test this service. Requires the service to be running.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}

/// Fulfillment Config Explorer panel. Manual SKU picker; on Resolve, mounts a
/// shared <RclTrace /> that self-fetches and renders the matched
/// (rcl_code, rule_code) + payload for each resolution flavor.
function RclExplorerPanel() {
  const [keyType, setKeyType] = useState<"sku_code" | "dark_store_id">("sku_code");
  const [keyValue, setKeyValue] = useState<string>("BB-SKU-001001");
  const [submitted, setSubmitted] = useState<{ product_code?: string; article?: string } | null>(
    null,
  );

  const onResolve = () => {
    const v = keyValue.trim();
    if (!v) return;
    setSubmitted(keyType === "sku_code" ? { product_code: v } : { article: v });
  };

  return (
    <div>
      <div className="flex items-center gap-2 mb-3">
        <Zap size={14} className="text-yellow-400" />
        <h2 className="text-sm font-semibold text-gray-300">Fulfillment Config Explorer</h2>
        <span className="text-[10px] px-1.5 py-0.5 rounded bg-yellow-900/30 text-yellow-400">live</span>
      </div>
      <div className="rounded border border-gray-800 bg-gray-900/30 p-4 space-y-3">
        <div className="flex items-center gap-2">
          <select
            value={keyType}
            onChange={(e) => setKeyType(e.target.value as "sku_code" | "dark_store_id")}
            className="px-2 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-300 focus:outline-none focus:border-blue-500"
          >
            <option value="sku_code">sku_code</option>
            <option value="dark_store_id">dark_store_id</option>
          </select>
          <input
            type="text"
            value={keyValue}
            onChange={(e) => setKeyValue(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && keyValue.trim() && onResolve()}
            className="flex-1 px-3 py-1.5 text-xs rounded bg-gray-800 border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500 font-mono"
            placeholder="BB-SKU-001001"
          />
          <button
            onClick={onResolve}
            disabled={!keyValue.trim()}
            className="px-3 py-1.5 text-xs rounded bg-indigo-700 text-white hover:bg-indigo-600 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            Resolve
          </button>
        </div>

        {submitted && (
          <div className="mt-2">
            <RclTrace
              productCode={submitted.product_code}
              article={submitted.article}
            />
          </div>
        )}
      </div>
    </div>
  );
}
