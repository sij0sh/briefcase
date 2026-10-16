use super::*;
pub fn validate(b: &Value, payloads: &Payloads) -> Result<BTreeMap<String, bool>> {
    schema(b, "knowledge", None)?;
    for assertion in rows(b, "assertions") {
        serde_json::from_value::<Assertion>(assertion.clone())?;
    }
    let ix = Index::new(b)?;
    validate_contexts(b, &ix, payloads)?;
    let registry: Value = serde_json::from_str(include_str!(
        "../../../contracts/v1/predicate-registry.json"
    ))?;
    validate_assertions(b, &ix, &registry)?;
    validate_derivations(b, &ix, payloads, &registry)?;
    validate_tail(b, &ix, payloads, &registry)
}

fn validate_contexts(b: &Value, ix: &Index, payloads: &Payloads) -> Result<()> {
    for s in rows(b, "snapshots") {
        let mut files = rows(s, "files").to_vec();
        files.sort_by_key(|f| text(f, "path"));
        let unique: BTreeSet<_> = files.iter().map(|f| text(f, "path")).collect();
        require(
            unique.len() == files.len(),
            "DUPLICATE_PATH",
            "Duplicate snapshot paths",
        )?;
        require(
            s["manifest_digest"] == identity::digest(&json!(files))?,
            "DIGEST",
            "Source manifest digest mismatch",
        )?;
    }
    for d in rows(b, "deployment_contexts") {
        let mut files = rows(d, "files").to_vec();
        files.sort_by_key(|f| text(f, "path"));
        let unique: BTreeSet<_> = files.iter().map(|f| text(f, "path")).collect();
        require(
            unique.len() == files.len(),
            "DUPLICATE_PATH",
            "Duplicate deployment input paths",
        )?;
        require(
            d["manifest_digest"] == identity::digest(&json!(files))?,
            "DIGEST",
            "Deployment input manifest digest mismatch",
        )?;
    }
    for build in rows(b, "build_contexts") {
        ix.field("snapshots", build, "snapshot_id")?;
    }
    for table in [
        "entity_versions",
        "runs",
        "pass_manifests",
        "input_packets",
        "authorities",
        "declaration_records",
        "unresolved_references",
        "semantic_binding_resolutions",
        "projections",
        "assertions",
    ] {
        for r in rows(b, table) {
            ix.context(&r["context"])?;
        }
    }
    for entity in rows(b, "entity_versions")
        .iter()
        .filter(|e| !e["source_evidence_ref_id"].is_null())
    {
        let reference = ix.field("evidence_refs", entity, "source_evidence_ref_id")?;
        let artifact = ix.field("artifacts", reference, "artifact_id")?;
        require(
            entity["context"]["kind"] == "source",
            "SCOPE",
            "Module source binding needs a source context",
        )?;
        let snapshot = ix.field("snapshots", &entity["context"], "snapshot_id")?;
        require(
            rows(snapshot, "files")
                .iter()
                .any(|f| f["path"] == artifact["path"] && f["digest"] == artifact["digest"]),
            "SCOPE",
            "Entity source binding outside snapshot",
        )?;
        require(
            rows(b, "assertions").iter().any(|a| {
                a["class"] == "evidence"
                    && a["proposition"]["object"]["entity_version_id"] == entity["id"]
                    && strings(&a["evidence_ref_ids"])
                        .contains(&text(entity, "source_evidence_ref_id"))
            }),
            "ORIGIN",
            "Entity source binding lacks a retained structure assertion",
        )?;
    }
    for r in rows(b, "runtime_contexts") {
        require(
            date_time(&text(r, "start"))? <= date_time(&text(r, "end"))?,
            "INTERVAL",
            "Runtime interval reversed",
        )?;
    }
    for s in rows(b, "session_contexts") {
        require(
            ["cited", "metadata"].contains(&s["retention"].as_str().unwrap_or("full"))
                || s.get("retained_digest").unwrap_or(&s["raw_digest"]) == &s["raw_digest"],
            "DIGEST",
            "Full retention must preserve the original transcript digest",
        )?;
        for key in ["raw_artifact_id", "normalized_artifact_id"] {
            let a = ix.field("artifacts", s, key)?;
            require(
                a["source_kind"] == "session",
                "ORIGIN",
                "Session context must cite retained session artifacts",
            )?;
        }
        require(
            ix.field("artifacts", s, "raw_artifact_id")?["digest"]
                == *s.get("retained_digest").unwrap_or(&s["raw_digest"]),
            "DIGEST",
            "Session raw digest differs from its artifact",
        )?;
        if let (Some(start), Some(end)) = (s["started_at"].as_str(), s["ended_at"].as_str()) {
            require(
                date_time(start)? <= date_time(end)?,
                "INTERVAL",
                "Session interval reversed",
            )?;
        }
    }
    for a in rows(b, "artifacts") {
        let path = std::path::Path::new(a["path"].as_str().unwrap());
        require(
            !path.is_absolute()
                && !path
                    .components()
                    .any(|p| matches!(p, std::path::Component::ParentDir)),
            "ARTIFACT_PATH",
            "Artifact locator must be a safe relative path",
        )?;
        if a["availability"] == "available" {
            let bytes = payloads
                .get(&text(a, "id"))
                .ok_or_else(|| format!("UNAVAILABLE: Missing {}", a["id"]))?;
            require(
                identity::bytes_digest(bytes) == a["digest"],
                "DIGEST",
                "Artifact content digest mismatch",
            )?;
        }
    }
    // Normalized observation artifacts are parsed once, not once per event reference.
    let mut events: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in rows(b, "evidence_refs") {
        let a = ix.field("artifacts", e, "artifact_id")?;
        if a["availability"] != "available" {
            continue;
        }
        let data = &payloads[&text(a, "id")];
        let loc = &e["locator"];
        if loc["kind"] == "span" {
            let start = loc["start"].as_u64().unwrap() as usize;
            let end = loc["end"].as_u64().unwrap() as usize;
            require(
                start < end && end <= data.len(),
                "ANCHOR",
                "Invalid byte span",
            )?;
            require(
                char_boundary(data, start) && std::str::from_utf8(&data[start..end]).is_ok(),
                "ANCHOR",
                "Span splits UTF-8 codepoint",
            )?;
        } else {
            require(
                a["source_kind"] == "runtime",
                "ORIGIN",
                "Event requires runtime artifact",
            )?;
            let aid = text(a, "id");
            if !events.contains_key(&aid) {
                let parsed = identity::parse(data)?;
                events.insert(
                    aid.clone(),
                    rows(&parsed, "events")
                        .iter()
                        .map(|r| text(r, "id"))
                        .collect(),
                );
            }
            require(
                events[&aid].contains(&text(loc, "event_id")),
                "ANCHOR",
                "Unknown event",
            )?;
        }
    }
    for r in rows(b, "runs") {
        ix.field("analysis_configs", r, "analysis_config_id")?;
        let c = &r["coverage"];
        require(
            c["completed"].as_u64() <= c["attempted"].as_u64(),
            "COVERAGE",
            "Completed exceeds attempted",
        )?;
        if c["resolution_complete"] == true {
            require(
                c["completed"] == c["attempted"]
                    && c["unsupported_count"] == 0
                    && c["unresolved_count"] == 0
                    && r["status"] == "complete",
                "COVERAGE",
                "Complete resolution conflicts with limitations",
            )?;
        }
    }
    for d in rows(b, "declaration_records") {
        let a = ix.field("authorities", d, "authority_id")?;
        ix.field("artifacts", d, "artifact_id")?;
        require(
            a["context"] == d["context"],
            "AUTHORITY",
            "Declaration exceeds authority scope",
        )?;
    }
    Ok(())
}

