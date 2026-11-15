// Findings for the Logs table: the local log tools run once on each stored log, in the
// background, and their results are cached per file hash in .findings/{hash}.json. The same
// cache answers the assistant's calls of these tools with the same arguments.

/// Bump when a tool's result or the findings change shape, so cached results are computed again.
const FINDINGS_VERSION: u64 = 2;

/// One writer at a time for .findings/.
static FINDINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One tool call: the tool, its arguments other than the file, and its result.
struct ToolRun {
    tool: &'static str,
    axis: Option<String>,
    start_us: Option<u64>,
    end_us: Option<u64>,
    result: Value,
}

impl ToolRun {
    fn args(&self) -> Value {
        tool_args(self.axis.as_deref(), self.start_us, self.end_us)
    }
}

fn tool_args(axis: Option<&str>, start_us: Option<u64>, end_us: Option<u64>) -> Value {
    let mut args = serde_json::Map::new();
    if let Some(axis) = axis {
        args.insert("axis".into(), json!(axis));
    }
    if let Some(us) = start_us {
        args.insert("start_us".into(), json!(us));
    }
    if let Some(us) = end_us {
        args.insert("end_us".into(), json!(us));
    }
    Value::Object(args)
}

/// The cache key of one call: the tool and its arguments other than the file.
fn tool_key(tool: &str, axis: Option<&str>, start_us: Option<u64>, end_us: Option<u64>) -> String {
    let axis = axis.map(str::to_ascii_lowercase).unwrap_or_default();
    let num = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_default();
    format!("{tool}|{axis}|{}|{}", num(start_us), num(end_us))
}

/// True when the log holds a gyro batch (ISBH/ISBD), the input filter review reads.
fn has_gyro_batch(bytes: &[u8]) -> bool {
    let formats = parse_formats(bytes);
    let mut seen = false;
    let mut incomplete = false;
    walk_only(bytes, &formats, &["ISBH"], |_, _, _| seen = true, &mut incomplete);
    seen
}

/// A call that was not made, with the reason. It is listed with the calls and is not cached.
fn skipped(reason: &str) -> Value {
    json!({ "ok": false, "skipped": true, "error": reason })
}

