//! Shared JSON-Schema accessors for `schemas/v1/testfile.json`.
//!
//! L1.3 (hover) needs top-level key descriptions. L1.4 (completion)
//! needs both the descriptions and the set of valid keys per YAML
//! scope — root, test-group, step. L3.5 (nested completion) adds a
//! tree walker that descends into subschemas via `$ref`, `properties`,
//! `items`, `additionalProperties`, and the `oneOf` / `anyOf` /
//! `allOf` combinators, so completion can offer schema-appropriate
//! children anywhere in the document — `request.*`, `assert.body.*`,
//! `capture.*`, `poll.*`, etc.
//!
//! Rather than duplicate schema parsing in every feature, all features
//! read from this module. The schema file is baked in with
//! `include_str!`, so clients do not need the workspace on disk at
//! runtime. Parsing is done exactly once per process behind a
//! `OnceLock`, so every subsequent lookup is an in-memory map read.
//!
//! ## Supported JSON Schema constructs (L3.5)
//!
//! The walker is deliberately scoped to the constructs the bundled
//! Tarn schema actually uses — not a full JSON Schema engine:
//!
//!   * `properties` (named children of an object)
//!   * `additionalProperties` (value schema for arbitrary keys; the
//!     walker descends into it when a [`PathSegment::Key`] does not
//!     match any concrete property)
//!   * `items` (array element schema — consumed by
//!     [`PathSegment::Index`])
//!   * `$ref` (local `#/...` JSON Pointer refs only; external refs
//!     are out of scope)
//!   * `oneOf` / `anyOf` / `allOf` (union descent — the walker tries
//!     every branch and merges the property sets it finds)
//!
//! Constructs we deliberately do NOT support: `patternProperties`
//! (the bundled schema does not use them), `if`/`then`/`else`,
//! `dependencies`, external refs, remote URIs.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Shared schema key cache. Populated lazily on first access.
///
/// The `get_*` helpers return borrowed slices into the cache so
/// callers never need to clone the descriptions.
#[derive(Debug)]
pub struct SchemaKeyCache {
    /// The full parsed schema document, kept so the nested-completion
    /// walker in [`children_at_schema_path`] can descend without
    /// re-parsing.
    schema: serde_json::Value,
    /// Map of every `properties` entry anywhere in the schema →
    /// its `description` (with local `$ref` chains resolved).
    descriptions: HashMap<String, String>,
    /// Top-level root keys of a `.tarn.yaml` test file.
    root_keys: Vec<&'static str>,
    /// Keys allowed on a named test group (`tests.<name>.*`).
    test_keys: Vec<&'static str>,
    /// Keys allowed on a single step (`steps[*]` / `setup[*]` / …).
    step_keys: Vec<&'static str>,
}

impl SchemaKeyCache {
    pub fn description(&self, key: &str) -> Option<&str> {
        self.descriptions.get(key).map(String::as_str)
    }

    pub fn descriptions(&self) -> &HashMap<String, String> {
        &self.descriptions
    }

    pub fn root_keys(&self) -> &[&'static str] {
        &self.root_keys
    }

    pub fn test_keys(&self) -> &[&'static str] {
        &self.test_keys
    }

    pub fn step_keys(&self) -> &[&'static str] {
        &self.step_keys
    }

    /// Access the raw parsed schema document. Used by tests that
    /// want to inspect specific subschemas without going through the
    /// walker.
    pub fn raw_schema(&self) -> &serde_json::Value {
        &self.schema
    }
}

/// One segment of a schema-navigation path.
///
/// Distinct from `tarn::outline::PathSegment` because the schema
/// walker cares about a third case: a mapping key that is not a
/// concrete `properties` entry (e.g. a JSONPath key inside
/// `assert.body`, or a capture name, or a named test group). Those
/// descend through `additionalProperties` rather than `properties`.
///
/// The YAML walker in `completion.rs` emits only [`PathSegment::Key`]
/// and [`PathSegment::Index`]; the schema walker internally treats a
/// `Key` that does not match any concrete property as descending
/// into `additionalProperties`. Callers that need to distinguish
/// "structural key" vs "free-form key" from the outside can inspect
/// the resolved children's [`SchemaField::kind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// A mapping key on the YAML side. Matches `properties.<key>` if
    /// present, otherwise descends through `additionalProperties`.
    Key(String),
    /// A sequence index. Descends through `items`.
    Index(usize),
}

