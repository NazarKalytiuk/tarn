use crate::error::TarnError;
use crate::interpolation::{self, Context};
use crate::model::{Step, TestFile};
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
}

/// Result of a single request in the benchmark.
#[derive(Debug, Clone)]
struct RequestResult {
    duration_ms: u64,
    status: u16,
    success: bool,
    error: Option<String>,
}

/// Aggregated benchmark results.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchResult {
    pub step_name: String,
    pub method: String,
    pub url: String,
    pub total_requests: u64,
    pub successful: u64,
    pub failed: u64,
    pub error_rate: f64,
    pub total_duration_ms: u64,
    pub throughput_rps: f64,
    pub latency: LatencyStats,
    pub status_codes: HashMap<u16, u64>,
    pub errors: Vec<String>,
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

/// Run a benchmark against a single step from a test file.
pub fn run_bench(
    test_file: &TestFile,
    step_index: usize,
    env: &HashMap<String, String>,
    opts: &BenchOptions,
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
    let headers = interpolation::interpolate_headers(&merged_headers, &ctx);
    let body = step
        .request
        .body
        .as_ref()
        .map(|b| interpolation::interpolate_json(b, &ctx));

    let method = step.request.method.clone();
    let step_name = step.name.clone();

    // Expected status from assertions (if any)
    let expected_status = step.assertions.as_ref().and_then(|a| a.status);

    // Run the benchmark using tokio runtime
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| TarnError::Http(format!("Failed to create async runtime: {}", e)))?;

    let result = rt.block_on(run_bench_async(
        &step_name,
        &method,
        &url,
        &headers,
        body.as_ref(),
        expected_status,
        opts,
    ))?;

    Ok(result)
}

async fn run_bench_async(
    step_name: &str,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&serde_json::Value>,
    expected_status: Option<u16>,
    opts: &BenchOptions,
) -> Result<BenchResult, TarnError> {
    let semaphore = Arc::new(Semaphore::new(opts.concurrency as usize));
    let completed = Arc::new(AtomicU64::new(0));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| TarnError::Http(format!("Failed to create HTTP client: {}", e)))?;

    let overall_start = Instant::now();

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
        let method = method.to_string();
        let url = url.to_string();
        let headers = headers.clone();
        let body = body.cloned();
        let completed = completed.clone();

        let handle = tokio::spawn(async move {
            let result = execute_single(
                &client,
                &method,
                &url,
                &headers,
                body.as_ref(),
                expected_status,
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
                duration_ms: 0,
                status: 0,
                success: false,
                error: Some(format!("Task failed: {}", e)),
            }),
        }
    }

    let total_duration_ms = overall_start.elapsed().as_millis() as u64;

    Ok(aggregate_results(
        step_name,
        method,
        url,
        results,
        total_duration_ms,
    ))
}

async fn execute_single(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&serde_json::Value>,
    expected_status: Option<u16>,
) -> RequestResult {
    let req_method = match method.to_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        _ => {
            return RequestResult {
                duration_ms: 0,
                status: 0,
                success: false,
                error: Some(format!("Unsupported method: {}", method)),
            }
        }
    };

    let mut builder = client.request(req_method, url);

    for (k, v) in headers {
        builder = builder.header(k, v);
    }

    if let Some(b) = body {
        builder = builder.json(b);
    }

    let start = Instant::now();
    match builder.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let duration_ms = start.elapsed().as_millis() as u64;
            // Consume body to complete the request
            let _ = resp.bytes().await;
            let success = expected_status.map(|e| e == status).unwrap_or(true);
            RequestResult {
                duration_ms,
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
            }
        }
        Err(e) => RequestResult {
            duration_ms: start.elapsed().as_millis() as u64,
            status: 0,
            success: false,
            error: Some(e.to_string()),
        },
    }
}