/// Runs the local log tools on one log: audit, filter review (with a gyro batch), motor balance,
/// MAGFit with its defaults (with compass records), PID review per axis, and PID review bounded to
/// each System ID run on that run's axis.
fn run_tools(bytes: &[u8]) -> Vec<ToolRun> {
    let mut runs = Vec::new();
    let run = |tool, axis: Option<&str>, start_us, end_us, result| ToolRun { tool, axis: axis.map(str::to_string), start_us, end_us, result };
    runs.push(run("log_audit", None, None, None, audit_bytes(bytes)));
    let filter = if has_gyro_batch(bytes) {
        filter_review_bytes(bytes, None, None, None)
    } else {
        skipped("The log has no gyro batch (ISBH, ISBD).")
    };
    runs.push(run("log_filter_review", None, None, None, filter));
    runs.push(run("log_motor_balance", None, None, None, motor_balance_bytes(bytes, None, None)));
    let magfit = if parse_formats(bytes).values().any(|fmt| fmt.name == "MAG") {
        magfit_bytes(bytes, 1, "iron", "none", true, None, None, None, None)
    } else {
        skipped("The log has no MAG records.")
    };
    runs.push(run("log_magfit", None, None, None, magfit));
    let Some(scan) = pid_scan(bytes, None, None) else {
        return runs;
    };
    let mut sid_runs = Vec::new();
    for axis in ["roll", "pitch", "yaw"] {
        let result = pid_review_scan(&scan, Some(axis));
        if sid_runs.is_empty() {
            sid_runs = result["system_id_runs"].as_array().cloned().unwrap_or_default();
        }
        runs.push(run("log_pid_review", Some(axis), None, None, result));
    }
    drop(scan);
    for sid in sid_runs {
        let Some(axis) = sid["axis"].as_str().filter(|a| matches!(*a, "roll" | "pitch" | "yaw")) else { continue };
        let Some(start) = sid["start_us"].as_u64() else { continue };
        let end = sid["end_us"].as_u64();
        let Some(bounded) = pid_scan(bytes, Some(start), end) else { continue };
        runs.push(run("log_pid_review", Some(axis), Some(start), end, pid_review_scan(&bounded, Some(axis))));
    }
    runs
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn num_text(v: &Value) -> String {
    match v.as_f64() {
        Some(n) if n.fract() == 0.0 => format!("{n:.0}"),
        Some(n) => format!("{}", (n * 10000.0).round() / 10000.0),
        None => "—".into(),
    }
}

/// "NAME from → to", or "NAME → to" when the result does not carry the current value.
fn change_text(change: &Value) -> String {
    let name = change["name"].as_str().unwrap_or("");
    if change["from"].is_null() {
        format!("{name} → {}", num_text(&change["to"]))
    } else {
        format!("{name} {} → {}", num_text(&change["from"]), num_text(&change["to"]))
    }
}

/// The findings of one tool call, taken from the tool's own numbers and its own rules.
fn findings_of(run: &ToolRun) -> Vec<Value> {
    let res = &run.result;
    if res["ok"].as_bool() != Some(true) {
        return Vec::new();
    }
    let finding = |severity: &str, kind: &str, text: String, values: Value| json!({
        "tool": run.tool,
        "severity": severity,
        "kind": kind,
        "text": text,
        "values": values,
        "args": run.args(),
    });
    match run.tool {
        "log_audit" => res["verdict"]["issues"].as_array().into_iter().flatten().map(|issue| {
            finding(
                issue["severity"].as_str().unwrap_or("warning"),
                "audit_issue",
                issue["message"].as_str().unwrap_or("").to_string(),
                json!({ "category": issue["category"] }),
            )
        }).collect(),
        "log_filter_review" => {
            let proposed = res["proposed_params"].as_object().cloned().unwrap_or_default();
            if proposed.is_empty() {
                return Vec::new();
            }
            let changes: Vec<Value> = proposed.iter().map(|(name, to)| json!({
                "name": name,
                "from": res["current_params"].get(name).cloned().unwrap_or(Value::Null),
                "to": to,
            })).collect();
            let list = changes.iter().map(|c| change_text(c)).collect::<Vec<_>>().join(", ");
            let tracked = res["tracked_notch_hz"].as_f64().map(|hz| format!(", notch tracked at {hz} Hz")).unwrap_or_default();
            let text = format!("Gyro noise peak at {} Hz{tracked}. proposed_params: {list}.", res["primary_peak_hz"].as_f64().unwrap_or(0.0));
            vec![finding("warning", "filter_proposal", text, json!({
                "peak_hz": res["primary_peak_hz"],
                "tracked_notch_hz": res["tracked_notch_hz"],
                "changes": changes,
            }))]
        }
        "log_motor_balance" => {
            if res["issues"].as_array().is_none_or(|issues| issues.is_empty()) {
                return Vec::new();
            }
            let motors: Vec<Value> = res["motors"].as_array().into_iter().flatten()
                .map(|m| json!({ "motor": m["motor"], "diff_pct": m["diff_pct"] }))
                .collect();
            let list = motors.iter().map(|m| format!("M{} {:+.1} %", m["motor"], m["diff_pct"].as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join(", ");
            let b = &res["balance"];
            let mut text = format!("Motor output against the mean: {list}.");
            if res["motors_count"].as_u64() == Some(4) {
                text.push_str(&format!(
                    " cg_pitch_bias_pct {:+.1}, cg_roll_bias_pct {:+.1}, yaw_twist_pct {:+.1}.",
                    b["cg_pitch_bias_pct"].as_f64().unwrap_or(0.0), b["cg_roll_bias_pct"].as_f64().unwrap_or(0.0), b["yaw_twist_pct"].as_f64().unwrap_or(0.0),
                ));
            }
            vec![finding("warning", "motor_balance", text, json!({
                "motors": motors,
                "cg_pitch_bias_pct": b["cg_pitch_bias_pct"],
                "cg_roll_bias_pct": b["cg_roll_bias_pct"],
                "yaw_twist_pct": b["yaw_twist_pct"],
            }))]
        }
        // MAGFit has one rule of its own: the logged COMPASS_ORIENT against the best-fit rotation.
        "log_magfit" => {
            let o = &res["orientation"];
            if o["matches"].as_bool() != Some(false) {
                return Vec::new();
            }
            let text = format!(
                "COMPASS_ORIENT in the log is {}; the best-fit rotation is {} (cost_ratio {}).",
                o["logged"].as_str().unwrap_or("?"), o["best_fit"].as_str().unwrap_or("?"), num_text(&o["cost_ratio"]),
            );
            vec![finding("warning", "magfit_orientation", text, json!({
                "logged": o["logged"],
                "best_fit": o["best_fit"],
                "cost_ratio": o["cost_ratio"],
            }))]
        }
        "log_pid_review" => {
            let proposed = res["proposed_params"].as_object().cloned().unwrap_or_default();
            let oscillation = res["key_intervals"].get("oscillation_event").cloned();
            if proposed.is_empty() && oscillation.is_none() {
                return Vec::new();
            }
            let m = &res["metrics"];
            let diagnosis: Vec<String> = res["diagnosis"].as_array().into_iter().flatten().filter_map(|d| d.as_str().map(str::to_string)).collect();
            let mut text = diagnosis.join(" ");
            // The margins the diagnosis does not already state.
            let mut margins = Vec::new();
            if let Some(pm) = m["phase_margin_deg"].as_f64().filter(|_| !text.contains("phase margin")) {
                margins.push(format!("phase_margin_deg {}", r1(pm)));
            }
            if let Some(gm) = m["gain_margin_db"].as_f64().filter(|_| !text.contains("gain margin")) {
                margins.push(format!("gain_margin_db {}", r1(gm)));
            }
            if !margins.is_empty() {
                text.push_str(&format!(" {}.", margins.join(", ")));
            }
            let changes: Vec<Value> = proposed.iter().map(|(name, to)| json!({
                "name": name,
                "from": res["current_params"].get(name).cloned().unwrap_or(Value::Null),
                "to": to,
            })).collect();
            if !changes.is_empty() {
                let list = changes.iter().map(|c| change_text(c)).collect::<Vec<_>>().join(", ");
                text.push_str(&format!(" proposed_params: {list}."));
            }
            let severity = if oscillation.is_some() { "warning" } else { "info" };
            vec![finding(severity, if oscillation.is_some() { "pid_oscillation" } else { "pid_proposal" }, text.trim().to_string(), json!({
                "axis": res["axis"],
                "overshoot_pct": m["overshoot_pct"],
                "phase_margin_deg": m["phase_margin_deg"],
                "gain_margin_db": m["gain_margin_db"],
                "oscillation_hz": oscillation.as_ref().map(|o| o["frequency_hz"].clone()),
                "diagnosis": diagnosis,
                "changes": changes,
            }))]
        }
        _ => Vec::new(),
    }
}

/// Findings and the calls made, for one log's bytes. Returns the cache document.
fn findings_document(bytes: &[u8]) -> Value {
    let runs = run_tools(bytes);
    let findings: Vec<Value> = runs.iter().flat_map(findings_of).collect();
    let calls: Vec<Value> = runs.iter().map(|run| {
        let mut call = json!({ "tool": run.tool, "args": run.args(), "ok": run.result["ok"].as_bool() == Some(true) });
        if let Some(error) = run.result["error"].as_str() {
            call["error"] = json!(error);
        }
        if run.result["skipped"].as_bool() == Some(true) {
            call["skipped"] = json!(true);
        }
        call
    }).collect();
    let mut results = serde_json::Map::new();
    // A call that was not made has no result: the assistant's own call of that tool runs it.
    for run in runs.into_iter().filter(|run| run.result["skipped"].as_bool() != Some(true)) {
        results.insert(tool_key(run.tool, run.axis.as_deref(), run.start_us, run.end_us), run.result);
    }
    json!({ "version": FINDINGS_VERSION, "findings": findings, "calls": calls, "results": results })
}

fn findings_dir() -> PathBuf {
    logs_root().join(".findings")
}

fn read_findings_file(hash: &str) -> Option<Value> {
    if !is_hex16(hash) {
        return None;
    }
    let raw = fs::read(findings_dir().join(format!("{hash}.json"))).ok()?;
    let doc: Value = serde_json::from_slice(&raw).ok()?;
    (doc["version"].as_u64() == Some(FINDINGS_VERSION)).then_some(doc)
}

fn write_findings_file(hash: &str, doc: &Value) {
    let _ = fs::create_dir_all(findings_dir());
    let path = findings_dir().join(format!("{hash}.json"));
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, doc.to_string()).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// The findings and calls of a log, without the full results: what a /logs/local row carries.
/// Kept in its own small file, so the list does not parse every cached result.
fn findings_summary(hash: &str) -> Value {
    if !is_hex16(hash) {
        return Value::Null;
    }
    fs::read(findings_dir().join(format!("{hash}.row.json")))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .filter(|row| row["version"].as_u64() == Some(FINDINGS_VERSION) && row["findings"].is_array())
        .map(|row| json!({ "findings": row["findings"], "calls": row["calls"] }))
        .unwrap_or(Value::Null)
}

/// Runs the tools on one stored log and caches the document under its hash. False when the log
/// could not be read.
fn analyze_log(id: &str) -> bool {
    let Ok(path) = resolve(id) else { return false };
    let Ok(bytes) = fs::read(&path) else { return false };
    // The key the Logs table reads the row by (the hash in the stored name, or of the bytes).
    let hash = stored_hash(id).unwrap_or_else(|| {
        let mut digest = Sha256::new();
        digest.update(&bytes);
        hex_prefix(&digest.finalize())
    });
    if read_findings_file(&hash).is_some_and(|doc| doc["findings"].is_array()) && !findings_summary(&hash).is_null() {
        return true;
    }
    let doc = findings_document(&bytes);
    drop(bytes);
    let _guard = FINDINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Keep results the assistant cached meanwhile; the tools' own runs replace the same keys.
    let mut merged = read_findings_file(&hash).unwrap_or_else(|| json!({ "version": FINDINGS_VERSION, "results": {} }));
    if let (Some(into), Some(from)) = (merged["results"].as_object_mut(), doc["results"].as_object()) {
        for (key, value) in from {
            into.insert(key.clone(), value.clone());
        }
    }
    merged["findings"] = doc["findings"].clone();
    merged["calls"] = doc["calls"].clone();
    write_findings_file(&hash, &merged);
    let row = json!({ "version": FINDINGS_VERSION, "findings": doc["findings"], "calls": doc["calls"] });
    let path = findings_dir().join(format!("{hash}.row.json"));
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, row.to_string()).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
    true
}

/// Logs the tools could not read in this session: not queued again until the app restarts.
static FINDINGS_FAILED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The background queue: one worker, one log at a time.
struct FindingsQueue {
    pending: std::collections::VecDeque<String>,
    running: Option<String>,
    worker: bool,
}

static FINDINGS_QUEUE: std::sync::Mutex<FindingsQueue> = std::sync::Mutex::new(FindingsQueue {
    pending: std::collections::VecDeque::new(),
    running: None,
    worker: false,
});

/// Queues a stored log for the findings run. A log already queued or running is not queued again.
pub fn queue_findings(id: &str) {
    if id.is_empty() || id.starts_with("cloud:") {
        return;
    }
    if FINDINGS_FAILED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|f| f == id) {
        return;
    }
    let mut queue = FINDINGS_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if queue.running.as_deref() == Some(id) || queue.pending.iter().any(|q| q == id) {
        return;
    }
    queue.pending.push_back(id.to_string());
    if !queue.worker {
        queue.worker = true;
        let spawned = std::thread::Builder::new().name("log-findings".into()).spawn(findings_worker);
        if spawned.is_err() {
            queue.worker = false;
        }
    }
}

fn findings_worker() {
    loop {
        let id = {
            let mut queue = FINDINGS_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            match queue.pending.pop_front() {
                Some(id) => {
                    queue.running = Some(id.clone());
                    id
                }
                None => {
                    queue.running = None;
                    queue.worker = false;
                    return;
                }
            }
        };
        // A log the tools cannot read, or a panic in one tool, leaves that log without findings; the worker goes on.
        if !std::panic::catch_unwind(|| analyze_log(&id)).unwrap_or(false) {
            FINDINGS_FAILED.lock().unwrap_or_else(|e| e.into_inner()).push(id);
        }
    }
}

/// True while the log waits in the queue or is being analyzed.
fn findings_pending(id: &str) -> bool {
    let queue = FINDINGS_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    queue.running.as_deref() == Some(id) || queue.pending.iter().any(|q| q == id)
}

/// The content hash of a stored log id, as the Logs table keys it.
fn stored_hash(id: &str) -> Option<String> {
    if id.starts_with("cloud:") {
        return id.strip_prefix("cloud:").filter(|h| is_hex16(h)).map(str::to_ascii_lowercase);
    }
    let facts = read_facts();
    content_hash(id, facts.get(id).unwrap_or(&Value::Null))
}

/// A cached result of one log tool call on this file, or the result of `compute`, which is then
/// cached. Only a result with ok true is kept.
pub fn cached_tool(
    file: &str,
    tool: &str,
    axis: Option<&str>,
    start_us: Option<u64>,
    end_us: Option<u64>,
    compute: impl FnOnce() -> Value,
) -> Value {
    let Some(hash) = stored_hash(file) else {
        return compute();
    };
    let key = tool_key(tool, axis, start_us, end_us);
    if let Some(hit) = read_findings_file(&hash).and_then(|doc| doc["results"].get(&key).cloned()) {
        return hit;
    }
    let value = compute();
    if value["ok"].as_bool() == Some(true) {
        let _guard = FINDINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = read_findings_file(&hash).unwrap_or_else(|| json!({ "version": FINDINGS_VERSION, "results": {} }));
        if !doc["results"].is_object() {
            doc["results"] = json!({});
        }
        doc["results"][key.as_str()] = value.clone();
        write_findings_file(&hash, &doc);
    }
    value
}

/// The row fields for /logs/local: `findings` (null until the run is done) and `findings_pending`.
/// A scanned log without findings is queued.
fn findings_row(id: &str, hash: Option<&str>, scanned: bool) -> (Value, bool) {
    let summary = hash.map(findings_summary).unwrap_or(Value::Null);
    if summary.is_null() && scanned && !id.starts_with("cloud:") {
        queue_findings(id);
    }
    let pending = summary.is_null() && findings_pending(id);
    (summary, pending)
}

/// The fields of a tool's result that the assistant's context carries: its numbers, without the
/// chart, the curve samples and the note.
fn result_digest(tool: &str, res: &Value) -> Value {
    if res["ok"].as_bool() != Some(true) {
        return json!({ "ok": false, "skipped": res["skipped"], "error": res["error"], "not_measured": res["not_measured"] });
    }
    let keys: &[&str] = match tool {
        "log_audit" => &["health", "vibrations", "power", "navigation", "motors", "flight", "modes", "verdict"],
        "log_filter_review" => &["sample_rate_hz", "primary_peak_hz", "tracked_notch_hz", "motor_hz", "motor_spread_hz", "estimated_hover_throttle", "harmonics", "current_params", "proposed_params", "simulation", "measured_post_filter", "needs_update"],
        "log_motor_balance" => &["hover_interval", "motors_count", "overall_avg_pwm", "motors", "balance", "issues"],
        "log_magfit" => &["compass", "fit_type", "samples", "coverage_pct", "orig_error_mgauss", "fit_error_mgauss", "reduction_pct", "orientation", "params", "coordinates", "analyzed_interval"],
        "log_pid_review" => &["axis", "controller", "source", "reference", "sample_hz", "logging", "system_id_runs", "windows", "windows_total", "not_measured", "measured_to_hz", "step_response_unavailable", "metrics", "margins_unavailable", "current_params", "proposed_params", "diagnosis", "key_intervals"],
        _ => &[],
    };
    let mut out = serde_json::Map::new();
    for key in keys {
        if let Some(value) = res.get(*key).filter(|v| !v.is_null()) {
            out.insert((*key).to_string(), value.clone());
        }
    }
    if tool == "log_audit" {
        // The lane switch list can be long; its length is in verdict and navigation counts.
        if let Some(nav) = out.get_mut("navigation").and_then(Value::as_object_mut) {
            if let Some(n) = nav.get("ekf_lane_switches").and_then(Value::as_array).map(Vec::len) {
                nav.insert("ekf_lane_switches".into(), json!(n));
            }
        }
    }
    Value::Object(out)
}

/// The cached tool results of a log, as text for the assistant's context: each call with its
/// arguments and its numbers, the findings, and each tool's note once. A log that is not stored on
/// this computer, or that the tools cannot read, has none. Runs the tools when they have not run.
pub fn findings_context(id: &str) -> Option<String> {
    let body = findings_detail(id, true)?;
    Some(format!(
        "ArduLoops ran the local log tools on this file when it was stored. calls lists each call with its arguments and the numbers it returned; the chart and curve samples are left out. The same call with the same arguments returns the full cached result. A call with skipped true was not made, and error says why. log_magfit ran with its defaults: compass 1, fit_type iron, motor_comp none. findings lists the rules those results fired, in the tools' own words and numbers; an empty list means none fired. notes has each tool's own reading of its fields.\n{body}"
    ))
}

/// The findings of a stored log with each call's numbers (`result_digest`), for the assistant's
/// context and the Logs tab's findings window. `analyze` runs the tools when they have not run.
pub fn findings_detail(id: &str, analyze: bool) -> Option<Value> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let hash = stored_hash(id)?;
    let mut doc = read_findings_file(&hash).filter(|doc| doc["findings"].is_array());
    if analyze && doc.is_none() && !id.starts_with("cloud:") && resolve(id).is_ok() {
        analyze_log(id);
        doc = read_findings_file(&hash).filter(|doc| doc["findings"].is_array());
    }
    let doc = doc?;
    let results = doc["results"].as_object()?;
    let mut notes = serde_json::Map::new();
    let calls: Vec<Value> = doc["calls"].as_array().into_iter().flatten().map(|call| {
        let tool = call["tool"].as_str().unwrap_or("");
        let args = &call["args"];
        let key = tool_key(tool, args["axis"].as_str(), args["start_us"].as_u64(), args["end_us"].as_u64());
        let res = results.get(&key).cloned().unwrap_or_else(|| json!({ "ok": false, "skipped": call["skipped"], "error": call["error"] }));
        if let Some(note) = res["note"].as_str() {
            notes.entry(tool.to_string()).or_insert_with(|| json!(note));
        }
        let mut shown = args.clone();
        shown["file"] = json!(id);
        json!({ "tool": tool, "args": shown, "result": result_digest(tool, &res) })
    }).collect();
    let findings: Vec<Value> = doc["findings"].as_array().into_iter().flatten().map(|f| json!({
        "tool": f["tool"],
        "severity": f["severity"],
        "text": f["text"],
        "args": f["args"],
    })).collect();
    Some(json!({ "file": id, "findings": findings, "calls": calls, "notes": notes }))
}

