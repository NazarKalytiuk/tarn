use crate::assert::types::RunResult;

/// Render test results as a self-contained HTML dashboard.
pub fn render(result: &RunResult) -> String {
    let json_data = super::json::render(result);
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Tarn Test Report</title>
<style>
{css}
</style>
</head>
<body>
<script>
const DATA = {json_data};
</script>
<div id="app"></div>
<script>
{js}
</script>
</body>
</html>"##,
        css = CSS,
        json_data = json_data,
        js = JS,
    )
}

const CSS: &str = r##"
:root {
  --bg: #0d1117;
  --bg-card: #161b22;
  --bg-hover: #1c2129;
  --bg-step: #1c2333;
  --border: #30363d;
  --text: #e6edf3;
  --text-dim: #8b949e;
  --text-muted: #6e7681;
  --green: #3fb950;
  --green-bg: rgba(63,185,80,0.12);
  --red: #f85149;
  --red-bg: rgba(248,81,73,0.12);
  --blue: #58a6ff;
  --blue-bg: rgba(88,166,255,0.12);
  --orange: #d29922;
  --orange-bg: rgba(210,153,34,0.12);
  --purple: #bc8cff;
  --radius: 12px;
  --radius-sm: 8px;
  --shadow: 0 2px 12px rgba(0,0,0,0.3);
  --font: -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif;
  --mono: 'SF Mono', 'Cascadia Code', 'Fira Code', Consolas, monospace;
}

* { margin: 0; padding: 0; box-sizing: border-box; }

body {
  font-family: var(--font);
  background: var(--bg);
  color: var(--text);
  line-height: 1.6;
  min-height: 100vh;
}

.container { max-width: 1200px; margin: 0 auto; padding: 24px; }

/* Header */
.header {
  text-align: center;
  padding: 48px 24px 32px;
  position: relative;
}
.header::after {
  content: '';
  position: absolute;
  bottom: 0;
  left: 50%;
  transform: translateX(-50%);
  width: 80px;
  height: 3px;
  border-radius: 2px;
  background: linear-gradient(90deg, var(--blue), var(--purple));
}
.logo {
  font-size: 14px;
  font-weight: 600;
  letter-spacing: 4px;
  text-transform: uppercase;
  color: var(--text-dim);
  margin-bottom: 8px;
}
.logo span { color: var(--blue); }
.header h1 { font-size: 28px; font-weight: 700; margin-bottom: 6px; }
.header .timestamp { font-size: 13px; color: var(--text-muted); font-family: var(--mono); }

/* Summary Cards */
.summary-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 16px;
  margin: 32px auto;
}
.summary-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 20px;
  text-align: center;
  transition: transform 0.15s, border-color 0.15s;
}
.summary-card:hover { transform: translateY(-2px); }
.summary-card.passed { border-color: var(--green); }
.summary-card.failed { border-color: var(--red); }
.summary-card .value {
  font-size: 36px;
  font-weight: 800;
  font-family: var(--mono);
  line-height: 1.2;
}
.summary-card .label {
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 1.5px;
  color: var(--text-dim);
  margin-top: 4px;
}
.summary-card .value.green { color: var(--green); }
.summary-card .value.red { color: var(--red); }
.summary-card .value.blue { color: var(--blue); }
.summary-card .value.dim { color: var(--text-dim); }

/* Status Banner */
.status-banner {
  text-align: center;
  padding: 16px;
  border-radius: var(--radius);
  font-size: 18px;
  font-weight: 700;
  letter-spacing: 2px;
  text-transform: uppercase;
  margin-bottom: 32px;
}
.status-banner.passed {
  background: var(--green-bg);
  color: var(--green);
  border: 1px solid rgba(63,185,80,0.3);
}
.status-banner.failed {
  background: var(--red-bg);
  color: var(--red);
  border: 1px solid rgba(248,81,73,0.3);
}

/* Progress Bar */
.progress-bar-wrap {
  width: 100%;
  height: 8px;
  background: var(--bg);
  border-radius: 4px;
  overflow: hidden;
  margin: 24px 0;
}
.progress-bar {
  height: 100%;
  border-radius: 4px;
  transition: width 0.6s ease;
}
.progress-bar.full { background: var(--green); }
.progress-bar.partial {
  background: linear-gradient(90deg, var(--green) var(--pass-pct), var(--red) var(--pass-pct));
}