fn aggregate_results(
    step_name: &str,
    method: &str,
    url: &str,
    results: Vec<RequestResult>,
    total_duration_ms: u64,
) -> BenchResult {
    let total = results.len() as u64;
    let successful = results.iter().filter(|r| r.success).count() as u64;
    let failed = total - successful;

    // Collect latencies from successful requests
    let mut latencies: Vec<u64> = results
        .iter()
        .filter(|r| r.success)
        .map(|r| r.duration_ms)
        .collect();
    latencies.sort();

    let latency = if latencies.is_empty() {
        LatencyStats {
            min_ms: 0,
            max_ms: 0,
            mean_ms: 0.0,
            median_ms: 0,
            p95_ms: 0,
            p99_ms: 0,
            stdev_ms: 0.0,
        }
    } else {
        let min = *latencies.first().unwrap();
        let max = *latencies.last().unwrap();
        let sum: u64 = latencies.iter().sum();
        let mean = sum as f64 / latencies.len() as f64;
        let median = percentile(&latencies, 50.0);
        let p95 = percentile(&latencies, 95.0);
        let p99 = percentile(&latencies, 99.0);

        // Standard deviation
        let variance = latencies
            .iter()
            .map(|&v| {
                let diff = v as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / latencies.len() as f64;
        let stdev = variance.sqrt();

        LatencyStats {
            min_ms: min,
            max_ms: max,
            mean_ms: (mean * 100.0).round() / 100.0,
            median_ms: median,
            p95_ms: p95,
            p99_ms: p99,
            stdev_ms: (stdev * 100.0).round() / 100.0,
        }
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
        status_codes,
        errors,
    }
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
        result.total_requests, // concurrency not stored; total is printed
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

    out.push('\n');
    out
}

/// Render benchmark results as JSON.
pub fn render_json(result: &BenchResult) -> String {
    serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            RequestResult {
                duration_ms: 10,
                status: 200,
                success: true,
                error: None,
            },
            RequestResult {
                duration_ms: 20,
                status: 200,
                success: true,
                error: None,
            },
            RequestResult {
                duration_ms: 30,
                status: 200,
                success: true,
                error: None,
            },
        ];
        let agg = aggregate_results("test", "GET", "http://localhost", results, 100);
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
            RequestResult {
                duration_ms: 10,
                status: 200,
                success: true,
                error: None,
            },
            RequestResult {
                duration_ms: 5,
                status: 500,
                success: false,
                error: Some("server error".into()),
            },
            RequestResult {
                duration_ms: 0,
                status: 0,
                success: false,
                error: Some("connection refused".into()),
            },
        ];
        let agg = aggregate_results("test", "GET", "http://localhost", results, 50);
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
        let results = vec![RequestResult {
            duration_ms: 0,
            status: 0,
            success: false,
            error: Some("err".into()),
        }];
        let agg = aggregate_results("test", "GET", "http://localhost", results, 10);
        assert_eq!(agg.successful, 0);
        assert_eq!(agg.failed, 1);
        assert_eq!(agg.latency.min_ms, 0);
        assert_eq!(agg.latency.mean_ms, 0.0);
    }

    #[test]
    fn aggregate_throughput() {
        let results = vec![
            RequestResult {
                duration_ms: 10,
                status: 200,
                success: true,
                error: None,
            },
            RequestResult {
                duration_ms: 10,
                status: 200,
                success: true,
                error: None,
            },
        ];
        // 2 requests in 100ms = 20 req/s
        let agg = aggregate_results("test", "GET", "http://localhost", results, 100);
        assert_eq!(agg.throughput_rps, 20.0);
    }

    #[test]
    fn aggregate_deduplicates_errors() {
        let results = vec![
            RequestResult {
                duration_ms: 0,
                status: 0,
                success: false,
                error: Some("same error".into()),
            },
            RequestResult {
                duration_ms: 0,
                status: 0,
                success: false,
                error: Some("same error".into()),
            },
            RequestResult {
                duration_ms: 0,
                status: 0,
                success: false,
                error: Some("different".into()),
            },
        ];
        let agg = aggregate_results("test", "GET", "http://localhost", results, 10);
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
        let result = BenchResult {
            step_name: "test".into(),
            method: "GET".into(),
            url: "http://localhost".into(),
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
            status_codes: HashMap::from([(200, 9), (500, 1)]),
            errors: vec!["server error".into()],
        };
        let json = render_json(&result);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["total_requests"], 10);
        assert_eq!(parsed["latency"]["p95_ms"], 45);
    }

    #[test]
    fn render_human_output() {
        let result = BenchResult {
            step_name: "health".into(),
            method: "GET".into(),
            url: "http://localhost/health".into(),
            total_requests: 100,
            successful: 100,
            failed: 0,
            error_rate: 0.0,
            total_duration_ms: 1500,
            throughput_rps: 66.67,
            latency: LatencyStats {
                min_ms: 2,
                max_ms: 50,
                mean_ms: 12.5,
                median_ms: 10,
                p95_ms: 35,
                p99_ms: 48,
                stdev_ms: 8.3,
            },
            status_codes: HashMap::from([(200, 100)]),
            errors: vec![],
        };
        let output = render_human(&result);
        assert!(output.contains("TARN BENCH"));
        assert!(output.contains("100 total"));
        assert!(output.contains("66.7 req/s"));
        assert!(output.contains("p95"));
        assert!(output.contains("p99"));
    }
}
