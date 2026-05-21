import { createStore } from "solid-js/store";
import type { DiscoverResult, ListedFile } from "../ipc/types";

export type ProjectState = {
  /** Absolute path of the opened project directory. */
  path: string | null;
  files: ListedFile[];
  loading: boolean;
  error: string | null;
};

export const [projectState, setProjectState] = createStore<ProjectState>({
  path: null,
  files: [],
  loading: false,
  error: null,
});

export function applyDiscovery(result: DiscoverResult) {
  setProjectState("files", result.files);
}

export function clearProject() {
  setProjectState({ path: null, files: [], loading: false, error: null });
}
