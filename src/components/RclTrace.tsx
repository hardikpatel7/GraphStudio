//! Per-product RCL trace card.
//!
//! Used by:
//!   - the "RCL Explorer" panel in `CoreServiceWorkspace` (manual product picker)
//!   - the `InspectorPanel` (when a DataView row's `rcl` link is clicked)
//!
//! Self-fetches via `/api/graph/articles/resolve-rcl`. Renders the
//! matched (rcl_code, rule_code) and resolved payload for each of the
//! three RCL flavors (DC Policy, Constraints, PSM) plus the input
//! hierarchy.

import { useEffect, useState } from "react";
import { Zap } from "lucide-react";
import { api, type ArticleGraphResolveRclResponse } from "@/api/client";

interface Props {
  productCode?: string;
  article?: string;
  /// When true, renders a compact heading suitable for an inspector.
  /// When false (default), renders inline content with no heading.
  withHeader?: boolean;
}

export function RclTrace({ productCode, article, withHeader = false }: Props) {
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<ArticleGraphResolveRclResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!productCode && !article) {
      setResult(null);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    setResult(null);
    const key = productCode ? { product_code: productCode } : { article: article! };
    api
      .articleGraphResolveRcl(key)
      .then((resp) => {
        if (!cancelled) setResult(resp);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [productCode, article]);

  return (
    <div className="space-y-3">
      {withHeader && (
        <div className="flex items-center gap-2">
          <Zap size={14} className="text-yellow-400" />
          <h2 className="text-sm font-semibold text-gray-200">RCL trace</h2>
          {productCode && (
            <span className="text-[11px] font-mono text-gray-500">
              product_code = {productCode}
            </span>
          )}
          {!productCode && article && (
            <span className="text-[11px] font-mono text-gray-500">
              article = {article}
            </span>
          )}
        </div>
      )}

      {loading && (
        <div className="text-xs text-gray-500">Resolving…</div>
      )}

      {error && (
        <div className="rounded bg-red-900/30 border border-red-800 px-3 py-2 text-xs text-red-300 font-mono">
          {error}
        </div>
      )}

      {result && (
        <div className="space-y-3">
          {/* Hierarchy */}
          {result.hierarchy && (
            <details open className="rounded bg-gray-950/50 border border-gray-800">
              <summary className="px-3 py-2 text-xs font-semibold text-gray-300 cursor-pointer hover:bg-gray-900/50">
                Hierarchy
              </summary>
              <div className="px-3 pb-3 text-xs font-mono text-gray-400 space-y-1">
                <div><span className="text-gray-500">article:</span> {result.hierarchy.article}</div>
                <div><span className="text-gray-500">product_code:</span> {result.hierarchy.product_code}</div>
                <div><span className="text-gray-500">l0:</span> {result.hierarchy.l0_name}</div>
                <div><span className="text-gray-500">l1:</span> {result.hierarchy.l1_name}</div>
                <div><span className="text-gray-500">l2:</span> {result.hierarchy.l2_name}</div>
                <div><span className="text-gray-500">l3:</span> {result.hierarchy.l3_name}</div>
                <div><span className="text-gray-500">l4:</span> {result.hierarchy.l4_name}</div>
                <div><span className="text-gray-500">l5:</span> {result.hierarchy.l5_name}</div>
                <div><span className="text-gray-500">brand:</span> {result.hierarchy.brand}</div>
                <div><span className="text-gray-500">channel:</span> {result.hierarchy.channel}</div>
              </div>
            </details>
          )}

          {/* DC Policy */}
          <details open className="rounded bg-gray-950/50 border border-gray-800">
            <summary className="px-3 py-2 text-xs font-semibold text-gray-300 cursor-pointer hover:bg-gray-900/50 flex items-center gap-2">
              DC Policy
              {result.dc_policy ? (
                <span className="text-[10px] px-1.5 py-0.5 rounded bg-green-900/40 text-green-400 font-mono">
                  rcl={result.dc_policy.rcl_code} rule={result.dc_policy.rule_code}
                </span>
              ) : (
                <span className="text-[10px] text-gray-600">no match</span>
              )}
            </summary>
            {result.dc_policy?.policy && (
              <div className="px-3 pb-3 text-xs font-mono text-gray-400 space-y-1">
                <div><span className="text-gray-500">default_store_groups:</span> [{result.dc_policy.policy.default_store_groups.join(", ")}]</div>
                <div><span className="text-gray-500">default_product_profile:</span> {result.dc_policy.policy.default_product_profile || "—"}</div>
                <div><span className="text-gray-500">dc_store_rule:</span> {result.dc_policy.policy.dc_store_rule}</div>
              </div>
            )}
          </details>

          {/* Constraints */}
          <details open className="rounded bg-gray-950/50 border border-gray-800">
            <summary className="px-3 py-2 text-xs font-semibold text-gray-300 cursor-pointer hover:bg-gray-900/50 flex items-center gap-2">
              Constraints
              {result.constraints ? (
                <span className="text-[10px] px-1.5 py-0.5 rounded bg-green-900/40 text-green-400 font-mono">
                  rcl={result.constraints.rcl_code} rule={result.constraints.rule_code}
                </span>
              ) : (
                <span className="text-[10px] text-gray-600">no match</span>
              )}
            </summary>
            {result.constraints && result.constraints.rows.length > 0 && (
              <div className="px-3 pb-3">
                <table className="w-full text-xs font-mono">
                  <thead>
                    <tr className="text-gray-500 border-b border-gray-800">
                      <th className="text-left py-1 px-2">psa_code</th>
                      <th className="text-right py-1 px-2">aps</th>
                      <th className="text-right py-1 px-2">wos</th>
                      <th className="text-right py-1 px-2">min</th>
                      <th className="text-right py-1 px-2">max</th>
                    </tr>
                  </thead>
                  <tbody>
                    {result.constraints.rows.map((r, i) => (
                      <tr key={i} className="text-gray-400">
                        <td className="py-1 px-2">{r.psa_code}</td>
                        <td className="py-1 px-2 text-right">{r.aps}</td>
                        <td className="py-1 px-2 text-right">{r.wos}</td>
                        <td className="py-1 px-2 text-right">{r.min_stock}</td>
                        <td className="py-1 px-2 text-right">{r.max_stock}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </details>

          {/* PSM */}
          <details className="rounded bg-gray-950/50 border border-gray-800">
            <summary className="px-3 py-2 text-xs font-semibold text-gray-300 cursor-pointer hover:bg-gray-900/50 flex items-center gap-2">
              PSM
              {result.psm ? (
                <span className="text-[10px] px-1.5 py-0.5 rounded bg-green-900/40 text-green-400 font-mono">
                  rcl={result.psm.rcl_code} rule={result.psm.rule_code}
                </span>
              ) : (
                <span className="text-[10px] text-gray-600">no match</span>
              )}
            </summary>
          </details>

          <div className="text-[10px] text-gray-600 pt-1">
            ruleset_version: {result.ruleset_version}
          </div>
        </div>
      )}
    </div>
  );
}
