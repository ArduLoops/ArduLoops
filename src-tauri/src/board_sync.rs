//! Board sync with the signed-in account (arduloops-api `/v1/boards/*`), so a pilot sees the same
//! boards on every device: the PC, a laptop at the field, the Android app.
//!
//! What syncs, per board (keyed by its MCU hardware id, the catalog's controller key):
//! - identity: board id, board name, vehicle type, first and last time on a link;
//! - the user's note (name on the first line) and vehicle profile;
//! - the last parameter set saved from the board, as a read-only snapshot.
//!
//! What does not: assistant chats. They can be long, hold whatever was pasted into them, and are
//! about one session at one bench; they stay on the device they were made on. Logs sync on their
//! own (cloud.rs). Nothing here talks to a vehicle: a snapshot from the account goes into
//! `cloud_boards`, never into `param_cache`, and is only shown. Restoring parameters stays the
//! explicit, confirmed action it is on the Params screen.
//!
//! Merging, the same rule as log stars and notes: per field group, the change with the later time
//! wins (note by `comment_at`, profile by `profile_at`, the whole parameter snapshot by its
//! `saved_at`); first seen is the earliest, last seen the latest. Times are the device clock in
//! unix seconds. The API applies the same rule, so devices may push in any order. A board linked on
//! a phone and later on the PC is one board, because both know it by the same hardware id.
//!
//! Removing a board from the account leaves a tombstone. Every device keeps its own copy, and pushes
//! the board again only with a change made after the removal (linked again, a new note).
//!
//! On by default whenever the user is signed in, like log stars and notes (they also sync without
//! a switch, and they already carry the board's id); a switch turns it off and drops the copy of the
//! account's boards from this device. Signed out: nothing leaves the device, cloud-only boards are
//! hidden, and edits made meanwhile are pushed after the next sign-in. Offline: the last list and
//! the fetched snapshots are shown as they were; the error is kept for the Cloud line, not raised.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::db::data_dir;

/// A sync runs this long after start-up, then every SWEEP, and soon after a change here.
const FIRST: Duration = Duration::from_secs(5);
const SWEEP: Duration = Duration::from_secs(300);
/// Opening the board list asks for a sync when the last one is older than this.
const FRESH_SECS: i64 = 60;

/// One sync at a time: the worker and "Sync now" wait for each other.
static RUNNING: Mutex<()> = Mutex::new(());
/// The worker's next run, and whether a change asked for it (true) or it is the periodic sweep.
static DUE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();
/// The last run: when, and what went wrong (offline, account full), for the Cloud line.
static LAST: Mutex<Option<Value>> = Mutex::new(None);

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn settings_path() -> PathBuf {
    data_dir().join("board-sync.json")
}

fn settings() -> Value {
    fs::read(settings_path()).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_else(|| json!({}))
}

fn save_settings(value: &Value) -> Result<(), String> {
    fs::write(settings_path(), value.to_string()).map_err(|e| e.to_string())
}

/// On unless the user turned it off.
pub fn enabled() -> bool {
    settings()["enabled"].as_bool().unwrap_or(true)
}

/// The account whose boards `cloud_boards` holds: (signed in, that account's id).
fn account() -> Option<String> {
    crate::agent::signed_in().then(crate::agent::session_email).flatten()
}

/// Whether cloud boards are shown: sync on, signed in, and the copy here is this account's.
pub fn visible() -> bool {
    enabled() && account().is_some_and(|who| settings()["account"].as_str() == Some(who.as_str()))
}

pub fn set_enabled(on: bool) -> Result<Value, String> {
    let mut value = settings();
    value["enabled"] = json!(on);
    if !on {
        value["account"] = Value::Null;
        crate::db::clear_cloud_boards()?;
    }
    save_settings(&value)?;
    if on {
        poke(0);
    }
    Ok(state())
}

/// Called on sign-out: the next person on this device does not see the boards of the last one.
pub fn forget_account() {
    let mut value = settings();
    value["account"] = Value::Null;
    let _ = save_settings(&value);
    let _ = crate::db::clear_cloud_boards();
    if let Ok(mut last) = LAST.lock() {
        *last = None;
    }
}

