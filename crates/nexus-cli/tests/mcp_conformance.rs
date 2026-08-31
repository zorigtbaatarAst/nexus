//! Drives the real `bughunter mcp` process over stdio.
//!
//! Written as a subprocess test on purpose. Both bugs found while building this layer were
//! invisible to anything that called the handler directly: the CLI held a lock on stdout
//! for the whole of `run()`, so the server deadlocked the moment it tried to answer, and
//! the server advertised no capabilities, so a client was entitled never to ask for the
//! tool list. Only a real client talking to a real process finds those.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn write_file(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, body).expect("write");
}

fn fixture(name: &str) -> PathBuf {
    // One directory per test: the harness runs them in parallel, and a shared project
    // makes them race on the database rather than on the thing under test.
    let root = std::env::temp_dir().join(format!("nexus-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    write_file(
        &root,
        "build.gradle",
        "implementation 'org.springframework.boot:spring-boot-starter:3.5.0'\n",
    );
    write_file(
        &root,
        "src/mn/pay/PaymentService.java",
        r#"
package mn.pay;
public class PaymentService {
    private final PaymentRepository repo;
    public Payment pay(String key) { return repo.save(key); }
}
"#,
    );
    write_file(
        &root,
        "src/mn/pay/PaymentRepository.java",
        r#"
package mn.pay;
public class PaymentRepository {
    public Payment save(String key) { return null; }
}
"#,
    );
    root
}

fn binary() -> PathBuf {
    // The test binary lives beside the CLI binary in the same profile directory.
    let mut p = std::env::current_exe().expect("current exe");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("nexus")
}

struct Server {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn start(root: &Path) -> Option<Self> {
        let bin = binary();
        if !bin.exists() {
            // `cargo test -p nexus-cli` builds the test harness but not necessarily the binary.
            eprintln!(
                "skipping: {} not built (run `cargo build` first)",
                bin.display()
            );
            return None;
        }
        let mut child = Command::new(bin)
            .arg("--project")
            .arg(root)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bughunter mcp");
        let reader = BufReader::new(child.stdout.take().expect("stdout"));
        Some(Server { child, reader })
    }

    fn send(&mut self, line: &str) {
        let stdin = self.child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{line}").expect("write");
        stdin.flush().expect("flush");
    }

    fn recv(&mut self) -> serde_json::Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).expect("read");
        assert!(n > 0, "server closed the connection without answering");
        serde_json::from_str(&line).expect("valid JSON-RPC")
    }

    fn call(&mut self, id: u32, name: &str, args: serde_json::Value) -> serde_json::Value {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
        ));
        let reply = self.recv();
        reply["result"]["structuredContent"].clone()
    }

    fn handshake(&mut self) -> serde_json::Value {
        self.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"conformance","version":"1"}}}"#);
        let init = self.recv();
        self.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        init
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_server_handshakes_advertises_tools_and_answers() {
    let root = fixture("handshake");
    let Some(mut s) = Server::start(&root) else {
        return;
    };

    let init = s.handshake();
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "a server that advertises no tools capability may never be asked for its tools: {}",
        init["result"]["capabilities"]
    );
    // The instructions have to keep pace with what the build actually does. This exists to
    // fail when a capability lands and the honesty about its limits does not.
    let instructions = init["result"]["instructions"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        instructions.contains("nothing is verified by reproduction"),
        "must say plainly that nothing is proven by running it: {instructions}"
    );
    assert!(
        instructions.contains("deterministic rules only"),
        "and that the rules do not reason about business logic: {instructions}"
    );

    s.send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let list = s.recv();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in [
        "nexus_get_project_context",
        "nexus_scan",
        "nexus_rescan",
        "nexus_get_changes",
        "nexus_get_impact",
        "nexus_get_symbol",
        "nexus_get_graph",
        "nexus_doctor",
        "bughunter_analyze",
        "nexus_get_findings",
        "nexus_get_finding",
        "nexus_ignore_finding",
        "nexus_record_finding",
        "nexus_record_fact",
        "nexus_get_known",
        "nexus_capabilities",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }

    // Every tool must answer. The deadlock this test exists to catch shows up as a read
    // that never returns, so simply getting a reply is the assertion.
    let ctx = s.call(3, "nexus_get_project_context", serde_json::json!({}));
    assert!(ctx["project"].is_string(), "{ctx}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn scan_then_impact_returns_a_trace_with_its_path() {
    let root = fixture("impact");
    let Some(mut s) = Server::start(&root) else {
        return;
    };
    s.handshake();

    let scan = s.call(2, "nexus_scan", serde_json::json!({}));
    assert!(scan["symbols_indexed"].as_u64().unwrap_or(0) > 0, "{scan}");

    let impact = s.call(
        3,
        "nexus_get_impact",
        serde_json::json!({"target": "mn.pay.PaymentRepository#save"}),
    );
    assert_eq!(impact["status"], "ok", "{impact}");
    let items = impact["items"].as_array().expect("items");
    assert!(
        items.iter().any(|i| i["fqn"]
            .as_str()
            .is_some_and(|f| f.contains("PaymentService"))),
        "the caller should be reached: {impact}"
    );
    // A result without its path is an assertion the caller cannot check.
    assert!(
        items[0]["path"].is_array(),
        "every result carries the chain that produced it"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_domain_failure_is_a_result_the_agent_can_act_on_not_a_protocol_error() {
    let root = fixture("failure");
    let Some(mut s) = Server::start(&root) else {
        return;
    };
    s.handshake();
    s.call(2, "nexus_scan", serde_json::json!({}));

    let missing = s.call(
        3,
        "nexus_get_impact",
        serde_json::json!({"target": "no.such.Symbol#nope"}),
    );
    // An agent can act on a result; a JSON-RPC error just makes it retry.
    assert_eq!(missing["status"], "not_found", "{missing}");

    let ambiguous = s.call(4, "nexus_get_impact", serde_json::json!({"target": "save"}));
    assert!(
        matches!(ambiguous["status"].as_str(), Some("ok") | Some("ambiguous")),
        "{ambiguous}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_agent_can_record_a_finding_and_it_is_recognized_next_time() {
    // This is what LLM independence means in practice. Until an agent can write a finding
    // back, only code compiled into Nexus can produce one — and that, not the absence of an
    // HTTP client, is what makes a system model-dependent.
    let root = fixture("writeback");
    let Some(mut s) = Server::start(&root) else {
        return;
    };
    s.handshake();
    s.call(2, "nexus_scan", serde_json::json!({}));

    let finding = serde_json::json!({
        "finding_type": "concurrency",
        "title": "duplicate payment under concurrency",
        "component": "PaymentService",
        "anchor_fqn": "mn.pay.PaymentService#pay(String)",
        "severity": "critical",
        "confidence": 0.95,
        "detector": "agent:reasoned",
        "structural_key": "payment.status,repo",
        "slug": "payment-duplicate-concurrent",
        "evidence": [{
            "file": "src/mn/pay/PaymentService.java",
            "line": 5,
            "note": "the exists() check and the insert are not in one transaction"
        }]
    });

    let recorded = s.call(
        3,
        "nexus_record_finding",
        serde_json::json!({"finding": finding}),
    );
    assert!(recorded["uid"].is_string(), "{recorded}");
    assert_eq!(recorded["is_new"], true);
    // A model may not grade its own work: 0.95 is capped.
    let listed = s.call(4, "nexus_get_findings", serde_json::json!({}));
    let f = &listed["findings"][0];
    assert!(
        f["confidence"].as_f64().unwrap_or(1.0) <= 0.75,
        "model confidence must be capped: {f}"
    );

    // Recorded again, it is recognized by fingerprint rather than duplicated — the whole
    // point of recording through the platform instead of into a chat log.
    let again = s.call(
        5,
        "nexus_record_finding",
        serde_json::json!({"finding": finding}),
    );
    assert_eq!(again["is_new"], false, "{again}");
    assert_eq!(again["uid"], recorded["uid"]);

    let all = s.call(6, "nexus_get_findings", serde_json::json!({}));
    assert_eq!(
        all["findings"].as_array().map(Vec::len),
        Some(1),
        "one row, not two"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_finding_without_checkable_evidence_is_refused() {
    let root = fixture("noevidence");
    let Some(mut s) = Server::start(&root) else {
        return;
    };
    s.handshake();
    s.call(2, "nexus_scan", serde_json::json!({}));

    let base = serde_json::json!({
        "finding_type": "logic", "title": "t", "component": "C",
        "anchor_fqn": null, "severity": "high", "confidence": 0.9,
        "detector": "agent:reasoned", "structural_key": "k", "slug": "s",
        "evidence": []
    });
    let refused = s.call(
        3,
        "nexus_record_finding",
        serde_json::json!({"finding": base}),
    );
    assert_eq!(refused["status"], "error", "{refused}");

    // Evidence pointing at a file that is not in the index is not evidence either: a model
    // describing a plausible problem in a file that does not exist must produce no rows.
    let mut fabricated = base.clone();
    fabricated["evidence"] =
        serde_json::json!([{"file": "does/not/exist.java", "line": 1, "note": "n"}]);
    let rejected = s.call(
        4,
        "nexus_record_finding",
        serde_json::json!({"finding": fabricated}),
    );
    assert_eq!(rejected["status"], "error", "{rejected}");

    let all = s.call(5, "nexus_get_findings", serde_json::json!({}));
    assert_eq!(
        all["findings"].as_array().map(Vec::len),
        Some(0),
        "nothing was stored"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_fact_survives_for_the_next_session() {
    let root = fixture("facts");
    let Some(mut s) = Server::start(&root) else {
        return;
    };
    s.handshake();
    s.call(2, "nexus_scan", serde_json::json!({}));
    s.call(
        3,
        "nexus_record_fact",
        serde_json::json!({
            "key": "arch.payment.idempotency",
            "claim": "Idempotency is enforced at the controller, not in PaymentService.",
            "subject": "mn.pay"
        }),
    );
    drop(s);

    // A new process: the point of persistence is that the next session starts with it.
    let Some(mut s2) = Server::start(&root) else {
        return;
    };
    s2.handshake();
    let known = s2.call(
        2,
        "nexus_get_known",
        serde_json::json!({"target": "mn.pay"}),
    );
    let claims: Vec<&str> = known["facts"]
        .as_array()
        .map(|a| a.iter().filter_map(|f| f["claim"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        claims
            .iter()
            .any(|c| c.contains("Idempotency is enforced at the controller")),
        "the fact must outlive the session that learned it: {known}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