#[cfg(test)]
mod findings_tests {
    use super::*;

    fn sitl(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dflog/testdata/sitl").join(format!("{name}.bin"));
        std::fs::read(&path).unwrap()
    }

    fn kinds(doc: &Value) -> Vec<(String, String)> {
        doc["findings"].as_array().unwrap().iter().map(|f| (f["kind"].as_str().unwrap().to_string(), f["values"]["category"].as_str().unwrap_or("").to_string())).collect()
    }

    fn has(doc: &Value, kind: &str, category: &str) -> bool {
        kinds(doc).iter().any(|(k, c)| k == kind && (category.is_empty() || c == category))
    }

    #[test]
    fn healthy_flight_has_no_findings() {
        let doc = findings_document(&sitl("healthy"));
        assert_eq!(doc["findings"], json!([]), "{}", doc["findings"]);
        // Every tool ran, and each result is cached under the key the assistant's call uses.
        let tools: Vec<&str> = doc["calls"].as_array().unwrap().iter().map(|c| c["tool"].as_str().unwrap()).collect();
        for tool in ["log_audit", "log_filter_review", "log_motor_balance"] {
            assert!(tools.contains(&tool), "{tools:?}");
        }
        assert_eq!(tools.iter().filter(|t| **t == "log_pid_review").count(), 3, "{tools:?}");
        let results = doc["results"].as_object().unwrap();
        for key in [tool_key("log_audit", None, None, None), tool_key("log_filter_review", None, None, None), tool_key("log_motor_balance", None, None, None), tool_key("log_pid_review", Some("Roll"), None, None)] {
            assert!(results.contains_key(&key), "{key}: {:?}", results.keys().collect::<Vec<_>>());
        }
    }