/// The Cloud line for boards: signed in, on, the last sync and its error, the account's count.
pub fn state() -> Value {
    let last = LAST.lock().ok().and_then(|l| l.clone()).unwrap_or(Value::Null);
    let signed_in = crate::agent::signed_in();
    json!({
        "signed_in": signed_in,
        "enabled": enabled(),
        "synced_at": last["at"],
        "error": last["error"],
        "count": last["count"],
        "limit": last["limit"],
    })
}

// ---------- the worker ----------

/// Starts the background sync. Called once with the bridge.
pub fn start() {
    if let Ok(mut due) = DUE.lock() {
        *due = Some((Instant::now() + FIRST, true));
    }
    std::thread::spawn(|| loop {
        {
            let Ok(mut due) = DUE.lock() else { return };
            loop {
                let (at, _) = *due.get_or_insert_with(|| (Instant::now() + SWEEP, false));
                let left = at.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    *due = None;
                    break;
                }
                due = match WAKE.wait_timeout(due, left) {
                    Ok((guard, _)) => guard,
                    Err(_) => return,
                };
            }
        }
        if enabled() && crate::agent::signed_in() {
            let _ = sync();
        }
    });
}

/// Asks for a sync `delay_secs` from now. A later change pushes a pending one back, so a burst of
/// edits (or a parameter download) goes up as one push; 0 runs it now.
pub fn poke(delay_secs: u64) {
    if let Ok(mut due) = DUE.lock() {
        let at = Instant::now() + Duration::from_secs(delay_secs);
        let next = match *due {
            Some((old, true)) if delay_secs > 0 => old.max(at),
            _ => at,
        };
        *due = Some((next, true));
        WAKE.notify_all();
    }
}

/// A sync when the board list is opened and the last sync is not fresh.
fn freshen() {
    let at = LAST.lock().ok().and_then(|l| l.as_ref().and_then(|v| v["at"].as_i64())).unwrap_or(0);
    if now() - at > FRESH_SECS {
        poke(0);
    }
}

fn record(error: Option<String>, list: Option<&Value>) {
    if let Ok(mut last) = LAST.lock() {
        let previous = last.clone().unwrap_or(Value::Null);
        *last = Some(json!({
            "at": now(),
            "error": error,
            "count": list.map(|l| l["count"].clone()).unwrap_or(previous["count"].clone()),
            "limit": list.map(|l| l["limit"].clone()).unwrap_or(previous["limit"].clone()),
        }));
    }
}

// ---------- one sync ----------

/// The newest change a local board carries, for comparing with a removal.
fn changed_at(row: &Value) -> i64 {
    ["last_seen", "comment_at", "profile_at", "params_at"].iter().map(|k| row[*k].as_i64().unwrap_or(0)).max().unwrap_or(0)
}

/// Whether the account's copy lacks something this device has. `same_params` is true when the
/// account's snapshot holds the same values as the one here (a re-save does not count as a change).
pub(crate) fn needs_push(local: &Value, cloud: Option<&Value>, same_params: bool) -> (bool, bool) {
    let at = |v: &Value, k: &str| v[k].as_i64().unwrap_or(0);
    let has_params = at(local, "params_count") > 0 && at(local, "params_at") > 0;
    let Some(cloud) = cloud else {
        return (true, has_params);
    };
    // A removed board comes back with everything this device has, once it has a later change.
    let removed = at(cloud, "deleted_at");
    if removed > 0 {
        let newer = changed_at(local) > removed;
        return (newer, newer && has_params);
    }
    let params = has_params && at(local, "params_at") > at(cloud, "params_at") && !same_params;
    let first = at(local, "first_seen");
    let push = params
        || at(local, "last_seen") > at(cloud, "last_seen")
        || (first > 0 && (at(cloud, "first_seen") == 0 || first < at(cloud, "first_seen")))
        || at(local, "comment_at") > at(cloud, "comment_at")
        || at(local, "profile_at") > at(cloud, "profile_at");
    (push, params)
}

/// Lists the account's boards, applies newer notes here, pushes what the account lacks and fetches
/// snapshots newer than the ones here. Errors are recorded, not raised to the UI.
pub fn sync() -> Result<Value, String> {
    let _one = RUNNING.lock().map_err(|_| "board sync lock".to_string())?;
    let result = sync_once();
    match &result {
        Ok(list) => record(list["refused"].as_str().map(str::to_string), Some(list)),
        Err(error) => {
            log::warn!("board sync: {error}");
            record(Some(error.clone()), None);
        }
    }
    result
}