/* File Cards */
.file-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  margin-bottom: 16px;
  overflow: hidden;
  box-shadow: var(--shadow);
}
.file-header {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 16px 20px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}
.file-header:hover { background: var(--bg-hover); }
.file-chevron {
  font-size: 12px;
  color: var(--text-muted);
  transition: transform 0.2s;
  flex-shrink: 0;
}
.file-card.open .file-chevron { transform: rotate(90deg); }
.file-status {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex-shrink: 0;
}
.file-status.passed { background: var(--green); box-shadow: 0 0 8px rgba(63,185,80,0.4); }
.file-status.failed { background: var(--red); box-shadow: 0 0 8px rgba(248,81,73,0.4); }
.file-name { font-weight: 600; flex: 1; }
.file-path { font-size: 12px; color: var(--text-muted); font-family: var(--mono); }
.file-meta {
  display: flex;
  gap: 16px;
  align-items: center;
  font-size: 13px;
  color: var(--text-dim);
}
.file-meta .count { font-family: var(--mono); }
.file-meta .pass { color: var(--green); }
.file-meta .fail { color: var(--red); }
.file-meta .duration { color: var(--text-muted); font-family: var(--mono); }
.file-body { display: none; padding: 0 20px 16px; }
.file-card.open .file-body { display: block; }

/* Sections: setup / tests / teardown */
.section-label {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 1.5px;
  color: var(--text-muted);
  padding: 12px 0 6px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 8px;
}

/* Test group */
.test-group {
  margin: 8px 0;
}
.test-group-header {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: background 0.15s;
}
.test-group-header:hover { background: var(--bg-hover); }
.test-group-name { font-weight: 600; font-size: 14px; }
.test-group-desc { color: var(--text-muted); font-size: 13px; margin-left: 4px; }
.test-group-body { display: none; padding-left: 16px; }
.test-group.open .test-group-body { display: block; }

/* Steps */
.step {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 8px 12px;
  border-radius: var(--radius-sm);
  margin: 2px 0;
  transition: background 0.15s;
}
.step:hover { background: var(--bg-step); }
.step-icon { flex-shrink: 0; font-size: 14px; margin-top: 2px; }
.step-icon.pass { color: var(--green); }
.step-icon.fail { color: var(--red); }
.step-info { flex: 1; min-width: 0; }
.step-name { font-size: 14px; }
.step-name.fail { color: var(--red); }
.step-duration {
  font-size: 12px;
  color: var(--text-muted);
  font-family: var(--mono);
  flex-shrink: 0;
}
.step-assertions {
  font-size: 12px;
  color: var(--text-dim);
  margin-top: 2px;
}

/* Duration bar */
.dur-bar-wrap {
  width: 60px;
  height: 4px;
  background: var(--bg);
  border-radius: 2px;
  overflow: hidden;
  flex-shrink: 0;
  margin-top: 8px;
}
.dur-bar {
  height: 100%;
  border-radius: 2px;
  background: var(--blue);
  opacity: 0.6;
  transition: width 0.4s;
}

/* Failure details */
.failure-block {
  background: var(--bg);
  border: 1px solid rgba(248,81,73,0.2);
  border-left: 3px solid var(--red);
  border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
  padding: 12px 16px;
  margin: 6px 0 6px 24px;
  font-size: 13px;
}
.failure-block .fail-assertion {
  color: var(--red);
  font-weight: 600;
  margin-bottom: 6px;
}
.failure-block .fail-row {
  display: flex;
  gap: 8px;
  margin: 3px 0;
  font-family: var(--mono);
  font-size: 12px;
}
.failure-block .fail-label {
  color: var(--text-muted);
  min-width: 70px;
  flex-shrink: 0;
}
.failure-block .fail-expected { color: var(--green); }
.failure-block .fail-actual { color: var(--red); }
.failure-block .fail-message {
  color: var(--text-dim);
  margin-top: 8px;
}
.diff-block {
  margin-top: 10px;
  background: #0b1220;
  border: 1px solid rgba(88,166,255,0.2);
  border-radius: var(--radius-sm);
  overflow: hidden;
}
.diff-title {
  padding: 8px 12px;
  font-size: 11px;
  letter-spacing: 1px;
  text-transform: uppercase;
  color: var(--text-muted);
  border-bottom: 1px solid rgba(88,166,255,0.15);
}
.diff-pre {
  margin: 0;
  padding: 12px;
  font-size: 11px;
  line-height: 1.5;
  overflow-x: auto;
}
.diff-line.add { color: var(--green); }
.diff-line.del { color: var(--red); }
.diff-line.meta { color: var(--blue); }

