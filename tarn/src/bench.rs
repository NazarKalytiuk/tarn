use crate::error::TarnError;
use crate::http;
use crate::interpolation::{self, Context};
use crate::model::{AuthConfig, HttpTransportConfig, HttpVersionPreference, Step, TestFile};
use base64::Engine;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

/// Options for a benchmark run.
#[derive(Debug, Clone)]
pub struct BenchOptions {
    /// Total number of requests to send
    pub requests: u64,
    /// Number of concurrent workers
    pub concurrency: u64,
    /// Ramp-up duration (gradually add workers)
    pub ramp_up: Option<Duration>,
    /// Optional CI-style threshold checks
    pub thresholds: BenchThresholds,
}

#[derive(Debug, Clone, Default)]
pub struct BenchThresholds {
    pub min_throughput_rps: Option<f64>,
    pub max_error_rate: Option<f64>,
    pub max_p95_ms: Option<u64>,
    pub max_p99_ms: Option<u64>,
}

/// Result of a single request in the benchmark.
#[derive(Debug, Clone)]
struct RequestResult {
    status: u16,
    success: bool,
    error: Option<String>,
    timings: RequestTimings,
}

#[derive(Debug, Clone)]
struct RequestTimings {
    total_ms: u64,
    ttfb_ms: u64,
    body_read_ms: u64,
}

#[derive(Debug, Clone)]
enum BenchPayload {
    Json(serde_json::Value),
    Form(IndexMap<String, String>),
}

fn effective_auth<'a>(step: &'a Step, test_file: &'a TestFile) -> Option<&'a AuthConfig> {
    step.request.auth.as_ref().or_else(|| {
        test_file
            .defaults
            .as_ref()
            .and_then(|defaults| defaults.auth.as_ref())
    })
}

fn apply_auth_header(
    headers: &mut HashMap<String, String>,
    auth: Option<&AuthConfig>,
    ctx: &Context,
) {
    if headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"))
    {
        return;
    }

    let Some(auth) = auth else {
        return;
    };

    if let Some(token) = auth.bearer.as_ref() {
        headers.insert(
            "Authorization".into(),
            format!("Bearer {}", interpolation::interpolate(token, ctx)),
        );
    } else if let Some(basic) = auth.basic.as_ref() {
        let username = interpolation::interpolate(&basic.username, ctx);
        let password = interpolation::interpolate(&basic.password, ctx);
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        headers.insert("Authorization".into(), format!("Basic {encoded}"));
    }
}

