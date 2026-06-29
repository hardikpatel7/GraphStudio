// Chart components rendered inside assistant messages.
//
// The model emits a fenced block:
//
//   ```chart
//   { "type": "bar", "title": "...", "data": [{"label":"...","value":1234}, ...] }
//   ```
//
// The markdown override in App.tsx intercepts blocks tagged
// `language-chart`, parses the JSON, and dispatches here. Bad input
// renders a small red error card inline rather than blowing up the message.
//
// SVG-only on purpose — adding a real chart library would double the
// bundle. These cover the four shapes that matter for retail planning
// answers (single number, ranked categories, time series, composition).

import type React from "react";

type CategoryDatum = { label: string; value: number };
type TimeDatum = { x: string | number; y: number };
type StackedDatum = { label: string; values: number[] };
type BulletDatum = { label: string; value: number; target?: number; ranges?: number[] };
type ChartSpec =
  | { type: "kpi";         label?: string; value: number | string; hint?: string; delta?: number; sparkline?: number[] }
  | { type: "bar";         title?: string; data: CategoryDatum[] }
  | { type: "line";        title?: string; x_label?: string; y_label?: string; data: TimeDatum[] }
  | { type: "pie";         title?: string; data: CategoryDatum[] }
  | { type: "stacked_bar"; title?: string; series: string[]; data: StackedDatum[] }
  | { type: "bullet";      title?: string; data: BulletDatum[] }
  | { type: "pareto";      title?: string; data: CategoryDatum[] }
  | { type: "funnel";      title?: string; data: CategoryDatum[] }
  | { type: "gauge";       title?: string; label?: string; value: number; target: number; unit?: string; thresholds?: [number, number] }
  | { type: "sparkline";   title?: string; data: number[] }
  | { type: "heatmap";     title?: string; x_labels: string[]; y_labels: string[]; data: number[][] }
  | { type: "treemap";     title?: string; data: Array<{ label: string; value: number; children?: Array<{ label: string; value: number }> }> }
  | { type: "histogram";   title?: string; x_label?: string; y_label?: string; data: number[]; bin_labels?: string[] }
  | { type: "slope";       title?: string; from_label?: string; to_label?: string; data: Array<{ label: string; from: number; to: number }> }
  | { type: "boxplot";     title?: string; data: Array<{ label: string; min: number; q1: number; median: number; q3: number; max: number }> }
  | { type: "waterfall";   title?: string; data: Array<{ label: string; value: number; total?: boolean }> }
  | { type: string; [k: string]: unknown };

// Series palette. Picked to match the theme accents and stay legible
// against the white card background. Re-used cyclically for >8 series.
const PALETTE = [
  "#6366f1", // indigo
  "#0ea5e9", // sky
  "#10b981", // emerald
  "#f59e0b", // amber
  "#f43f5e", // rose
  "#a855f7", // purple
  "#06b6d4", // cyan
  "#84cc16", // lime
];

export function ChartBlock({ raw, onPick }: { raw: string; onPick?: (label: string) => void }) {
  let spec: ChartSpec | null = null;
  try {
    spec = JSON.parse(raw) as ChartSpec;
  } catch (e) {
    return <ChartError msg="Couldn't parse chart spec as JSON" detail={(e as Error).message} raw={raw} />;
  }
  if (!spec || typeof spec !== "object" || typeof spec.type !== "string") {
    return <ChartError msg='Chart spec must be {"type": "kpi" | "bar" | "line" | "pie", ...}' raw={raw} />;
  }
  switch (spec.type) {
    case "kpi":         return <Kpi         spec={spec as Extract<ChartSpec, { type: "kpi" }>} />;
    case "bar":         return <Bar         spec={spec as Extract<ChartSpec, { type: "bar" }>} onPick={onPick} />;
    case "line":        return <Line        spec={spec as Extract<ChartSpec, { type: "line" }>} />;
    case "pie":         return <Pie         spec={spec as Extract<ChartSpec, { type: "pie" }>} onPick={onPick} />;
    case "stacked_bar": return <StackedBar  spec={spec as Extract<ChartSpec, { type: "stacked_bar" }>} onPick={onPick} />;
    case "bullet":      return <Bullet      spec={spec as Extract<ChartSpec, { type: "bullet" }>} />;
    case "pareto":      return <Pareto      spec={spec as Extract<ChartSpec, { type: "pareto" }>} onPick={onPick} />;
    case "funnel":      return <Funnel      spec={spec as Extract<ChartSpec, { type: "funnel" }>} onPick={onPick} />;
    case "gauge":       return <Gauge       spec={spec as Extract<ChartSpec, { type: "gauge" }>} />;
    case "sparkline":   return <Sparkline   spec={spec as Extract<ChartSpec, { type: "sparkline" }>} />;
    case "heatmap":     return <Heatmap     spec={spec as Extract<ChartSpec, { type: "heatmap" }>} onPick={onPick} />;
    case "treemap":     return <Treemap     spec={spec as Extract<ChartSpec, { type: "treemap" }>} onPick={onPick} />;
    case "histogram":   return <Histogram   spec={spec as Extract<ChartSpec, { type: "histogram" }>} />;
    case "slope":       return <Slope       spec={spec as Extract<ChartSpec, { type: "slope" }>} onPick={onPick} />;
    case "boxplot":     return <BoxPlot     spec={spec as Extract<ChartSpec, { type: "boxplot" }>} onPick={onPick} />;
    case "waterfall":   return <Waterfall   spec={spec as Extract<ChartSpec, { type: "waterfall" }>} />;
    default:            return <ChartError msg={`Unknown chart type "${String(spec.type)}"`} raw={raw} />;
  }
}

function ChartError({ msg, detail, raw }: { msg: string; detail?: string; raw: string }) {
  return (
    <div className="rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs text-rose-700 my-2">
      <div className="font-medium">Chart render error</div>
      <div className="mt-1">{msg}</div>
      {detail && <div className="mt-0.5 font-mono opacity-70">{detail}</div>}
      <details className="mt-2">
        <summary className="cursor-pointer text-rose-600 hover:text-rose-800">show spec</summary>
        <pre className="mt-1 font-mono text-[11px] whitespace-pre-wrap break-all opacity-80">{raw}</pre>
      </details>
    </div>
  );
}

function Kpi({ spec }: { spec: Extract<ChartSpec, { type: "kpi" }> }) {
  const value = spec.value ?? 0;
  const isUp = (spec.delta ?? 0) >= 0;
  // Optional inline trend. Tucked beside the value so the KPI stays
  // compact; only renders when the model supplies a sparkline array.
  const spark = Array.isArray(spec.sparkline)
    ? spec.sparkline.filter((n) => typeof n === "number")
    : [];
  // Fill the widget body via `h-full w-full flex flex-col`. The KPI
  // used to be `inline-block` which sized to content — in a row of
  // 5 KPIs, the ones with hints / sparklines were taller than the
  // bare ones, and the row showed jagged tops/bottoms. Filling the
  // body lets the row's items-stretch make every slot identical.
  return (
    <div className="rounded-xl border border-slate-200 bg-gradient-to-br from-indigo-50 via-white to-blue-50 p-4 h-full w-full flex flex-col">
      {spec.label && (
        <div className="text-[11px] uppercase tracking-wider text-slate-500 font-medium">{spec.label}</div>
      )}
      <div className="mt-0.5 flex items-end gap-3">
        <div className="text-3xl font-semibold text-slate-900 tabular-nums leading-none">
          {formatVal(value)}
        </div>
        {spark.length >= 2 && <MiniSparkline values={spark} width={88} height={28} />}
      </div>
      {spec.delta != null && (
        <div className={`mt-1.5 text-xs font-medium ${isUp ? "text-emerald-600" : "text-rose-600"}`}>
          {isUp ? "▲" : "▼"} {formatVal(Math.abs(spec.delta))}
        </div>
      )}
      {spec.hint && <div className="mt-1 text-xs text-slate-500">{spec.hint}</div>}
    </div>
  );
}

