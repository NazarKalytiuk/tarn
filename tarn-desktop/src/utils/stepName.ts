/**
 * Parse a step name like `POST /auth/login` into a method + path pair.
 * When the name doesn't match the HTTP-method convention, returns
 * `{ method: null, path: <whole name> }` so the tree falls back to the
 * raw label.
 */
export function parseStepName(name: string): {
  method: string | null;
  path: string;
} {
  const m = name.match(/^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+(.+)$/i);
  if (m) return { method: m[1].toUpperCase(), path: m[2] };
  return { method: null, path: name };
}
