/**
 * Strip the project root from a discovery-returned file path. Tarn's
 * `list --format json` emits absolute paths when invoked with a `cwd`
 * argument; the UI shows them relative to the opened project so paths
 * stay short and screenshotable.
 *
 * Tolerates the `/./` segment that `tarn list` sometimes inserts.
 */
export function relativePath(project: string | null, file: string): string {
  if (!project) return file;
  const cleanProject = project.replace(/\/+$/, "");
  const cleanFile = file.replace(/\/\.\//g, "/");
  if (cleanFile === cleanProject) return "";
  if (cleanFile.startsWith(cleanProject + "/")) {
    return cleanFile.slice(cleanProject.length + 1);
  }
  return cleanFile;
}