// ── Gauge ───────────────────────────────────────────────────────────────
//
// Half-arc gauge for "value vs target" reads. The arc spans 180°
// from -90° (left) to +90° (right). Foreground arc length is
// proportional to value/target (clamped to [0, 1.2] so a 20% over-
// target doesn't blow out the visual). Color follows optional
// `thresholds: [low, high]`: <low = red, [low, high] = amber,
// >=high = green; defaults to thresholds at 50% and 80% of target.
function Gauge({ spec }: { spec: Extract<ChartSpec, { type: "gauge" }> }) {
  if (typeof spec.value !== "number" || typeof spec.target !== "number" || spec.target <= 0) {
    return <ChartError msg="gauge requires numeric `value` and positive `target`" raw={JSON.stringify(spec)} />;
  }
  const unit = spec.unit ?? "";
  const [lo, hi] = Array.isArray(spec.thresholds) && spec.thresholds.length === 2
    ? spec.thresholds as [number, number]
    : [spec.target * 0.5, spec.target * 0.8];
  const color = spec.value < lo ? "#f43f5e" : spec.value < hi ? "#f59e0b" : "#10b981";
  const ratio = Math.max(0, Math.min(spec.value / spec.target, 1.2));
  // SVG layout
  const W = 200, H = 130;
  const CX = W / 2, CY = H - 18;
  const R = 78;
  const STROKE = 14;
  // Half arc from 180° to 360° (i.e. -X axis to +X axis, sweeping over the top).
  const arcPath = (fromAngle: number, toAngle: number) => {
    const p = (a: number) => [CX + R * Math.cos((a * Math.PI) / 180), CY + R * Math.sin((a * Math.PI) / 180)];
    const [x1, y1] = p(fromAngle);
    const [x2, y2] = p(toAngle);
    const large = Math.abs(toAngle - fromAngle) > 180 ? 1 : 0;
    return `M ${x1} ${y1} A ${R} ${R} 0 ${large} 1 ${x2} ${y2}`;
  };
  const fromAngle = 180;
  const toAngle = 180 + ratio * 180;
  const pct = ((spec.value / spec.target) * 100).toFixed(1);
  const gaugeTooltip = `${spec.label ?? "value"}: ${formatVal(spec.value)}${unit} of target ${formatVal(spec.target)}${unit} (${pct}%)`;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 flex flex-col items-center">
      {spec.title && <div className="text-sm font-medium text-slate-800 self-start mb-1">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        <title>{gaugeTooltip}</title>
        {/* background arc */}
        <path d={arcPath(180, 360)} stroke="#e2e8f0" strokeWidth={STROKE} fill="none" strokeLinecap="round">
          <title>{`target ${formatVal(spec.target)}${unit}`}</title>
        </path>
        {/* foreground arc */}
        <path d={arcPath(fromAngle, toAngle)} stroke={color} strokeWidth={STROKE} fill="none" strokeLinecap="round">
          <title>{gaugeTooltip}</title>
        </path>
        {/* target tick */}
        {(() => {
          const tickA = 180 + 180; // target maps to 100%; mark on the outer rim
          const inner = R - STROKE / 2 - 2;
          const outer = R + STROKE / 2 + 2;
          const ax = CX + inner * Math.cos((tickA * Math.PI) / 180);
          const ay = CY + inner * Math.sin((tickA * Math.PI) / 180);
          const bx = CX + outer * Math.cos((tickA * Math.PI) / 180);
          const by = CY + outer * Math.sin((tickA * Math.PI) / 180);
          return (
            <line x1={ax} y1={ay} x2={bx} y2={by} stroke="#475569" strokeWidth={2}>
              <title>{`target marker: ${formatVal(spec.target)}${unit}`}</title>
            </line>
          );
        })()}
        <text x={CX} y={CY - 18} textAnchor="middle" fontSize="22" fontWeight={600} fill="#0f172a" className="tabular-nums">
          {formatVal(spec.value)}{unit}
        </text>
        <text x={CX} y={CY - 2} textAnchor="middle" fontSize="10" fill="#64748b">
          target {formatVal(spec.target)}{unit}
        </text>
      </svg>
      {spec.label && <div className="text-xs text-slate-500 mt-0.5">{spec.label}</div>}
    </div>
  );
}

// ── Sparkline ───────────────────────────────────────────────────────────
//
// Standalone trend tile. Same shape as `MiniSparkline` below but with
// title, taller, and the latest value labeled.
function Sparkline({ spec }: { spec: Extract<ChartSpec, { type: "sparkline" }> }) {
  const data = (spec.data ?? []).filter((n) => typeof n === "number");
  if (data.length < 2) {
    return <ChartError msg="sparkline.data needs at least 2 numbers" raw={JSON.stringify(spec)} />;
  }
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <div className="flex items-end gap-3">
        <div className="text-xl font-semibold text-slate-900 tabular-nums leading-none">
          {formatVal(data[data.length - 1])}
        </div>
        <MiniSparkline values={data} width={220} height={48} />
      </div>
    </div>
  );
}

// Tiny line-plus-area sparkline used both inline in the KPI card and
// as the body of the standalone Sparkline widget. No axes, no labels.
function MiniSparkline({ values, width, height }: { values: number[]; width: number; height: number }) {
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  const PAD = 2;
  const innerW = width - PAD * 2;
  const innerH = height - PAD * 2;
  const xToPx = (i: number) => PAD + (i / Math.max(values.length - 1, 1)) * innerW;
  const yToPx = (y: number) => PAD + innerH - ((y - min) / span) * innerH;
  const pts = values.map((v, i) => `${xToPx(i)},${yToPx(v)}`).join(" ");
  const lastX = xToPx(values.length - 1);
  const area = `M ${xToPx(0)} ${PAD + innerH} L ${pts.split(" ").join(" L ")} L ${lastX} ${PAD + innerH} Z`;
  const trend = values[values.length - 1] - values[0];
  const trendStr = trend >= 0 ? `+${formatVal(trend)}` : formatVal(trend);
  const seriesTooltip = `${values.length} points · ${formatVal(min)} → ${formatVal(max)} · latest ${formatVal(values[values.length - 1])} (Δ ${trendStr})`;
  return (
    <svg width={width} height={height} className="block">
      <title>{seriesTooltip}</title>
      <path d={area} fill="#6366f1" opacity={0.12} />
      <polyline fill="none" stroke="#6366f1" strokeWidth={1.6} points={pts} />
      {values.map((v, i) => (
        <circle key={i} cx={xToPx(i)} cy={yToPx(v)} r={i === values.length - 1 ? 2.2 : 1.6} fill="#6366f1" opacity={i === values.length - 1 ? 1 : 0}>
          <title>{`#${i + 1}: ${formatVal(v)}`}</title>
        </circle>
      ))}
    </svg>
  );
}

