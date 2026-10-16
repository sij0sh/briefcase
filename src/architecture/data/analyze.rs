use super::*;
/// Add data Evidence and the lineage pass to a captured TS/JS view as an
/// append-only overlay. Rerunning on unchanged inputs reuses the overlay.
pub fn analyze(l: &mut Ledger, o: &Options, options: &typescript::ScanOptions) -> Result<Value> {
    require(
        !o.historical,
        "DATA",
        "Data analysis needs current eligible source inputs",
    )?;
    let mut strict = o.clone();
    strict.allow_partial = false;
    let v = query::loaded(l, &strict)?;
    require(
        v.scope == "source",
        "SCOPE",
        "Data analysis runs on a source view",
    )?;
    require(
        rows(&v.bundle, "entity_versions").iter().any(|e| {
            e["kind"] == "module"
                && (e["language"] == "typescript" || e["language"] == "javascript")
        }),
        "DATA",
        "Data lineage requires a TS/JS structure view",
    )?;
    let watermark = l.watermark()?;
    let ctx = knowledge::context(&v.bundle);
    let snapshot = knowledge::row(&v.bundle, "snapshots", &text(&ctx, "snapshot_id"))?.clone();
    let mut payloads = l.payloads(&v.bundle)?;
    let mut files = BTreeMap::new();
    for f in rows(&snapshot, "files") {
        let artifact = rows(&v.bundle, "artifacts")
            .iter()
            .find(|a| {
                a["path"] == f["path"]
                    && a["digest"] == f["digest"]
                    && a["source_kind"] != "transport"
            })
            .ok_or("DATA: Missing retained input")?;
        let bytes = payloads[&text(artifact, "id")].clone();
        require(
            identity::bytes_digest(&bytes) == f["digest"],
            "DIGEST",
            "Corrupt retained input",
        )?;
        files.insert(text(f, "path"), bytes);
    }
    let capture = Capture {
        root: ".".into(),
        captured_at_unix_ms: None,
            inventory: serde_json::Value::Null,
        files,
        diagnostics: vec![],
        exclusions: vec![],
        complete: snapshot["capture_state"] == "complete",
    };
    let key = identity::id(
        "data-overlay",
        &json!({"view_id":v.id,"context":ctx,"implementation":identity::bytes_digest(&[include_bytes!("../data.rs").as_slice(), include_bytes!("schema_parse.rs").as_slice(), include_bytes!("produce.rs").as_slice(), include_bytes!("finish.rs").as_slice(), include_bytes!("analyze.rs").as_slice()].concat()),
            "adapter":identity::bytes_digest(include_bytes!("../data.cjs")),"pass_engine":identity::bytes_digest(include_bytes!("../passes.rs")),
            "project":options.project}),
    )?;
    if l.has_overlay(&key)? {
        let execution = l.record_execution(
            &v.id,
            &json!({"operation":"analyze-data","overlay_id":key,"reused":true}),
        )?;
        return Ok(
            json!({"view_id":v.id,"context":ctx,"execution_id":execution,"overlay_id":key,"reused":true}),
        );
    }
    let mut b = v.bundle.clone();
    let original: BTreeSet<_> = rows(&b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .collect();
    let produced = produce(&mut b, &mut payloads, &capture, &ctx, options)?;
    let selected: BTreeSet<_> = rows(&b, "assertions")
        .iter()
        .map(|a| text(a, "id"))
        .filter(|id| !original.contains(id))
        .collect();
    let diagnostics = json!({"data_coverage":produced.coverage,"limitations":produced.limitations});
    let prepared = Prepared::extension(
        b,
        payloads,
        v.generation,
        &selected,
        &[
            contracts::Capability::Evidence,
            contracts::Capability::Derived,
        ],
        diagnostics.clone(),
    )?;
    l.commit_extension(
        &key,
        "source",
        std::slice::from_ref(&v.id),
        &prepared,
        &selected,
        watermark,
    )?;
    l.rebuild(&v.id)?;
    let execution = l.record_execution(
        &v.id,
        &json!({"operation":"analyze-data","overlay_id":key,"reused":false}),
    )?;
    let (reused, recomputed, invalidated) = l.pass_changes(&v.id)?;
    Ok(
        json!({"view_id":v.id,"context":ctx,"execution_id":execution,"overlay_id":key,"reused":false,
        "coverage":produced.coverage,"limitations":produced.limitations,
        "pass_artifacts":{"reused":reused,"recomputed":recomputed,"invalidated":invalidated}}),
    )
}
