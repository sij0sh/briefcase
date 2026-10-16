//! Advisory freshness of pinned publications and a bounded live Git check.
//! None of these observations changes retained architecture authority.
use super::{
    contracts::rows,
    ledger::{Ledger, View},
    native_host, runtime_observations, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub(crate) fn for_views(l: &Ledger, views: &[View]) -> Result<Value> {
    let pins: Vec<Value> = views.iter().map(|v| {
        let capture = &v.diagnostics["capture"];
        json!({"view_id":v.id,"scope":v.scope,
            "capture_state":rows(&v.bundle,"snapshots").first().map(|s|s["capture_state"].clone()).unwrap_or_else(||v.diagnostics["capture_state"].clone()),
            "captured_at":capture["captured_at"],"captured_at_unix_ms":capture["captured_at_unix_ms"],"capture_head":capture["capture_head"],"repository_root":capture["repository_root"],
            "stale":v.stale,"projection_check":"checked","projection_state":if v.stale {"stale_or_invalidated"} else {"consistent"},"invalidated":v.invalidated})
    }).collect();
    envelope(l, pins)
}

fn envelope(l: &Ledger, mut pins: Vec<Value>) -> Result<Value> {
    for pin in &mut pins {
        let captured = pin["captured_at_unix_ms"].as_u64();
        pin["capture_time_state"] = json!(if captured.is_some() {
            "known"
        } else {
            "unknown"
        });
        let now = now_millis();
        pin["captured_at"] = captured
            .map(|ms| json!(runtime_observations::timestamp(ms as i128 * 1_000_000)))
            .unwrap_or(Value::Null);
        pin["age_seconds"] = captured
            .filter(|ms| *ms <= now)
            .map(|ms| json!((now - ms) / 1000))
            .unwrap_or(Value::Null);
        pin["clock_skew"] = json!(captured.is_some_and(|ms| ms > now));
        if captured.is_none() {
            pin["capture_time_reason"] = json!(if pin["scope"] == "source" {
                "This publication predates capture-time recording; it is not evidence of staleness. The next sync that re-reads changed sources records the time."
            } else {
                "This scope is not a source capture; its records carry their own event times."
            });
        }
    }
    let primary = pins
        .iter()
        .find(|p| p["scope"] == "source")
        .or_else(|| pins.first())
        .cloned()
        .unwrap_or(Value::Null);
    let mut envelope = json!({"view_id":primary["view_id"],"captured_at":primary["captured_at"],"age_seconds":primary["age_seconds"],
        "capture_time_state":primary["capture_time_state"],"capture_state":primary["capture_state"],"stale":primary["stale"],
        "views":pins,"pending_or_interrupted":l.pending_source()?,
        "dirty_hit_paths":{"state":"not_requested","paths":[],"dirty_entry_count":null},
        "interpretation":"Capture age and projection validity do not establish equality with live files. Dirty hit paths overlap Git working-tree changes; they may already be captured."});
    if let Some(reason) = primary.get("capture_time_reason") {
        envelope["capture_time_reason"] = reason.clone();
    }
    Ok(envelope)
}

/// Bounded Git reads use an isolated process, never repository hooks or shell expansion.
fn git(root:&Path,args:&[&str])->Result<Vec<u8>> {
    let executable=native_host::executable(Path::new("git"),"Git")?;
    let mut command=native_host::command(&executable,root);
    command.env("GIT_OPTIONAL_LOCKS","0").args(["-c","core.fsmonitor=false","-c","core.untrackedCache=false"]).args(args);
    Ok(native_host::run(command,&[],Instant::now()+Duration::from_secs(2))?.stdout)
}
pub(crate) fn head(root:&Path)->Result<String> {
    let raw=git(root,&["rev-parse","--verify","HEAD^{commit}"])?;
    let oid=std::str::from_utf8(&raw)?.trim();
    super::require([40,64].contains(&oid.len())&&oid.bytes().all(|c|c.is_ascii_hexdigit()),"FRESHNESS","Invalid Git HEAD")?;
    Ok(oid.into())
}
fn git_alignment(root:&Path,capture:Option<&str>)->Value {
    let live=match head(root) {Ok(head)=>head,Err(error)=>return json!({"state":"unavailable","capture_head":capture,
        "live_head":null,"commits_behind":null,"reason":error.to_string().chars().take(400).collect::<String>()})};
    let Some(capture)=capture else {return json!({"state":"unknown_capture_head","capture_head":null,"live_head":live,"commits_behind":null,"changed_paths":[]});};
    let compare=(||->Result<Value>{
        let range=format!("{capture}...{live}");
        let raw=git(root,&["rev-list","--left-right","--count",&range])?;
        let numbers=std::str::from_utf8(&raw)?.split_whitespace().map(str::parse::<u64>).collect::<std::result::Result<Vec<_>,_>>()?;
        super::require(numbers.len()==2,"FRESHNESS","Invalid Git revision counts")?;
        let raw=git(root,&["diff","--name-only","-z",capture,&live,"--"])?;
        let paths=raw.split(|b|*b==0).filter(|p|!p.is_empty()).map(std::str::from_utf8).collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(json!({"state":if capture==live {"same_head"}else if numbers[0]>0 {"diverged"}else{"behind"},
            "capture_head":capture,"live_head":live,"commits_behind":numbers[1],"commits_ahead":numbers[0],
            "changed_paths":paths.iter().take(50).collect::<Vec<_>>(),"changed_path_count":paths.len(),"omitted_path_count":paths.len().saturating_sub(50),
            "scope":"Committed-tree changes between capture HEAD and live HEAD; working-tree changes are reported separately."}))
    })();
    compare.unwrap_or_else(|error|json!({"state":"unavailable_comparison","capture_head":capture,"live_head":live,
        "commits_behind":null,"reason":error.to_string().chars().take(400).collect::<String>()}))
}

fn collect_pins(value: &Value, pins: &mut BTreeSet<String>) {
    // Pins come from known result envelopes, never from retained source,
    // transcript text, arbitrary resource attributes or assertion payloads.
    for pointer in [
        "/pinned_views",
        "/views",
        "/view_id",
        "/view",
        "/source_view",
        "/base_view",
        "/before_view",
        "/after_view",
        "/current_view",
        "/scope/source_view",
        "/scope/view",
        "/scope/base_view",
        "/scope/deployment_view",
        "/scope/runtime_views",
        "/provenance/source_view",
        "/provenance/view",
        "/provenance/base_view",
        "/provenance/deployment_view",
        "/provenance/runtime_views",
        "/pins/source/view_id",
        "/pins/deployment/view_id",
    ] {
        if let Some(v) = value.pointer(pointer) {
            for pin in v
                .as_str()
                .into_iter()
                .chain(v.as_array().into_iter().flatten().filter_map(Value::as_str))
            {
                if pin.starts_with("view:") {
                    pins.insert(pin.into());
                }
            }
        }
    }
    for pointer in ["/sessions", "/pins/runtime", "/explanations"] {
        for item in value
            .pointer(pointer)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(pin) = item["view_id"].as_str().filter(|p| p.starts_with("view:")) {
                pins.insert(pin.into());
            }
            if pointer == "/explanations" {
                collect_pins(item, pins);
            }
        }
    }
}

