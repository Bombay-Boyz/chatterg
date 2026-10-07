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

// ---- exit codes, --restart, --force-resume, locking ------------------------

mod m1 {
    use super::{chatterg, json, predicate};
    use mockito::{Mock, Server, ServerGuard};
    use std::path::{Path, PathBuf};

    async fn card(server: &mut ServerGuard) -> Mock {
        let rpc_url = format!("{}/rpc", server.url());

        server
            .mock("GET", "/.well-known/agent-card.json")
            .with_status(200)
            .with_body(
                json!({"name": "Mock", "supportedInterfaces": [
                    {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
                ]})
                .to_string(),
            )
            .create_async()
            .await
    }

    fn reply(text: &str) -> String {
        json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
            {"kind": "text", "text": text}]}}})
        .to_string()
    }

    async fn answering(server: &mut ServerGuard, text: &str, hits: usize) -> Mock {
        server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply(text))
            .expect(hits)
            .create_async()
            .await
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn run(server: &ServerGuard, questions: &Path, store: &Path) -> assert_cmd::Command {
        let mut command = chatterg();
        command
            .arg(server.url())
            .arg(questions)
            .args(["--cooldown", "0", "--max-waits", "1", "--store"])
            .arg(store);
        command
    }

    #[test]
    fn help_lists_exit_codes_and_new_flags() {
        chatterg()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("EXIT CODES"))
            .stdout(predicate::str::contains("--force-resume"))
            .stdout(predicate::str::contains("--restart"));
    }

    #[test]
    fn restart_and_force_resume_cannot_be_combined() {
        chatterg()
            .args(["http://localhost:1", "questions.yaml", "--restart", "--force-resume"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("cannot be used with"));
    }

    #[test]
    fn errors_exit_with_code_1() {
        chatterg().args(["http://localhost:1", "/no/such/file.txt"]).assert().code(1);
    }

    #[tokio::test]
    async fn completed_run_exits_0() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = answering(&mut server, "Acme", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "Name?\n");

        run(&server, &questions, &dir.path().join("c.db")).assert().code(0);
    }

    #[tokio::test]
    async fn run_ended_by_abort_policy_exits_2() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = answering(&mut server, "not valid", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(
            dir.path(),
            "q.yaml",
            "questions:\n  - {id: stage, question: Stage?, required: true, type: enum, values: [pilot]}\n",
        );
        let store = dir.path().join("c.db");

        run(&server, &questions, &store)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("ended early"));

        // re-running an aborted run does not ask again and is still "ended early"
        run(&server, &questions, &store).assert().code(2);
    }

    #[tokio::test]
    async fn agent_that_stays_unavailable_exits_3() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _limited = server.mock("POST", "/rpc").with_status(429).create_async().await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "Name?\n");

        run(&server, &questions, &dir.path().join("c.db"))
            .assert()
            .code(3)
            .stderr(predicate::str::contains("still unavailable"));
    }

    #[tokio::test]
    async fn restart_archives_the_old_run_and_asks_again() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let rpc = answering(&mut server, "Acme", 2).await; // once per run

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "Name?\n");
        let store = dir.path().join("c.db");

        run(&server, &questions, &store).assert().success();
        run(&server, &questions, &store)
            .arg("--restart")
            .assert()
            .success()
            .stderr(predicate::str::contains("archived the previous run"));

        rpc.assert_async().await;

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one archive expected");
    }

    #[tokio::test]
    async fn rerunning_a_finished_run_does_not_ask_again() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let rpc = answering(&mut server, "Acme", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "Name?\n");
        let store = dir.path().join("c.db");

        run(&server, &questions, &store).assert().success();
        run(&server, &questions, &store)
            .assert()
            .success()
            .stdout(predicate::str::contains("Acme"));

        rpc.assert_async().await; // exactly one request in total
    }

    #[tokio::test]
    async fn edited_questions_file_is_refused_then_forced() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("c.db");

        // Run 1: answers the first question, then the agent stays unavailable.
        let mut first = Server::new_async().await;
        let _card1 = card(&mut first).await;
        let _ok = answering(&mut first, "A1", 1).await;
        let _limited = first.mock("POST", "/rpc").with_status(429).create_async().await;

        let original = write(dir.path(), "q.txt", "One?\nTwo?\n");
        run(&first, &original, &store).assert().code(3);

        // The file now has a third question appended.
        let edited = write(dir.path(), "q.txt", "One?\nTwo?\nThree?\n");

        let mut second = Server::new_async().await;
        let _card2 = card(&mut second).await;
        let _rest = answering(&mut second, "more", 2).await;

        run(&second, &edited, &store)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("changed since this run started"))
            .stderr(predicate::str::contains("1 added (q003)"))
            .stderr(predicate::str::contains("--force-resume"));

        run(&second, &edited, &store)
            .arg("--force-resume")
            .assert()
            .success()
            .stdout(predicate::str::contains("q003"));
    }

    #[test]
    fn a_store_in_use_by_another_run_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("c.db");
        let questions = write(dir.path(), "q.txt", "Name?\n");

        let _held = chatterg::storage::sqlite::SqliteStore::open(&store).unwrap();

        chatterg()
            .args(["http://localhost:1"])
            .arg(&questions)
            .arg("--store")
            .arg(&store)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("another chatterg is already using"));
    }
}
