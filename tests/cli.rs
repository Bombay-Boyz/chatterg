use assert_cmd::Command;
use mockito::Server;
use predicates::prelude::*;
use serde_json::json;

fn chatterg() -> Command {
    Command::cargo_bin("chatterg").unwrap()
}

#[test]
fn help_documents_all_arguments() {
    chatterg()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("AGENT_URL"))
        .stdout(predicate::str::contains("QUESTIONS_FILE"))
        .stdout(predicate::str::contains("--store <SQLITE_PATH>"))
        .stdout(predicate::str::contains("agent card"));
}

#[test]
fn missing_arguments_fail() {
    chatterg().assert().failure().stderr(predicate::str::contains("Usage"));
    chatterg().arg("http://localhost").assert().failure();
}

#[test]
fn invalid_questionnaire_path_fails_cleanly() {
    chatterg()
        .args(["http://localhost:1", "/no/such/questions.yaml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read questionnaire"))
        .stderr(predicate::str::contains("/no/such/questions.yaml"));
}

#[test]
fn invalid_store_path_fails_cleanly() {
    chatterg()
        .args(["http://localhost:1", "questions.yaml", "--store", "/no/such/dir/x.db"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot open database"));
}

#[test]
fn empty_questionnaire_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.yaml");
    std::fs::write(&path, "questions: []\n").unwrap();

    chatterg()
        .args(["http://localhost:1"])
        .arg(&path)
        .arg("--store")
        .arg(dir.path().join("x.db"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("no questions"));
}

#[tokio::test]
async fn end_to_end_against_mock_agent_then_resume_is_a_noop() {
    let mut server = Server::new_async().await;
    let rpc_url = format!("{}/rpc", server.url());

    let _card = server
        .mock("GET", "/agent/.well-known/agent-card.json")
        .with_status(200)
        .with_body(
            json!({"name": "Mock", "supportedInterfaces": [
                {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
            ]})
            .to_string(),
        )
        .create_async()
        .await;

    let rpc = server
        .mock("POST", "/rpc")
        .with_status(200)
        .with_body(
            json!({"jsonrpc": "2.0", "id": 1, "result": {"kind": "task", "id": "t",
                "status": {"state": "completed", "message": {"parts": [
                    {"kind": "text", "text": "Acme"}]}}}})
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let dir = tempfile::tempdir().unwrap();
    let questions = dir.path().join("q.yaml");
    std::fs::write(
        &questions,
        "questions:\n  - {id: company, question: Name?, required: true, type: string}\n",
    )
    .unwrap();
    let store = dir.path().join("c.db");
    let target = format!("{}/agent", server.url());

    for _ in 0..2 {
        chatterg()
            .arg(&target)
            .arg(&questions)
            .arg("--store")
            .arg(&store)
            .assert()
            .success()
            .stdout(predicate::str::contains("company"))
            .stdout(predicate::str::contains("Acme"));
    }

    // Second invocation resumed a completed conversation: the agent was asked once only.
    rpc.assert_async().await;
}

#[tokio::test]
async fn plain_text_questions_run_end_to_end_with_flags() {
    let mut server = Server::new_async().await;
    let rpc_url = format!("{}/rpc", server.url());

    let _card = server
        .mock("GET", "/.well-known/agent-card.json")
        .with_status(200)
        .with_body(
            json!({"name": "Mock", "supportedInterfaces": [
                {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
            ]})
            .to_string(),
        )
        .create_async()
        .await;

    // Always evasive: should be asked RETRIES + 1 = 2 times per question, then moved past.
    let rpc = server
        .mock("POST", "/rpc")
        .with_status(200)
        .with_body(
            json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
                {"kind": "text", "text": "Let's set up a meeting"}]}}})
            .to_string(),
        )
        .expect(4)
        .create_async()
        .await;

    let dir = tempfile::tempdir().unwrap();
    let questions = dir.path().join("questions.txt");
    std::fs::write(&questions, "What is a zeolite?\nWhat is Nxtbrane?\n").unwrap();

    chatterg()
        .arg(server.url())
        .arg(&questions)
        .args(["--retries", "1", "--store"])
        .arg(dir.path().join("c.db"))
        .assert()
        .success()
        .stdout(predicate::str::contains("q001"))
        .stdout(predicate::str::contains("q002"));

    rpc.assert_async().await;
}

#[tokio::test]
async fn rate_limited_agent_is_paused_then_resumed() {
    let mut server = Server::new_async().await;
    let rpc_url = format!("{}/rpc", server.url());

    let _card = server
        .mock("GET", "/.well-known/agent-card.json")
        .with_status(200)
        .with_body(
            json!({"name": "Mock", "supportedInterfaces": [
                {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
            ]})
            .to_string(),
        )
        .create_async()
        .await;

    let limited = server.mock("POST", "/rpc").with_status(429).expect(1).create_async().await;
    let ok = server
        .mock("POST", "/rpc")
        .with_status(200)
        .with_body(
            json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
                {"kind": "text", "text": "A crystalline aluminosilicate."}]}}})
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let dir = tempfile::tempdir().unwrap();
    let questions = dir.path().join("q.txt");
    std::fs::write(&questions, "What is a zeolite?\n").unwrap();

    chatterg()
        .arg(server.url())
        .arg(&questions)
        .args(["--cooldown", "0", "--store"])
        .arg(dir.path().join("c.db"))
        .assert()
        .success()
        .stdout(predicate::str::contains("A crystalline aluminosilicate."))
        .stderr(predicate::str::contains("[1/1] q001"))
        .stderr(predicate::str::contains("agent unavailable"))
        .stderr(predicate::str::contains("cooldown 1/30"));

    limited.assert_async().await;
    ok.assert_async().await;
}

#[test]
fn help_documents_cooldown_options() {
    chatterg()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--cooldown <SECONDS>"))
        .stdout(predicate::str::contains("--max-waits <N>"))
        .stdout(predicate::str::contains("--delay <SECONDS>"))
        .stdout(predicate::str::contains("--timeout <SECONDS>"));
}