/// Aggregated benchmark results.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchResult {
    pub step_name: String,
    pub method: String,
    pub url: String,
    pub concurrency: u64,
    pub ramp_up_ms: Option<u64>,
    pub total_requests: u64,
    pub successful: u64,
    pub failed: u64,
    pub error_rate: f64,
    pub total_duration_ms: u64,
    pub throughput_rps: f64,
    pub latency: LatencyStats,
    pub timings: TimingBreakdown,
    pub status_codes: HashMap<u16, u64>,
    pub errors: Vec<String>,
    pub gates: Vec<BenchGateResult>,
    pub passed_gates: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LatencyStats {
    pub min_ms: u64,
    pub max_ms: u64,
    pub mean_ms: f64,
    pub median_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
    pub stdev_ms: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TimingBreakdown {
    pub total: LatencyStats,
    pub ttfb: LatencyStats,
    pub body_read: LatencyStats,
    pub connect: Option<LatencyStats>,
    pub tls: Option<LatencyStats>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchGateResult {
    pub name: String,
    pub passed: bool,
    pub expected: String,
    pub actual: String,
    pub message: String,
}

/// Run a benchmark against a single step from a test file.
pub fn run_bench(
    test_file: &TestFile,
    step_index: usize,
    env: &HashMap<String, String>,
    opts: &BenchOptions,
    http_config: &HttpTransportConfig,
) -> Result<BenchResult, TarnError> {
    let step = resolve_step(test_file, step_index)?;

    // Build interpolation context
    let ctx = Context {
        env: env.clone(),
        captures: HashMap::new(),
    };

    // Interpolate the request once (captures won't work in bench mode)
    let url = interpolation::interpolate(&step.request.url, &ctx);
    let mut merged_headers = test_file
        .defaults
        .as_ref()
        .map(|d| d.headers.clone())
        .unwrap_or_default();
    for (k, v) in &step.request.headers {
        merged_headers.insert(k.clone(), v.clone());
    }
    apply_auth_header(&mut merged_headers, effective_auth(step, test_file), &ctx);
    let payload = if let Some(ref form) = step.request.form {
        let form = interpolation::interpolate_string_map(form, &ctx);
        merged_headers
            .entry("Content-Type".to_string())
            .or_insert_with(|| "application/x-www-form-urlencoded".to_string());
        Some(BenchPayload::Form(form))
    } else {
        step.request
            .body
            .as_ref()
            .map(|b| BenchPayload::Json(interpolation::interpolate_json(b, &ctx)))
    };
    let headers = interpolation::interpolate_headers(&merged_headers, &ctx);

    let method = step.request.method.clone();
    let step_name = step.name.clone();

    // Expected status from assertions (if any) — extract exact status for bench mode
    let expected_status = step.assertions.as_ref().and_then(|a| {
        a.status.as_ref().and_then(|s| match s {
            crate::model::StatusAssertion::Exact(code) => Some(*code),
            _ => None, // Bench mode only supports exact status checks
        })
    });

    let bench_req = BenchRequest {
        step_name: &step_name,
        method: &method,
        url: &url,
        headers: &headers,
        payload: payload.as_ref(),
        expected_status,
    };

    // Run the benchmark using tokio runtime
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| TarnError::Http(format!("Failed to create async runtime: {}", e)))?;

    let result = rt.block_on(run_bench_async(&bench_req, opts, http_config))?;

    Ok(result)
}

struct BenchRequest<'a> {
    step_name: &'a str,
    method: &'a str,
    url: &'a str,
    headers: &'a HashMap<String, String>,
    payload: Option<&'a BenchPayload>,
    expected_status: Option<u16>,
}

