//! Standalone diagnostic for the observed retained-JSON corruption.
//! cargo run --release --example retained-json-stress -- view.json 40
//! No ledger, SQLite, producers, or Briefcase retrieval code runs here.
use serde_json::{json, Value};

fn string_difference(left: &str, right: &str, path: &str) -> Option<Value> {
    if left.len() != right.len() {
        return Some(
            json!({"path":path,"kind":"string_length","left":left.len(),"right":right.len(),
            "left_address":format!("{:p}",left.as_ptr()),"right_address":format!("{:p}",right.as_ptr())}),
        );
    }
    left.bytes()
        .zip(right.bytes())
        .enumerate()
        .find(|(_, (a, b))| a != b)
        .map(|(offset, (a, b))| {
            let lp = left.as_ptr().wrapping_add(offset);
            let rp = right.as_ptr().wrapping_add(offset);
            json!({"path":path,"kind":"string_byte","offset":offset,"left":a,"right":b,"xor":a^b,
            "left_address":format!("{lp:p}"),"right_address":format!("{rp:p}"),
            "left_page_offset":lp as usize%4096,"right_page_offset":rp as usize%4096})
        })
}
fn difference(left: &Value, right: &Value, path: &str) -> Option<Value> {
    match (left, right) {
        (Value::String(a), Value::String(b)) => string_difference(a, b, path),
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(
                    json!({"path":path,"kind":"array_length","left":a.len(),"right":b.len()}),
                );
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (x, y))| difference(x, y, &format!("{path}/{i}")))
        }
        (Value::Object(a), Value::Object(b)) => {
            if a.len() != b.len() {
                return Some(
                    json!({"path":path,"kind":"object_length","left":a.len(),"right":b.len()}),
                );
            }
            a.iter().zip(b).find_map(|((ka, va), (kb, vb))| {
                string_difference(ka, kb, &format!("{path}/<key>")).or_else(|| {
                    difference(
                        va,
                        vb,
                        &format!("{path}/{}", ka.replace('~', "~0").replace('/', "~1")),
                    )
                })
            })
        }
        _ if left != right => Some(json!({"path":path,"kind":"scalar","left":left,"right":right})),
        _ => None,
    }
}
fn check(left: &Value, right: &Value, round: usize, comparison: &str) {
    if left != right {
        let first = difference(left, right, "");
        eprintln!(
            "{}",
            json!({"round":round,"comparison":comparison,"equal":false,"first_difference":first})
        );
        // Avoid dumping the entire source or destructing a corrupted graph.
        std::process::exit(1);
    }
}
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: retained-json-stress view.json [rounds]");
    let rounds: usize = args
        .next()
        .map(|n| n.parse().expect("rounds must be an integer"))
        .unwrap_or(40);
    let raw = std::fs::read(path).expect("read JSON input");
    for round in 0..rounds {
        let doc: Value = serde_json::from_slice(&raw).expect("parse JSON");
        let a = doc.clone();
        let b = a.clone();
        let c = b.clone();
        check(&a, &b, round, "a/b");
        check(&c, &doc, round, "c/original");
        drop(doc);
        drop(b);
        drop(c);
        drop(a);
        println!(
            "round {round}: {} bytes cloned, checked, and dropped",
            raw.len()
        );
    }
}