function Bar({ spec, onPick }: { spec: Extract<ChartSpec, { type: "bar" }>; onPick?: (label: string) => void }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string");
  if (data.length === 0) {
    return <ChartError msg="bar.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  const max = Math.max(...data.map((d) => d.value), 1);
  const clickable = !!onPick;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <div className="space-y-1.5">
        {data.map((d, i) => {
          const pct = Math.max(0, (d.value / max) * 100);
          const color = PALETTE[i % PALETTE.length];
          const rowTip = `${d.label}: ${formatVal(d.value)}`;
          const inner = (
            <>
              <div className="text-[11px] text-slate-600 w-32 flex-shrink-0 truncate text-left" title={rowTip}>
                {d.label}
              </div>
              <div className="flex-1 h-5 bg-slate-100/60 rounded-md relative overflow-hidden" title={rowTip}>
                <div
                  className="h-full rounded-md transition-all"
                  style={{ width: `${pct}%`, backgroundColor: color, opacity: 0.85 }}
                  title={rowTip}
                />
              </div>
              <div className="text-[11px] text-slate-700 tabular-nums w-16 flex-shrink-0 text-right font-medium" title={rowTip}>
                {formatVal(d.value)}
              </div>
            </>
          );
          return clickable ? (
            <button
              key={i}
              type="button"
              onClick={() => onPick!(d.label)}
              className="flex items-center gap-2 w-full text-left rounded-md hover:bg-indigo-50/60 px-1 -mx-1 transition cursor-pointer"
              title={`${rowTip} — click to drill in`}
            >
              {inner}
            </button>
          ) : (
            <div key={i} className="flex items-center gap-2" title={rowTip}>
              {inner}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Line({ spec }: { spec: Extract<ChartSpec, { type: "line" }> }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.y === "number");
  if (data.length === 0) {
    return <ChartError msg="line.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }

  // Fixed canvas so the chart fits the bubble without flex math; the
  // wrapper allows horizontal scroll when there are many x values that
  // would otherwise collide.
  const W = 520;
  const H = 180;
  const PAD = { l: 36, r: 14, t: 18, b: 22 };
  const ys = data.map((d) => d.y);
  const minY = Math.min(...ys, 0);
  const maxY = Math.max(...ys, 1);
  const span = maxY - minY || 1;
  const innerW = W - PAD.l - PAD.r;
  const innerH = H - PAD.t - PAD.b;
  const xToPx = (i: number) => PAD.l + (i / Math.max(data.length - 1, 1)) * innerW;
  const yToPx = (y: number) => PAD.t + innerH - ((y - minY) / span) * innerH;

  // Smooth-ish polyline plus dots. Area fill underneath keeps the trend
  // visible even when the line is thin.
  const pts = data.map((d, i) => `${xToPx(i)},${yToPx(d.y)}`).join(" ");
  const area = `M ${xToPx(0)} ${yToPx(minY)} L ${pts.split(" ").join(" L ")} L ${xToPx(data.length - 1)} ${yToPx(minY)} Z`;

  // Gridlines at 0%, 50%, 100% of the y-range.
  const gridY = [minY, minY + span / 2, maxY];

  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        {gridY.map((v, i) => (
          <g key={i}>
            <line x1={PAD.l} x2={W - PAD.r} y1={yToPx(v)} y2={yToPx(v)} stroke="#e2e8f0" strokeWidth={1} />
            <text x={PAD.l - 4} y={yToPx(v) + 3} fontSize="10" fill="#94a3b8" textAnchor="end" className="tabular-nums">
              {formatVal(v)}
            </text>
          </g>
        ))}
        <path d={area} fill="#6366f1" opacity={0.08} />
        <polyline fill="none" stroke="#6366f1" strokeWidth={2} points={pts} />
        {data.map((d, i) => (
          <circle key={i} cx={xToPx(i)} cy={yToPx(d.y)} r={2.5} fill="#6366f1">
            <title>{`${d.x}: ${formatVal(d.y)}`}</title>
          </circle>
        ))}
        <text x={PAD.l}     y={H - 4} fontSize="10" fill="#94a3b8">{String(data[0].x)}</text>
        <text x={W - PAD.r} y={H - 4} fontSize="10" fill="#94a3b8" textAnchor="end">{String(data[data.length - 1].x)}</text>
      </svg>
    </div>
  );
}

function Pie({ spec, onPick }: { spec: Extract<ChartSpec, { type: "pie" }>; onPick?: (label: string) => void }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.value === "number");
  if (data.length === 0) {
    return <ChartError msg="pie.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  const total = data.reduce((s, d) => s + d.value, 0) || 1;
  const R_OUTER = 60;
  const R_INNER = 36; // donut hole
  const SIZE = 140;
  const CX = SIZE / 2;
  const CY = SIZE / 2;
  let acc = 0;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 flex items-center gap-4">
      <svg width={SIZE} height={SIZE} className="flex-shrink-0">
        {data.map((d, i) => {
          const startA = (acc / total) * 2 * Math.PI;
          acc += d.value;
          const endA = (acc / total) * 2 * Math.PI;
          const path = donutSlice(CX, CY, R_OUTER, R_INNER, startA, endA);
          return <path
            key={i}
            d={path}
            fill={PALETTE[i % PALETTE.length]}
            style={onPick ? { cursor: "pointer" } : undefined}
            onClick={onPick ? () => onPick(d.label) : undefined}
          >
            <title>{`${d.label}: ${formatVal(d.value)} (${((d.value / total) * 100).toFixed(1)}%)`}</title>
          </path>;
        })}
      </svg>
      <div className="flex-1 min-w-0">
        {spec.title && <div className="text-sm font-medium text-slate-800 mb-1.5">{spec.title}</div>}
        <div className="space-y-1">
          {data.map((d, i) => {
            const pct = (d.value / total) * 100;
            return (
              <div key={i} className="flex items-center gap-2 text-xs">
                <span className="w-3 h-3 rounded-sm flex-shrink-0" style={{ backgroundColor: PALETTE[i % PALETTE.length] }} />
                <span className="text-slate-700 flex-1 truncate">{d.label}</span>
                <span className="text-slate-600 tabular-nums font-medium">{formatVal(d.value)}</span>
                <span className="text-slate-400 tabular-nums w-10 text-right">{pct.toFixed(0)}%</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

// ── Stacked bar ─────────────────────────────────────────────────────────
//
// One horizontal row per data entry. Each row's `values` array
// (parallel to `series`) is rendered as adjacent colored segments.
// Segment widths are proportional to that row's total; rows are
// normalized to the widest row so visual length comparisons match
// the absolute totals across rows.
function StackedBar({
  spec, onPick,
}: {
  spec: Extract<ChartSpec, { type: "stacked_bar" }>;
  onPick?: (label: string) => void;
}) {
  const series = Array.isArray(spec.series) ? spec.series : [];
  const data = (spec.data ?? []).filter(
    (d) => d && typeof d.label === "string" && Array.isArray(d.values),
  );
  if (data.length === 0 || series.length === 0) {
    return <ChartError msg="stacked_bar requires `series` and `data` arrays" raw={JSON.stringify(spec)} />;
  }
  const totals = data.map((d) => d.values.reduce((s, v) => s + (typeof v === "number" ? v : 0), 0));
  const maxTotal = Math.max(...totals, 1);
  const clickable = !!onPick;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <div className="flex flex-wrap gap-x-3 gap-y-1 mb-2 text-[11px]">
        {series.map((s, i) => (
          <span key={s} className="inline-flex items-center gap-1">
            <span className="inline-block w-2.5 h-2.5 rounded-sm" style={{ backgroundColor: PALETTE[i % PALETTE.length] }} />
            <span className="text-slate-600">{s}</span>
          </span>
        ))}
      </div>
      <div className="space-y-1.5">
        {data.map((d, i) => {
          const rowTotal = totals[i] || 0;
          const widthPct = (rowTotal / maxTotal) * 100;
          const inner = (
            <>
              <div className="text-[11px] text-slate-600 w-32 flex-shrink-0 truncate text-left" title={d.label}>{d.label}</div>
              <div className="flex-1 h-5 bg-slate-100/60 rounded-md overflow-hidden flex" style={{ maxWidth: `${widthPct}%`, minWidth: rowTotal > 0 ? "12px" : "0" }}>
                {d.values.map((v, j) => {
                  const seg = (v / Math.max(rowTotal, 1)) * 100;
                  return (
                    <div
                      key={j}
                      style={{ width: `${seg}%`, backgroundColor: PALETTE[j % PALETTE.length], opacity: 0.85 }}
                      title={`${series[j] ?? `series ${j}`}: ${formatVal(v)}`}
                    />
                  );
                })}
              </div>
              <div className="text-[11px] text-slate-700 tabular-nums w-16 flex-shrink-0 text-right font-medium">{formatVal(rowTotal)}</div>
            </>
          );
          return clickable ? (
            <button key={i} type="button" onClick={() => onPick!(d.label)}
              className="flex items-center gap-2 w-full text-left rounded-md hover:bg-indigo-50/60 px-1 -mx-1 transition cursor-pointer">
              {inner}
            </button>
          ) : (
            <div key={i} className="flex items-center gap-2">{inner}</div>
          );
        })}
      </div>
    </div>
  );
}

// ── Bullet chart ────────────────────────────────────────────────────────
//
// Actual-vs-target with optional qualitative bands. Each row:
//   - ranges[] paints background bands (light gray gradient, low→high)
//   - value renders as a solid bar across the front
//   - target (if present) renders as a vertical tick mark
//
// Useful for OH vs policy max, in-stock % vs target.
function Bullet({ spec }: { spec: Extract<ChartSpec, { type: "bullet" }> }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.value === "number");
  if (data.length === 0) {
    return <ChartError msg="bullet.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  // Per-row scale: max of {value, target, last range}. Keeps a row's
  // own metric self-contained — comparing rows isn't the point of a
  // bullet, comparing actual-vs-target within a row is.
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <div className="space-y-2.5">
        {data.map((d, i) => {
          const ranges = Array.isArray(d.ranges) ? d.ranges.filter((n) => typeof n === "number") : [];
          const lastRange = ranges.length > 0 ? Math.max(...ranges) : 0;
          const scale = Math.max(d.value, d.target ?? 0, lastRange, 1);
          const valuePct = (d.value / scale) * 100;
          const targetPct = d.target != null ? (d.target / scale) * 100 : null;
          // Range bands rendered as concentric increasing widths so the
          // furthest band is the "good/high" zone (lightest fill).
          const sortedRanges = [...ranges].sort((a, b) => a - b);
          const rowTip = `${d.label}: ${formatVal(d.value)}${d.target != null ? ` (target ${formatVal(d.target)})` : ""}${sortedRanges.length > 0 ? ` · bands ${sortedRanges.map(formatVal).join("/")}` : ""}`;
          return (
            <div key={i} className="flex items-center gap-3" title={rowTip}>
              <div className="text-[11px] text-slate-600 w-32 flex-shrink-0 truncate text-left" title={rowTip}>{d.label}</div>
              <div className="flex-1 h-5 rounded-md relative overflow-hidden bg-slate-50" title={rowTip}>
                {sortedRanges.map((r, j) => {
                  const w = (r / scale) * 100;
                  const shade = 245 - Math.min(j, 4) * 24; // 245→149 across up to 5 bands
                  return (
                    <div key={j} className="absolute top-0 bottom-0 left-0 rounded-md"
                      style={{ width: `${w}%`, backgroundColor: `rgb(${shade},${shade},${shade + 6})`, zIndex: j + 1 }}
                      title={`band ≤ ${formatVal(r)}`} />
                  );
                })}
                <div className="absolute top-1/2 left-0 -translate-y-1/2 rounded-md"
                  style={{ width: `${valuePct}%`, height: "10px", backgroundColor: PALETTE[i % PALETTE.length], opacity: 0.92, zIndex: 20 }}
                  title={`value: ${formatVal(d.value)}`} />
                {targetPct != null && (
                  <div className="absolute top-1 bottom-1 w-[3px] bg-slate-900 rounded-sm"
                    style={{ left: `calc(${targetPct}% - 1.5px)`, zIndex: 30 }} title={`target: ${formatVal(d.target!)}`} />
                )}
              </div>
              <div className="text-[11px] tabular-nums w-32 flex-shrink-0 text-right text-slate-700 font-medium">
                {formatVal(d.value)}
                {d.target != null && <span className="text-slate-400"> / {formatVal(d.target)}</span>}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── Pareto chart ────────────────────────────────────────────────────────
//
// Bar chart + cumulative-% line overlay. Bars sorted descending by
// value (we don't re-sort the model's order — it may have intended
// natural ordering); cumulative line is derived as running sum /
// total. The line crosses 80% to call out the 80/20 break.
function Pareto({ spec, onPick }: { spec: Extract<ChartSpec, { type: "pareto" }>; onPick?: (label: string) => void }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.value === "number");
  if (data.length === 0) {
    return <ChartError msg="pareto.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  const total = data.reduce((s, d) => s + d.value, 0) || 1;
  const maxVal = Math.max(...data.map((d) => d.value), 1);
  let acc = 0;
  const cumulativePcts: number[] = [];
  for (const d of data) {
    acc += d.value;
    cumulativePcts.push((acc / total) * 100);
  }
  const W = Math.max(360, data.length * 48);
  const H = 200;
  const PAD = { l: 36, r: 40, t: 18, b: 36 };
  const innerW = W - PAD.l - PAD.r;
  const innerH = H - PAD.t - PAD.b;
  const barSlot = innerW / data.length;
  const barW = barSlot * 0.6;
  const xCenter = (i: number) => PAD.l + barSlot * (i + 0.5);
  const yForBar = (v: number) => PAD.t + innerH - (v / maxVal) * innerH;
  const yForPct = (p: number) => PAD.t + innerH - (p / 100) * innerH;
  const linePts = cumulativePcts.map((p, i) => `${xCenter(i)},${yForPct(p)}`).join(" ");
  const clickable = !!onPick;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        {/* 80% gridline */}
        <line x1={PAD.l} x2={W - PAD.r} y1={yForPct(80)} y2={yForPct(80)} stroke="#cbd5e1" strokeWidth={1} strokeDasharray="3 3" />
        <text x={W - PAD.r + 2} y={yForPct(80) + 3} fontSize="10" fill="#64748b">80%</text>
        {data.map((d, i) => {
          const x = xCenter(i) - barW / 2;
          const y = yForBar(d.value);
          const h = (PAD.t + innerH) - y;
          return (
            <g key={i} style={clickable ? { cursor: "pointer" } : undefined} onClick={clickable ? () => onPick!(d.label) : undefined}>
              <rect x={x} y={y} width={barW} height={h} fill={PALETTE[0]} opacity={0.85} rx={2}>
                <title>{`${d.label}: ${formatVal(d.value)} (${((d.value/total)*100).toFixed(1)}%)`}</title>
              </rect>
              <text x={xCenter(i)} y={H - PAD.b + 12} fontSize="9" fill="#475569" textAnchor="middle">
                {d.label.length > 8 ? d.label.slice(0, 7) + "…" : d.label}
              </text>
            </g>
          );
        })}
        <polyline fill="none" stroke="#f43f5e" strokeWidth={1.8} points={linePts} />
        {cumulativePcts.map((p, i) => (
          <circle key={i} cx={xCenter(i)} cy={yForPct(p)} r={2.5} fill="#f43f5e">
            <title>{`cumulative ${p.toFixed(1)}%`}</title>
          </circle>
        ))}
      </svg>
    </div>
  );
}

// ── Funnel chart ────────────────────────────────────────────────────────
//
// Successively narrowing horizontal bars centered horizontally. Each
// step's width is proportional to its value (relative to step 0,
// the widest). Step-over-step drop-off shown to the right of each bar.
function Funnel({ spec, onPick }: { spec: Extract<ChartSpec, { type: "funnel" }>; onPick?: (label: string) => void }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.value === "number");
  if (data.length === 0) {
    return <ChartError msg="funnel.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  const head = data[0]?.value ?? 1;
  const clickable = !!onPick;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <div className="space-y-1.5">
        {data.map((d, i) => {
          const pct = Math.max(0.5, (d.value / Math.max(head, 1)) * 100);
          const dropPct = i === 0
            ? null
            : (((data[i - 1].value - d.value) / Math.max(data[i - 1].value, 1)) * 100);
          const inner = (
            <>
              <div className="text-[11px] text-slate-600 w-40 flex-shrink-0 truncate text-left" title={d.label}>{d.label}</div>
              <div className="flex-1 h-7 relative flex items-center">
                <div
                  className="rounded-md mx-auto flex items-center justify-center text-[11px] font-medium text-white tabular-nums"
                  style={{ width: `${pct}%`, height: "100%", backgroundColor: PALETTE[i % PALETTE.length], opacity: 0.9 - i * 0.05 }}
                  title={`${d.label}: ${formatVal(d.value)}`}
                >
                  {formatVal(d.value)}
                </div>
              </div>
              <div className="text-[11px] text-slate-500 tabular-nums w-16 flex-shrink-0 text-right">
                {dropPct != null ? `−${dropPct.toFixed(0)}%` : ""}
              </div>
            </>
          );
          return clickable ? (
            <button key={i} type="button" onClick={() => onPick!(d.label)}
              className="flex items-center gap-2 w-full text-left rounded-md hover:bg-indigo-50/60 px-1 -mx-1 transition cursor-pointer">
              {inner}
            </button>
          ) : (
            <div key={i} className="flex items-center gap-2">{inner}</div>
          );
        })}
      </div>
    </div>
  );
}

// ── Heatmap ─────────────────────────────────────────────────────────────
//
// 2D grid of colored cells: one row per `y_labels` entry, one column
// per `x_labels` entry. Color scale is a single-hue linear ramp from
// min→max across the matrix (low = very light indigo, high = deep
// indigo). Each cell carries a tooltip with the exact value.
function Heatmap({
  spec, onPick,
}: {
  spec: Extract<ChartSpec, { type: "heatmap" }>;
  onPick?: (label: string) => void;
}) {
  const xs = Array.isArray(spec.x_labels) ? spec.x_labels : [];
  const ys = Array.isArray(spec.y_labels) ? spec.y_labels : [];
  const data = Array.isArray(spec.data) ? spec.data : [];
  if (xs.length === 0 || ys.length === 0 || data.length === 0) {
    return <ChartError msg="heatmap needs x_labels, y_labels, and data" raw={JSON.stringify(spec)} />;
  }
  const flat: number[] = data.flatMap((row) => (Array.isArray(row) ? row.filter((v) => typeof v === "number") : []));
  const min = Math.min(...flat, 0);
  const max = Math.max(...flat, 1);
  const span = max - min || 1;
  const shade = (v: number) => {
    // Single-hue ramp on indigo (#6366f1 = rgb(99,102,241)) — alpha
    // proportional to (v − min) / span. Keeps zero-ish cells visible
    // as faint tints rather than pure white.
    const a = 0.06 + 0.86 * Math.max(0, Math.min((v - min) / span, 1));
    return `rgba(99,102,241,${a.toFixed(3)})`;
  };
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 w-full overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <div className="inline-block min-w-full">
        {/* Header row */}
        <div className="flex" style={{ paddingLeft: "120px" }}>
          {xs.map((x) => (
            <div key={x} className="text-[10px] text-slate-600 font-medium px-1 truncate flex-1 min-w-[64px] text-center" title={x}>
              {x}
            </div>
          ))}
        </div>
        {ys.map((y, yi) => (
          <div key={y} className="flex items-stretch mt-0.5">
            <div className="text-[11px] text-slate-700 font-medium pr-2 truncate flex items-center" style={{ width: "120px" }} title={y}>
              {y}
            </div>
            {xs.map((x, xi) => {
              const v = (data[yi] && typeof data[yi][xi] === "number") ? data[yi][xi] : 0;
              const cellTitle = `${y} · ${x}: ${formatVal(v)}`;
              const inner = (
                <div
                  className="flex-1 min-w-[64px] h-7 rounded-sm flex items-center justify-center text-[10.5px] tabular-nums"
                  style={{ backgroundColor: shade(v), color: (v - min) / span > 0.55 ? "white" : "#334155" }}
                  title={cellTitle}
                >
                  {formatVal(v)}
                </div>
              );
              return onPick ? (
                <button key={`${y}-${x}`} type="button" onClick={() => onPick(`${y}|${x}`)} className="flex-1 min-w-[64px] mx-0.5 cursor-pointer">
                  {inner}
                </button>
              ) : (
                <div key={`${y}-${x}`} className="flex-1 min-w-[64px] mx-0.5">{inner}</div>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Treemap ─────────────────────────────────────────────────────────────
//
// Slice-and-dice layout (NOT squarified, for SVG-only simplicity): at
// each level, the parent rectangle is split into proportionally-sized
// strips. Top-level splits horizontally; children inside each strip
// split vertically. One level of nesting; if a top-level entry has no
// children, the whole strip is a single block. Color cycles through
// PALETTE per top-level entry; children use the same hue at lower
// opacity to read as siblings.
function Treemap({
  spec, onPick,
}: {
  spec: Extract<ChartSpec, { type: "treemap" }>;
  onPick?: (label: string) => void;
}) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.value === "number" && d.value > 0);
  if (data.length === 0) {
    return <ChartError msg="treemap.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  // Pure-CSS responsive: render at a fixed viewBox coordinate space
  // (the math is unchanged) and let `width=100% height=100%` +
  // `preserveAspectRatio="none"` stretch the rendered output into
  // whatever box the widget shell gives us. NO ResizeObserver = no
  // chance of a measure → render → measure loop. The trade-off is
  // text gets non-uniformly scaled when the box is tall-and-thin or
  // short-and-wide; for treemap labels (already small and rect-aligned)
  // this reads fine.
  const W = 1000, H = 560;
  const total = data.reduce((s, d) => s + d.value, 0) || 1;
  let cursorX = 0;
  return (
    // Treemap reads poorly in a short box (rectangles collapse to
    // thin strips). Reserve a generous min-height AND cap with a
    // max-height — the SVG uses `preserveAspectRatio="none"` so it
    // stretches the viewBox into whatever CSS box we hand it, but
    // without a ceiling it would grow indefinitely on a single-
    // widget dashboard. Browser-level scroll handles anything below.
    <div className="rounded-xl border border-slate-200 bg-white p-2 w-full flex flex-col min-h-[560px] max-h-[720px]">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-1 flex-shrink-0">{spec.title}</div>}
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="block w-full flex-1">
        {data.map((parent, pi) => {
          const stripW = (parent.value / total) * W;
          const x0 = cursorX;
          cursorX += stripW;
          const color = PALETTE[pi % PALETTE.length];
          const kids = Array.isArray(parent.children)
            ? parent.children.filter((c) => c && typeof c.label === "string" && typeof c.value === "number" && c.value > 0)
            : [];
          if (kids.length === 0) {
            return (
              <g
                key={pi}
                style={onPick ? { cursor: "pointer" } : undefined}
                onClick={onPick ? () => onPick(parent.label) : undefined}
              >
                <rect x={x0 + 1} y={1} width={Math.max(0, stripW - 2)} height={H - 2} fill={color} opacity={0.85} rx={3}>
                  <title>{`${parent.label}: ${formatVal(parent.value)}`}</title>
                </rect>
                <text x={x0 + 6} y={16} fontSize="11" fontWeight={600} fill="white">{truncFit(parent.label, stripW - 12)}</text>
                <text x={x0 + 6} y={30} fontSize="10" fill="white" opacity={0.85}>{formatVal(parent.value)}</text>
              </g>
            );
          }
          // Vertical slices for children inside this strip.
          const kidTotal = kids.reduce((s, c) => s + c.value, 0) || 1;
          let cursorY = 0;
          return (
            <g key={pi}>
              {kids.map((kid, ki) => {
                const sliceH = (kid.value / kidTotal) * H;
                const y0 = cursorY;
                cursorY += sliceH;
                const opacity = 0.55 + (0.4 * (1 - ki / Math.max(kids.length - 1, 1)));
                return (
                  <g
                    key={ki}
                    style={onPick ? { cursor: "pointer" } : undefined}
                    onClick={onPick ? () => onPick(`${parent.label}|${kid.label}`) : undefined}
                  >
                    <rect x={x0 + 1} y={y0 + 1} width={Math.max(0, stripW - 2)} height={Math.max(0, sliceH - 2)} fill={color} opacity={opacity} rx={2}>
                      <title>{`${parent.label} / ${kid.label}: ${formatVal(kid.value)}`}</title>
                    </rect>
                    {sliceH > 18 && (
                      <text x={x0 + 5} y={y0 + 13} fontSize="10" fontWeight={500} fill="white">{truncFit(kid.label, stripW - 10)}</text>
                    )}
                    {sliceH > 32 && (
                      <text x={x0 + 5} y={y0 + 26} fontSize="9" fill="white" opacity={0.85}>{formatVal(kid.value)}</text>
                    )}
                  </g>
                );
              })}
              {stripW > 36 && (
                <text x={x0 + 4} y={H - 5} fontSize="10" fontWeight={600} fill="#0f172a">{truncFit(parent.label, stripW - 8)}</text>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}

/** Crude width-fitter for SVG labels: ~6.6 px per char at 11px. */
function truncFit(s: string, pxBudget: number): string {
  if (pxBudget <= 8) return "";
  const maxChars = Math.max(1, Math.floor(pxBudget / 6.6));
  return s.length <= maxChars ? s : s.slice(0, Math.max(1, maxChars - 1)) + "…";
}

// ── Histogram ───────────────────────────────────────────────────────────
//
// Vertical bars with bin counts. `data` is the bin counts left-to-
// right; `bin_labels` (optional) align with them. Y axis is counts;
// X labels render under bars (rotated when too crowded).
function Histogram({ spec }: { spec: Extract<ChartSpec, { type: "histogram" }> }) {
  const counts = (spec.data ?? []).filter((n) => typeof n === "number");
  if (counts.length === 0) {
    return <ChartError msg="histogram.data is empty" raw={JSON.stringify(spec)} />;
  }
  const labels = Array.isArray(spec.bin_labels) ? spec.bin_labels.map(String) : counts.map((_, i) => String(i));
  const max = Math.max(...counts, 1);
  const W = Math.max(360, counts.length * 28);
  const H = 200;
  const PAD = { l: 36, r: 12, t: 14, b: 38 };
  const innerW = W - PAD.l - PAD.r;
  const innerH = H - PAD.t - PAD.b;
  const slot = innerW / counts.length;
  const barW = slot * 0.78;
  // 3 gridlines.
  const grid = [0, Math.round(max / 2), max];
  const rotateLabels = counts.length > 12;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        {grid.map((g, i) => {
          const y = PAD.t + innerH - (g / max) * innerH;
          return (
            <g key={i}>
              <line x1={PAD.l} x2={W - PAD.r} y1={y} y2={y} stroke="#e2e8f0" strokeWidth={1} />
              <text x={PAD.l - 4} y={y + 3} fontSize="10" fill="#94a3b8" textAnchor="end" className="tabular-nums">{g}</text>
            </g>
          );
        })}
        {counts.map((c, i) => {
          const x = PAD.l + slot * i + (slot - barW) / 2;
          const h = (c / max) * innerH;
          const y = PAD.t + innerH - h;
          const cx = PAD.l + slot * i + slot / 2;
          return (
            <g key={i}>
              <rect x={x} y={y} width={barW} height={h} fill={PALETTE[0]} opacity={0.85} rx={2}>
                <title>{`${labels[i] ?? i}: ${c}`}</title>
              </rect>
              <text
                x={rotateLabels ? cx : cx}
                y={rotateLabels ? H - PAD.b + 26 : H - PAD.b + 12}
                fontSize="9"
                fill="#475569"
                textAnchor={rotateLabels ? "end" : "middle"}
                transform={rotateLabels ? `rotate(-40, ${cx}, ${H - PAD.b + 26})` : undefined}
              >
                {labels[i] ?? i}
              </text>
            </g>
          );
        })}
        {spec.x_label && (
          <text x={PAD.l + innerW / 2} y={H - 2} fontSize="10" fill="#64748b" textAnchor="middle">{spec.x_label}</text>
        )}
        {spec.y_label && (
          <text x={10} y={PAD.t} fontSize="10" fill="#64748b">{spec.y_label}</text>
        )}
      </svg>
    </div>
  );
}

// ── Slope chart ─────────────────────────────────────────────────────────
//
// Two-period comparison: one column of `from` values, one column of
// `to` values, with a line connecting each entity's pair. Lines slope
// up = green, down = rose, flat = slate. Labels render at both ends.
function Slope({
  spec, onPick,
}: {
  spec: Extract<ChartSpec, { type: "slope" }>;
  onPick?: (label: string) => void;
}) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.from === "number" && typeof d.to === "number");
  if (data.length === 0) {
    return <ChartError msg="slope.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  const allVals = [...data.map((d) => d.from), ...data.map((d) => d.to)];
  const min = Math.min(...allVals);
  const max = Math.max(...allVals);
  const span = max - min || 1;
  const W = 460, H = Math.max(200, data.length * 18);
  const PAD = { l: 120, r: 120, t: 22, b: 22 };
  const innerH = H - PAD.t - PAD.b;
  const xL = PAD.l;
  const xR = W - PAD.r;
  const yFor = (v: number) => PAD.t + innerH - ((v - min) / span) * innerH;
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        {/* Axis labels */}
        {spec.from_label && (
          <text x={xL} y={14} fontSize="11" fontWeight={600} fill="#475569" textAnchor="middle">{spec.from_label}</text>
        )}
        {spec.to_label && (
          <text x={xR} y={14} fontSize="11" fontWeight={600} fill="#475569" textAnchor="middle">{spec.to_label}</text>
        )}
        {/* Vertical guide lines */}
        <line x1={xL} x2={xL} y1={PAD.t} y2={H - PAD.b} stroke="#e2e8f0" strokeWidth={1} />
        <line x1={xR} x2={xR} y1={PAD.t} y2={H - PAD.b} stroke="#e2e8f0" strokeWidth={1} />
        {data.map((d, i) => {
          const y1 = yFor(d.from);
          const y2 = yFor(d.to);
          const color = d.to > d.from ? "#10b981" : d.to < d.from ? "#f43f5e" : "#94a3b8";
          return (
            <g
              key={i}
              style={onPick ? { cursor: "pointer" } : undefined}
              onClick={onPick ? () => onPick(d.label) : undefined}
            >
              <line x1={xL} y1={y1} x2={xR} y2={y2} stroke={color} strokeWidth={1.5} opacity={0.85} />
              <circle cx={xL} cy={y1} r={3} fill={color} />
              <circle cx={xR} cy={y2} r={3} fill={color} />
              {/* left label = entity name + from value */}
              <text x={xL - 6} y={y1 + 3} fontSize="10" fill="#334155" textAnchor="end">
                {truncFit(d.label, 100)} · {formatVal(d.from)}
              </text>
              {/* right label = to value (entity name is implied) */}
              <text x={xR + 6} y={y2 + 3} fontSize="10" fill="#334155" textAnchor="start">
                {formatVal(d.to)}
              </text>
              <title>{`${d.label}: ${formatVal(d.from)} → ${formatVal(d.to)}`}</title>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

// ── Box plot / range bar ────────────────────────────────────────────────
//
// Horizontal 5-number summary per category: min/max whiskers, q1→q3
// box, median tick inside. All boxes share one global x-scale so eye
// comparisons across categories are honest. Useful when knowing the
// *spread* matters as much as the central tendency.
function BoxPlot({
  spec, onPick,
}: {
  spec: Extract<ChartSpec, { type: "boxplot" }>;
  onPick?: (label: string) => void;
}) {
  const data = (spec.data ?? []).filter((d) =>
    d && typeof d.label === "string" &&
    typeof d.min === "number" && typeof d.q1 === "number" &&
    typeof d.median === "number" && typeof d.q3 === "number" &&
    typeof d.max === "number"
  );
  if (data.length === 0) {
    return <ChartError msg="boxplot.data needs {label, min, q1, median, q3, max} entries" raw={JSON.stringify(spec)} />;
  }
  const allMin = Math.min(...data.map((d) => d.min));
  const allMax = Math.max(...data.map((d) => d.max));
  const span = allMax - allMin || 1;
  const ROW = 26;
  const W = 560;
  const PAD = { l: 130, r: 24, t: 10, b: 22 };
  const innerW = W - PAD.l - PAD.r;
  const H = PAD.t + PAD.b + ROW * data.length;
  const xFor = (v: number) => PAD.l + ((v - allMin) / span) * innerW;
  // Three x-axis ticks: min, midpoint, max.
  const ticks = [allMin, allMin + span / 2, allMax];
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        {/* Gridlines + tick labels */}
        {ticks.map((t, i) => (
          <g key={i}>
            <line x1={xFor(t)} x2={xFor(t)} y1={PAD.t} y2={H - PAD.b + 4} stroke="#e2e8f0" strokeWidth={1} strokeDasharray={i === 1 ? "3 3" : undefined} />
            <text x={xFor(t)} y={H - PAD.b + 16} fontSize="10" fill="#94a3b8" textAnchor="middle" className="tabular-nums">
              {formatVal(t)}
            </text>
          </g>
        ))}
        {data.map((d, i) => {
          const cy = PAD.t + ROW * i + ROW / 2;
          const xMin = xFor(d.min);
          const xQ1 = xFor(d.q1);
          const xMed = xFor(d.median);
          const xQ3 = xFor(d.q3);
          const xMax = xFor(d.max);
          const color = PALETTE[i % PALETTE.length];
          return (
            <g
              key={i}
              style={onPick ? { cursor: "pointer" } : undefined}
              onClick={onPick ? () => onPick(d.label) : undefined}
            >
              {/* Whiskers */}
              <line x1={xMin} x2={xMax} y1={cy} y2={cy} stroke="#94a3b8" strokeWidth={1} />
              <line x1={xMin} x2={xMin} y1={cy - 6} y2={cy + 6} stroke="#94a3b8" strokeWidth={1} />
              <line x1={xMax} x2={xMax} y1={cy - 6} y2={cy + 6} stroke="#94a3b8" strokeWidth={1} />
              {/* Box */}
              <rect x={xQ1} y={cy - 8} width={Math.max(1, xQ3 - xQ1)} height={16} fill={color} opacity={0.78} rx={2} />
              {/* Median */}
              <line x1={xMed} x2={xMed} y1={cy - 8} y2={cy + 8} stroke="#0f172a" strokeWidth={2} />
              {/* Label */}
              <text x={PAD.l - 8} y={cy + 3} fontSize="11" fill="#334155" textAnchor="end">
                {truncFit(d.label, PAD.l - 16)}
              </text>
              <title>
                {`${d.label}\nmin ${formatVal(d.min)} · q1 ${formatVal(d.q1)} · median ${formatVal(d.median)} · q3 ${formatVal(d.q3)} · max ${formatVal(d.max)}`}
              </title>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

// ── Waterfall ───────────────────────────────────────────────────────────
//
// Vertical bars showing how a running total builds up. Two kinds:
//   - `total: true`  → milestone bar planted on the zero baseline
//                       (start balance, end balance, subtotal).
//   - non-total      → increment bar; positive bars go up from the
//                       previous running value, negative go down.
// Increment bars get connecting dotted lines so the eye traces the
// running total. Positive = emerald, negative = rose, total = slate.
function Waterfall({ spec }: { spec: Extract<ChartSpec, { type: "waterfall" }> }) {
  const data = (spec.data ?? []).filter((d) => d && typeof d.label === "string" && typeof d.value === "number");
  if (data.length === 0) {
    return <ChartError msg="waterfall.data is empty or malformed" raw={JSON.stringify(spec)} />;
  }
  // Compute per-bar (start, end) running values. Totals reset to zero
  // base; non-totals accumulate.
  type Seg = { label: string; start: number; end: number; isTotal: boolean; value: number };
  const segs: Seg[] = [];
  let running = 0;
  for (const d of data) {
    if (d.total === true) {
      segs.push({ label: d.label, start: 0, end: d.value, isTotal: true, value: d.value });
      running = d.value;
    } else {
      const start = running;
      const end = running + d.value;
      segs.push({ label: d.label, start, end, isTotal: false, value: d.value });
      running = end;
    }
  }
  const allVals = segs.flatMap((s) => [s.start, s.end]);
  const min = Math.min(...allVals, 0);
  const max = Math.max(...allVals, 0);
  const span = max - min || 1;
  const W = Math.max(420, segs.length * 64);
  const H = 240;
  const PAD = { l: 44, r: 12, t: 24, b: 56 };
  const innerW = W - PAD.l - PAD.r;
  const innerH = H - PAD.t - PAD.b;
  const slot = innerW / segs.length;
  const barW = slot * 0.62;
  const yFor = (v: number) => PAD.t + innerH - ((v - min) / span) * innerH;
  const xCenter = (i: number) => PAD.l + slot * (i + 0.5);
  return (
    <div className="rounded-xl border border-slate-200 bg-white p-3 my-2 overflow-x-auto">
      {spec.title && <div className="text-sm font-medium text-slate-800 mb-2">{spec.title}</div>}
      <svg width={W} height={H} className="block">
        {/* Zero baseline */}
        <line x1={PAD.l} x2={W - PAD.r} y1={yFor(0)} y2={yFor(0)} stroke="#94a3b8" strokeWidth={1} strokeDasharray="3 3" />
        <text x={PAD.l - 4} y={yFor(0) + 3} fontSize="10" fill="#94a3b8" textAnchor="end" className="tabular-nums">0</text>
        {/* Y axis min / max ticks */}
        <text x={PAD.l - 4} y={yFor(max) + 3} fontSize="10" fill="#94a3b8" textAnchor="end" className="tabular-nums">{formatVal(max)}</text>
        {min < 0 && (
          <text x={PAD.l - 4} y={yFor(min) + 3} fontSize="10" fill="#94a3b8" textAnchor="end" className="tabular-nums">{formatVal(min)}</text>
        )}
        {segs.map((s, i) => {
          const xc = xCenter(i);
          const x = xc - barW / 2;
          const top = Math.min(yFor(s.start), yFor(s.end));
          const bot = Math.max(yFor(s.start), yFor(s.end));
          const h = Math.max(2, bot - top);
          let color = "#10b981";
          if (s.isTotal) color = "#64748b";
          else if (s.value < 0) color = "#f43f5e";
          // Connector to next bar — only between two non-total or
          // a non-total and the following total, to trace running.
          const next = segs[i + 1];
          const connectorY = yFor(s.end);
          return (
            <g key={i}>
              {next && (
                <line
                  x1={x + barW}
                  x2={xCenter(i + 1) - barW / 2}
                  y1={connectorY}
                  y2={connectorY}
                  stroke="#cbd5e1"
                  strokeWidth={1}
                  strokeDasharray="2 3"
                />
              )}
              <rect x={x} y={top} width={barW} height={h} fill={color} opacity={0.88} rx={2}>
                <title>{`${s.label}: ${formatVal(s.value)}${s.isTotal ? "" : ` (running ${formatVal(s.end)})`}`}</title>
              </rect>
              {/* Value label above/below the bar */}
              <text
                x={xc}
                y={s.value >= 0 || s.isTotal ? top - 4 : bot + 12}
                fontSize="10"
                fill="#334155"
                textAnchor="middle"
                className="tabular-nums"
              >
                {s.isTotal ? formatVal(s.value) : (s.value >= 0 ? `+${formatVal(s.value)}` : formatVal(s.value))}
              </text>
              {/* X-axis label */}
              <text
                x={xc}
                y={H - PAD.b + 14}
                fontSize="10"
                fill="#475569"
                textAnchor="end"
                transform={`rotate(-30, ${xc}, ${H - PAD.b + 14})`}
              >
                {truncFit(s.label, 80)}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

/** Donut slice path. startA/endA in radians, measured clockwise from 12 o'clock. */
function donutSlice(cx: number, cy: number, rOut: number, rIn: number, startA: number, endA: number): string {
  const large = endA - startA > Math.PI ? 1 : 0;
  const p = (a: number, r: number) => [cx + r * Math.sin(a), cy - r * Math.cos(a)];
  const [x1o, y1o] = p(startA, rOut);
  const [x2o, y2o] = p(endA,   rOut);
  const [x2i, y2i] = p(endA,   rIn);
  const [x1i, y1i] = p(startA, rIn);
  return [
    `M ${x1o} ${y1o}`,
    `A ${rOut} ${rOut} 0 ${large} 1 ${x2o} ${y2o}`,
    `L ${x2i} ${y2i}`,
    `A ${rIn}  ${rIn}  0 ${large} 0 ${x1i} ${y1i}`,
    "Z",
  ].join(" ");
}

function formatVal(v: unknown): string {
  if (typeof v === "number") {
    if (!Number.isFinite(v)) return String(v);
    const abs = Math.abs(v);
    if (abs >= 1_000_000_000) return (v / 1_000_000_000).toFixed(2) + "B";
    if (abs >= 1_000_000)     return (v / 1_000_000).toFixed(2) + "M";
    if (abs >= 10_000)        return (v / 1_000).toFixed(1) + "K";
    if (Number.isInteger(v))  return v.toLocaleString();
    return v.toFixed(2);
  }
  return String(v);
}

// React not directly referenced but needed for the JSX runtime — silences
// `react` unused-import linting in tsconfig modes that don't auto-inject.
export type _Touch = React.ReactNode;
