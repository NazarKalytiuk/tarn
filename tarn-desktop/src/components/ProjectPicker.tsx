import { open } from "@tauri-apps/plugin-dialog";
import { discoverTests, listEnvironments } from "../ipc";
import {
  applyDiscovery,
  setProjectState,
} from "../stores/project";
import { rememberProject } from "../stores/recent";
import { setEnvironments } from "../stores/toolbar";

/**
 * Visual surfaces for opening a project live in the Titlebar and the
 * EmptyState component (per the design handoff). This file is now
 * just the two action helpers they both use.
 */

export async function openProjectAtPath(path: string): Promise<void> {
  setProjectState({ path, loading: true, error: null });
  try {
    const [discovery, envs] = await Promise.all([
      discoverTests(path),
      listEnvironments(path),
    ]);
    applyDiscovery(discovery);
    setEnvironments(envs);
    setProjectState("loading", false);
    await rememberProject(path);
  } catch (err) {
    setProjectState({
      loading: false,
      error: err instanceof Error ? err.message : String(err),
    });
  }
}

export async function pickProject(): Promise<void> {
  const selected = await open({ directory: true, multiple: false });
  if (typeof selected !== "string") return;
  await openProjectAtPath(selected);
}