/* Request/Response for failures */
.req-resp-block {
  margin: 8px 0 8px 24px;
  font-size: 12px;
  font-family: var(--mono);
}
.req-resp-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 6px;
}
.req-resp-toggle {
  color: var(--blue);
  cursor: pointer;
  font-size: 12px;
  padding: 4px 0;
  user-select: none;
}
.req-resp-toggle:hover { text-decoration: underline; }
.copy-btn {
  border: 1px solid rgba(88,166,255,0.3);
  background: rgba(88,166,255,0.08);
  color: var(--blue);
  border-radius: 999px;
  padding: 4px 10px;
  font-size: 11px;
  cursor: pointer;
}
.copy-btn:hover { background: rgba(88,166,255,0.16); }
.req-resp-content {
  display: none;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 12px;
  margin-top: 6px;
  color: var(--text-dim);
  line-height: 1.5;
}
.req-resp-content.open { display: block; }
.req-label { color: var(--orange); font-weight: 600; margin-bottom: 4px; display: block; }
.payload-panel {
  margin-top: 10px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  overflow: hidden;
}
.payload-panel summary {
  cursor: pointer;
  list-style: none;
  padding: 8px 10px;
  background: rgba(255,255,255,0.02);
  color: var(--text);
  font-size: 11px;
  letter-spacing: 1px;
  text-transform: uppercase;
}
.payload-panel summary::-webkit-details-marker { display: none; }
.payload-pre {
  margin: 0;
  padding: 12px;
  white-space: pre-wrap;
  word-break: break-word;
  overflow-x: auto;
  max-height: 260px;
  overflow-y: auto;
}

/* Assertion details list */
.assert-details-toggle {
  font-size: 12px;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 0;
}
.assert-details-toggle:hover { color: var(--text-dim); }
.assert-details-toggle .chevron {
  font-size: 10px;
  transition: transform 0.15s;
  display: inline-block;
}
.assert-details-toggle.open .chevron { transform: rotate(90deg); }
.assert-details {
  display: none;
  margin: 4px 0 4px 0;
  padding: 0;
  list-style: none;
}
.assert-details.open { display: block; }
.assert-detail {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 3px 0;
  font-size: 12px;
  font-family: var(--mono);
  color: var(--text-dim);
  border-bottom: 1px solid rgba(48,54,61,0.4);
}
.assert-detail:last-child { border-bottom: none; }
.assert-detail .ad-icon { flex-shrink: 0; font-size: 11px; }
.assert-detail .ad-icon.pass { color: var(--green); }
.assert-detail .ad-icon.fail { color: var(--red); }
.assert-detail .ad-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.assert-detail .ad-val { color: var(--text-muted); font-size: 11px; flex-shrink: 0; max-width: 300px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.assert-detail .ad-val.pass { color: var(--green); opacity: 0.7; }
.assert-detail .ad-val.fail { color: var(--red); }

/* Footer */
.footer {
  text-align: center;
  padding: 32px;
  font-size: 12px;
  color: var(--text-muted);
}
.footer a { color: var(--blue); text-decoration: none; }
.footer a:hover { text-decoration: underline; }

/* Animations */
@keyframes fadeIn { from { opacity: 0; transform: translateY(8px); } to { opacity: 1; transform: translateY(0); } }
.file-card { animation: fadeIn 0.3s ease both; }
.file-card:nth-child(2) { animation-delay: 0.05s; }
.file-card:nth-child(3) { animation-delay: 0.1s; }
.file-card:nth-child(4) { animation-delay: 0.15s; }
.file-card:nth-child(5) { animation-delay: 0.2s; }

/* Scrollbar */
::-webkit-scrollbar { width: 6px; height: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--text-muted); }
"##;