/// Ordered path from the document root to a YAML node, expressed in
/// schema-walker vocabulary. Wraps a `Vec<PathSegment>` so the type
/// system prevents accidental mixups with outline's path type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaPath(pub Vec<PathSegment>);

impl SchemaPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, seg: PathSegment) {
        self.0.push(seg);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }
}

/// A single child offered by [`children_at_schema_path`].
///
/// The `name` is the YAML key the completion item should insert.
/// `description` is the schema's `description` field for that key
/// when available. `kind` tells the caller whether this child came
/// from `properties` (a structural key, the common case) or from an
/// `additionalProperties` branch (a free-form key — e.g. one of the
/// `BodyAssertionOperators` offered inside `assert.body.*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaField {
    pub name: String,
    pub description: Option<String>,
    pub kind: SchemaFieldKind,
}

/// Whether a schema field came from `properties` or from a looser
/// `additionalProperties` branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFieldKind {
    /// A named child from a `properties` block.
    Property,
    /// A matcher / operator child surfaced because the caller is
    /// inside the value schema of an `additionalProperties` branch
    /// (e.g. `assert.body."$.id"` → `BodyAssertionOperators`). The
    /// distinction matters to the caller so it can style matcher
    /// completions differently.
    Matcher,
}

/// Walk the cached schema tree along `path` and return the set of
/// valid children at the destination node.
///
/// The walk is pure and deterministic: given the same cache and the
/// same path, the output is the same. Implementation-wise the walker
/// resolves `$ref`s eagerly, descends through every combinator
/// (`oneOf` / `anyOf` / `allOf`), and merges property sets from
/// every branch it can descend into. An empty path yields the root
/// schema's top-level properties.
///
/// Returns an empty `Vec` when the path cannot be resolved — either
/// because a segment did not match any child, because a `$ref` could
/// not be resolved locally, or because the schema shape at the
/// destination has no child keys to offer.
pub fn children_at_schema_path(cache: &SchemaKeyCache, path: &SchemaPath) -> Vec<SchemaField> {
    let schemas = resolve_path(&cache.schema, path);
    if schemas.is_empty() {
        return Vec::new();
    }
    let mut merged: BTreeMap<String, SchemaField> = BTreeMap::new();
    for schema in schemas {
        collect_children(&cache.schema, schema, &mut merged);
    }
    merged.into_values().collect()
}

/// Walk `path` starting at `root` and return every schema node that
/// the walk lands on. `oneOf` / `anyOf` / `allOf` at each step fan
/// out into multiple candidates; the walker keeps all of them so
/// downstream code can merge their child sets.
fn resolve_path<'a>(root: &'a serde_json::Value, path: &SchemaPath) -> Vec<&'a serde_json::Value> {
    let mut current: Vec<&serde_json::Value> = vec![root];
    for segment in path.segments() {
        let mut next: Vec<&serde_json::Value> = Vec::new();
        for node in current {
            // Resolve the node (follow $ref, descend into combinators)
            // into its set of concrete object-shaped candidates before
            // applying the segment.
            let candidates = unwrap_schema_node(root, node);
            for cand in candidates {
                match segment {
                    PathSegment::Key(key) => {
                        if let Some(child) = descend_into_key(root, cand, key) {
                            next.extend(child);
                        }
                    }
                    PathSegment::Index(_) => {
                        if let Some(child) = descend_into_index(root, cand) {
                            next.extend(child);
                        }
                    }
                }
            }
        }
        // Dedupe by identity to avoid exponential explosion when
        // several union branches resolve to the same subschema.
        next = dedupe_by_ptr(next);
        if next.is_empty() {
            return Vec::new();
        }
        current = next;
    }
    dedupe_by_ptr(current)
}

