export class Position {
  constructor(public readonly line: number, public readonly character: number) {}
}

export class Range {
  constructor(public readonly start: Position, public readonly end: Position) {}
}

export class Location {
  constructor(public readonly uri: unknown, public readonly range: Range) {}
}

export class MarkdownString {
  supportHtml = false;
  isTrusted: boolean | undefined = undefined;
  constructor(public readonly value: string = "") {}
}

export class TestMessage {
  expectedOutput: string | undefined;
  actualOutput: string | undefined;
  location: Location | undefined;
  constructor(public readonly message: string | MarkdownString) {}
}

export interface Disposable {
  dispose(): void;
}

/**
 * Minimal `vscode.CancellationToken` implementation good enough for
 * the NAZ-279 MCP backend unit tests. The real VS Code token fires an
 * event when `.cancel()` is invoked on the parent `Source`; we mimic
 * the same contract so `TarnMcpClient.run()` can subscribe and tear
 * down a pending JSON-RPC request on cancel.
 */
class CancellationToken {
  private _cancelled = false;
  private listeners: Array<() => void> = [];

  get isCancellationRequested(): boolean {
    return this._cancelled;
  }

  onCancellationRequested(listener: () => void): Disposable {
    if (this._cancelled) {
      listener();
      return { dispose: () => {} };
    }
    this.listeners.push(listener);
    return {
      dispose: () => {
        const idx = this.listeners.indexOf(listener);
        if (idx >= 0) {
          this.listeners.splice(idx, 1);
        }
      },
    };
  }

  /** Internal helper used by {@link CancellationTokenSource}. */
  _fire(): void {
    if (this._cancelled) return;
    this._cancelled = true;
    for (const listener of this.listeners) {
      try {
        listener();
      } catch {
        /* ignore: test mock */
      }
    }
    this.listeners = [];
  }
}

export class CancellationTokenSource {
  readonly token: CancellationToken = new CancellationToken();
  cancel(): void {
    this.token._fire();
  }
  dispose(): void {
    // Real VS Code CancellationTokenSource fires cancel on dispose
    // only when `cancelWhenDisposed` is set; for the unit-test mock
    // we leave the token in whatever state `cancel()` last put it.
  }
}

export const Uri = {
  file(p: string) {
    return { fsPath: p, toString: () => `file://${p}`, path: p };
  },
  parse(s: string) {
    return { fsPath: s, toString: () => s, path: s };
  },
};

type ConfigEntries = Record<string, unknown>;
let mockConfigEntries: ConfigEntries = {};

/**
 * Test-only helper: seed the fake `vscode.workspace.getConfiguration`
 * with a map of `"tarn.key" -> value` so unit tests can exercise the
 * setting-reading helpers in `config.ts` without bootstrapping a full
 * extension host. Call with `{}` (or no argument) to reset.
 */
export function __setMockConfig(entries: ConfigEntries = {}): void {
  mockConfigEntries = { ...entries };
}

export const workspace = {
  getConfiguration(section?: string): {
    get<T>(key: string, defaultValue?: T): T | undefined;
  } {
    return {
      get<T>(key: string, defaultValue?: T): T | undefined {
        const fullKey = section ? `${section}.${key}` : key;
        if (fullKey in mockConfigEntries) {
          return mockConfigEntries[fullKey] as T;
        }
        return defaultValue;
      },
    };
  },
};

const shownInformationMessages: string[] = [];

/**
 * Test-only helper: inspect the toast queue. Returned array is a
 * snapshot; tests call this between interactions to assert that a
 * notification fired exactly once.
 */
export function __getShownInformationMessages(): string[] {
  return [...shownInformationMessages];
}

export function __clearShownInformationMessages(): void {
  shownInformationMessages.length = 0;
}

export const window = {
  async showWarningMessage(
    _message: string,
    ..._items: string[]
  ): Promise<string | undefined> {
    return undefined;
  },
  async showInformationMessage(
    message: string,
    ..._items: string[]
  ): Promise<string | undefined> {
    shownInformationMessages.push(message);
    return undefined;
  },
  async showErrorMessage(
    _message: string,
    ..._items: string[]
  ): Promise<string | undefined> {
    return undefined;
  },
};

/**
 * Minimal `vscode.l10n` stub for unit tests.
 *
 * VS Code's real implementation looks up `message` in a bundle keyed
 * by locale and falls back to the English key when no translation is
 * available. Unit tests always see the EN fallback, so we faithfully
 * reproduce the fallback: return `message` with `{N}` placeholders
 * substituted by positional args.
 */
export const l10n = {
  t(message: string, ...args: Array<string | number | boolean>): string {
    if (args.length === 0) {
      return message;
    }
    return message.replace(/\{(\d+)\}/g, (match, indexStr) => {
      const index = Number(indexStr);
      if (Number.isInteger(index) && index >= 0 && index < args.length) {
        return String(args[index]);
      }
      return match;
    });
  },
  bundle: undefined as Record<string, string> | undefined,
  uri: undefined,
};

export default {
  Position,
  Range,
  Location,
  MarkdownString,
  TestMessage,
  CancellationTokenSource,
  Uri,
  workspace,
  window,
  l10n,
};
