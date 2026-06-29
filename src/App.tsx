import { useEffect, useState } from "react";
import { api, type Identity } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";
import { WorkspaceLayout } from "@/layouts/WorkspaceLayout";
import { DataViewWorkspace } from "@/components/workspace/DataViewWorkspace";
import { SharedPipelineWorkspace } from "@/components/workspace/SharedPipelineWorkspace";
import { DimensionWorkspace } from "@/components/workspace/DimensionWorkspace";
import { FilterConfigWorkspace } from "@/components/workspace/FilterConfigWorkspace";
import { ConnectionWorkspace } from "@/components/workspace/ConnectionWorkspace";
import { SourcesWorkspace } from "@/components/workspace/SourcesWorkspace";
import { CoreServiceWorkspace } from "@/components/workspace/CoreServiceWorkspace";
import { QueryWorkspace } from "@/components/workspace/QueryWorkspace";
import { CrossFilterWorkspace } from "@/components/workspace/CrossFilterWorkspace";
import { MemoryWorkspace } from "@/components/workspace/MemoryWorkspace";
import { GraphDesigner } from "@/components/workspace/GraphDesigner";
import { FeedbackWorkspace } from "@/components/workspace/FeedbackWorkspace";

function WorkspaceContent() {
  const selected = useWorkspaceStore((s) => s.selected);
  const activeTab = useWorkspaceStore((s) => s.activeTab);

  if (activeTab === "query") {
    return <QueryWorkspace />;
  }

  if (activeTab === "cross_filter") {
    return <CrossFilterWorkspace />;
  }

  if (activeTab === "memory") {
    return <MemoryWorkspace />;
  }

  if (activeTab === "feedback") {
    return <FeedbackWorkspace />;
  }

  if (!selected) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500 text-sm">
        Select an item from the sidebar
      </div>
    );
  }

  switch (selected.type) {
    case "dataview":
      return <DataViewWorkspace dataviewId={selected.id} />;
    case "shared_pipeline":
      return <SharedPipelineWorkspace key={selected.id} pipelineId={selected.id} />;
    case "dimension":
      return <DimensionWorkspace dimensionId={selected.id} />;
    case "filter_config":
      return <FilterConfigWorkspace filterConfigId={selected.id} />;
    case "source":
      return <SourcesWorkspace sourceId={selected.id} />;
    case "connection":
      return <ConnectionWorkspace connectionId={selected.id} />;
    case "core_service":
      return <CoreServiceWorkspace serviceId={selected.id} />;
    case "graph":
      return <GraphDesigner key={selected.id} graphId={selected.id} />;
    default:
      return (
        <div className="flex items-center justify-center h-full text-gray-500 text-sm">
          Select an item from the sidebar
        </div>
      );
  }
}

export default function App() {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getIdentity()
      .then(setIdentity)
      .catch((e) => setError(e.message ?? String(e)));
  }, []);

  if (error) {
    return (
      <div className="flex items-center justify-center h-screen text-red-500 text-sm font-mono p-8">
        Failed to load tenant identity: {error}
      </div>
    );
  }

  if (!identity) {
    return (
      <div className="flex items-center justify-center h-screen text-gray-400 text-sm">
        Loading…
      </div>
    );
  }

  return (
    <WorkspaceLayout
      tenantId={identity.id}
      identity={identity}
      workspace={<WorkspaceContent />}
    />
  );
}