/// Follow a schema node's `$ref` if present, then expand combinators
/// (`oneOf`, `anyOf`, `allOf`) into their concrete branch nodes. The
/// result is the set of candidate schema nodes a path segment can
/// look for children inside.
fn unwrap_schema_node<'a>(
    root: &'a serde_json::Value,
    node: &'a serde_json::Value,
) -> Vec<&'a serde_json::Value> {
    let mut out: Vec<&serde_json::Value> = Vec::new();
    let mut stack: Vec<&serde_json::Value> = vec![node];
    let mut seen: Vec<*const serde_json::Value> = Vec::new();
    while let Some(n) = stack.pop() {
        let ptr = n as *const serde_json::Value;
        if seen.contains(&ptr) {
            continue;
        }
        seen.push(ptr);
        // Follow $ref first.
        if let Some(serde_json::Value::String(r)) = n.get("$ref") {
            if let Some(target) = resolve_local_ref(root, r) {
                stack.push(target);
                continue;
            }
        }
        // Push the node itself as a candidate.
        out.push(n);
        // And fan out through combinators.
        for key in ["oneOf", "anyOf", "allOf"] {
            if let Some(serde_json::Value::Array(branches)) = n.get(key) {
                for branch in branches {
                    stack.push(branch);
                }
            }
        }
    }
    dedupe_by_ptr(out)
}

/// Given an object-shaped schema node `schema` and a mapping key
/// `key`, return the schema nodes that describe the value for that
/// key. A key that matches a `properties.<key>` entry wins; otherwise
/// the walker descends through `additionalProperties`. Arrays and
/// primitives don't have keys — they yield `None`.
fn descend_into_key<'a>(
    _root: &'a serde_json::Value,
    schema: &'a serde_json::Value,
    key: &str,
) -> Option<Vec<&'a serde_json::Value>> {
    if let Some(serde_json::Value::Object(props)) = schema.get("properties") {
        if let Some(child) = props.get(key) {
            return Some(vec![child]);
        }
    }
    if let Some(addp) = schema.get("additionalProperties") {
        // `additionalProperties: false` — nothing to offer.
        if addp.as_bool() == Some(false) {
            return None;
        }
        // `additionalProperties: true` or an object schema — use the
        // object form as the value schema; skip pure `true` because
        // it's unconstrained and carries no child structure.
        if addp.is_object() {
            return Some(vec![addp]);
        }
    }
    None
}

/// Given an array-shaped schema node, return its `items` schema.
fn descend_into_index<'a>(
    _root: &'a serde_json::Value,
    schema: &'a serde_json::Value,
) -> Option<Vec<&'a serde_json::Value>> {
    let items = schema.get("items")?;
    Some(vec![items])
}

/// Collect every concrete property child visible from `schema`,
/// merging across combinator branches. Child fields from
/// `properties` are tagged [`SchemaFieldKind::Property`]; child
/// fields reached through `additionalProperties` are tagged
/// [`SchemaFieldKind::Matcher`].
fn collect_children(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    out: &mut BTreeMap<String, SchemaField>,
) {
    // The walker reaches this point already having applied the full
    // path — so it's interested in the children of `schema` itself.
    // Unwrap one more time to fan through $ref / combinators.
    let candidates = unwrap_schema_node(root, schema);
    for cand in candidates {
        collect_direct_children(root, cand, out);
    }
}

fn collect_direct_children(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    out: &mut BTreeMap<String, SchemaField>,
) {
    if let Some(serde_json::Value::Object(props)) = schema.get("properties") {
        for (key, prop) in props {
            let description = extract_description(root, prop);
            out.entry(key.clone()).or_insert_with(|| SchemaField {
                name: key.clone(),
                description,
                kind: SchemaFieldKind::Property,
            });
        }
    }
    // `additionalProperties` with an object schema is also a source
    // of "children" — but only structurally meaningful when that
    // object schema itself has `properties` or combinator branches
    // with properties. Recurse one level so the matcher grammar
    // under `assert.body."$.id"` is reachable by
    // [`children_at_schema_path`] callers that don't push an extra
    // synthetic path segment.
    if let Some(addp) = schema.get("additionalProperties") {
        if addp.is_object() {
            let sub_candidates = unwrap_schema_node(root, addp);
            for sub in sub_candidates {
                if let Some(serde_json::Value::Object(props)) = sub.get("properties") {
                    for (key, prop) in props {
                        let description = extract_description(root, prop);
                        out.entry(key.clone()).or_insert_with(|| SchemaField {
                            name: key.clone(),
                            description,
                            kind: SchemaFieldKind::Matcher,
                        });
                    }
                }
            }
        }
    }
}

