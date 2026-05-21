import { Store } from "@tauri-apps/plugin-store";
import { createSignal } from "solid-js";

const STORE_FILE = "studio.json";
const KEY = "recent_projects";
const MAX = 5;

const [recentProjects, setRecentProjects] = createSignal<string[]>([]);
export { recentProjects };

let storePromise: Promise<Store> | null = null;

async function store(): Promise<Store> {
  if (!storePromise) {
    storePromise = Store.load(STORE_FILE);
  }
  return storePromise;
}

export async function loadRecentProjects(): Promise<void> {
  try {
    const s = await store();
    const raw = await s.get<string[]>(KEY);
    setRecentProjects(Array.isArray(raw) ? raw.slice(0, MAX) : []);
  } catch {
    setRecentProjects([]);
  }
}

export async function rememberProject(path: string): Promise<void> {
  const s = await store();
  const next = [path, ...recentProjects().filter((p) => p !== path)].slice(0, MAX);
  setRecentProjects(next);
  await s.set(KEY, next);
  await s.save();
}

export async function forgetProject(path: string): Promise<void> {
  const s = await store();
  const next = recentProjects().filter((p) => p !== path);
  setRecentProjects(next);
  await s.set(KEY, next);
  await s.save();
}