async fn run_bench_async(
    req: &BenchRequest<'_>,
    opts: &BenchOptions,
    http_config: &HttpTransportConfig,
) -> Result<BenchResult, TarnError> {
    let semaphore = Arc::new(Semaphore::new(opts.concurrency as usize));
    let completed = Arc::new(AtomicU64::new(0));

    let client = http::build_async_client_with_timeout(http_config, Some(Duration::from_secs(30)))?;

    let overall_start = Instant::now();
    let http_version = http_config.http_version;

    let mut handles = Vec::with_capacity(opts.requests as usize);

    for i in 0..opts.requests {
        // Ramp-up: stagger initial requests
        if let Some(ramp) = opts.ramp_up {
            if i < opts.concurrency {
                let delay_per_worker = ramp / opts.concurrency as u32;
                let delay = delay_per_worker * i as u32;
                tokio::time::sleep(delay).await;
            }
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let method = req.method.to_string();
        let url = req.url.to_string();
        let headers = req.headers.clone();
        let payload = req.payload.cloned();
        let completed = completed.clone();
        let expected_status = req.expected_status;

        let handle = tokio::spawn(async move {
            let result = execute_single(
                &client,
                &method,
                &url,
                &headers,
                payload.as_ref(),
                expected_status,
                http_version,
            )
            .await;
            completed.fetch_add(1, Ordering::Relaxed);
            drop(permit);
            result
        });

        handles.push(handle);
    }

    // Collect results
    let mut results = Vec::with_capacity(opts.requests as usize);
    for handle in handles {
        match handle.await {
            Ok(r) => results.push(r),
            Err(e) => results.push(RequestResult {
                status: 0,
                success: false,
                error: Some(format!("Task failed: {}", e)),
                timings: RequestTimings {
                    total_ms: 0,
                    ttfb_ms: 0,
                    body_read_ms: 0,
                },
            }),
        }
    }

    let total_duration_ms = overall_start.elapsed().as_millis() as u64;

    let mut result = aggregate_results(
        req.step_name,
        req.method,
        req.url,
        opts.concurrency,
        opts.ramp_up,
        results,
        total_duration_ms,
    );
    result.gates = evaluate_gates(&result, &opts.thresholds);
    result.passed_gates = result.gates.iter().all(|gate| gate.passed);

    Ok(result)
}

async fn execute_single(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    payload: Option<&BenchPayload>,
    expected_status: Option<u16>,
    http_version: Option<HttpVersionPreference>,
) -> RequestResult {
    let req_method = match reqwest::Method::from_bytes(method.trim().as_bytes()) {
        Ok(method) => method,
        Err(error) => {
            return RequestResult {
                status: 0,
                success: false,
                error: Some(format!("Invalid HTTP method '{}': {}", method, error)),
                timings: RequestTimings {
                    total_ms: 0,
                    ttfb_ms: 0,
                    body_read_ms: 0,
                },
            }
        }
    };

    let mut builder = client.request(req_method, url);

    builder = match http_version {
        Some(HttpVersionPreference::Http1_1) => builder.version(reqwest::Version::HTTP_11),
        Some(HttpVersionPreference::Http2) => builder.version(reqwest::Version::HTTP_2),
        None => builder,
    };

    for (k, v) in headers {
        builder = builder.header(k, v);
    }

    if let Some(payload) = payload {
        builder = match payload {
            BenchPayload::Json(body) => builder.json(body),
            BenchPayload::Form(form) => match http::encode_form_body(form) {
                Ok(body) => builder.body(body),
                Err(error) => {
                    return RequestResult {
                        status: 0,
                        success: false,
                        error: Some(error.to_string()),
                        timings: RequestTimings {
                            total_ms: 0,
                            ttfb_ms: 0,
                            body_read_ms: 0,
                        },
                    }
                }
            },
        };
    }

    let start = Instant::now();
    match builder.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let ttfb_ms = start.elapsed().as_millis() as u64;
            let body_start = Instant::now();
            // Consume body to complete the request
            let _ = resp.bytes().await;
            let body_read_ms = body_start.elapsed().as_millis() as u64;
            let duration_ms = ttfb_ms.saturating_add(body_read_ms);
            let success = expected_status.map(|e| e == status).unwrap_or(true);
            RequestResult {
                status,
                success,
                error: if success {
                    None
                } else {
                    Some(format!(
                        "Expected status {}, got {}",
                        expected_status.unwrap(),
                        status
                    ))
                },
                timings: RequestTimings {
                    total_ms: duration_ms,
                    ttfb_ms,
                    body_read_ms,
                },
            }
        }
        Err(e) => RequestResult {
            status: 0,
            success: false,
            error: Some(e.to_string()),
            timings: RequestTimings {
                total_ms: 0,
                ttfb_ms: 0,
                body_read_ms: 0,
            },
        },
    }
}