/// Inline `description` wins; otherwise follow a local `$ref` and use
/// the referenced schema's `description`. External refs are ignored.
fn extract_description(root: &serde_json::Value, prop: &serde_json::Value) -> Option<String> {
    if let Some(serde_json::Value::String(desc)) = prop.get("description") {
        return Some(desc.clone());
    }
    if let Some(serde_json::Value::String(r)) = prop.get("$ref") {
        if let Some(target) = resolve_local_ref(root, r) {
            if let Some(serde_json::Value::String(desc)) = target.get("description") {
                return Some(desc.clone());
            }
        }
    }
    None
}

/// Top-level keys of a `.tarn.yaml` test file. Kept in source as a
/// stable hard-coded list (rather than derived from the schema at
/// runtime) so the set the completion provider offers is obvious by
/// inspection and has a single place to update when the schema grows.
const ROOT_KEYS: &[&str] = &[
    "version",
    "name",
    "description",
    "tags",
    "env",
    "cookies",
    "redaction",
    "defaults",
    "setup",
    "teardown",
    "tests",
    "steps",
];

/// Keys allowed on a named test group inside `tests:`. Driven by the
/// `$defs/TestGroup.properties` block in the schema.
const TEST_KEYS: &[&str] = &["description", "tags", "steps"];

/// Keys allowed on a single step. Driven by `$defs/Step.properties`.
///
/// `include` is offered as a sibling because `StepOrInclude` allows
/// either a step or an include directive at the same array slot —
/// completion surfaces both so the user can pick.
const STEP_KEYS: &[&str] = &[
    "name",
    "request",
    "command",
    "capture",
    "assert",
    "retries",
    "timeout",
    "connect_timeout",
    "follow_redirects",
    "max_redirs",
    "delay",
    "poll",
    "script",
    "cookies",
    "include",
];

/// Return the process-wide cached [`SchemaKeyCache`]. Parses the
/// schema on first call; every subsequent call is an atomic load.
pub fn schema_key_cache() -> &'static SchemaKeyCache {
    static CACHE: OnceLock<SchemaKeyCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        let raw = include_str!("../schemas/v1/testfile.json");
        let schema = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
        let descriptions = if schema.is_null() {
            HashMap::new()
        } else {
            let mut out = HashMap::new();
            collect_descriptions_recursive(&schema, &schema, &mut out);
            out
        };
        SchemaKeyCache {
            schema,
            descriptions,
            root_keys: ROOT_KEYS.to_vec(),
            test_keys: TEST_KEYS.to_vec(),
            step_keys: STEP_KEYS.to_vec(),
        }
    })
}

/// Top-level + nested descriptions from `testfile.json`.
///
/// Walks the whole schema document; the first description encountered
/// for a given key wins. Top-level `properties` are visited before
/// `$defs`, so `status`, `body`, `headers` resolve to the top-level
/// `Assertion` descriptions rather than their inner re-definitions.
fn collect_descriptions_recursive(
    root: &serde_json::Value,
    value: &serde_json::Value,
    out: &mut HashMap<String, String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(props)) = map.get("properties") {
                for (key, prop) in props {
                    if let Some(desc) = extract_description(root, prop) {
                        out.entry(key.clone()).or_insert(desc);
                    }
                }
            }
            for v in map.values() {
                collect_descriptions_recursive(root, v, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_descriptions_recursive(root, v, out);
            }
        }
        _ => {}
    }
}

