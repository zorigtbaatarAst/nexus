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
    let root = std::env::temp_dir().join(format!("bh-mcp-{name}-{}", std::process::id()));
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
    p.join("bughunter")
}

struct Server {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn start(root: &Path) -> Option<Self> {
        let bin = binary();
        if !bin.exists() {
            // `cargo test -p bh-cli` builds the test harness but not necessarily the binary.
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
        instructions.contains("runs no tests"),
        "must say plainly that nothing is verified by reproduction: {instructions}"
    );
    assert!(
        instructions.contains("deterministic detectors only"),
        "and that the detectors do not reason about business logic: {instructions}"
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
        "bughunter_get_project_context",
        "bughunter_scan",
        "bughunter_rescan",
        "bughunter_get_changes",
        "bughunter_get_impact",
        "bughunter_get_symbol",
        "bughunter_get_graph",
        "bughunter_doctor",
        "bughunter_find_bugs",
        "bughunter_get_bugs",
        "bughunter_get_bug",
        "bughunter_ignore_bug",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }

    // Every tool must answer. The deadlock this test exists to catch shows up as a read
    // that never returns, so simply getting a reply is the assertion.
    let ctx = s.call(3, "bughunter_get_project_context", serde_json::json!({}));
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

    let scan = s.call(2, "bughunter_scan", serde_json::json!({}));
    assert!(scan["symbols_indexed"].as_u64().unwrap_or(0) > 0, "{scan}");

    let impact = s.call(
        3,
        "bughunter_get_impact",
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
    s.call(2, "bughunter_scan", serde_json::json!({}));

    let missing = s.call(
        3,
        "bughunter_get_impact",
        serde_json::json!({"target": "no.such.Symbol#nope"}),
    );
    // An agent can act on a result; a JSON-RPC error just makes it retry.
    assert_eq!(missing["status"], "not_found", "{missing}");

    let ambiguous = s.call(
        4,
        "bughunter_get_impact",
        serde_json::json!({"target": "save"}),
    );
    assert!(
        matches!(ambiguous["status"].as_str(), Some("ok") | Some("ambiguous")),
        "{ambiguous}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
