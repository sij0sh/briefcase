#![allow(dead_code)]
use serde_json::{json,Value};
use std::path::PathBuf;
use briefcase::architecture::{capture::Capture,contracts::{rows,text},deployment,identity,ledger::Ledger,runtime_observations,service};
pub struct Fixture {
    pub dir:tempfile::TempDir,
    pub root:PathBuf,
    pub db:PathBuf,
    pub ledger:Ledger,
    pub view:String,
}
impl Fixture {
    pub fn new()->Self {
        let dir=tempfile::tempdir().unwrap();let root=dir.path().join("source");let db=dir.path().join("context.db");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"),"pub fn start(){let _=std::process::Command::new(\"/opt/demo-tool\");}\n").unwrap();
        std::fs::write(root.join("Dockerfile"),"FROM scratch\nCOPY src /app\n").unwrap();
        std::fs::write(root.join("README.md"),"src/lib.rs starts the demo-tool.\n").unwrap();
        let ledger=service::open_context(&db,&root).unwrap();let view=ledger.resolve("latest").unwrap();
        Self{dir,root,db,ledger,view}
    }
    pub fn call(&self,name:&str,args:Value)->Value {service::call(&self.ledger,name,&args).unwrap()}
    pub fn recapture(&mut self){
        let repo=self.ledger.repository().unwrap();let generation=self.ledger.begin(&repo,None).unwrap();
        let produced=Capture::read(&self.root,None).unwrap().text_publication(&repo,generation).unwrap();
        self.view=self.ledger.commit(generation,&produced).unwrap();self.ledger.publish(&self.view,true).unwrap();
    }
    pub fn deployment(&mut self,label:&str)->(String,String){
        let desired=self.dir.path().join(format!("desired-{label}"));std::fs::create_dir_all(&desired).unwrap();
        let image=serde_json::to_vec(&json!({"schemaVersion":2,"config":{"digest":format!("sha256:{}","a".repeat(64))},"layers":[]})).unwrap();
        let digest=identity::bytes_digest(&image);std::fs::write(desired.join("image.json"),&image).unwrap();
        std::fs::copy(self.root.join("Dockerfile"),desired.join("Dockerfile")).unwrap();
        let view=self.ledger.load(&self.view).unwrap();let snapshot=&rows(&view.bundle,"snapshots")[0];
        let files=rows(snapshot,"files");let dockerfile=files.iter().find(|f|f["path"]=="Dockerfile").unwrap();
        let manifest=json!({"schema_version":"1.0","kind":"briefcase-build-manifest","repository":"demo","image_digest":digest,
            "source_snapshot_id":snapshot["id"],"context_files":files,"context_digest":identity::digest(&json!(files)).unwrap(),
            "dockerfile":dockerfile,"oci_manifest":"image.json"});
        std::fs::write(desired.join("build.json"),serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        std::fs::write(desired.join("deployment.yaml"),format!("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: demo\n  namespace: shop\nspec:\n  selector:\n    matchLabels:\n      app: demo\n  template:\n    metadata:\n      labels:\n        app: demo\n    spec:\n      containers:\n        - name: demo\n          image: demo@{digest}\n          ports:\n            - containerPort: 8080\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: demo\n  namespace: shop\nspec:\n  selector:\n    app: demo\n  ports:\n    - port: 80\n      targetPort: 8080\n")).unwrap();
        let result=deployment::capture(&mut self.ledger,&desired,&deployment::CaptureOptions{environment:"staging".into(),source_view:Some(self.view.clone()),label:Some(label.into())}).unwrap();
        (text(&result,"view_id"),digest)
    }
    pub fn runtime(&mut self,digest:&str,name_only:bool,label:&str)->String{
        let mut attrs=vec![json!({"key":"service.name","value":{"stringValue":"demo"}}),
            json!({"key":"deployment.environment.name","value":{"stringValue":"staging"}})];
        if !name_only {attrs.push(json!({"key":"container.image.id","value":{"stringValue":format!("demo@{digest}")}}));}
        let mut caller_attrs=attrs.clone();caller_attrs[0]=json!({"key":"service.name","value":{"stringValue":"caller"}});
        let raw=serde_json::to_vec(&json!({"resourceSpans":[
            {"resource":{"attributes":caller_attrs},"scopeSpans":[{"scope":{"name":"fixture"},"spans":[{
                "traceId":"0123456789abcdef0123456789abcdef","spanId":"fedcba9876543210","kind":3,"name":"call",
                "startTimeUnixNano":"1000000000","endTimeUnixNano":"2000000000","status":{"code":1}}]}]},
            {"resource":{"attributes":attrs},"scopeSpans":[{"scope":{"name":"fixture"},"spans":[{
                "traceId":"0123456789abcdef0123456789abcdef","spanId":"0123456789abcdef","parentSpanId":"fedcba9876543210","kind":2,"name":"request",
                "startTimeUnixNano":"1000000000","endTimeUnixNano":"2000000000","status":{"code":1}}]}]}]})).unwrap();
        let result=runtime_observations::ingest(&mut self.ledger,&raw,&runtime_observations::IngestOptions{environment:"staging".into(),
            interval_start:"1970-01-01T00:00:00Z".into(),interval_end:"1970-01-01T00:01:00Z".into(),collector:"test-collector".into(),
            sampling:"sampled".into(),label:Some(label.into()),source_name:"inline.jsonl".into()}).unwrap();
        text(&result,"view_id")
    }
}