fn hit_paths(value: &Value, paths: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if ["commit", "session"]
                .iter()
                .any(|kind| value["source_kind"] == *kind)
            {
                return;
            }
            for (key, item) in object {
                if ["path", "file", "source_locator"].contains(&key.as_str()) {
                    if let Some(path) = item.as_str().filter(|s| {
                        !s.starts_with("git:")
                            && !s.starts_with("session:")
                            && !Path::new(s).is_absolute()
                    }) {
                        paths.insert(path.into());
                    }
                } else if ![
                    "text",
                    "evidence_text",
                    "freshness",
                    "configuration",
                    "profile",
                ]
                .contains(&key.as_str())
                {
                    hit_paths(item, paths);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                hit_paths(item, paths);
            }
        }
        _ => {}
    }
}

pub(crate) fn working_paths(root: &Path) -> Result<BTreeSet<String>> {
    let git = native_host::executable(Path::new("git"), "Git")?;
    let mut command = native_host::command(&git, root);
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ]);
    let output = native_host::run(command, &[], Instant::now() + Duration::from_secs(2))?;
    parse_status(&output.stdout)
}

fn parse_status(bytes: &[u8]) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    let mut entries = bytes.split(|b| *b == 0).filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        super::require(
            entry.len() >= 4 && entry[2] == b' ',
            "FRESHNESS",
            "Malformed Git status record",
        )?;
        paths.insert(std::str::from_utf8(&entry[3..])?.to_owned());
        if entry[..2].iter().any(|c| *c == b'R' || *c == b'C') {
            let original = entries
                .next()
                .ok_or("FRESHNESS: Missing Git rename source")?;
            paths.insert(std::str::from_utf8(original)?.to_owned());
        }
    }
    Ok(paths)
}