fn aggregate_results(
    step_name: &str,
    method: &str,
    url: &str,
    concurrency: u64,
    ramp_up: Option<Duration>,
    results: Vec<RequestResult>,
    total_duration_ms: u64,
) -> BenchResult {
    let total = results.len() as u64;
    let successful = results.iter().filter(|r| r.success).count() as u64;
    let failed = total - successful;

    // Collect timings from successful requests
    let mut total_latencies: Vec<u64> = results
        .iter()
        .filter(|r| r.success)
        .map(|r| r.timings.total_ms)
        .collect();
    let mut ttfb_latencies: Vec<u64> = results
        .iter()
        .filter(|r| r.success)
        .map(|r| r.timings.ttfb_ms)
        .collect();
    let mut body_read_latencies: Vec<u64> = results
        .iter()
        .filter(|r| r.success)
        .map(|r| r.timings.body_read_ms)
        .collect();
    total_latencies.sort();
    ttfb_latencies.sort();
    body_read_latencies.sort();

    let latency = summarize_latencies(&total_latencies);
    let timings = TimingBreakdown {
        total: latency.clone(),
        ttfb: summarize_latencies(&ttfb_latencies),
        body_read: summarize_latencies(&body_read_latencies),
        connect: None,
        tls: None,
    };

    // Status code distribution
    let mut status_codes: HashMap<u16, u64> = HashMap::new();
    for r in &results {
        if r.status > 0 {
            *status_codes.entry(r.status).or_insert(0) += 1;
        }
    }

    // Unique errors (limit to 10)
    let mut errors: Vec<String> = Vec::new();
    for r in &results {
        if let Some(ref e) = r.error {
            if errors.len() < 10 && !errors.contains(e) {
                errors.push(e.clone());
            }
        }
    }

    let throughput = if total_duration_ms > 0 {
        (total as f64 / total_duration_ms as f64) * 1000.0
    } else {
        0.0
    };

    BenchResult {
        step_name: step_name.to_string(),
        method: method.to_string(),
        url: url.to_string(),
        concurrency,
        ramp_up_ms: ramp_up.map(|duration| duration.as_millis() as u64),
        total_requests: total,
        successful,
        failed,
        error_rate: if total > 0 {
            (failed as f64 / total as f64) * 100.0
        } else {
            0.0
        },
        total_duration_ms,
        throughput_rps: (throughput * 100.0).round() / 100.0,
        latency,
        timings,
        status_codes,
        errors,
        gates: Vec::new(),
        passed_gates: true,
    }
}

fn summarize_latencies(latencies: &[u64]) -> LatencyStats {
    if latencies.is_empty() {
        return LatencyStats {
            min_ms: 0,
            max_ms: 0,
            mean_ms: 0.0,
            median_ms: 0,
            p95_ms: 0,
            p99_ms: 0,
            stdev_ms: 0.0,
        };
    }

    let min = *latencies.first().unwrap();
    let max = *latencies.last().unwrap();
    let sum: u64 = latencies.iter().sum();
    let mean = sum as f64 / latencies.len() as f64;
    let median = percentile(latencies, 50.0);
    let p95 = percentile(latencies, 95.0);
    let p99 = percentile(latencies, 99.0);
    let variance = latencies
        .iter()
        .map(|&value| {
            let diff = value as f64 - mean;
            diff * diff
        })
        .sum::<f64>()
        / latencies.len() as f64;

    LatencyStats {
        min_ms: min,
        max_ms: max,
        mean_ms: (mean * 100.0).round() / 100.0,
        median_ms: median,
        p95_ms: p95,
        p99_ms: p99,
        stdev_ms: (variance.sqrt() * 100.0).round() / 100.0,
    }
}

fn evaluate_gates(result: &BenchResult, thresholds: &BenchThresholds) -> Vec<BenchGateResult> {
    let mut gates = Vec::new();

    if let Some(min_rps) = thresholds.min_throughput_rps {
        let passed = result.throughput_rps >= min_rps;
        gates.push(BenchGateResult {
            name: "throughput_rps".into(),
            passed,
            expected: format!(">= {:.2}", min_rps),
            actual: format!("{:.2}", result.throughput_rps),
            message: if passed {
                "Throughput gate passed".into()
            } else {
                "Throughput dropped below the configured floor".into()
            },
        });
    }

    if let Some(max_error_rate) = thresholds.max_error_rate {
        let passed = result.error_rate <= max_error_rate;
        gates.push(BenchGateResult {
            name: "error_rate".into(),
            passed,
            expected: format!("<= {:.2}", max_error_rate),
            actual: format!("{:.2}", result.error_rate),
            message: if passed {
                "Error-rate gate passed".into()
            } else {
                "Error rate exceeded the configured ceiling".into()
            },
        });
    }

    if let Some(max_p95_ms) = thresholds.max_p95_ms {
        let passed = result.latency.p95_ms <= max_p95_ms;
        gates.push(BenchGateResult {
            name: "latency_p95_ms".into(),
            passed,
            expected: format!("<= {}", max_p95_ms),
            actual: result.latency.p95_ms.to_string(),
            message: if passed {
                "P95 latency gate passed".into()
            } else {
                "P95 latency exceeded the configured ceiling".into()
            },
        });
    }

    if let Some(max_p99_ms) = thresholds.max_p99_ms {
        let passed = result.latency.p99_ms <= max_p99_ms;
        gates.push(BenchGateResult {
            name: "latency_p99_ms".into(),
            passed,
            expected: format!("<= {}", max_p99_ms),
            actual: result.latency.p99_ms.to_string(),
            message: if passed {
                "P99 latency gate passed".into()
            } else {
                "P99 latency exceeded the configured ceiling".into()
            },
        });
    }

    gates
}

fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn resolve_step(test_file: &TestFile, step_index: usize) -> Result<&Step, TarnError> {
    // Try flat steps first
    if !test_file.steps.is_empty() {
        return test_file.steps.get(step_index).ok_or_else(|| {
            TarnError::Config(format!(
                "Step index {} out of range (file has {} steps)",
                step_index,
                test_file.steps.len()
            ))
        });
    }

    // Then try first test group's steps
    if let Some((_, group)) = test_file.tests.iter().next() {
        return group.steps.get(step_index).ok_or_else(|| {
            TarnError::Config(format!(
                "Step index {} out of range (test has {} steps)",
                step_index,
                group.steps.len()
            ))
        });
    }

    Err(TarnError::Config("No steps found in test file".into()))
}

/// Render benchmark results as human-readable output.
pub fn render_human(result: &BenchResult) -> String {
    use colored::Colorize;

    let mut out = String::new();

    out.push_str(&format!(
        "\n {} {} {} — {} requests, {} concurrent\n\n",
        "TARN BENCH".bold().white().on_blue(),
        result.method.bold(),
        result.url.dimmed(),
        result.total_requests,
        result.concurrency,
    ));

    // Request summary
    let ok_str = result.successful.to_string().green();
    let fail_str = if result.failed > 0 {
        result.failed.to_string().red()
    } else {
        result.failed.to_string().dimmed()
    };
    out.push_str(&format!(
        "  {:<14} {} total, {} ok, {} failed ({:.1}%)\n",
        "Requests:".bold(),
        result.total_requests,
        ok_str,
        fail_str,
        result.error_rate
    ));

    // Duration & throughput
    let dur = if result.total_duration_ms >= 1000 {
        format!("{:.2}s", result.total_duration_ms as f64 / 1000.0)
    } else {
        format!("{}ms", result.total_duration_ms)
    };
    out.push_str(&format!("  {:<14} {}\n", "Duration:".bold(), dur));
    out.push_str(&format!(
        "  {:<14} {:.1} req/s\n",
        "Throughput:".bold(),
        result.throughput_rps
    ));

    // Latency
    out.push_str(&format!("\n  {}:\n", "Latency".bold()));
    out.push_str(&format!("    {:<10} {}ms\n", "min", result.latency.min_ms));
    out.push_str(&format!(
        "    {:<10} {}ms\n",
        "p50", result.latency.median_ms
    ));
    out.push_str(&format!(
        "    {:<10} {}ms\n",
        "p95".yellow(),
        result.latency.p95_ms.to_string().yellow()
    ));
    out.push_str(&format!(
        "    {:<10} {}ms\n",
        "p99".red(),
        result.latency.p99_ms.to_string().red()
    ));
    out.push_str(&format!("    {:<10} {}ms\n", "max", result.latency.max_ms));
    out.push_str(&format!(
        "    {:<10} {:.2}ms\n",
        "stdev", result.latency.stdev_ms
    ));

    out.push_str(&format!("\n  {}:\n", "Timings".bold()));
    out.push_str(&format!(
        "    {:<10} p50={}ms p95={}ms p99={}ms\n",
        "ttfb",
        result.timings.ttfb.median_ms,
        result.timings.ttfb.p95_ms,
        result.timings.ttfb.p99_ms
    ));
    out.push_str(&format!(
        "    {:<10} p50={}ms p95={}ms p99={}ms\n",
        "body-read",
        result.timings.body_read.median_ms,
        result.timings.body_read.p95_ms,
        result.timings.body_read.p99_ms
    ));
    out.push_str("    connect    n/a (reqwest client does not expose phase timing)\n");
    out.push_str("    tls        n/a (reqwest client does not expose phase timing)\n");

    // Status codes
    if !result.status_codes.is_empty() {
        out.push_str(&format!("\n  {}:\n", "Status codes".bold()));
        let mut codes: Vec<_> = result.status_codes.iter().collect();
        codes.sort_by_key(|(code, _)| *code);
        for (code, count) in codes {
            let code_str = if *code >= 200 && *code < 300 {
                code.to_string().green().to_string()
            } else if *code >= 400 {
                code.to_string().red().to_string()
            } else {
                code.to_string()
            };
            out.push_str(&format!("    {} — {} responses\n", code_str, count));
        }
    }

    // Errors
    if !result.errors.is_empty() {
        out.push_str(&format!("\n  {}:\n", "Errors".bold().red()));
        for e in &result.errors {
            out.push_str(&format!("    - {}\n", e.red()));
        }
    }

    if !result.gates.is_empty() {
        out.push_str(&format!("\n  {}:\n", "CI gates".bold()));
        for gate in &result.gates {
            let status = if gate.passed {
                "PASS".green().to_string()
            } else {
                "FAIL".red().to_string()
            };
            out.push_str(&format!(
                "    [{}] {} expected {} actual {}\n",
                status, gate.name, gate.expected, gate.actual
            ));
        }
    }

    out.push('\n');
    out
}

