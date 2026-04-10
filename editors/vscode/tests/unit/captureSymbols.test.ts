import { describe, it, expect } from "vitest";
import {
  buildCaptureIndex,
  findCaptureReferences,
} from "../../src/language/completion/captures";

const SOURCE = `name: Fixture
setup:
  - name: Authenticate
    request:
      method: POST
      url: "http://localhost/auth"
    capture:
      auth_token: "$.token"
      session_id: "$.sid"

tests:
  create_flow:
    steps:
      - name: Create user
        request:
          method: POST
          url: "http://localhost/users"
        capture:
          user_id: "$.id"
      - name: Verify user
        request:
          method: GET
          url: "http://localhost/users/{{ capture.user_id }}"
        capture:
          display_name: "$.name"
      - name: Delete user
        request:
          method: DELETE
          url: "http://localhost/users/{{ capture.user_id }}"
        assert:
          status: 204
  other_flow:
    steps:
      - name: Unrelated
        request:
          method: GET
          url: "http://localhost/ping"
        capture:
          pong: "$.pong"

teardown:
  - name: Clean up
    request:
      method: POST
      url: "http://localhost/cleanup/{{ capture.auth_token }}"
`;

describe("buildCaptureIndex", () => {
  it("returns undefined on YAML parse errors", () => {
    const broken = `name: "unclosed
tests:
  t:
    steps: []
`;
    expect(buildCaptureIndex(broken)).toBeUndefined();
  });

  it("collects captures from setup, tests, and other tests", () => {
    const index = buildCaptureIndex(SOURCE);
    expect(index).toBeDefined();
    const names = index!.declarations.map((d) => d.name).sort();
    expect(names).toEqual([
      "auth_token",
      "display_name",
      "pong",
      "session_id",
      "user_id",
    ]);
  });

  it("records correct phase and test name for each declaration", () => {
    const index = buildCaptureIndex(SOURCE);
    expect(index).toBeDefined();
    const authToken = index!.findByName("auth_token")[0];
    expect(authToken.phase).toBe("setup");
    expect(authToken.testName).toBeUndefined();

    const userId = index!.findByName("user_id")[0];
    expect(userId.phase).toBe("test");
    expect(userId.testName).toBe("create_flow");
    expect(userId.stepIndex).toBe(0);

    const pong = index!.findByName("pong")[0];
    expect(pong.testName).toBe("other_flow");
  });

  it("findDeclarationAt returns the declaration whose key contains the offset", () => {
    const index = buildCaptureIndex(SOURCE);
    const userIdOffset = SOURCE.indexOf("user_id: ") + 3; // inside the key
    const decl = index!.findDeclarationAt(userIdOffset);
    expect(decl?.name).toBe("user_id");
  });

  it("findDeclarationAt returns undefined for offsets outside every key", () => {
    const index = buildCaptureIndex(SOURCE);
    const outside = SOURCE.indexOf("http://localhost/users") + 5;
    expect(index!.findDeclarationAt(outside)).toBeUndefined();
  });
});

describe("findCaptureReferences", () => {
  it("finds every `{{ capture.NAME }}` in the source", () => {
    const refs = findCaptureReferences(SOURCE);
    const names = refs.map((r) => r.name).sort();
    expect(names).toEqual(["auth_token", "user_id", "user_id"]);
  });

  it("filters by name when a filter is provided", () => {
    const refs = findCaptureReferences(SOURCE, "user_id");
    expect(refs).toHaveLength(2);
    for (const ref of refs) {
      expect(ref.name).toBe("user_id");
      // Offset must point at the identifier itself.
      expect(SOURCE.slice(ref.nameStart, ref.nameEnd)).toBe("user_id");
    }
  });

  it("ignores tokens that aren't capture interpolations", () => {
    const source = `
url: "{{ env.base_url }}/users/{{ $uuid }}/{{ capture.token }}"
`;
    const refs = findCaptureReferences(source);
    expect(refs).toHaveLength(1);
    expect(refs[0].name).toBe("token");
  });

  it("handles whitespace variations inside the interpolation", () => {
    const source = `{{capture.one}} {{ capture.two }} {{  capture.three  }}`;
    const refs = findCaptureReferences(source);
    expect(refs.map((r) => r.name).sort()).toEqual(["one", "three", "two"]);
  });

  it("returns an empty list when there are no capture references", () => {
    expect(findCaptureReferences("")).toEqual([]);
    expect(
      findCaptureReferences("url: http://localhost/plain"),
    ).toEqual([]);
  });
});
