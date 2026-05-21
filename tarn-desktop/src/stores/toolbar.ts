import { createStore } from "solid-js/store";
import type { Environment } from "../ipc/types";

/** State that belongs to the toolbar: which env, which tags, which
 * past run is being viewed. Lives separately from the project tree
 * and the run state because it's user-controlled selection, not
 * derived from the CLI. */
export type ToolbarState = {
  environments: Environment[];
  /** Selected env name; `null` means base/default. */
  activeEnv: string | null;
  /** Tags currently filtering the tree (AND semantics, matches CLI). */
  activeTags: string[];
};

export const [toolbarState, setToolbarState] = createStore<ToolbarState>({
  environments: [],
  activeEnv: null,
  activeTags: [],
});

export function setEnvironments(envs: Environment[]) {
  setToolbarState("environments", envs);
}

export function setActiveEnv(name: string | null) {
  setToolbarState("activeEnv", name);
}

export function toggleTag(tag: string) {
  setToolbarState("activeTags", (tags) =>
    tags.includes(tag) ? tags.filter((t) => t !== tag) : [...tags, tag],
  );
}

export function clearTags() {
  setToolbarState("activeTags", []);
}

export function resetToolbar() {
  setToolbarState({ environments: [], activeEnv: null, activeTags: [] });
}