    #[test]
    fn each_fault_log_shows_its_finding() {
        let notch = findings_document(&sitl("notch"));
        let filter = notch["findings"].as_array().unwrap().iter().find(|f| f["kind"] == json!("filter_proposal")).expect("filter finding");
        assert!(filter["values"]["changes"].as_array().unwrap().iter().any(|c| c["name"] == json!("INS_HNTCH_FREQ")), "{filter}");
        assert!(filter["text"].as_str().unwrap().contains("INS_HNTCH_FREQ 60 → "), "{filter}");

        assert!(has(&findings_document(&sitl("vibration")), "audit_issue", "vibration"));
        assert!(has(&findings_document(&sitl("ekf_lane")), "audit_issue", "ekf"));
        let gps = findings_document(&sitl("gps_loss"));
        assert!(has(&gps, "audit_issue", "ekf") && has(&gps, "audit_issue", "gps"), "{:?}", kinds(&gps));
        let crash = findings_document(&sitl("crash"));
        let lost = crash["findings"].as_array().unwrap().iter().find(|f| f["values"]["category"] == json!("crash")).expect("crash finding");
        assert_eq!(lost["severity"], json!("critical"), "{lost}");

        // The PID finding carries the tool's margins and its proposal, with the axis it was read on.
        let pid = findings_document(&sitl("pid"));
        let tune = pid["findings"].as_array().unwrap().iter().find(|f| f["tool"] == json!("log_pid_review")).expect("pid finding");
        assert!(tune["values"]["phase_margin_deg"].is_number(), "{tune}");
        assert!(tune["args"]["axis"].is_string(), "{tune}");
        assert!(!tune["values"]["changes"].as_array().unwrap().is_empty(), "{tune}");
        assert!(tune["text"].as_str().unwrap().contains("proposed_params"), "{tune}");
    }