/// Attach a uniform freshness envelope without reloading/validating large
/// projections. Already verified query pins retain their stronger check label.
/// Live Git is checked only against an explicitly supplied invocation root.
pub(crate) fn attach(l: &Ledger, result: &mut Value, root: Option<&Path>) -> Result<()> {
    if !result.is_object() {
        return Ok(());
    }
    let existing = result["freshness"].clone();
    let pins = if let Some(pins) = existing["views"].as_array() {
        pins.clone()
    } else {
        let mut ids = BTreeSet::new();
        collect_pins(result, &mut ids);
        ids.into_iter()
            .map(|id| l.view_freshness(&id))
            .collect::<Result<Vec<_>>>()?
    };
    let mut freshness = envelope(l, pins)?;
    if let Some(fallback) = existing.get("fallback") {
        freshness["fallback"] = fallback.clone();
    }
    if let Some(root) = root {
        let recorded_roots: BTreeSet<_> = freshness["views"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["scope"] == "source")
            .filter_map(|p| p["repository_root"].as_str())
            .collect();
        let root_alignment = if recorded_roots.is_empty() {
            "unknown"
        } else if recorded_roots.iter().all(|p| Path::new(p) == root) {
            "matches"
        } else {
            "mismatch"
        };
        // Counts against a root the capture did not record cannot be
        // reconciled with that capture; report no numbers rather than guesses.
        if root_alignment != "matches" {
            freshness["dirty_hit_paths"] = json!({"state":"unknown","root_alignment":root_alignment,"paths":[],"dirty_entry_count":null,
            "reason":if root_alignment == "mismatch" {
                "Invocation root differs from the pinned capture root; no live paths were checked."
            } else {
                "The pinned capture did not record its repository root; live Git status cannot be aligned with it. Use git status/diff directly."
            }});
            result["freshness"] = freshness;
            return Ok(());
        }
        let capture_head=freshness["views"].as_array().into_iter().flatten().find(|p|p["view_id"]==freshness["view_id"]).and_then(|p|p["capture_head"].as_str()).map(str::to_owned);
        freshness["git"]=git_alignment(root,capture_head.as_deref());
        let mut hits = BTreeSet::new();
        hit_paths(result, &mut hits);
        freshness["dirty_hit_paths"] = match working_paths(root) {
            Ok(dirty) => {
                let overlaps: Vec<_> = hits.intersection(&dirty).cloned().collect();
                json!({"state":"checked","root_alignment":root_alignment,"dirty_entry_count":dirty.len(),"overlap_count":overlaps.len(),
                    "paths":overlaps.iter().take(20).collect::<Vec<_>>(),"omitted_path_count":overlaps.len().saturating_sub(20),
                    "scope":"Returned hit paths only; paths beyond the live diagnostic bound remain unknown."})
            }
            Err(error) => {
                json!({"state":"unavailable","root_alignment":root_alignment,"paths":[],"dirty_entry_count":null,"reason":error.to_string().chars().take(400).collect::<String>()})
            }
        };
    }
    result["freshness"] = freshness;
    Ok(())
}

/// Keep dirty overlap scoped to rendered hits after presentation reduction.
/// Paths beyond the diagnostic bound stay explicitly unknown, not zero.
pub(crate) fn filter_returned_hits(result: &mut Value) {
    if result["freshness"]["dirty_hit_paths"]["state"] != "checked" {
        return;
    }
    let mut hits = BTreeSet::new();
    if result.get("items").is_some() {
        hit_paths(&result["items"], &mut hits);
    } else {
        hit_paths(result, &mut hits);
    }
    let dirty = &mut result["freshness"]["dirty_hit_paths"];
    let omitted = dirty["omitted_path_count"].as_u64().unwrap_or(0);
    if let Some(paths) = dirty["paths"].as_array_mut() {
        paths.retain(|p| p.as_str().is_some_and(|p| hits.contains(p)));
    }
    let visible = dirty["paths"].as_array().map_or(0, Vec::len);
    dirty["overlap_count"] = if omitted == 0 {
        json!(visible)
    } else {
        Value::Null
    };
    dirty["scope"] =
        json!("Returned hit paths only; paths beyond the live diagnostic bound remain unknown.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_paths_preserve_spaces_unicode_renames_and_untracked_entries() {
        let paths =
            parse_status(b" M src/a b.rs\0R  src/new.rs\0src/old.rs\0?? docs/\xc3\xa9.md\0")
                .unwrap();
        assert_eq!(
            paths,
            BTreeSet::from([
                "src/a b.rs".into(),
                "src/new.rs".into(),
                "src/old.rs".into(),
                "docs/é.md".into()
            ])
        );
        assert!(parse_status(b"R  new\0").is_err());
    }
}
