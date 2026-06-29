import type { Tool } from "./tool.js";
import { materializeTool } from "./tools/materialize.js";
import { statusTool } from "./tools/status.js";
import { describeTool } from "./tools/describe.js";
import { glossaryTool } from "./tools/glossary.js";
import { filterValuesTool } from "./tools/filter_values.js";
import { listTool } from "./tools/list.js";
import { queryTool } from "./tools/query.js";
import { articleDetailTool } from "./tools/article_detail.js";
import { submitFeedbackTool } from "./tools/feedback.js";
import { listGraphsTool } from "./tools/list_graphs.js";
import { describeGraphTool } from "./tools/describe_graph.js";
import { graphNodeTool } from "./tools/graph_node.js";
import { graphTraverseTool } from "./tools/graph_traverse.js";
import { graphCrossFilterTool } from "./tools/graph_cross_filter.js";
import { listSourcesTool } from "./tools/list_sources.js";
import { describeSourceTool } from "./tools/describe_source.js";
import { listDataViewsTool } from "./tools/list_dataviews.js";
import { describeDataViewTool } from "./tools/describe_dataview.js";
import { duckDbQueryTool } from "./tools/duckdb_query.js";
import { resolveFilterValuesTool } from "./tools/resolve_filter_values.js";
import { dataViewReadTool } from "./tools/dataview_read.js";
import { introspectDataViewTool } from "./tools/introspect_dataview.js";
import { listConnectionsTool } from "./tools/list_connections.js";
import { clickhouseDictionaryTool } from "./tools/clickhouse_dictionary.js";
import { clickhouseQueryTool } from "./tools/clickhouse_query.js";

export const TOOLS: Tool[] = [
  materializeTool,
  statusTool,
  describeTool,
  glossaryTool,
  filterValuesTool,
  listTool,
  queryTool,
  articleDetailTool,
  submitFeedbackTool,
  listGraphsTool,
  describeGraphTool,
  graphNodeTool,
  graphTraverseTool,
  graphCrossFilterTool,
  listSourcesTool,
  describeSourceTool,
  listDataViewsTool,
  describeDataViewTool,
  duckDbQueryTool,
  resolveFilterValuesTool,
  dataViewReadTool,
  introspectDataViewTool,
  listConnectionsTool,
  clickhouseDictionaryTool,
  clickhouseQueryTool,
];