/// Resolve `#/path/to/schema`. External refs return `None`.
fn resolve_local_ref<'a>(
    root: &'a serde_json::Value,
    reference: &str,
) -> Option<&'a serde_json::Value> {
    let path = reference.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        let unescaped = segment.replace("~1", "/").replace("~0", "~");
        current = current.get(&unescaped)?;
    }
    Some(current)
}

/// Dedupe a `Vec<&serde_json::Value>` by pointer identity so a
/// diamond-shaped schema graph (two combinator branches both
/// pointing at the same `$ref`) doesn't generate duplicate work.
fn dedupe_by_ptr(input: Vec<&serde_json::Value>) -> Vec<&serde_json::Value> {
    let mut out: Vec<&serde_json::Value> = Vec::with_capacity(input.len());
    let mut seen: Vec<*const serde_json::Value> = Vec::with_capacity(input.len());
    for v in input {
        let ptr = v as *const serde_json::Value;
        if !seen.contains(&ptr) {
            seen.push(ptr);
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(schema_json: &str) -> SchemaKeyCache {
        let schema: serde_json::Value = serde_json::from_str(schema_json).unwrap();
        let mut descriptions = HashMap::new();
        collect_descriptions_recursive(&schema, &schema, &mut descriptions);
        SchemaKeyCache {
            schema,
            descriptions,
            root_keys: Vec::new(),
            test_keys: Vec::new(),
            step_keys: Vec::new(),
        }
    }

    #[test]
    fn cache_root_keys_match_known_top_level_fields() {
        let cache = schema_key_cache();
        let roots = cache.root_keys();
        assert!(roots.contains(&"name"));
        assert!(roots.contains(&"env"));
        assert!(roots.contains(&"tests"));
        assert!(roots.contains(&"steps"));
        assert!(roots.contains(&"defaults"));
    }

    #[test]
    fn cache_step_keys_match_step_schema() {
        let cache = schema_key_cache();
        let steps = cache.step_keys();
        assert!(steps.contains(&"name"));
        assert!(steps.contains(&"request"));
        assert!(steps.contains(&"capture"));
        assert!(steps.contains(&"assert"));
        assert!(steps.contains(&"poll"));
        assert!(steps.contains(&"include"));
    }

    #[test]
    fn cache_test_keys_match_testgroup_schema() {
        let cache = schema_key_cache();
        let tests = cache.test_keys();
        assert!(tests.contains(&"description"));
        assert!(tests.contains(&"steps"));
        assert!(tests.contains(&"tags"));
    }

    #[test]
    fn descriptions_include_top_level_keys() {
        let cache = schema_key_cache();
        for key in &["name", "env", "defaults", "setup", "tests"] {
            assert!(
                cache.description(key).is_some(),
                "missing description for `{key}`"
            );
        }
    }

    #[test]
    fn descriptions_include_nested_assertion_keys() {
        let cache = schema_key_cache();
        // Walked out of `$defs/Assertion.properties`.
        assert!(cache.description("status").is_some());
        assert!(cache.description("body").is_some());
        assert!(cache.description("headers").is_some());
    }

    #[test]
    fn schema_parses_successfully() {
        // If the bundled schema ever gets corrupted this is the
        // fastest signal — the cache would return an empty map.
        let cache = schema_key_cache();
        assert!(!cache.descriptions().is_empty());
    }

    // ---------------- children_at_schema_path ----------------

    #[test]
    fn children_at_empty_path_returns_top_level_properties() {
        let cache = schema_key_cache();
        let children = children_at_schema_path(cache, &SchemaPath::new());
        let names: Vec<_> = children.iter().map(|f| f.name.as_str()).collect();
        for want in [
            "name", "env", "tests", "steps", "defaults", "setup", "teardown",
        ] {
            assert!(
                names.contains(&want),
                "missing `{want}` in top-level children: {names:?}"
            );
        }
    }

    #[test]
    fn children_descend_through_properties_one_level() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "request": {
                        "type": "object",
                        "properties": {
                            "method": { "type": "string", "description": "HTTP method" },
                            "url": { "type": "string" }
                        }
                    }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("request".into()));
        let kids = children_at_schema_path(&cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"method"));
        assert!(names.contains(&"url"));
        let method = kids.iter().find(|f| f.name == "method").unwrap();
        assert_eq!(method.description.as_deref(), Some("HTTP method"));
        assert_eq!(method.kind, SchemaFieldKind::Property);
    }

    #[test]
    fn children_descend_through_array_items() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "Step name" }
                            }
                        }
                    }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        let kids = children_at_schema_path(&cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["name"]);
        assert_eq!(
            kids[0].description.as_deref(),
            Some("Step name"),
            "array items description should be preserved"
        );
    }

    #[test]
    fn children_resolve_local_ref_segment() {
        let cache = synthetic(
            r##"{
                "type": "object",
                "properties": {
                    "poll": { "$ref": "#/$defs/Poll" }
                },
                "$defs": {
                    "Poll": {
                        "type": "object",
                        "properties": {
                            "until": { "type": "object", "description": "Stop condition" },
                            "interval": { "type": "string" }
                        }
                    }
                }
            }"##,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("poll".into()));
        let kids = children_at_schema_path(&cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"until"));
        assert!(names.contains(&"interval"));
        let until = kids.iter().find(|f| f.name == "until").unwrap();
        assert_eq!(until.description.as_deref(), Some("Stop condition"));
    }

    #[test]
    fn children_merge_across_oneof_branches() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "shape": {
                        "oneOf": [
                            { "type": "object", "properties": { "a": { "type": "string" } } },
                            { "type": "object", "properties": { "b": { "type": "number" } } }
                        ]
                    }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("shape".into()));
        let kids = children_at_schema_path(&cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn children_merge_across_anyof_and_allof() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "x": {
                        "anyOf": [
                            { "type": "object", "properties": { "alpha": { "type": "string" } } }
                        ],
                        "allOf": [
                            { "type": "object", "properties": { "beta": { "type": "number" } } }
                        ]
                    }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("x".into()));
        let kids = children_at_schema_path(&cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn children_descend_into_additional_properties_matcher_kind() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "body": {
                        "type": "object",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "eq": { "description": "equal" },
                                "gt": { "description": "greater than" }
                            }
                        }
                    }
                }
            }"#,
        );
        // Cursor is inside `body:` itself — the walker surfaces the
        // additionalProperties value schema's children as Matcher kind
        // so the caller knows these are operator keys under an
        // open-ended JSONPath key.
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("body".into()));
        let kids = children_at_schema_path(&cache, &path);
        let eq = kids.iter().find(|f| f.name == "eq").expect("missing eq");
        assert_eq!(eq.kind, SchemaFieldKind::Matcher);
        assert_eq!(eq.description.as_deref(), Some("equal"));
        let gt = kids.iter().find(|f| f.name == "gt").expect("missing gt");
        assert_eq!(gt.kind, SchemaFieldKind::Matcher);
    }

    #[test]
    fn children_descend_through_additional_properties_key_into_matcher_children() {
        // Schema shaped like Tarn's `assert.body.<json-path>` — the
        // walker must accept any key at the `body.*` level as a
        // passthrough into the additionalProperties value schema.
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "body": {
                        "type": "object",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "eq": {},
                                "matches": { "type": "string" },
                                "length": { "type": "integer" }
                            }
                        }
                    }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("body".into()));
        path.push(PathSegment::Key("$.id".into()));
        let kids = children_at_schema_path(&cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"eq"));
        assert!(names.contains(&"matches"));
        assert!(names.contains(&"length"));
        // Descended through an additionalProperties value schema, but
        // the children themselves came from `properties`, so they're
        // Property kind.
        let eq = kids.iter().find(|f| f.name == "eq").unwrap();
        assert_eq!(eq.kind, SchemaFieldKind::Property);
    }

    #[test]
    fn children_invalid_path_returns_empty() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "request": { "type": "object", "properties": { "url": { "type": "string" } } }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("nonexistent".into()));
        let kids = children_at_schema_path(&cache, &path);
        assert!(kids.is_empty());
    }

    #[test]
    fn children_missing_ref_target_returns_empty() {
        let cache = synthetic(
            r##"{
                "type": "object",
                "properties": {
                    "poll": { "$ref": "#/$defs/Missing" }
                }
            }"##,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("poll".into()));
        let kids = children_at_schema_path(&cache, &path);
        assert!(kids.is_empty());
    }

    #[test]
    fn children_index_on_non_array_returns_empty() {
        let cache = synthetic(
            r#"{
                "type": "object",
                "properties": {
                    "request": { "type": "object", "properties": { "url": { "type": "string" } } }
                }
            }"#,
        );
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("request".into()));
        path.push(PathSegment::Index(0));
        let kids = children_at_schema_path(&cache, &path);
        assert!(kids.is_empty());
    }

    // ---------------- Real-schema smoke tests ----------------

    #[test]
    fn real_schema_request_children_are_request_fields() {
        let cache = schema_key_cache();
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("request".into()));
        let kids = children_at_schema_path(cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"method"), "missing method: {names:?}");
        assert!(names.contains(&"url"), "missing url: {names:?}");
        assert!(names.contains(&"headers"), "missing headers: {names:?}");
        assert!(names.contains(&"body"), "missing body: {names:?}");
        assert!(names.contains(&"form"), "missing form: {names:?}");
        assert!(names.contains(&"multipart"), "missing multipart: {names:?}");
        let url = kids.iter().find(|f| f.name == "url").unwrap();
        assert!(url.description.is_some());
    }

    #[test]
    fn real_schema_assert_body_jsonpath_children_are_matchers() {
        let cache = schema_key_cache();
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("assert".into()));
        path.push(PathSegment::Key("body".into()));
        path.push(PathSegment::Key("$.id".into()));
        let kids = children_at_schema_path(cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        // Pick a handful of BodyAssertionOperators keys.
        for want in [
            "eq", "gt", "gte", "lt", "lte", "contains", "matches", "length", "type", "is_uuid",
        ] {
            assert!(
                names.contains(&want),
                "missing matcher `{want}` in {names:?}"
            );
        }
    }

    #[test]
    fn real_schema_poll_children_are_pollconfig_fields() {
        let cache = schema_key_cache();
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("poll".into()));
        let kids = children_at_schema_path(cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"until"));
        assert!(names.contains(&"interval"));
        assert!(names.contains(&"max_attempts"));
    }

    #[test]
    fn real_schema_capture_value_offers_extended_capture_keys() {
        let cache = schema_key_cache();
        // capture.<name> descends into a oneOf(string, ExtendedCapture)
        // so the union of children is the ExtendedCapture properties.
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("steps".into()));
        path.push(PathSegment::Index(0));
        path.push(PathSegment::Key("capture".into()));
        path.push(PathSegment::Key("user_id".into()));
        let kids = children_at_schema_path(cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        // ExtendedCapture fields.
        assert!(names.contains(&"header"));
        assert!(names.contains(&"cookie"));
        assert!(names.contains(&"jsonpath"));
        assert!(names.contains(&"regex"));
    }

    #[test]
    fn real_schema_tests_named_group_children_include_steps() {
        let cache = schema_key_cache();
        let mut path = SchemaPath::new();
        path.push(PathSegment::Key("tests".into()));
        path.push(PathSegment::Key("main".into()));
        let kids = children_at_schema_path(cache, &path);
        let names: Vec<_> = kids.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"steps"));
        assert!(names.contains(&"description"));
        assert!(names.contains(&"tags"));
    }

    #[test]
    fn bundled_schema_matches_authoritative_source() {
        // The bundled copy at tarn-lsp/schemas/v1/testfile.json must stay
        // in sync with the authoritative copy at schemas/v1/testfile.json
        // (repo root). This test catches drift — if it fails, copy the
        // updated schema into tarn-lsp/schemas/v1/.
        let bundled = include_str!("../schemas/v1/testfile.json");
        let authoritative = std::fs::read_to_string("schemas/v1/testfile.json")
            .expect("Cannot read schemas/v1/testfile.json — run tests from the workspace root");
        assert_eq!(
            bundled, authoritative,
            "tarn-lsp/schemas/v1/testfile.json is out of sync with schemas/v1/testfile.json"
        );
    }
}
