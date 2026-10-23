//! Data producer and lineage coverage through brief/neighbors, without a data product API.
#[path="context_support/mod.rs"]
mod context_support;
use context_support::Fixture;
use serde_json::json;
use briefcase::architecture::{capture::Capture,contracts::{self,rows,text},data,query::Options,typescript};
/// The vendored compiler pin: `npm ci --ignore-scripts --prefix tools/typescript`.
fn compiler()->std::path::PathBuf{std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tools/typescript/node_modules/typescript/lib/typescript.js")}
fn fixture()->Fixture{
    let mut f=Fixture::new();
    for (path,body) in [
        ("tsconfig.json",r#"{"compilerOptions":{"strict":true,"target":"ES2022","module":"ESNext"},"include":["src/**/*.ts"]}"#),
        ("src/users.ts","import { PrismaClient } from '@prisma/client';\nconst db=new PrismaClient();\nexport async function users(){return db.user.findMany({select:{id:true}});}\nexport async function dynamic(query:string){return db.$queryRawUnsafe(query);}\n"),
        ("prisma/schema.prisma","model User {\n  id String @id\n  @@map(\"users\")\n}\n"),
        ("prisma/database.json",r#"{"schema_version":"1.0","logical_database":"demo","default_schema":"public"}"#),
        ("prisma/migrations/001_init/migration.sql","CREATE TABLE \"users\" (\"id\" TEXT NOT NULL PRIMARY KEY);\n"),
    ]{let path=f.root.join(path);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,body).unwrap();}
    let repo=f.ledger.repository().unwrap();let generation=f.ledger.begin(&repo,None).unwrap();
    let options=typescript::ScanOptions{calls:true,compiler:Some(compiler()),..Default::default()};
    let produced=typescript::produce(Capture::read(&f.root,None).unwrap(),&repo,generation,&options).unwrap();
    f.view=f.ledger.commit(generation,&produced).unwrap();f.ledger.publish(&f.view,true).unwrap();
    data::analyze(&mut f.ledger,&Options{view:f.view.clone(),..Default::default()},&options).unwrap();
    f
}
#[test]
fn data_facts_remain_valid_evidence_and_are_generic_relationships(){
    let f=fixture();let view=f.ledger.load(&f.view).unwrap();contracts::validate(&view.bundle,&f.ledger.payloads(&view.bundle).unwrap()).unwrap();
    let reads=f.call("neighbors",json!({"target":"src/users.ts","predicate":"READS","max_bytes":1048576}));
    let edge=rows(&reads,"items").first().expect("Prisma read remains visible through neighbors");
    assert!(["evidence","derived"].contains(&edge["class"].as_str().unwrap()));
    let proof=f.call("explain",json!({"report_id":reads["report_id"],"ref":edge["id"],"max_bytes":1048576}));assert!(!rows(&proof,"assertions").is_empty());
    let packet=f.call("brief",json!({"task":"refactor src/users.ts","depth":3,"max_bytes":1048576}));
    let selected:Vec<_>=packet["slice"].as_object().unwrap().values().flat_map(|items|items.as_array().unwrap()).map(|a|text(a,"path")).collect();
    assert!(selected.contains(&"prisma/schema.prisma".into()));
}
#[test]
fn unsupported_data_forms_remain_explicit_boundaries(){
    let f=fixture();let packet=f.call("brief",json!({"task":"debug src/users.ts","max_bytes":1048576,"limit":1000}));
    assert!(rows(&packet,"not_covered").iter().any(|gap|gap["area"]=="producer_boundary"));
    assert_eq!(packet["outcome"],"partial");
}