    #[test]
    fn system_id_runs_are_reviewed_on_their_own_axis_and_interval() {
        let doc = findings_document(&sitl("sysid"));
        let bounded: Vec<&Value> = doc["calls"].as_array().unwrap().iter().filter(|c| c["args"]["start_us"].is_u64()).collect();
        assert_eq!(bounded.iter().map(|c| c["args"]["axis"].as_str().unwrap()).collect::<Vec<_>>(), ["roll", "pitch"], "{}", doc["calls"]);
        assert!(bounded.iter().all(|c| c["ok"] == json!(true) && c["args"]["end_us"].as_u64() > c["args"]["start_us"].as_u64()), "{}", doc["calls"]);
        // SITL's rate loops track cleanly in the chirps: no PID rule fires.
        assert!(!doc["findings"].as_array().unwrap().iter().any(|f| f["tool"] == json!("log_pid_review")), "{}", doc["findings"]);
    }

    #[test]
    fn magfit_runs_with_its_defaults_and_a_skipped_call_is_not_cached() {
        // MAGFit fits the offsets of magfit.bin; its orientation check matches, so no rule fires.
        let doc = findings_document(&sitl("magfit"));
        let call = doc["calls"].as_array().unwrap().iter().find(|c| c["tool"] == json!("log_magfit")).expect("magfit call");
        assert_eq!(call["ok"], json!(true), "{call}");
        let res = &doc["results"][tool_key("log_magfit", None, None, None).as_str()];
        assert!(res["fit_error_mgauss"].as_f64().unwrap() < res["orig_error_mgauss"].as_f64().unwrap(), "{res}");
        assert!(result_digest("log_magfit", res)["params"].is_object());
        assert!(!doc["findings"].as_array().unwrap().iter().any(|f| f["tool"] == json!("log_magfit")), "{}", doc["findings"]);
        // Without a gyro batch the filter review is listed as not made, and nothing is cached for it.
        let filter = doc["calls"].as_array().unwrap().iter().find(|c| c["tool"] == json!("log_filter_review")).unwrap();
        assert_eq!(filter["skipped"], json!(true), "{filter}");
        assert!(doc["results"].get(tool_key("log_filter_review", None, None, None)).is_none());
    }

    #[test]
    fn the_assistant_digest_keeps_the_numbers_and_drops_the_curves() {
        let doc = findings_document(&sitl("pid"));
        let res = &doc["results"][tool_key("log_pid_review", Some("pitch"), None, None).as_str()];
        let digest = result_digest("log_pid_review", res);
        assert!(digest["metrics"]["phase_margin_deg"].is_number(), "{digest}");
        assert!(digest.get("frequency_response").is_none() && digest.get("step_response").is_none() && digest.get("note").is_none(), "{digest}");
        assert!(digest.to_string().len() * 2 < res.to_string().len(), "{} vs {}", digest.to_string().len(), res.to_string().len());
    }

    #[test]
    #[ignore]
    fn findings_dump() {
        for name in ["healthy", "notch", "vibration", "ekf_lane", "gps_loss", "crash", "magfit", "motors", "pid", "sysid", "plane_pid"] {
            let doc = findings_document(&sitl(name));
            println!("== {name}\n{}\n{}",serde_json::to_string_pretty(&doc["findings"]).unwrap(), doc["calls"]);
        }
    }
}
