# Tarn Studio — Design Brief

A native desktop app that turns a CLI test runner into a calm, beautiful surface for running and investigating API tests. The product exists today as a command-line tool that engineers love; Studio is for the people who don't live in the terminal and for the moments when even the engineers want something visual.

This brief is the only context you need. **You decide everything about layout, navigation, and UX flow.** What we lock down is the audience, the brand, the features that must reach users, and the data that flows through the app. Anything not stated as a constraint is yours to design.

---

## Elevator pitch

**Quiet, fast API testing.** Open a project, watch tests light up in real time, click into the ones that broke, see exactly what the server returned and why the assertion failed. No noise, no babysitter copy, no "you have 3 errors!" red banners. The CLI's restraint, in pixels.

---

## Who uses this

Three personas matter. Design for all of them, but if you have to choose, optimize for Sasha and Mira.

**Sasha — QA engineer at a B2B SaaS, 7 years experience.**
Doesn't write YAML, but reads it carefully. Runs the dev team's test suite against staging before every release and against prod after deploys. Spends 70% of her time inside failed steps: what did the test send, what came back, why doesn't the assertion match. She copies curl commands out of Tarn and pastes them to backend engineers in Slack with a "is this expected?" Lives in dark mode everywhere. Has 11 tabs open in her browser and a terminal she avoids.

**Mira — fullstack developer who pair-programs with Claude.**
Claude wrote 40% of her test suite. She runs Studio while Claude generates more tests, watches them stream in, clicks into failures to decide: is the test wrong, or is the API wrong? She knows YAML cold but only wants to look at it when a step is broken. Cmd-key shortcuts matter — she'll trash a tool that forces her to mouse for every action.

**Theo — DevRel for the company shipping Tarn.**
Doesn't run tests for real, but needs Tarn Studio to look great at 1280×800 on the marketing landing page. Records short screen recordings of a live run for social posts. If the empty state isn't beautiful, the homepage isn't beautiful. If the "all passing" state isn't satisfying, neither is the demo.

Notably absent: the kind of user who'd want a Postman-style request builder. That user is served by Postman. Don't design for them.

---

## The promise (and the brand)

The CLI's tagline carries over: **quiet, fast API testing.** A terminal is quiet because it's a terminal. A GUI has to earn quietness. That earns translates to:

