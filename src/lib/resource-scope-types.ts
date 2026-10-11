import type { AppError, ResourceListRequest, ResourceSummary } from "./types";

export interface ResourceScopeFailure {
	clusterContext: string;
	request: ResourceListRequest;
	error: AppError;
}

export interface ResourceScopeResult {
	resources: ResourceSummary[];
	completedRequests: ResourceListRequest[];
	failures: ResourceScopeFailure[];
}