fn validate_assertions(b: &Value, ix: &Index, registry: &Value) -> Result<()> {
    for r in rows(b, "assertions") {
        let prop = &r["proposition"];
        let ctx = &r["context"];
        let name = text(prop, "predicate");
        let rule = &registry["predicates"][&name];
        require(
            !rule.is_null() && rule["status"] == "active",
            "PREDICATE",
            "Unregistered or reserved predicate",
        )?;
        require(
            strings(&rule["classes"]).contains(&text(r, "class"))
                && strings(&rule["contexts"]).contains(&text(ctx, "kind")),
            "CLASS",
            "Forbidden predicate/class/context",
        )?;
        let subject = ix.field("entity_versions", prop, "subject_id")?;
        require(
            strings(&rule["subject_kinds"]).contains(&text(subject, "kind")),
            "KIND",
            "Invalid subject kind",
        )?;
        require(
            contains(ctx, &subject["context"]),
            "SCOPE",
            "Subject outside scope",
        )?;
        let target = if prop["object"]["kind"] == "entity" {
            let target = ix.field("entity_versions", &prop["object"], "entity_version_id")?;
            require(
                strings(&rule["object_kinds"]).contains(&text(target, "kind")),
                "KIND",
                "Invalid object kind",
            )?;
            require(
                contains(ctx, &target["context"]),
                "SCOPE",
                "Object outside scope",
            )?;
            if ctx["kind"] == "comparison" {
                require(
                    subject["context"] != target["context"],
                    "SCOPE",
                    "Lineage must connect different versions",
                )?;
            }
            Some(target)
        } else {
            require(rule["literal"] == true, "KIND", "Literal forbidden")?;
            None
        };
        match r["class"].as_str().unwrap() {
            "evidence" => {
                let run = ix.field("runs", &r["origin"], "run_id")?;
                require(
                    run["context"] == *ctx,
                    "SCOPE",
                    "Evidence run scope differs",
                )?;
                require(
                    run["status"] != "failed",
                    "ORIGIN",
                    "Failed run cannot emit evidence",
                )?;
                for eid in strings(&r["evidence_ref_ids"]) {
                    let e = ix.get("evidence_refs", &eid)?;
                    let a = ix.field("artifacts", e, "artifact_id")?;
                    if ctx["kind"] == "source" {
                        let snap = ix.field("snapshots", ctx, "snapshot_id")?;
                        require(
                            rows(snap, "files")
                                .iter()
                                .any(|f| f["path"] == a["path"] && f["digest"] == a["digest"]),
                            "SCOPE",
                            "Evidence absent from source manifest",
                        )?;
                    }
                    if ctx["kind"] == "session" {
                        let session = ix.field("session_contexts", ctx, "session_context_id")?;
                        require(
                            a["id"] == session["raw_artifact_id"],
                            "SCOPE",
                            "Session evidence must cite its retained transcript",
                        )?;
                    }
                    if ctx["kind"] == "deployment" {
                        let inputs =
                            ix.field("deployment_contexts", ctx, "deployment_context_id")?;
                        require(
                            rows(inputs, "files")
                                .iter()
                                .any(|f| f["path"] == a["path"] && f["digest"] == a["digest"]),
                            "SCOPE",
                            "Evidence absent from deployment input manifest",
                        )?;
                    }
                }
                require(
                    (name == "TRANSFORMS") == !r["transformation"].is_null(),
                    "CLASS",
                    "Only TRANSFORMS Evidence carries a typed transformation",
                )?;
                // Each producer kind writes only its registered evidence vocabulary.
                let authority: &[(&str, &[&str])] = &[
                    ("MAPS_TO", &["schema"]),
                    ("READS", &["semantic"]),
                    ("WRITES", &["semantic"]),
                    ("TRANSFORMS", &["semantic"]),
                    ("ROUTES_TO", &["semantic"]),
                    ("BUILDS", &["build"]),
                    ("DEPLOYS_AS", &["deployment"]),
                    ("CALLS_OBSERVED", &["runtime"]),
                    ("CONNECTION_OBSERVED", &["runtime"]),
                    ("RECORDED_USER_MESSAGE", &["session"]),
                    ("READ_PATH", &["session"]),
                    ("EDITED_PATH", &["session"]),
                    ("RAN_COMMAND", &["session"]),
                ];
                if let Some((_, kinds)) = authority.iter().find(|(p, _)| *p == name) {
                    require(
                        kinds.contains(&run["producer_kind"].as_str().unwrap_or("")),
                        "ORIGIN",
                        "Producer kind lacks authority for this predicate",
                    )?;
                }
                let scoped: &[(&str, &[&str])] = &[
                    ("build", &["BUILDS"]),
                    ("deployment", &["DEPLOYS_AS", "DECLARES_CONNECTION"]),
                    ("runtime", &["CALLS_OBSERVED", "CONNECTION_OBSERVED"]),
                    (
                        "session",
                        &[
                            "RECORDED_USER_MESSAGE",
                            "READ_PATH",
                            "EDITED_PATH",
                            "RAN_COMMAND",
                        ],
                    ),
                ];
                if let Some((_, predicates)) =
                    scoped.iter().find(|(k, _)| run["producer_kind"] == *k)
                {
                    require(
                        predicates.contains(&name.as_str()),
                        "ORIGIN",
                        "Producer kind has no generalized write authority",
                    )?;
                }
                if name == "CALLS_STATIC" {
                    let res = &r["resolution"];
                    require(
                        run["producer_kind"] == "semantic"
                            && res["method"] == "compiler"
                            && res["status"] == "resolved",
                        "RESOLUTION",
                        "Calls require semantic callsite resolution",
                    )?;
                    require(
                        target.is_some() && res["candidate_ids"] == json!([target.unwrap()["id"]]),
                        "RESOLUTION",
                        "Call candidate differs from target",
                    )?;
                    for v in [subject, target.unwrap()] {
                        require(
                            ["compiler", "exact_mapping"]
                                .contains(&v["identity_quality"].as_str().unwrap_or("")),
                            "RESOLUTION",
                            "Syntax identity cannot be a compiler call target",
                        )?;
                    }
                }
                if ["CALLS_OBSERVED", "CONNECTION_OBSERVED"].contains(&name.as_str()) {
                    require(
                        run["producer_kind"] == "runtime",
                        "ORIGIN",
                        "Observation requires runtime producer",
                    )?;
                }
            }
            "derived" => {
                let d = ix.field("derivations", &r["origin"], "derivation_id")?;
                if let Some(mid) = d["manifest_id"].as_str() {
                    require(
                        ix.get("pass_manifests", mid)?["context"] == *ctx,
                        "SCOPE",
                        "Derived result differs from pinned manifest scope",
                    )?;
                }
                require(
                    d["deterministic"] == true,
                    "CLASS",
                    "Nondeterministic result must be Claim",
                )?;
                require(
                    strings(&r["supports"])
                        .iter()
                        .all(|s| strings(&d["input_assertion_ids"]).contains(s)),
                    "INPUT_PACKET",
                    "Support outside pass inputs",
                )?;
            }
            "claim" => {
                let inf = ix.field("inferences", &r["origin"], "inference_id")?;
                let p = ix.field("input_packets", inf, "packet_id")?;
                require(p["context"] == *ctx, "SCOPE", "Packet scope differs")?;
                require(
                    strings(&r["supports"])
                        .iter()
                        .all(|s| strings(&p["assertion_ids"]).contains(s)),
                    "INPUT_PACKET",
                    "Claim support outside packet",
                )?;
                if r["model_score"]["semantics"] == "calibrated" {
                    ix.field("artifacts", &r["model_score"], "calibration_artifact_id")?;
                }
            }
            "declaration" => {
                let d = ix.field("declaration_records", &r["origin"], "declaration_record_id")?;
                require(d["context"] == *ctx, "SCOPE", "Declaration scope differs")?;
            }
            _ => unreachable!(),
        }
        for s in strings(&r["supports"]) {
            require(
                contains(ctx, &ix.get("assertions", &s)?["context"]),
                "SCOPE",
                "Support outside scope",
            )?;
        }
    }
    Ok(())
}