fn sync_once() -> Result<Value, String> {
    if !enabled() {
        return Err("Board sync is off.".into());
    }
    let Some(who) = account() else {
        return Err("Sign in required.".into());
    };
    // Another account's copy goes before this one's arrives.
    let mut saved = settings();
    if saved["account"].as_str() != Some(who.as_str()) {
        crate::db::clear_cloud_boards()?;
        saved["account"] = json!(who);
        save_settings(&saved)?;
    }

    let list = crate::agent::cloud_post("/v1/boards/list", &json!({}))?;
    let rows = list["boards"].as_array().cloned().unwrap_or_default();
    crate::db::store_cloud_boards(&rows)?;
    let cloud: HashMap<String, Value> = rows.iter().filter_map(|r| Some((r["key"].as_str()?.to_string(), r.clone()))).collect();

    let mut full = false;
    // A board the account refuses (too large, a note it cannot take) does not hold up the others.
    let mut refused: Option<String> = None;
    for local in crate::db::board_sync_rows()? {
        let Some(key) = local["key"].as_str().map(str::to_string) else { continue };
        let theirs = cloud.get(&key);
        if let Some(c) = theirs.filter(|c| c["deleted_at"].as_i64().unwrap_or(0) == 0) {
            let _ = crate::db::merge_board_note(
                &key,
                c["comment"].as_str().unwrap_or(""),
                c["comment_at"].as_i64().unwrap_or(0),
                &c["profile"],
                c["profile_at"].as_i64().unwrap_or(0),
            );
        }
        let mine = crate::db::local_board_params(&key)?;
        let same = match (&mine, crate::db::cloud_board_params(&key)?) {
            (Some((params, _, _)), Some((Value::Object(theirs), _, _))) => *params == theirs,
            _ => false,
        };
        let (push, with_params) = needs_push(&local, theirs, same);
        if !push || (full && theirs.is_none()) {
            continue;
        }
        let mut body = json!({
            "key": key,
            "board_id": local["board_id"],
            "board_name": local["board_name"],
            "vehicle": local["vehicle"],
            "first_seen": local["first_seen"],
            "last_seen": local["last_seen"],
            "comment": local["comment"],
            "comment_at": local["comment_at"],
            "profile": local["profile"],
            "profile_at": local["profile_at"],
        });
        if with_params {
            if let Some((params, at, frame)) = &mine {
                body["params"] = Value::Object(params.clone());
                body["params_at"] = json!(at);
                body["params_frame"] = json!(frame);
            }
        }
        match crate::agent::cloud_post("/v1/boards/push", &body) {
            Ok(done) => {
                crate::db::store_cloud_board(&done["board"])?;
                if with_params {
                    if let Some((params, at, _)) = &mine {
                        crate::db::store_cloud_params(&key, &Value::Object(params.clone()), *at)?;
                    }
                }
            }
            // The account holds as many boards as it allows: the others still update.
            Err(error) if error.contains("as many boards") => full = true,
            Err(error) => {
                log::warn!("board sync: {key}: {error}");
                refused.get_or_insert(error);
            }
        }
    }

    fetch_newer_snapshots()?;
    let mut out = list;
    // The count after this sync's pushes, not the one listed before them.
    out["count"] = json!(crate::db::cloud_boards()?.iter().filter(|c| c["deleted_at"].as_i64().unwrap_or(0) == 0).count());
    out["full"] = json!(full);
    if full {
        refused = Some("This account keeps as many boards as it allows.".into());
    }
    if let Some(error) = refused {
        // sync() records it, so the Cloud line shows why a board is not in the account.
        out["refused"] = json!(error);
    }
    Ok(out)
}

/// Fetches the account's parameter snapshots that are newer than the ones here, so the boards can
/// be read offline at the field.
fn fetch_newer_snapshots() -> Result<(), String> {
    let local: HashMap<String, i64> = crate::db::board_sync_rows()?
        .into_iter()
        .filter_map(|r| Some((r["key"].as_str()?.to_string(), r["params_at"].as_i64().unwrap_or(0))))
        .collect();
    for row in crate::db::cloud_boards()? {
        let (Some(key), at) = (row["key"].as_str(), row["params_at"].as_i64().unwrap_or(0)) else { continue };
        if row["deleted_at"].as_i64().unwrap_or(0) > 0 || at == 0 || at <= local.get(key).copied().unwrap_or(0) {
            continue;
        }
        if crate::db::cloud_board_params(key)?.is_some_and(|(_, fetched, _)| fetched == at) {
            continue;
        }
        fetch_snapshot(key)?;
    }
    Ok(())
}