const JS: &str = r##"
(function() {
  const d = DATA;
  const app = document.getElementById('app');
  window.__tarnRequests = window.__tarnRequests || {};

  // Helpers
  const esc = s => String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
  const fmtDur = ms => ms >= 1000 ? (ms/1000).toFixed(2)+'s' : ms+'ms';
  const pct = (a,b) => b === 0 ? 100 : Math.round(a/b*100);

  const totalSteps = d.summary.steps.total;
  const passedSteps = d.summary.steps.passed;
  const failedSteps = d.summary.steps.failed;
  const passPct = pct(passedSteps, totalSteps);
  const status = d.summary.status;

  let html = '';

  // Header
  html += `<div class="header">
    <div class="logo"><span>&#x2B21;</span> TARN</div>
    <h1>Test Report</h1>
    <div class="timestamp">${esc(d.timestamp)}</div>
  </div>`;

  html += '<div class="container">';

  // Status banner
  html += `<div class="status-banner ${status === 'PASSED' ? 'passed' : 'failed'}">
    ${status === 'PASSED' ? '&#10003; ALL TESTS PASSED' : '&#10007; SOME TESTS FAILED'}
  </div>`;

  // Summary cards
  html += '<div class="summary-grid">';
  html += `<div class="summary-card"><div class="value blue">${totalSteps}</div><div class="label">Total Steps</div></div>`;
  html += `<div class="summary-card passed"><div class="value green">${passedSteps}</div><div class="label">Passed</div></div>`;
  html += `<div class="summary-card ${failedSteps > 0 ? 'failed' : ''}"><div class="value ${failedSteps > 0 ? 'red' : 'dim'}">${failedSteps}</div><div class="label">Failed</div></div>`;
  html += `<div class="summary-card"><div class="value dim">${fmtDur(d.duration_ms)}</div><div class="label">Duration</div></div>`;
  html += `<div class="summary-card"><div class="value dim">${d.summary.files}</div><div class="label">Files</div></div>`;
  html += `<div class="summary-card"><div class="value dim">${d.summary.tests}</div><div class="label">Tests</div></div>`;
  html += '</div>';

  // Progress bar
  if (totalSteps > 0) {
    const style = failedSteps === 0
      ? 'width:100%;" class="progress-bar full'
      : `width:100%;--pass-pct:${passPct}%;" class="progress-bar partial`;
    html += `<div class="progress-bar-wrap"><div style="${style}"></div></div>`;
  }

  // Find max duration for relative bars
  let maxDur = 1;
  d.files.forEach(f => {
    (f.setup||[]).forEach(s => { if(s.duration_ms > maxDur) maxDur = s.duration_ms; });
    (f.tests||[]).forEach(t => { (t.steps||[]).forEach(s => { if(s.duration_ms > maxDur) maxDur = s.duration_ms; }); });
    (f.teardown||[]).forEach(s => { if(s.duration_ms > maxDur) maxDur = s.duration_ms; });
  });

  // File cards
  d.files.forEach((file, fi) => {
    const fStatus = file.status === 'PASSED' ? 'passed' : 'failed';
    const fOpen = file.status === 'FAILED' ? ' open' : '';
    html += `<div class="file-card${fOpen}" data-file="${fi}">`;

    // File header
    html += `<div class="file-header" onclick="toggleFile(${fi})">
      <span class="file-chevron">&#9654;</span>
      <span class="file-status ${fStatus}"></span>
      <span class="file-name">${esc(file.name)}</span>
      <span class="file-path">${esc(file.file)}</span>
      <span class="file-meta">
        <span class="count pass">${file.summary.passed}&#10003;</span>
        <span class="count fail">${file.summary.failed}&#10007;</span>
        <span class="duration">${fmtDur(file.duration_ms)}</span>
      </span>
    </div>`;

    html += `<div class="file-body" id="file-body-${fi}">`;

    // Setup
    if (file.setup && file.setup.length > 0) {
      html += '<div class="section-label">Setup</div>';
      file.setup.forEach(step => { html += renderStep(step, maxDur); });
    }

    // Tests
    if (file.tests && file.tests.length > 0) {
      html += '<div class="section-label">Tests</div>';
      file.tests.forEach((test, ti) => {
        const tOpen = test.status === 'FAILED' ? ' open' : '';
        const tStatus = test.status === 'PASSED' ? 'passed' : 'failed';
        html += `<div class="test-group${tOpen}" data-test="${fi}-${ti}">`;
        html += `<div class="test-group-header" onclick="toggleTest('${fi}-${ti}')">
          <span class="file-chevron">&#9654;</span>
          <span class="file-status ${tStatus}"></span>
          <span class="test-group-name">${esc(test.name)}</span>
          ${test.description ? '<span class="test-group-desc">&mdash; '+esc(test.description)+'</span>' : ''}
          <span class="file-meta">
            <span class="duration">${fmtDur(test.duration_ms)}</span>
          </span>
        </div>`;
        html += `<div class="test-group-body">`;
        (test.steps||[]).forEach(step => { html += renderStep(step, maxDur); });
        html += '</div></div>';
      });
    }

    // Teardown
    if (file.teardown && file.teardown.length > 0) {
      html += '<div class="section-label">Teardown</div>';
      file.teardown.forEach(step => { html += renderStep(step, maxDur); });
    }

    html += '</div></div>';
  });

  // Footer
  html += `<div class="footer">Generated by <a href="#">Tarn</a> &middot; ${esc(d.timestamp)}</div>`;
  html += '</div>';

  app.innerHTML = html;

  // Step renderer
  function renderStep(step, maxDur) {
    const pass = step.status === 'PASSED';
    const icon = pass ? '&#10003;' : '&#10007;';
    const iconClass = pass ? 'pass' : 'fail';
    const nameClass = pass ? '' : ' fail';
    const durW = Math.max(4, Math.round(step.duration_ms / maxDur * 100));
    const details = (step.assertions && step.assertions.details) || [];
    const hasDetails = details.length > 0;
    const assertInfo = step.assertions
      ? `${step.assertions.passed}/${step.assertions.total} assertions`
      : '';
    const detailId = 'ad-' + Math.random().toString(36).substr(2,8);

    let h = `<div class="step">
      <span class="step-icon ${iconClass}">${icon}</span>
      <div class="step-info">
        <div class="step-name${nameClass}">${esc(step.name)}</div>`;

    // Assertion summary — clickable to expand details
    if (hasDetails) {
      h += `<div class="assert-details-toggle" onclick="toggleDetails('${detailId}', this)">
        <span class="chevron">&#9654;</span> ${assertInfo}
      </div>`;
      h += `<ul class="assert-details" id="${detailId}">`;
      details.forEach(a => {
        const aIcon = a.passed ? '&#10003;' : '&#10007;';
        const aClass = a.passed ? 'pass' : 'fail';
        const valText = a.passed
          ? esc(a.actual)
          : esc(a.expected) + ' &#8800; ' + esc(a.actual);
        h += `<li class="assert-detail">
          <span class="ad-icon ${aClass}">${aIcon}</span>
          <span class="ad-name">${esc(a.assertion)}</span>
          <span class="ad-val ${aClass}">${valText}</span>
        </li>`;
      });
      h += '</ul>';
    } else if (assertInfo) {
      h += `<div class="step-assertions">${assertInfo}</div>`;
    }

    // Failure blocks for failed assertions
    if (!pass && step.assertions && step.assertions.failures) {
      step.assertions.failures.forEach(f => {
        h += `<div class="failure-block">
          <div class="fail-assertion">${esc(f.assertion)}</div>
          <div class="fail-row"><span class="fail-label">expected:</span><span class="fail-expected">${esc(f.expected)}</span></div>
          <div class="fail-row"><span class="fail-label">actual:</span><span class="fail-actual">${esc(f.actual)}</span></div>
          ${f.message ? `<div class="fail-message">${esc(f.message)}</div>` : ''}
          ${f.diff ? renderDiff(f.diff) : ''}
        </div>`;
      });

      // Request/Response
      if (step.request || step.response) {
        const id = 'rr-' + Math.random().toString(36).substr(2,8);
        const curlId = 'curl-' + Math.random().toString(36).substr(2,8);
        if (step.request) {
          window.__tarnRequests[curlId] = step.request;
        }
        h += `<div class="req-resp-block">
          <div class="req-resp-actions">
            <div class="req-resp-toggle" onclick="toggleRR('${id}')">&#9654; Show request/response</div>
            ${step.request ? `<button class="copy-btn" onclick="copyCurl('${curlId}')">Copy cURL</button>` : ''}
          </div>
          <div class="req-resp-content" id="${id}">`;
        if (step.request) {
          h += `<span class="req-label">REQUEST</span>${esc(step.request.method)} ${esc(step.request.url)}`;
          if (step.request.headers) {
            h += renderPayloadPanel('Request headers', renderHeaders(step.request.headers));
          }
          if (step.request.body) {
            h += renderPayloadPanel('Request body', toPretty(step.request.body));
          }
        }
        if (step.response) {
          h += `\n<span class="req-label">RESPONSE ${step.response.status}</span>`;
          if (step.response.headers) {
            h += renderPayloadPanel('Response headers', renderHeaders(step.response.headers));
          }
          if (step.response.body) {
            h += renderPayloadPanel('Response body', toPretty(step.response.body));
          }
        }
        h += '</div></div>';
      }
    }

    h += `</div>
      <span class="step-duration">${fmtDur(step.duration_ms)}</span>
      <div class="dur-bar-wrap"><div class="dur-bar" style="width:${durW}%"></div></div>
    </div>`;
    return h;
  }

  // Toggle functions
  window.toggleFile = function(i) {
    const el = document.querySelector(`.file-card[data-file="${i}"]`);
    if (el) el.classList.toggle('open');
  };
  window.toggleTest = function(key) {
    const el = document.querySelector(`.test-group[data-test="${key}"]`);
    if (el) el.classList.toggle('open');
  };
  window.toggleRR = function(id) {
    const el = document.getElementById(id);
    if (el) el.classList.toggle('open');
  };
  window.toggleDetails = function(id, btn) {
    const el = document.getElementById(id);
    if (el) el.classList.toggle('open');
    if (btn) btn.classList.toggle('open');
  };
  window.copyCurl = function(id) {
    const request = window.__tarnRequests[id];
    if (!request) return;
    const lines = [`curl -X ${shellEscape(request.method)} ${shellEscape(request.url)}`];
    Object.entries(request.headers || {}).forEach(([key, value]) => {
      lines.push(`  -H ${shellEscape(`${key}: ${value}`)}`);
    });
    if (request.body !== null && request.body !== undefined) {
      lines.push(`  --data-raw ${shellEscape(toPretty(request.body))}`);
    }
    copyText(lines.join(' \\\n'));
  };
  function renderHeaders(headers) {
    return Object.entries(headers || {}).map(([key, value]) => `${key}: ${value}`).join('\n');
  }
  function renderPayloadPanel(label, content) {
    return `<details class="payload-panel">
      <summary>${esc(label)}</summary>
      <pre class="payload-pre">${esc(content)}</pre>
    </details>`;
  }
  function renderDiff(diff) {
    const lines = diff.split('\n').map(line => {
      let cls = '';
      if (line.startsWith('+')) cls = 'add';
      else if (line.startsWith('-')) cls = 'del';
      else if (line.startsWith('@@')) cls = 'meta';
      return `<div class="diff-line ${cls}">${esc(line)}</div>`;
    }).join('');
    return `<div class="diff-block"><div class="diff-title">Unified diff</div><pre class="diff-pre">${lines}</pre></div>`;
  }
  function toPretty(value) {
    return typeof value === 'string' ? value : JSON.stringify(value, null, 2);
  }
  function shellEscape(value) {
    return `'${String(value).replace(/'/g, `'\"'\"'`)}'`;
  }
  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text);
    }
  }
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::*;
    use std::collections::HashMap;

    fn make_passing_run() -> RunResult {
        RunResult {
            duration_ms: 150,
            file_results: vec![FileResult {
                file: "test.tarn.yaml".into(),
                name: "Test Suite".into(),
                passed: true,
                duration_ms: 150,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![StepResult {
                    name: "Auth".into(),
                    description: None,
                    debug: false,
                    passed: true,
                    duration_ms: 50,
                    assertion_results: vec![AssertionResult::pass("status", "200", "200")],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
                }],
                test_results: vec![TestResult {
                    name: "my_test".into(),
                    description: Some("A test description".into()),
                    passed: true,
                    duration_ms: 80,
                    step_results: vec![
                        StepResult {
                            name: "GET /users".into(),
                            description: None,
                            debug: false,
                            passed: true,
                            duration_ms: 40,
                            assertion_results: vec![
                                AssertionResult::pass("status", "200", "200"),
                                AssertionResult::pass("body $.name", "\"Alice\"", "\"Alice\""),
                            ],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                            location: None,
                        },
                        StepResult {
                            name: "POST /users".into(),
                            description: None,
                            debug: false,
                            passed: true,
                            duration_ms: 40,
                            assertion_results: vec![AssertionResult::pass("status", "201", "201")],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                            location: None,
                        },
                    ],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![StepResult {
                    name: "Cleanup".into(),
                    description: None,
                    debug: false,
                    passed: true,
                    duration_ms: 20,
                    assertion_results: vec![],
                    request_info: None,
                    response_info: None,
                    error_category: None,
                    response_status: None,
                    response_summary: None,
                    captures_set: vec![],
                    location: None,
                }],
            }],
        }
    }

    fn make_failing_run() -> RunResult {
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer secret".into());

        RunResult {
            duration_ms: 200,
            file_results: vec![FileResult {
                file: "crud.tarn.yaml".into(),
                name: "CRUD Tests".into(),
                passed: false,
                duration_ms: 200,
                redaction: crate::model::RedactionConfig::default(),
                redacted_values: vec![],
                setup_results: vec![],
                test_results: vec![TestResult {
                    name: "create_user".into(),
                    description: Some("Create and verify user".into()),
                    passed: false,
                    duration_ms: 200,
                    step_results: vec![
                        StepResult {
                            name: "Create user".into(),
                            description: None,
                            debug: false,
                            passed: true,
                            duration_ms: 100,
                            assertion_results: vec![AssertionResult::pass("status", "201", "201")],
                            request_info: None,
                            response_info: None,
                            error_category: None,
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                            location: None,
                        },
                        StepResult {
                            name: "Verify user".into(),
                            description: None,
                            debug: false,
                            passed: false,
                            duration_ms: 80,
                            assertion_results: vec![
                                AssertionResult::pass("status", "200", "200"),
                                AssertionResult::fail(
                                    "body $.email",
                                    "\"jane@example.com\"",
                                    "\"other@example.com\"",
                                    "JSONPath $.email: expected \"jane@example.com\", got \"other@example.com\"",
                                ),
                            ],
                            request_info: Some(RequestInfo {
                                method: "GET".into(),
                                url: "http://localhost:3000/users/usr_123".into(),
                                headers,
                                body: None,
                                multipart: None,
                            }),
                            response_info: Some(ResponseInfo {
                                status: 200,
                                headers: HashMap::new(),
                                body: Some(serde_json::json!({"email": "other@example.com"})),
                            }),
                            error_category: None,
                            response_status: None,
                            response_summary: None,
                            captures_set: vec![],
                            location: None,
                        },
                    ],
                    captures: HashMap::new(),
                }],
                teardown_results: vec![],
            }],
        }
    }

    #[test]
    fn html_is_valid_structure() {
        let output = render(&make_passing_run());
        assert!(output.starts_with("<!DOCTYPE html>"));
        assert!(output.contains("<html"));
        assert!(output.contains("</html>"));
        assert!(output.contains("<style>"));
        assert!(output.contains("</style>"));
        assert!(output.contains("const DATA ="));
    }

    #[test]
    fn html_contains_test_data() {
        let output = render(&make_passing_run());
        assert!(output.contains("Test Suite"));
        assert!(output.contains("my_test"));
        assert!(output.contains("TARN"));
    }

    #[test]
    fn html_contains_json_data() {
        let output = render(&make_passing_run());
        assert!(output.contains("\"status\": \"PASSED\""));
    }

    #[test]
    fn html_contains_failure_info() {
        let output = render(&make_failing_run());
        assert!(output.contains("\"status\": \"FAILED\""));
        assert!(output.contains("CRUD Tests"));
        assert!(output.contains("Copy cURL"));
        assert!(output.contains("Unified diff"));
    }

    #[test]
    fn html_contains_css() {
        let output = render(&make_passing_run());
        assert!(output.contains("--bg: #0d1117"));
        assert!(output.contains("--green: #3fb950"));
    }

    #[test]
    fn html_contains_javascript() {
        let output = render(&make_passing_run());
        assert!(output.contains("toggleFile"));
        assert!(output.contains("toggleTest"));
    }

    #[test]
    fn html_self_contained() {
        let output = render(&make_passing_run());
        // No external links
        assert!(!output.contains("href=\"http"));
        assert!(!output.contains("src=\"http"));
    }
}
