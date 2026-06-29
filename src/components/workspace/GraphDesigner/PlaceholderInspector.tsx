import { Info } from "lucide-react";
import type { Selection } from "./FormView";

/// Stub inspector for tree node types that don't have a real
/// inspector yet (Hierarchy / Level / Relation / Metric). Surfaces
/// what was clicked + which phase will deliver real editing + a
/// hint to drop to the TOML tab for now.
///
/// Replaced incrementally as Phases 2-4 ship their inspectors.
export function PlaceholderInspector({ selection }: { selection: Selection }) {
  if (!selection) return null;

  let title = "";
  let phase = "";
  switch (selection.type) {
    case "hierarchy":
      title = `Hierarchy: ${selection.name}`;
      phase = "Phase 2 (Hierarchies + Levels)";
      break;
    case "level":
      title = `Level: ${selection.hierarchy}.${selection.levelId}`;
      phase = "Phase 2 (Hierarchies + Levels)";
      break;
    case "relation":
      title = `Relation #${selection.index}`;
      phase = "Phase 3 (Relations)";
      break;
    case "metric":
      title = `Metric: ${selection.sourceAlias}.${selection.name}`;
      phase = "Phase 4 (Metrics)";
      break;
    default:
      return null;
  }

  return (
    <div className="p-6 space-y-3">
      <div>
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-0.5">
          Selected
        </div>
        <h2 className="text-base font-medium text-gray-100 font-mono">{title}</h2>
      </div>

      <div className="rounded border border-blue-900/60 bg-blue-950/20 p-3 text-[11px] text-blue-300/90">
        <div className="flex items-start gap-1.5">
          <Info size={12} className="mt-0.5 shrink-0" />
          <div>
            <div className="font-medium text-blue-200 mb-1">
              Inspector for this type ships in {phase}.
            </div>
            <div>
              For now, edit this element in the{" "}
              <span className="text-blue-100 font-medium">TOML</span> tab
              (toggle in the header). The form's other sections
              (Sources, …) keep working — switching modes preserves
              your unsaved edits.
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