fn fetch_snapshot(key: &str) -> Result<(Value, i64, String), String> {
    let got = crate::agent::cloud_post("/v1/boards/params", &json!({ "key": key }))?;
    let at = got["params_at"].as_i64().unwrap_or(0);
    crate::db::store_cloud_params(key, &got["params"], at)?;
    Ok((got["params"].clone(), at, got["frame"].as_str().unwrap_or("").to_string()))
}

/// Removes a board from the account for every device. The copy on each device stays.
pub fn remove(key: &str) -> Result<Value, String> {
    if !crate::agent::signed_in() {
        return Err("Sign in required.".into());
    }
    let done = crate::agent::cloud_post("/v1/boards/remove", &json!({ "key": key, "at": now() }))?;
    let at = done["deleted_at"].as_i64().unwrap_or_else(now);
    crate::db::store_cloud_board(&json!({ "key": key, "deleted_at": at, "updated_at": now() }))?;
    Ok(json!({ "ok": true, "key": key, "deleted_at": at }))
}

// ---------- what the UI and the assistant read ----------

/// The account's live copy of a board, if it is shown.
fn cloud_row(cloud: &[Value], key: &str) -> Option<Value> {
    cloud.iter().find(|c| c["key"] == key && c["deleted_at"].as_i64().unwrap_or(0) == 0).cloned()
}

/// The boards on this device and, when sync is on and signed in, the account's boards that were
/// never linked here (`source: "cloud"`, read only). Each local board says where it stands with the
/// account in `cloud`: synced, pending, removed, off or signed_out.
pub fn boards(linked: &str) -> Result<Value, String> {
    let mut out = crate::db::boards(linked)?;
    let visible = visible();
    let signed_in = crate::agent::signed_in();
    if enabled() && signed_in {
        freshen();
    }
    let cloud = if visible { crate::db::cloud_boards().unwrap_or_default() } else { Vec::new() };
    let mine: HashMap<String, Value> = crate::db::board_sync_rows()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| Some((r["key"].as_str()?.to_string(), r)))
        .collect();
    let mut rows = out["boards"].as_array().cloned().unwrap_or_default();
    for row in rows.iter_mut() {
        let key = row["key"].as_str().unwrap_or("").to_string();
        row["source"] = json!("local");
        row["cloud"] = json!(if !signed_in {
            "signed_out"
        } else if !enabled() {
            "off"
        } else if let Some(c) = cloud.iter().find(|c| c["key"] == key.as_str()) {
            let same = cloud_params_match(&key);
            if c["deleted_at"].as_i64().unwrap_or(0) > 0 && !mine.get(&key).is_some_and(|l| needs_push(l, Some(c), same).0) {
                "removed"
            } else if mine.get(&key).is_some_and(|l| needs_push(l, Some(c), same).0) {
                "pending"
            } else {
                "synced"
            }
        } else {
            "pending"
        });
        if let Some(c) = cloud_row(&cloud, &key) {
            // The board as the account knows it: seen later elsewhere, a newer parameter snapshot.
            row["last_seen_anywhere"] = json!(row["last_seen"].as_i64().unwrap_or(0).max(c["last_seen"].as_i64().unwrap_or(0)));
            if c["params_at"].as_i64().unwrap_or(0) > row["params_at"].as_i64().unwrap_or(0) {
                row["params"] = c["params_count"].clone();
                row["params_at"] = c["params_at"].clone();
                row["params_source"] = json!("cloud");
            }
        }
    }
    for c in cloud.iter().filter(|c| c["deleted_at"].as_i64().unwrap_or(0) == 0) {
        let key = c["key"].as_str().unwrap_or("");
        if key.is_empty() || mine.contains_key(key) {
            continue;
        }
        rows.push(json!({
            "key": key,
            "board_id": c["board_id"],
            "board_name": c["board_name"],
            "vehicle": c["vehicle"],
            "comment": c["comment"],
            "profile": c["profile"],
            "first_seen": c["first_seen"],
            "last_seen": c["last_seen"],
            "last_seen_anywhere": c["last_seen"],
            "linked": false,
            "params": c["params_count"],
            "params_at": c["params_at"],
            "params_source": "cloud",
            "logs": crate::db::board_log_count(key),
            "chats": 0,
            "source": "cloud",
            "cloud": "cloud_only",
        }));
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r["last_seen_anywhere"].as_i64().or(r["last_seen"].as_i64()).unwrap_or(0)));
    out["boards"] = Value::Array(rows);
    out["sync"] = state();
    Ok(out)
}

