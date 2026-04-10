import { z } from "zod";

export const failureCategory = z.enum([
  "assertion_failed",
  "connection_error",
  "timeout",
  "parse_error",
  "capture_error",
  "unresolved_template",
]);

export const errorCode = z.enum([
  "assertion_mismatch",
  "capture_extraction_failed",
  "poll_condition_not_met",
  "request_timed_out",
  "connection_refused",
  "dns_resolution_failed",
  "tls_verification_failed",
  "redirect_limit_exceeded",
  "network_error",
  "interpolation_failed",
  "validation_failed",
  "configuration_error",
  "parse_error",
]);

export const statusEnum = z.enum(["PASSED", "FAILED"]);

const assertionDetailSchema = z.object({
  assertion: z.string(),
  passed: z.boolean(),
  expected: z.string().optional(),
  actual: z.string().optional(),
  message: z.string().optional(),
  diff: z.string().optional(),
});

const assertionsSchema = z.object({
  total: z.number(),
  passed: z.number(),
  failed: z.number(),
  details: z.array(assertionDetailSchema).optional(),
  failures: z.array(assertionDetailSchema).optional(),
});

const requestSchema = z.object({
  method: z.string(),
  url: z.string(),
  headers: z.record(z.string()).optional(),
  body: z.unknown().optional(),
});

const responseSchema = z.object({
  status: z.number(),
  headers: z.record(z.string()).optional(),
  body: z.unknown().optional(),
});

export const stepResultSchema = z.object({
  name: z.string(),
  status: statusEnum,
  duration_ms: z.number(),
  response_status: z.number().optional(),
  response_summary: z.string().optional(),
  captures_set: z.array(z.string()).optional(),
  assertions: assertionsSchema.optional(),
  failure_category: failureCategory.optional(),
  error_code: errorCode.optional(),
  remediation_hints: z.array(z.string()).optional(),
  request: requestSchema.optional(),
  response: responseSchema.optional(),
});

export const testResultSchema = z.object({
  name: z.string(),
  description: z.string().nullable().optional(),
  status: statusEnum,
  duration_ms: z.number(),
  steps: z.array(stepResultSchema),
});

export const fileResultSchema = z.object({
  file: z.string(),
  name: z.string(),
  status: statusEnum,
  duration_ms: z.number(),
  summary: z.object({
    total: z.number(),
    passed: z.number(),
    failed: z.number(),
  }),
  setup: z.array(stepResultSchema).optional(),
  tests: z.array(testResultSchema),
  teardown: z.array(stepResultSchema).optional(),
});

export const reportSchema = z.object({
  schema_version: z.number().optional(),
  version: z.string().optional(),
  timestamp: z.string().optional(),
  duration_ms: z.number(),
  files: z.array(fileResultSchema),
  summary: z.object({
    files: z.number(),
    tests: z.number(),
    steps: z.object({
      total: z.number(),
      passed: z.number(),
      failed: z.number(),
    }),
    status: statusEnum,
  }),
});

export type Report = z.infer<typeof reportSchema>;
export type FileResult = z.infer<typeof fileResultSchema>;
export type TestResult = z.infer<typeof testResultSchema>;
export type StepResult = z.infer<typeof stepResultSchema>;
export type AssertionDetail = z.infer<typeof assertionDetailSchema>;

export function parseReport(raw: string): Report {
  const json = JSON.parse(raw);
  return reportSchema.parse(json);
}

const validateErrorSchema = z.object({
  message: z.string(),
  line: z.number().int().nonnegative().optional(),
  column: z.number().int().nonnegative().optional(),
});

const validateFileSchema = z.object({
  file: z.string(),
  valid: z.boolean(),
  errors: z.array(validateErrorSchema),
});

export const validateReportSchema = z.object({
  files: z.array(validateFileSchema),
  error: z.string().optional(),
});

export type ValidateReport = z.infer<typeof validateReportSchema>;
export type ValidateFileResult = z.infer<typeof validateFileSchema>;
export type ValidateError = z.infer<typeof validateErrorSchema>;

export function parseValidateReport(raw: string): ValidateReport {
  const json = JSON.parse(raw);
  return validateReportSchema.parse(json);
}

const envEntrySchema = z.object({
  name: z.string(),
  source_file: z.string(),
  vars: z.record(z.string()),
});

export const envReportSchema = z.object({
  project_root: z.string().optional(),
  default_env_file: z.string().optional(),
  environments: z.array(envEntrySchema),
});

export type EnvReport = z.infer<typeof envReportSchema>;
export type EnvEntry = z.infer<typeof envEntrySchema>;

export function parseEnvReport(raw: string): EnvReport {
  const json = JSON.parse(raw);
  return envReportSchema.parse(json);
}