/// Render benchmark results as JSON.
pub fn render_json(result: &BenchResult) -> String {
    serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".to_string())
}

/// Render benchmark results as a single-row CSV summary.
pub fn render_csv(result: &BenchResult) -> String {
    let mut lines = vec![[
        "step_name",
        "method",
        "url",
        "concurrency",
        "requests",
        "successful",
        "failed",
        "error_rate",
        "throughput_rps",
        "latency_p50_ms",
        "latency_p95_ms",
        "latency_p99_ms",
        "ttfb_p50_ms",
        "ttfb_p95_ms",
        "ttfb_p99_ms",
        "body_read_p50_ms",
        "body_read_p95_ms",
        "body_read_p99_ms",
        "passed_gates",
    ]
    .join(",")];

    lines.push(
        vec![
            csv_escape(&result.step_name),
            csv_escape(&result.method),
            csv_escape(&result.url),
            result.concurrency.to_string(),
            result.total_requests.to_string(),
            result.successful.to_string(),
            result.failed.to_string(),
            format!("{:.2}", result.error_rate),
            format!("{:.2}", result.throughput_rps),
            result.latency.median_ms.to_string(),
            result.latency.p95_ms.to_string(),
            result.latency.p99_ms.to_string(),
            result.timings.ttfb.median_ms.to_string(),
            result.timings.ttfb.p95_ms.to_string(),
            result.timings.ttfb.p99_ms.to_string(),
            result.timings.body_read.median_ms.to_string(),
            result.timings.body_read.p95_ms.to_string(),
            result.timings.body_read.p99_ms.to_string(),
            result.passed_gates.to_string(),
        ]
        .join(","),
    );

    lines.join("\n") + "\n"
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_result(
        duration_ms: u64,
        status: u16,
        success: bool,
        error: Option<&str>,
    ) -> RequestResult {
        RequestResult {
            status,
            success,
            error: error.map(str::to_string),
            timings: RequestTimings {
                total_ms: duration_ms,
                ttfb_ms: duration_ms / 2,
                body_read_ms: duration_ms.saturating_sub(duration_ms / 2),
            },
        }
    }

    fn sample_bench_result() -> BenchResult {
        BenchResult {
            step_name: "test".into(),
            method: "GET".into(),
            url: "http://localhost".into(),
            concurrency: 10,
            ramp_up_ms: None,
            total_requests: 10,
            successful: 9,
            failed: 1,
            error_rate: 10.0,
            total_duration_ms: 500,
            throughput_rps: 20.0,
            latency: LatencyStats {
                min_ms: 5,
                max_ms: 50,
                mean_ms: 20.0,
                median_ms: 18,
                p95_ms: 45,
                p99_ms: 50,
                stdev_ms: 12.5,
            },
            timings: TimingBreakdown {
                total: LatencyStats {
                    min_ms: 5,
                    max_ms: 50,
                    mean_ms: 20.0,
                    median_ms: 18,
                    p95_ms: 45,
                    p99_ms: 50,
                    stdev_ms: 12.5,
                },
                ttfb: LatencyStats {
                    min_ms: 2,
                    max_ms: 20,
                    mean_ms: 8.0,
                    median_ms: 7,
                    p95_ms: 18,
                    p99_ms: 20,
                    stdev_ms: 3.5,
                },
                body_read: LatencyStats {
                    min_ms: 1,
                    max_ms: 30,
                    mean_ms: 12.0,
                    median_ms: 10,
                    p95_ms: 27,
                    p99_ms: 30,
                    stdev_ms: 7.0,
                },
                connect: None,
                tls: None,
            },
            status_codes: HashMap::from([(200, 9), (500, 1)]),
            errors: vec!["server error".into()],
            gates: Vec::new(),
            passed_gates: true,
        }
    }

    #[tokio::test]
    async fn execute_single_rejects_invalid_method_token() {
        let client = reqwest::Client::new();
        let result = execute_single(
            &client,
            "BAD METHOD",
            "http://127.0.0.1:1",
            &HashMap::new(),
            None,
            None,
            None,
        )
        .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Invalid HTTP method"));
    }

    #[test]
    fn percentile_empty() {
        assert_eq!(percentile(&[], 50.0), 0);
    }

    #[test]
    fn percentile_single() {
        assert_eq!(percentile(&[42], 50.0), 42);
        assert_eq!(percentile(&[42], 99.0), 42);
    }

    #[test]
    fn percentile_multiple() {
        let data: Vec<u64> = (1..=100).collect();
        // p50 of 1..=100: index round(0.5*99)=50, data[50]=51
        assert_eq!(percentile(&data, 50.0), 51);
        assert_eq!(percentile(&data, 95.0), 95);
        assert_eq!(percentile(&data, 99.0), 99);
    }

    #[test]
    fn percentile_small_set() {
        let data = vec![5, 10, 15, 20, 25];
        assert_eq!(percentile(&data, 50.0), 15);
    }

    #[test]
    fn aggregate_all_success() {
        let results = vec![
            request_result(10, 200, true, None),
            request_result(20, 200, true, None),
            request_result(30, 200, true, None),
        ];
        let agg = aggregate_results("test", "GET", "http://localhost", 3, None, results, 100);
        assert_eq!(agg.total_requests, 3);
        assert_eq!(agg.successful, 3);
        assert_eq!(agg.failed, 0);
        assert_eq!(agg.error_rate, 0.0);
        assert_eq!(agg.latency.min_ms, 10);
        assert_eq!(agg.latency.max_ms, 30);
        assert_eq!(agg.latency.median_ms, 20);
        assert_eq!(*agg.status_codes.get(&200).unwrap(), 3);
    }

    #[test]
    fn aggregate_mixed_results() {
        let results = vec![
            request_result(10, 200, true, None),
            request_result(5, 500, false, Some("server error")),
            request_result(0, 0, false, Some("connection refused")),
        ];
        let agg = aggregate_results("test", "GET", "http://localhost", 2, None, results, 50);
        assert_eq!(agg.total_requests, 3);
        assert_eq!(agg.successful, 1);
        assert_eq!(agg.failed, 2);
        assert!(agg.error_rate > 60.0);
        assert_eq!(agg.errors.len(), 2);
        // Latency only from successful requests
        assert_eq!(agg.latency.min_ms, 10);
        assert_eq!(agg.latency.max_ms, 10);
    }

    #[test]
    fn aggregate_all_failures() {
        let results = vec![request_result(0, 0, false, Some("err"))];
        let agg = aggregate_results("test", "GET", "http://localhost", 1, None, results, 10);
        assert_eq!(agg.successful, 0);
        assert_eq!(agg.failed, 1);
        assert_eq!(agg.latency.min_ms, 0);
        assert_eq!(agg.latency.mean_ms, 0.0);
    }

    #[test]
    fn aggregate_throughput() {
        let results = vec![
            request_result(10, 200, true, None),
            request_result(10, 200, true, None),
        ];
        // 2 requests in 100ms = 20 req/s
        let agg = aggregate_results("test", "GET", "http://localhost", 2, None, results, 100);
        assert_eq!(agg.throughput_rps, 20.0);
    }

    #[test]
    fn aggregate_deduplicates_errors() {
        let results = vec![
            request_result(0, 0, false, Some("same error")),
            request_result(0, 0, false, Some("same error")),
            request_result(0, 0, false, Some("different")),
        ];
        let agg = aggregate_results("test", "GET", "http://localhost", 1, None, results, 10);
        assert_eq!(agg.errors.len(), 2);
    }

    #[test]
    fn resolve_step_flat_steps() {
        let yaml = r#"
name: test
steps:
  - name: first
    request:
      method: GET
      url: "http://localhost"
  - name: second
    request:
      method: POST
      url: "http://localhost"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let step = resolve_step(&tf, 0).unwrap();
        assert_eq!(step.name, "first");
        let step = resolve_step(&tf, 1).unwrap();
        assert_eq!(step.name, "second");
        assert!(resolve_step(&tf, 5).is_err());
    }

    #[test]
    fn resolve_step_test_groups() {
        let yaml = r#"
name: test
tests:
  my_test:
    steps:
      - name: grouped
        request:
          method: GET
          url: "http://localhost"
"#;
        let tf: crate::model::TestFile = serde_yaml::from_str(yaml).unwrap();
        let step = resolve_step(&tf, 0).unwrap();
        assert_eq!(step.name, "grouped");
    }

    #[test]
    fn render_json_output() {
        let result = sample_bench_result();
        let json = render_json(&result);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["total_requests"], 10);
        assert_eq!(parsed["latency"]["p95_ms"], 45);
        assert_eq!(parsed["timings"]["ttfb"]["p95_ms"], 18);
    }

    #[test]
    fn render_human_output() {
        let mut result = sample_bench_result();
        result.step_name = "health".into();
        result.url = "http://localhost/health".into();
        result.total_requests = 100;
        result.successful = 100;
        result.failed = 0;
        result.error_rate = 0.0;
        result.total_duration_ms = 1500;
        result.throughput_rps = 66.67;
        result.status_codes = HashMap::from([(200, 100)]);
        result.errors.clear();
        let output = render_human(&result);
        assert!(output.contains("TARN BENCH"));
        assert!(output.contains("100 total"));
        assert!(output.contains("66.7 req/s"));
        assert!(output.contains("p95"));
        assert!(output.contains("p99"));
        assert!(output.contains("body-read"));
    }

    #[test]
    fn render_csv_output() {
        let csv = render_csv(&sample_bench_result());
        assert!(csv.contains("throughput_rps"));
        assert!(csv.contains("http://localhost"));
    }

    #[test]
    fn evaluate_gates_reports_failures() {
        let mut result = sample_bench_result();
        result.throughput_rps = 15.0;
        result.error_rate = 12.0;
        result.latency.p95_ms = 80;
        let gates = evaluate_gates(
            &result,
            &BenchThresholds {
                min_throughput_rps: Some(20.0),
                max_error_rate: Some(5.0),
                max_p95_ms: Some(50),
                max_p99_ms: None,
            },
        );
        assert_eq!(gates.len(), 3);
        assert!(gates.iter().all(|gate| !gate.passed));
    }
}