fn cloud_params_match(key: &str) -> bool {
    match (crate::db::local_board_params(key), crate::db::cloud_board_params(key)) {
        (Ok(Some((mine, _, _))), Ok(Some((Value::Object(theirs), _, _)))) => mine == theirs,
        _ => false,
    }
}

/// The newest parameter snapshot of a board: the one saved here, or the account's when it is newer
/// (`source: "cloud"`). Fetched on demand when the account's is newer and not here yet; offline,
/// the one here is returned.
pub fn board_params(key: &str) -> Result<Value, String> {
    let mut local = crate::db::board_params(key)?;
    local["source"] = json!("local");
    if !visible() {
        return Ok(local);
    }
    let cloud = crate::db::cloud_boards().unwrap_or_default();
    let Some(row) = cloud_row(&cloud, key) else {
        return Ok(local);
    };
    let cloud_at = row["params_at"].as_i64().unwrap_or(0);
    if cloud_at <= local["saved_at"].as_i64().unwrap_or(0) {
        return Ok(local);
    }
    let snapshot = match crate::db::cloud_board_params(key)? {
        Some((params, at, frame)) if at == cloud_at => Some((params, at, frame)),
        _ => fetch_snapshot(key).ok(),
    };
    Ok(match snapshot {
        Some((params, at, frame)) => json!({
            "key": key, "saved_at": at, "frame": frame, "board_name": row["board_name"], "params": params, "source": "cloud",
        }),
        None => {
            local["cloud_newer"] = json!(cloud_at);
            local
        }
    })
}

#[cfg(test)]
mod tests {
    use super::needs_push;
    use serde_json::json;

    fn local() -> serde_json::Value {
        json!({ "key": "abcd1234", "first_seen": 100, "last_seen": 200, "comment_at": 150, "profile_at": 0, "params_at": 190, "params_count": 900 })
    }

    #[test]
    fn a_board_the_account_lacks_goes_up_with_its_parameters() {
        assert_eq!(needs_push(&local(), None, false), (true, true));
        let no_params = json!({ "key": "abcd1234", "last_seen": 200, "params_count": 0, "params_at": 0 });
        assert_eq!(needs_push(&no_params, None, false), (true, false));
    }

    #[test]
    fn nothing_goes_up_when_the_account_is_as_new() {
        let cloud = json!({ "first_seen": 100, "last_seen": 300, "comment_at": 150, "profile_at": 0, "params_at": 190 });
        assert_eq!(needs_push(&local(), Some(&cloud), false), (false, false));
    }

    #[test]
    fn only_newer_fields_count_and_a_resaved_snapshot_is_not_new() {
        let cloud = json!({ "first_seen": 100, "last_seen": 200, "comment_at": 100, "profile_at": 0, "params_at": 100 });
        assert_eq!(needs_push(&local(), Some(&cloud), false), (true, true));
        assert_eq!(needs_push(&local(), Some(&cloud), true), (true, false), "the note is newer, the parameters are the same");
        let earlier = json!({ "first_seen": 150, "last_seen": 200, "comment_at": 150, "profile_at": 0, "params_at": 190 });
        assert_eq!(needs_push(&local(), Some(&earlier), false), (true, false), "an earlier first link goes up");
    }

    #[test]
    fn a_removed_board_goes_up_again_only_with_a_later_change() {
        let removed = json!({ "deleted_at": 250 });
        assert_eq!(needs_push(&local(), Some(&removed), false), (false, false));
        let mut relinked = local();
        relinked["last_seen"] = json!(300);
        assert_eq!(needs_push(&relinked, Some(&removed), false), (true, true), "it comes back whole, from this device");
    }
}