fn validate_derivations(
    b: &Value,
    ix: &Index,
    payloads: &Payloads,
    registry: &Value,
) -> Result<()> {
    for d in rows(b, "derivations") {
        for id in strings(&d["input_assertion_ids"]) {
            ix.get("assertions", &id)?;
        }
        for id in strings(&d["input_artifact_ids"])
            .into_iter()
            .chain(strings(&d["output_artifact_ids"]))
        {
            ix.get("artifacts", &id)?;
        }
        if let Some(mid) = d["manifest_id"].as_str() {
            let m = ix.get("pass_manifests", mid)?;
            require(
                d["pass_name"] == m["pass_name"]
                    && d["implementation_digest"] == m["implementation_digest"]
                    && d["configuration_digest"] == m["configuration_digest"],
                "PASS_MANIFEST",
                "Derivation differs from pinned pass identity",
            )?;
            require(
                strings(&d["input_assertion_ids"])
                    .iter()
                    .all(|id| strings(&m["input_assertion_ids"]).contains(id))
                    && d["input_artifact_ids"] == m["input_artifact_ids"]
                    && d["output_artifact_ids"] == m["output_artifact_ids"],
                "PASS_MANIFEST",
                "Derivation exceeds its pinned inputs",
            )?;
        }
    }
    for m in rows(b, "pass_manifests") {
        for id in strings(&m["input_assertion_ids"]) {
            require(
                contains(&m["context"], &ix.get("assertions", &id)?["context"]),
                "SCOPE",
                "Pass input outside pinned context",
            )?;
        }
        for id in strings(&m["input_artifact_ids"])
            .into_iter()
            .chain(strings(&m["output_artifact_ids"]))
        {
            ix.get("artifacts", &id)?;
        }
        for id in strings(&m["dependencies"]) {
            require(
                id != "latest",
                "UNPINNED",
                "Pass dependency must be immutable",
            )?;
            let dependency = ix.get("pass_manifests", &id)?;
            require(
                dependency["context"] == m["context"],
                "SCOPE",
                "Pass dependency exceeds context",
            )?;
            if m["status"] == "complete" {
                require(
                    dependency["status"] == "complete",
                    "COVERAGE",
                    "Partial prerequisite cannot satisfy completed pass",
                )?;
            }
        }
        for requirement in strings(&m["coverage_requirements"]) {
            let (predicate, guarantee) = requirement
                .split_once(':')
                .ok_or("COVERAGE: Invalid predicate coverage requirement")?;
            require(
                registry["predicates"][predicate]["status"] == "active"
                    && ["exact_supported", "sound_overapproximation"].contains(&guarantee),
                "COVERAGE",
                "Invalid pass predicate guarantee",
            )?;
            if m["status"] == "complete" {
                require(
                    rows(&m["coverage"], "guarantees")
                        .iter()
                        .any(|g| g["predicate"] == predicate && g["kind"] == guarantee),
                    "COVERAGE",
                    "Completed pass lacks its predicate-specific guarantee",
                )?;
            }
        }
        let coverage = &m["coverage"];
        require(
            coverage["completed"].as_u64() <= coverage["attempted"].as_u64(),
            "COVERAGE",
            "Invalid pass input coverage",
        )?;
        if m["status"] == "complete" {
            require(
                coverage["resolution_complete"] == true
                    && coverage["unsupported_count"] == 0
                    && coverage["unresolved_count"] == 0
                    && coverage["attempted"] == coverage["completed"],
                "COVERAGE",
                "Completed pass requires complete prerequisites",
            )?;
        }
        let mut identity_input = m.clone();
        identity_input.as_object_mut().unwrap().remove("id");
        require(
            m["id"] == identity::id("pass-manifest", &identity_input)?,
            "IDENTITY",
            "Pass manifest identity mismatch",
        )?;
    }
    pub(crate) fn pass_visit(
        id: &str,
        ix: &Index,
        stack: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<()> {
        require(
            !stack.contains(id),
            "PASS_CYCLE",
            "Cyclic pinned pass manifests",
        )?;
        if done.contains(id) {
            return Ok(());
        }
        stack.insert(id.into());
        for dependency in strings(&ix.get("pass_manifests", id)?["dependencies"]) {
            pass_visit(&dependency, ix, stack, done)?;
        }
        stack.remove(id);
        done.insert(id.into());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for m in rows(b, "pass_manifests") {
        pass_visit(&text(m, "id"), ix, &mut BTreeSet::new(), &mut done)?;
    }

    for p in rows(b, "input_packets") {
        for id in strings(&p["assertion_ids"]) {
            require(
                contains(&p["context"], &ix.get("assertions", &id)?["context"]),
                "SCOPE",
                "Packet input outside scope",
            )?;
        }
        require(
            p["digest"]
                == identity::digest(
                    &json!({"context":p["context"],"assertion_ids":p["assertion_ids"]}),
                )?,
            "DIGEST",
            "Packet digest mismatch",
        )?;
    }
    for packet in rows(b, "input_packets")
        .iter()
        .filter(|p| !p["artifact_id"].is_null())
    {
        let a = ix.field("artifacts", packet, "artifact_id")?;
        let raw = payloads
            .get(&text(a, "id"))
            .ok_or("INPUT_PACKET: Missing retained packet")?;
        let wire = identity::parse(raw)?;
        identity::canonical(&wire)?;
        require(
            wire["context"] == packet["context"]
                && wire["assertion_ids"] == packet["assertion_ids"]
                && wire["projection_ids"] == packet["projection_ids"],
            "INPUT_PACKET",
            "Retained packet differs from selected membership",
        )?;
        for id in strings(&packet["projection_ids"]) {
            let projection = ix.get("projections", &id)?;
            if packet["context"]["kind"] == "comparison" {
                // Lineage packets span two pinned source views. Each projection
                // must belong to one of the compared snapshots with its own
                // retained watermark; inputs must sit in a projection of their
                // own snapshot.
                require(
                    projection["context"]["kind"] == "source"
                        && strings(&packet["context"]["snapshot_ids"])
                            .contains(&text(&projection["context"], "snapshot_id"))
                        && projection["ledger_watermark"]
                            == wire["projection_watermarks"][id.as_str()],
                    "FRESHNESS",
                    "Packet projection mismatch",
                )?;
            } else {
                require(
                    projection["context"] == packet["context"]
                        && projection["ledger_watermark"] == wire["ledger_watermark"],
                    "FRESHNESS",
                    "Packet projection mismatch",
                )?;
                require(
                    strings(&packet["assertion_ids"])
                        .iter()
                        .all(|id| strings(&projection["input_assertion_ids"]).contains(id)),
                    "INPUT_PACKET",
                    "Packet inputs not in pinned projection",
                )?;
            }
        }
        if packet["context"]["kind"] == "comparison" {
            let mut pinned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for pid in strings(&packet["projection_ids"]) {
                let projection = ix.get("projections", &pid)?;
                pinned
                    .entry(text(&projection["context"], "snapshot_id"))
                    .or_default()
                    .extend(strings(&projection["input_assertion_ids"]));
            }
            for aid in strings(&packet["assertion_ids"]) {
                let assertion = ix.get("assertions", &aid)?;
                require(
                    pinned
                        .get(&text(&assertion["context"], "snapshot_id"))
                        .is_some_and(|inputs| inputs.contains(&aid)),
                    "INPUT_PACKET",
                    "Packet inputs not in pinned projection",
                )?;
            }
        }
        let packet_ids: BTreeSet<_> = strings(&packet["assertion_ids"]).into_iter().collect();
        require(
            rows(&wire, "assertions").len() == packet_ids.len(),
            "INPUT_PACKET",
            "Packet assertion count mismatch",
        )?;
        for assertion in rows(&wire, "assertions") {
            require(
                packet_ids.contains(&text(assertion, "id"))
                    && ix.get("assertions", &text(assertion, "id"))? == assertion
                    && assertion["class"] == "evidence",
                "INPUT_PACKET",
                "Packet evidence differs from canonical assertion",
            )?;
        }
        for source in rows(&wire, "sources") {
            require(
                packet_ids.contains(&text(source, "assertion_id")),
                "INPUT_PACKET",
                "Source outside packet support",
            )?;
            let a = ix.get("assertions", &text(source, "assertion_id"))?;
            require(
                strings(&a["evidence_ref_ids"]).contains(&text(source, "evidence_ref_id")),
                "INPUT_PACKET",
                "Packet source not supported by selected assertion",
            )?;
            let er = ix.field("evidence_refs", source, "evidence_ref_id")?;
            let artifact = ix.field("artifacts", er, "artifact_id")?;
            require(
                artifact["id"] == source["artifact_id"] && artifact["path"] == source["path"],
                "INPUT_PACKET",
                "Packet source artifact mismatch",
            )?;
            let raw = &payloads[&text(artifact, "id")];
            let start = source["source_byte_range"]["start"]
                .as_u64()
                .ok_or("INPUT_PACKET: Missing source start")? as usize;
            let end = source["source_byte_range"]["end"]
                .as_u64()
                .ok_or("INPUT_PACKET: Missing source end")? as usize;
            require(
                start >= er["locator"]["start"].as_u64().unwrap_or(0) as usize
                    && end <= er["locator"]["end"].as_u64().unwrap_or(0) as usize
                    && start < end
                    && end <= raw.len(),
                "INPUT_PACKET",
                "Packet source range exceeds canonical support",
            )?;
            require(
                std::str::from_utf8(&raw[start..end])?
                    == source["evidence_text"].as_str().unwrap_or(""),
                "INPUT_PACKET",
                "Packet source changed retained bytes",
            )?;
        }
        if packet["context"]["kind"] != "comparison" {
            ix.field("artifacts", packet, "taxonomy_artifact_id")?;
        }
    }
    Ok(())
}

fn validate_tail(
    b: &Value,
    ix: &Index,
    payloads: &Payloads,
    registry: &Value,
) -> Result<BTreeMap<String, bool>> {
    for inf in rows(b, "inferences") {
        let packet = ix.field("input_packets", inf, "packet_id")?;
        if !inf["output_artifact_id"].is_null() {
            for (artifact, digest) in [
                ("output_artifact_id", "output_digest"),
                ("prompt_artifact_id", "prompt_digest"),
                ("configuration_artifact_id", "configuration_digest"),
            ] {
                let a = ix.field("artifacts", inf, artifact)?;
                require(
                    a["digest"] == inf[digest],
                    "DIGEST",
                    "Inference artifact identity mismatch",
                )?;
            }
            if packet["context"]["kind"] != "comparison" {
                ix.field("artifacts", inf, "taxonomy_artifact_id")?;
            }
            let request = ix.field("artifacts", inf, "request_artifact_id")?;
            let request = identity::parse(&payloads[&text(request, "id")])?;
            let wire = ix.field("artifacts", packet, "artifact_id")?;
            let wire = identity::parse(&payloads[&text(wire, "id")])?;
            require(
                request["packet"] == wire,
                "INPUT_PACKET",
                "Exact provider request differs from retained packet",
            )?;
            let mut input = inf.clone();
            input.as_object_mut().unwrap().remove("id");
            require(
                inf["id"] == identity::id("inference", &input)?,
                "IDENTITY",
                "Inference identity mismatch",
            )?;
            let output = ix.field("artifacts", inf, "output_artifact_id")?;
            let output = identity::parse(&payloads[&text(output, "id")])?;
            for a in rows(b, "assertions")
                .iter()
                .filter(|a| a["class"] == "claim" && a["origin"]["inference_id"] == inf["id"])
            {
                // Proposal correspondence is predicate-specific. Domain claims
                // bind a taxonomy label; lineage claims bind two retained
                // entity versions. No other Claim predicate is admissible.
                let predicate = text(&a["proposition"], "predicate");
                require(
                    ["BELONGS_TO_DOMAIN", "LIKELY_SAME_ENTITY"].contains(&predicate.as_str()),
                    "INPUT_PACKET",
                    "Claim differs from validated retained proposal",
                )?;
                let matched = rows(&output, "proposals").iter().any(|p| {
                    p["predicate"] == predicate
                        && p["supports"] == a["supports"]
                        && p["score"] == a["model_score"]
                        && match predicate.as_str() {
                            "BELONGS_TO_DOMAIN" => {
                                p["target_id"] == a["proposition"]["subject_id"]
                                    && ix
                                        .field(
                                            "entity_versions",
                                            &a["proposition"]["object"],
                                            "entity_version_id",
                                        )
                                        .is_ok_and(|domain| p["domain"] == domain["canonical_key"])
                            }
                            _ => {
                                p["subject_id"] == a["proposition"]["subject_id"]
                                    && p["object_id"]
                                        == a["proposition"]["object"]["entity_version_id"]
                            }
                        }
                });
                require(
                    matched,
                    "INPUT_PACKET",
                    "Claim differs from validated retained proposal",
                )?;
                let mut input = a.clone();
                input.as_object_mut().unwrap().remove("id");
                input.as_object_mut().unwrap().remove("model_score");
                require(
                    a["id"] == identity::id("assertion", &input)?,
                    "IDENTITY",
                    "Claim identity mismatch",
                )?;
            }
        }
    }
    let memo = taint(b)?;
    for u in rows(b, "unresolved_references") {
        let s = ix.field("entity_versions", u, "source_entity_id")?;
        ix.field("evidence_refs", u, "evidence_ref_id")?;
        require(
            s["context"] == u["context"],
            "SCOPE",
            "Unresolved scope differs",
        )?;
        if let Some(predicate) = u["predicate"].as_str() {
            require(
                registry["predicates"][predicate]["status"] == "active"
                    && strings(&registry["predicates"][predicate]["contexts"])
                        .contains(&text(&u["context"], "kind")),
                "PREDICATE",
                "Unresolved predicate is not active in this context",
            )?;
        }
        for cid in strings(&u["candidate_ids"]) {
            require(
                ix.get("entity_versions", &cid)?["context"] == u["context"],
                "SCOPE",
                "Candidate scope differs",
            )?;
        }
        if u["status"] == "ambiguous" {
            require(
                strings(&u["candidate_ids"]).len() >= 2,
                "RESOLUTION",
                "Ambiguity needs multiple candidates",
            )?;
        }
    }
    semantic_bindings(b, ix)?;
    for p in rows(b, "projections") {
        for aid in strings(&p["source_artifact_ids"]) {
            ix.get("artifacts", &aid)?;
        }
        for id in strings(&p["input_assertion_ids"]) {
            let r = ix.get("assertions", &id)?;
            require(
                contains(&p["context"], &r["context"]),
                "SCOPE",
                "Projection input outside scope",
            )?;
            require(
                strings(&p["eligible_classes"]).contains(&text(r, "class")),
                "VIEW",
                "Forbidden projection class",
            )?;
            require(
                !memo[&id] || strings(&p["eligible_classes"]).contains(&"claim".into()),
                "TAINT",
                "Inferred result in evidence projection",
            )?;
        }
    }
    Ok(memo)
}