- **No toast notifications.** Status changes through restrained color and the counter, never a popup.
- **No alarms.** A failed test does not turn the screen red. It's marked, calmly.
- **No progress bars.** A determinate one lies (we don't know total duration); an indeterminate one fidgets. Status dots tell the story.
- **No babysitter copy.** No "Looks like something went wrong! Here are some tips!" No "Did you know?" cards. The user knows what they're doing.
- **No splash screen.** The first paint should already be useful.

**Aesthetic reference apps.** Look at, in this order:

- **Linear** — calm, opinionated, dense-but-readable. The closest spiritual sibling.
- **Raycast** — chrome-less elegance, beautiful empty states, generous keyboard shortcuts.
- **Vercel dashboard** — deployment cards, restrained color, status communicated without alarm.
- **Arc settings** — typography-led, color-restrained, comfortable density.

**Aesthetic anti-references.**

- **Postman/Insomnia** — too loud, too colorful, too many panels visible at once.
- **Cypress dashboard** — close in shape but too busy at the top.
- **Jenkins, TeamCity, generic CI UIs** — functional, dated, no warmth.

**Typography.** System sans for chrome (San Francisco on macOS, Segoe UI on Windows, Inter as a web fallback). Monospace for any payload, header value, URL, or code snippet. No web fonts — Studio ships offline-first.

**Color.** Status colors are non-negotiable and the only saturated colors in the app:

- Passed → green (a confident green, not lime).
- Failed → red (warm, not crimson — it shouldn't feel like a fire alarm).
- Running → amber.
- Skipped → muted slate.
- Pending → low-saturation slate.

Beyond that: one accent color for primary CTAs and active selection (e.g. an indigo or purple — designer's call). The rest is a slate/neutral gray scale. Color is signal, never decoration. A passing run should look composed in monochrome plus green dots.

**Motion.** Sparingly.

- Subtle ~1 Hz opacity pulse on the amber "running" dot.
- 150 ms color transitions on status change.
- 100 ms expand/collapse on tree nodes.
- That's it. No bouncing, no shimmer, no parallax, no shine, no confetti when everything passes.

**Iconography.** Lucide or Phosphor (the designer chooses). No emoji in chrome. Status indicators are filled circles or another minimal shape — _not_ ✓/✗ glyphs, _not_ traffic-light squares.

---

## What users need to do

In rough order of frequency. Translate these into surfaces however you see fit.

1. **Open a project.** A project is a folder on disk that contains `.tarn.yaml` test files.
2. **See what tests exist.** Browse the test catalogue: files, the named tests inside them, the steps inside each test.
3. **Run all tests, or a subset.** By file, by test, by step, or filtered by tag.
4. **Watch tests run live.** As each step finishes, the UI updates to reflect status, duration, and assertion outcome. This is the heartbeat of the product — the streaming UX has to feel alive.
5. **Investigate a failure.** Click into a failed step → see the assertion message, what was expected vs what arrived, the request that was sent, the response that came back. Copy as curl.
6. **Switch environments.** Pick `staging`, `prod`, `local`, etc. Each maps to a `tarn.env.<name>.yaml` file in the project root.
7. **Filter by tag.** Show only tests tagged with `auth`, `smoke`, `regression`, etc. Tags are declared in the YAML.
8. **Cancel a long-running run.** Kill the CLI cleanly.
9. **Browse past runs.** See the last N runs, including failures. (Phase 3 — designer should sketch the affordance, full content is later.)
10. **Read and (later) edit the YAML.** Phase 2. The brief lists this so the layout can leave room.

---

## How data flows through the app

You don't need to implement this, but understanding it shapes the visual hierarchy. **Use real shapes in mockups, not lorem ipsum.**

### Discovery (when a project opens)

```json
{
  "files": [
    {
      "file": "tests/smoke.tarn.yaml",
      "name": "Smoke checks",
      "tags": ["smoke"],
      "tests": [
        {
          "name": "Auth flow",
          "steps": [
            {"kind": "request", "name": "POST /auth/login"},
            {"kind": "request", "name": "GET /users/me"}
          ]
        }
      ],
      "steps": []
    },
    {
      "file": "tests/billing.tarn.yaml",
      "name": "Billing",
      "tags": ["billing", "slow"],
      "tests": [...],
      "steps": []
    }
  ]
}
```

A file may have either named `tests` (each with its own steps) or a flat `steps` array. Both shapes coexist.

### Running (live, while the run unfolds)

A stream of newline-delimited JSON objects arriving one at a time, in real time:

```
{"event":"file_started","file":"tests/smoke.tarn.yaml","file_name":"Smoke checks"}
{"event":"step_finished","step":"POST /auth/login","step_index":0,"status":"PASSED","duration_ms":48,"progress":{"index":1,"total":2}}
{"event":"step_finished","step":"GET /users/me","step_index":1,"status":"FAILED","duration_ms":91,
  "assertion_failures":[{"assertion":"status","expected":"200","actual":"401",
                         "message":"Expected HTTP status 200, got 401"}],
  "error_code":"assertion_mismatch","failure_category":"assertion_failed"}
{"event":"test_finished","test":"Auth flow","status":"FAILED","duration_ms":139,
  "steps":{"total":2,"passed":1,"failed":1}}
{"event":"file_finished","file":"tests/smoke.tarn.yaml","status":"FAILED","duration_ms":150}
{"event":"done","duration_ms":160,"summary":{"files":1,"status":"FAILED",
  "steps":{"total":2,"passed":1,"failed":1}}}
```

Status arrives **step-by-step, not all at once.** The streaming feel is the product. There's no `step_started` event — a step is "running" while there's no `step_finished` for it yet.

### Step detail (after a run finishes)

```json
{
  "name": "GET /users/me",
  "status": "FAILED",
  "duration_ms": 91,
  "location": {"file": "tests/smoke.tarn.yaml", "line": 23, "column": 7},
  "assertions": {
    "total": 1, "passed": 0, "failed": 1,
    "failures": [{
      "assertion": "status",
      "expected": "200",
      "actual": "401",
      "message": "Expected HTTP status 200, got 401"
    }]
  },
  "request": {
    "method": "GET",
    "url": "https://api.example.com/users/me",
    "headers": {"Authorization": "Bearer eyJ***", "Accept": "application/json"}
  },
  "response": {
    "status": 401,
    "headers": {"content-type": "application/json", "x-request-id": "req_018f"},
    "body": {"error": "token_expired", "expires_at": "2026-05-21T14:02:11Z"}
  },
  "remediation_hints": [
    "Inspect `assertions.failures` expected vs actual values and update the DSL or the service response.",
    "Use the recorded `response` payload to realign assertions and captures with the actual API output."
  ]
}
```

For a passed step, `assertions` is the only block that matters; `request` and `response` are present but visually quieter.

For a failed step, this is the marquee surface. Design accordingly.

---

## Real-world content for mockups

When you populate mocks, use these — they look like the actual product. Avoid `foo`, `bar`, lorem ipsum.

**File names.** `smoke.tarn.yaml`, `auth.tarn.yaml`, `crud.tarn.yaml`, `users-api.tarn.yaml`, `billing.tarn.yaml`, `webhooks.tarn.yaml`.

**Test names.** `Auth flow`, `User CRUD`, `Billing upgrade`, `Webhook delivery`, `Edge cases`, `Pagination`, `Rate limiting`.

**Step names (these become method + path).** `POST /auth/login`, `GET /users/me`, `PATCH /users/42`, `DELETE /users/42`, `POST /billing/subscriptions`, `POST /webhooks/test`, `GET /users?cursor=eyJp…`.

**Tags.** `smoke`, `auth`, `crud`, `billing`, `regression`, `slow`, `flaky`, `webhooks`.

**Assertion messages.** `Expected HTTP status 200, got 404`, `Expected $.token to be string, got null`, `Expected duration < 200ms, got 1240ms`, `Expected $.user.email to match /^.+@.+$/, got "not-an-email"`.

**Environments.** `staging`, `prod`, `local`, `qa`, `eu-staging`.

**Project paths.** `~/work/acme-api`, `~/code/internal-tools`, `~/play/api-sandbox`.

**Durations.** `12ms`, `48ms`, `91ms`, `1.2s`, `4.7s`. Most things are sub-200ms.

**Counts.** Test suites run from 5 to ~80 steps total. Most projects have 3-10 files. The tree should look comfortable at both ends — generous at low counts, scannable at high counts.

---

## States the design must cover

At minimum, the deliverable should mock every one of these:

1. **First launch.** No project has ever been opened on this machine. No recents.
2. **Empty, with recent projects.** Last 3-5 paths the user opened previously.
3. **Project just opened, discovery completing.** Probably ≤300 ms — but design must handle it gracefully without a heavy "loading…" treatment.
4. **Project open, idle.** Test catalogue visible, no run in flight.
5. **Run in progress, ~30% done.** This is the marquee mid-stream state — Theo will screenshot this.
6. **Run finished, all green.** Calm satisfaction.
7. **Run finished, with failures.** Quiet, but clearly signaled. No alarms.
8. **Step selected — passed.** Minimal detail. Status, duration, "1 of 1 assertion passed". `request`/`response` are reachable but not in the user's face.
9. **Step selected — failed.** Full detail surface. Sasha and Mira live here.
10. **Run cancelled.** User killed an in-flight run. Steps that finished keep their status; steps that didn't return to pending. The footer should make the cancellation explicit without making the user feel they did something wrong.
11. **Error.** The `tarn` binary couldn't be found; or a project couldn't be parsed. Clear, non-alarming, with a specific next action.

---

## Features that must be reachable from the UI

(Not screens — capabilities. You decide what's a screen, a panel, a dropdown, a context menu, a keyboard shortcut.)

- Open project dialog.
- Recent projects.
- Run all.
- Cancel run (only while running).
- Run a single file / a single test / a single step. Granularity matters: power users will want to re-run just one step after a fix.
- Environment picker (one selected at a time; default is the base `tarn.env.yaml`).
- Tag filter (multiple tags, AND semantics — both filters the tree and constrains the next run).
- Run history (Phase 3 — sketch the affordance, full content is later).
- Step detail with assertions / request / response / copy-as-curl.
- Re-run selected step.
- Open YAML at the failing step's location (Phase 2 — leave room).
- Settings (Phase 2; minimal — binary path override, maybe a font-size toggle).

Optional features for you to consider including or not, with rationale:

- Search / Cmd+K palette. Justify whether the typical test count makes one worth its weight.
- Sidebar collapse to icon rail.
- Multi-step selection (run a curated subset across multiple files).
- A "story mode" that animates a recent run for screenshots (DevRel-specific).

---

## Hard constraints

- **Desktop only.** Tauri 2 (Rust shell hosting a WebView). Target window: 1280×800 min, expected use at 1440×900 to 2560×1440. The chrome should breathe at large sizes and tighten at smaller ones — no horizontal scroll at min width.
- **Dark theme only.** Not "dark by default" — the only mode. Don't waste time on light variants.
- **No external fonts** at runtime. System sans + system mono.
- **Animations are functional.** A status dot pulses because it's running. A color transitions because status changed. No decorative motion.
- **Accessibility.**
  - Status must be communicable to a color-blind user. Every red/green/amber signal must also carry a non-color cue: a label, a shape, a position. Don't rely on color alone.
  - Keyboard reachability for every primary action. Tab order makes sense.
  - Contrast at WCAG AA minimum for body text.
- **Implementability.** The mockups must be implementable in HTML/CSS/Tailwind without exotic dependencies. Solid.js + Tailwind v4 + CodeMirror 6 is the stack — keep components within the realm of what these can render cleanly.

---

## Out of scope — do not design

- Light theme.
- Mobile, tablet, responsive small layouts.
- Onboarding tour / tutorial overlay.
- Quick-start cards in the empty state (we explicitly do not want them — the empty state must feel curated, not "here are some things you could do").
- Marketing pages (those live elsewhere).
- Account, billing, team management — Tarn is a local-only tool. No login, ever.
- Multi-window or detachable panels.
- AI chat assistant inside the app.
- Aggregate analytics / trend dashboards — Tarn doesn't do longitudinal analysis. Each run is a snapshot.
- A request-builder for ad-hoc HTTP calls. That's Postman's job.

---

## What we want back

1. **A point of view on layout.** Pick one direction and defend it with a sentence. We are not asking for three variants of the same idea — we want one good answer.

2. **High-fidelity mockups for, at minimum:**
   - Empty state, first launch.
   - Empty state, with recent projects.
   - Project just opened, idle.
   - Run mid-stream (this is the heartbeat — invest here).
   - Run finished, all passed.
   - Run finished, with failures.
   - Step selected — passed.
   - Step selected — failed (this is the marquee surface — invest here too).

3. **Treatments for the small components**, in the chosen design language:
   - Status indicator (the single most-used atom — show passed, failed, running, skipped, pending).
   - Primary button.
   - Secondary / ghost button.
   - Tag chip (active and inactive).
   - Dropdown (env picker, history).
   - Text field (for the future YAML editor placeholder).
   - Tabs or panel headers (whatever you choose for organizing the detail surface).
   - Code/payload block (monospace, for JSON bodies).

4. **A short notes document covering:**
   - Interaction model: how the user navigates between files / tests / steps. What happens on click vs hover vs keyboard.
   - Motion: what animates, what doesn't, durations.
   - Keyboard shortcuts: a defensible minimal set.
   - Empty state copy: the exact words.

5. **One thing we haven't asked for.** A small, sharp idea that makes the product better than this brief asked. The brief is a constraint, not a ceiling. If you see a missing affordance or a quietly delightful detail we missed, include it.

---

## How to use the brief, if you're a designer

- Read the personas first. If you can't picture Sasha, the rest won't land.
- Find a real Tarn-CLI screenshot or run output to ground your understanding of the data. The streaming JSON is the soul of the product.
- Make at least one of the mockups feel like something Theo would actually post. If your "all passed" state is just a screen with green checks, you've shipped less than the brief allows.
- Push back. If a constraint here is wrong, write a note. The brief is opinionated, but it's also a draft.

---

## Context we left out on purpose

We had earlier discussions about specific layouts (two-column vs three-column), specific organizing principles (tabs vs accordion), specific iconography (dots vs glyphs). **None of that is part of this brief.** Those decisions are yours. We don't want a designer who recreates a list — we want one who asks "given these users and this data, what does the calmest possible surface look like?"
