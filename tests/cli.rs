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

// ---- chatterg report ----------------------------------------------------------------

mod report_command {
    use super::{chatterg, json, predicate};
    use mockito::{Server, ServerGuard};
    use std::path::{Path, PathBuf};

    /// Runs a small questionnaire against a mock agent and leaves a notebook behind.
    async fn finished_run(dir: &Path, questions: &str) -> (PathBuf, PathBuf) {
        let mut server: ServerGuard = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());

        let _card = server
            .mock("GET", "/.well-known/agent-card.json")
            .with_status(200)
            .with_body(
                json!({"name": "Mock Agent", "supportedInterfaces": [
                    {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
                ]})
                .to_string(),
            )
            .create_async()
            .await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(
                json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
                    {"kind": "text", "text": "<b>Crystalline</b> & porous"}]}}})
                .to_string(),
            )
            .create_async()
            .await;

        let questions_path = dir.join("q.txt");
        std::fs::write(&questions_path, questions).unwrap();
        let store = dir.join("c.db");

        chatterg()
            .arg(server.url())
            .arg(&questions_path)
            .args(["--cooldown", "0", "--store"])
            .arg(&store)
            .assert()
            .success();

        (questions_path, store)
    }

    #[test]
    fn help_lists_the_report_command() {
        chatterg()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("report"))
            .stdout(predicate::str::contains("<AGENT_URL>"));

        chatterg()
            .args(["report", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--format"))
            .stdout(predicate::str::contains("--overwrite"))
            .stdout(predicate::str::contains("--questions"));
    }

    #[test]
    fn the_normal_run_form_still_needs_both_arguments() {
        chatterg()
            .arg("http://localhost:1")
            .assert()
            .failure()
            .stderr(predicate::str::contains("QUESTIONS_FILE"));
    }

    #[tokio::test]
    async fn markdown_goes_to_the_screen_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let (_, store) = finished_run(dir.path(), "What is a zeolite?\n").await;

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .assert()
            .success()
            .stdout(predicate::str::contains("# Questionnaire report"))
            .stdout(predicate::str::contains("Mock Agent"))
            .stdout(predicate::str::contains("> <b>Crystalline</b> & porous"));
    }

    #[tokio::test]
    async fn the_format_follows_the_output_file_extension() {
        let dir = tempfile::tempdir().unwrap();
        let (_, store) = finished_run(dir.path(), "What is a zeolite?\n").await;
        let out = dir.path().join("report.html");

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .arg("--out")
            .arg(&out)
            .assert()
            .success()
            .stderr(predicate::str::contains("wrote"));

        let html = std::fs::read_to_string(&out).unwrap();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("&lt;b&gt;Crystalline&lt;/b&gt; &amp; porous"));
        assert!(!html.contains("<b>Crystalline</b>"));
    }

    #[tokio::test]
    async fn csv_and_json_can_be_chosen_explicitly() {
        let dir = tempfile::tempdir().unwrap();
        let (_, store) = finished_run(dir.path(), "What is a zeolite?\n").await;

        chatterg()
            .args(["report", "--format", "csv", "--store"])
            .arg(&store)
            .assert()
            .success()
            .stdout(predicate::str::starts_with("id,section,question,answer,status"));

        let output = chatterg()
            .args(["report", "--format", "json", "--store"])
            .arg(&store)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["summary"]["answered"], 1);
    }

    #[tokio::test]
    async fn an_existing_output_file_is_not_replaced_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let (_, store) = finished_run(dir.path(), "What is a zeolite?\n").await;
        let out = dir.path().join("report.md");
        std::fs::write(&out, "precious").unwrap();

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .arg("--out")
            .arg(&out)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("already exists"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "precious");

        chatterg()
            .args(["report", "--overwrite", "--store"])
            .arg(&store)
            .arg("--out")
            .arg(&out)
            .assert()
            .success();
        assert!(std::fs::read_to_string(&out).unwrap().contains("# Questionnaire report"));
    }

    #[test]
    fn overwrite_without_out_is_a_usage_error() {
        chatterg()
            .args(["report", "--overwrite"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--out"));
    }

    #[tokio::test]
    async fn the_questions_file_adds_sections() {
        let dir = tempfile::tempdir().unwrap();
        let (questions, store) = finished_run(
            dir.path(),
            "## Basics\nWhat is a zeolite?\n## Nxtbrane\nWhat is Nxtbrane?\n",
        )
        .await;

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .arg("--questions")
            .arg(&questions)
            .assert()
            .success()
            .stdout(predicate::str::contains("### Basics"))
            .stdout(predicate::str::contains("### Nxtbrane"));
    }

    #[tokio::test]
    async fn a_different_questions_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (_, store) = finished_run(dir.path(), "What is a zeolite?\n").await;

        let other = dir.path().join("other.txt");
        std::fs::write(&other, "Something else entirely?\n").unwrap();

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .arg("--questions")
            .arg(&other)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("does not match the one this run used"));
    }

    #[test]
    fn a_missing_notebook_is_a_clear_error_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("nope.db");

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("no such notebook"));

        assert!(!store.exists());
    }

    #[tokio::test]
    async fn a_report_can_be_made_while_another_chatterg_holds_the_notebook() {
        let dir = tempfile::tempdir().unwrap();
        let (_, store) = finished_run(dir.path(), "What is a zeolite?\n").await;

        let _running = chatterg::storage::sqlite::SqliteStore::open(&store).unwrap();

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .assert()
            .success()
            .stdout(predicate::str::contains("# Questionnaire report"));
    }

    #[test]
    fn an_empty_notebook_has_nothing_to_report() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("c.db");
        drop(chatterg::storage::sqlite::SqliteStore::open(&store).unwrap());

        chatterg()
            .args(["report", "--store"])
            .arg(&store)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("does not contain a run yet"));
    }
}

// ---- automatic reports (--report) --------------------------------------------------

mod auto_report {
    use super::{chatterg, json, predicate};
    use mockito::{Mock, Server, ServerGuard};
    use predicates::prelude::PredicateBooleanExt;
    use std::path::{Path, PathBuf};

    async fn agent(server: &mut ServerGuard, text: &str, hits: usize) -> (Mock, Mock) {
        let rpc_url = format!("{}/rpc", server.url());

        let card = server
            .mock("GET", "/.well-known/agent-card.json")
            .with_status(200)
            .with_body(
                json!({"name": "Mock Agent", "supportedInterfaces": [
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
                json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
                    {"kind": "text", "text": text}]}}})
                .to_string(),
            )
            .expect(hits)
            .create_async()
            .await;

        (card, rpc)
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn run(server: &ServerGuard, questions: &Path, dir: &Path) -> assert_cmd::Command {
        let mut command = chatterg();
        command
            .arg(server.url())
            .arg(questions)
            .args(["--cooldown", "0", "--max-waits", "0", "--store"])
            .arg(dir.join("c.db"));
        command
    }

    #[tokio::test]
    async fn a_finished_run_always_prints_the_done_note() {
        let mut server = Server::new_async().await;
        let _agent = agent(&mut server, "Acme", 2).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\n");

        run(&server, &questions, dir.path())
            .assert()
            .success()
            .stderr(predicate::str::contains("Done: 2 questions"))
            .stderr(predicate::str::contains("2 answered"));
    }

    #[tokio::test]
    async fn reports_are_written_in_the_format_of_their_file_ending_with_sections() {
        let mut server = Server::new_async().await;
        let _agent = agent(&mut server, "<b>Acme</b>", 2).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "## Basics\nOne?\n## More\nTwo?\n");
        let md = dir.path().join("out.md");
        let html = dir.path().join("out.html");

        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(&md)
            .arg("--report")
            .arg(&html)
            .assert()
            .success()
            .stderr(predicate::str::contains("Report:"))
            .stderr(predicate::str::contains("out.md"))
            .stderr(predicate::str::contains("out.html"));

        let md_text = std::fs::read_to_string(&md).unwrap();
        assert!(md_text.contains("# Questionnaire report"));
        assert!(
            md_text.contains("### Basics") && md_text.contains("### More"),
            "sections must come from the questions file"
        );

        let html_text = std::fs::read_to_string(&html).unwrap();
        assert!(html_text.starts_with("<!doctype html>"));
        assert!(html_text.contains("&lt;b&gt;Acme&lt;/b&gt;"));
    }

    #[tokio::test]
    async fn missing_folders_are_created() {
        let mut server = Server::new_async().await;
        let _agent = agent(&mut server, "Acme", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");
        let report = dir.path().join("reports").join("2026").join("r.csv");

        run(&server, &questions, dir.path()).arg("--report").arg(&report).assert().success();

        assert!(std::fs::read_to_string(&report).unwrap().starts_with("id,section,question"));
    }

    #[tokio::test]
    async fn a_bad_report_name_is_refused_before_anything_is_asked() {
        let mut server = Server::new_async().await;
        let (_card, rpc) = agent(&mut server, "Acme", 0).await; // the agent must not be called

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");

        run(&server, &questions, dir.path())
            .args(["--report", "notes.txt"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("cannot tell the report format"));

        rpc.assert_async().await;
        assert!(!dir.path().join("c.db").exists(), "not even the notebook may be created");
    }

    #[tokio::test]
    async fn an_existing_report_file_is_refused_up_front_unless_overwrite_is_given() {
        let mut server = Server::new_async().await;
        let (_card, rpc) = agent(&mut server, "Acme", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");
        let report = write(dir.path(), "r.md", "precious");

        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(&report)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("already exists"));
        assert_eq!(std::fs::read_to_string(&report).unwrap(), "precious");

        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(&report)
            .arg("--overwrite")
            .assert()
            .success();
        assert!(std::fs::read_to_string(&report).unwrap().contains("# Questionnaire report"));

        rpc.assert_async().await; // only the second command asked anything
    }

    #[tokio::test]
    async fn a_report_may_never_replace_the_questions_file_or_the_notebook() {
        let mut server = Server::new_async().await;
        let (_card, rpc) = agent(&mut server, "Acme", 0).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "questions.md", "One?\n");

        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(&questions)
            .arg("--overwrite")
            .assert()
            .code(1)
            .stderr(predicate::str::contains("would overwrite your questions file"));
        assert_eq!(std::fs::read_to_string(&questions).unwrap(), "One?\n");

        // the notebook is protected too (only matters once it exists)
        std::fs::write(dir.path().join("c.db"), "").unwrap();
        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(dir.path().join("c.db"))
            .arg("--overwrite")
            .assert()
            .code(1);

        rpc.assert_async().await;
    }

    #[test]
    fn the_same_report_twice_is_an_error() {
        chatterg()
            .args(["http://localhost:1", "questions.yaml", "--report", "r.md", "--report", "r.md"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("given twice"));
    }

    #[test]
    fn overwrite_needs_a_report() {
        chatterg()
            .args(["http://localhost:1", "questions.yaml", "--overwrite"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--report"));
    }

    #[tokio::test]
    async fn a_run_that_ends_early_writes_no_report() {
        let mut server = Server::new_async().await;
        let _agent = agent(&mut server, "not valid", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(
            dir.path(),
            "q.yaml",
            "questions:\n  - {id: stage, question: Stage?, required: true, type: enum, values: [pilot]}\n",
        );
        let report = dir.path().join("r.md");

        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(&report)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("Done:").not());

        assert!(!report.exists());
    }

    #[tokio::test]
    async fn a_run_that_gives_up_writes_no_report() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = server
            .mock("GET", "/.well-known/agent-card.json")
            .with_status(200)
            .with_body(
                json!({"name": "M", "supportedInterfaces": [
                    {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}]})
                .to_string(),
            )
            .create_async()
            .await;
        let _limited = server.mock("POST", "/rpc").with_status(429).create_async().await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");
        let report = dir.path().join("r.md");

        run(&server, &questions, dir.path()).arg("--report").arg(&report).assert().code(3);

        assert!(!report.exists());
    }

    #[tokio::test]
    async fn if_the_report_cannot_be_written_the_run_is_still_safe_and_the_message_says_how_to_retry()
     {
        let mut server = Server::new_async().await;
        let _agent = agent(&mut server, "Acme", 1).await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");
        // a *file* where a folder would have to be created
        let blocker = write(dir.path(), "blocker", "x");
        let report = blocker.join("sub").join("r.md");

        run(&server, &questions, dir.path())
            .arg("--report")
            .arg(&report)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("the run finished and is saved"))
            .stderr(predicate::str::contains("chatterg report --store"));

        // the advice works: the notebook holds the finished run
        chatterg()
            .args(["report", "--store"])
            .arg(dir.path().join("c.db"))
            .assert()
            .success()
            .stdout(predicate::str::contains("# Questionnaire report"));
    }

    #[tokio::test]
    async fn help_documents_the_report_switches() {
        chatterg()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("--report <FILE>"))
            .stdout(predicate::str::contains("--overwrite"));
    }
}

// ---- the friendly workflow: add / list / remove / run -------------------------------------

mod friendly {
    use super::{chatterg, json, predicate};
    use chatterg::storage::{Store, sqlite::SqliteStore};
    use mockito::{Mock, Server, ServerGuard};
    use predicates::prelude::PredicateBooleanExt;
    use std::path::{Path, PathBuf};

    async fn card(server: &mut ServerGuard) -> Mock {
        let rpc_url = format!("{}/rpc", server.url());

        server
            .mock("GET", "/.well-known/agent-card.json")
            .with_status(200)
            .with_body(
                json!({"name": "Mock Agent", "supportedInterfaces": [
                    {"url": rpc_url, "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}
                ]})
                .to_string(),
            )
            .create_async()
            .await
    }

    async fn answering(server: &mut ServerGuard, text: &str, hits: usize) -> Mock {
        server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(
                json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
                    {"kind": "text", "text": text}]}}})
                .to_string(),
            )
            .expect(hits)
            .create_async()
            .await
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// `chatterg run <server>` inside `dir`, with no waiting.
    fn run(dir: &Path, server: &ServerGuard) -> assert_cmd::Command {
        let mut command = chatterg();
        command.current_dir(dir).args([
            "run",
            &server.url(),
            "--cooldown",
            "0",
            "--max-waits",
            "0",
        ]);
        command
    }

    fn run_folders(dir: &Path) -> Vec<PathBuf> {
        let mut folders: Vec<_> = match std::fs::read_dir(dir.join("runs")) {
            Ok(entries) => entries.filter_map(Result::ok).map(|entry| entry.path()).collect(),
            Err(_) => Vec::new(),
        };
        folders.sort();
        folders
    }

    async fn working_notebook_is_empty(dir: &Path) -> bool {
        SqliteStore::open_read_only(dir.join("chatterg.db"))
            .unwrap()
            .load()
            .await
            .unwrap()
            .is_none()
    }

    // ---- run ------------------------------------------------------------------------

    #[tokio::test]
    async fn run_asks_everything_saves_a_report_and_empties_the_notebook() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = answering(&mut server, "<b>Acme</b>", 2).await;

        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "questions.txt", "## Basics\nOne?\nTwo?\n");

        run(dir.path(), &server)
            .assert()
            .code(0)
            .stdout(predicate::str::contains("Done: 2 questions"))
            .stdout(predicate::str::contains("Saved to runs/"))
            .stdout(predicate::str::contains("report.html"))
            .stdout(predicate::str::contains("The notebook is empty again"))
            .stderr(predicate::str::contains("[2/2] q002"));

        let folders = run_folders(dir.path());
        assert_eq!(folders.len(), 1);
        let mut files: Vec<_> = std::fs::read_dir(&folders[0])
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(
            files,
            ["answers.csv", "notebook.db", "questions.txt", "report.html", "report.md"]
        );

        let html = std::fs::read_to_string(folders[0].join("report.html")).unwrap();
        assert!(html.contains("&lt;b&gt;Acme&lt;/b&gt;") && !html.contains("<b>Acme</b>"));
        assert!(
            std::fs::read_to_string(folders[0].join("report.md")).unwrap().contains("### Basics")
        );

        // your question bank is left exactly as it was, ready for the next run
        assert_eq!(
            std::fs::read_to_string(dir.path().join("questions.txt")).unwrap(),
            "## Basics\nOne?\nTwo?\n"
        );
        assert!(working_notebook_is_empty(dir.path()).await);
    }

    #[tokio::test]
    async fn running_again_asks_the_bot_again_and_saves_a_second_folder() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let rpc = answering(&mut server, "Acme", 4).await; // 2 questions x 2 runs

        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "questions.txt", "One?\nTwo?\n");

        run(dir.path(), &server).assert().success();
        run(dir.path(), &server).assert().success();

        rpc.assert_async().await;
        assert_eq!(run_folders(dir.path()).len(), 2);
        assert!(working_notebook_is_empty(dir.path()).await);
    }

    #[tokio::test]
    async fn a_questions_file_and_a_runs_folder_can_be_chosen() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = answering(&mut server, "Acme", 1).await;

        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "other.txt", "Only one?\n");

        run(dir.path(), &server)
            .args(["other.txt", "--runs-dir", "saved/answers"])
            .assert()
            .success();

        let saved: Vec<_> =
            std::fs::read_dir(dir.path().join("saved").join("answers")).unwrap().collect();
        assert_eq!(saved.len(), 1);
        assert!(!dir.path().join("runs").exists());
    }

    #[test]
    fn without_a_question_bank_the_message_says_how_to_start() {
        let dir = tempfile::tempdir().unwrap();

        chatterg()
            .current_dir(dir.path())
            .args(["run", "http://localhost:1"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("there is no questions.txt"))
            .stderr(predicate::str::contains("chatterg add"));

        assert!(!dir.path().join("chatterg.db").exists(), "nothing may be created");
    }

    #[tokio::test]
    async fn a_run_that_ends_early_is_saved_too_and_the_notebook_is_emptied() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = answering(&mut server, "not valid", 1).await;

        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "questions.yaml",
            "questions:\n  - {id: stage, question: Stage?, required: true, type: enum, values: [pilot]}\n",
        );

        run(dir.path(), &server)
            .arg("questions.yaml")
            .assert()
            .code(2)
            .stdout(predicate::str::contains("Stopped early"))
            .stdout(predicate::str::contains("Saved to runs/"))
            .stdout(predicate::str::contains("questions.yaml"));

        assert_eq!(run_folders(dir.path()).len(), 1);
        assert!(
            working_notebook_is_empty(dir.path()).await,
            "an ended run must not block the next one"
        );
    }

    #[tokio::test]
    async fn an_interrupted_run_carries_on_and_is_saved_when_done() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _first = answering(&mut server, "A1", 1).await;
        let _limited = server.mock("POST", "/rpc").with_status(429).create_async().await;

        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "questions.txt", "One?\nTwo?\n");

        run(dir.path(), &server)
            .assert()
            .code(3)
            .stderr(predicate::str::contains("Your progress is saved"));
        assert!(run_folders(dir.path()).is_empty(), "an unfinished run is not put away");
        assert!(!working_notebook_is_empty(dir.path()).await);

        // the bot recovers
        server.reset();
        let _card = card(&mut server).await;
        let _rest = answering(&mut server, "A2", 1).await;

        run(dir.path(), &server)
            .assert()
            .success()
            .stderr(predicate::str::contains("Carrying on: 1 of 2 questions are already done"))
            .stdout(predicate::str::contains("Done: 2 questions"));

        let folders = run_folders(dir.path());
        assert_eq!(folders.len(), 1);
        let csv = std::fs::read_to_string(folders[0].join("answers.csv")).unwrap();
        assert!(csv.contains("A1") && csv.contains("A2"));
        assert!(working_notebook_is_empty(dir.path()).await);
    }

    #[tokio::test]
    async fn an_unfinished_run_for_another_bot_is_not_mixed_up_with_this_one() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "questions.txt", "One?\nTwo?\n");

        // bot A answers once, then is unavailable
        let mut bot_a = Server::new_async().await;
        let _card_a = card(&mut bot_a).await;
        let _ok = answering(&mut bot_a, "from A", 1).await;
        let _limited = bot_a.mock("POST", "/rpc").with_status(429).create_async().await;
        run(dir.path(), &bot_a).assert().code(3);

        // asking bot B now must not silently reuse A's answers
        let mut bot_b = Server::new_async().await;
        let _card_b = card(&mut bot_b).await;
        let rpc_b = answering(&mut bot_b, "from B", 0).await;

        run(dir.path(), &bot_b)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("an unfinished run for"))
            .stderr(predicate::str::contains("--restart"));
        rpc_b.assert_async().await;

        // --restart puts A's run aside and asks B everything
        let mut bot_c = Server::new_async().await;
        let _card_c = card(&mut bot_c).await;
        let _rpc_c = answering(&mut bot_c, "from C", 2).await;

        run(dir.path(), &bot_c).arg("--restart").assert().success();

        let csv = std::fs::read_to_string(run_folders(dir.path())[0].join("answers.csv")).unwrap();
        assert!(csv.contains("from C") && !csv.contains("from A"));

        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            .count();
        assert_eq!(backups, 1, "the abandoned run is kept as a backup");
    }

    #[tokio::test]
    async fn a_finished_run_left_in_the_notebook_is_put_away_before_the_new_one_starts() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = answering(&mut server, "Acme", 2).await; // once for the old run, once for the new

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "questions.txt", "One?\n");

        // the advanced form keeps the finished notebook, as it always did
        chatterg()
            .current_dir(dir.path())
            .arg(server.url())
            .arg(&questions)
            .args(["--cooldown", "0"])
            .assert()
            .success();
        assert!(!working_notebook_is_empty(dir.path()).await);

        run(dir.path(), &server)
            .assert()
            .success()
            .stderr(predicate::str::contains("never put away"));

        assert_eq!(run_folders(dir.path()).len(), 2, "the old run and the new run");
        assert!(working_notebook_is_empty(dir.path()).await);
    }

    #[test]
    fn help_shows_the_quick_start() {
        chatterg()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("QUICK START"))
            .stdout(predicate::str::contains("chatterg add"))
            .stdout(predicate::str::contains("run "))
            .stdout(predicate::str::contains("list"))
            .stdout(predicate::str::contains("remove"));

        chatterg()
            .args(["run", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--runs-dir"))
            .stdout(predicate::str::contains("--delay"))
            .stdout(predicate::str::contains("[QUESTIONS_FILE]"));
    }

    // ---- the question bank -------------------------------------------------------------

    #[test]
    fn add_list_and_remove_work_on_questions_txt() {
        let dir = tempfile::tempdir().unwrap();
        let in_dir = || {
            let mut command = chatterg();
            command.current_dir(dir.path());
            command
        };

        in_dir()
            .args(["add", "What is a zeolite?"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Added as question 1 in questions.txt"))
            .stdout(predicate::str::contains(".bak").not());

        in_dir()
            .args(["add", "What is Nxtbrane?", "--section", "Nxtbrane"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Added as question 2"))
            .stdout(predicate::str::contains("questions.txt.bak"));

        in_dir()
            .arg("list")
            .assert()
            .success()
            .stdout(predicate::str::contains("questions.txt: 2 questions"))
            .stdout(predicate::str::contains("## Nxtbrane"))
            .stdout(predicate::str::contains(" 2  What is Nxtbrane?"));

        in_dir()
            .args(["remove", "1"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Removed question 1"))
            .stdout(predicate::str::contains("What is a zeolite?"));

        in_dir().arg("list").assert().success().stdout(predicate::str::contains("1 question\n"));

        assert_eq!(
            std::fs::read_to_string(dir.path().join("questions.txt")).unwrap(),
            "\n## Nxtbrane\nWhat is Nxtbrane?\n"
        );
    }

    #[test]
    fn bank_mistakes_are_explained_and_change_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(dir.path(), "questions.txt", "One?\n");
        let in_dir = || {
            let mut command = chatterg();
            command.current_dir(dir.path());
            command
        };

        in_dir()
            .args(["add", "ONE?"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("already in the bank as number 1"));
        in_dir()
            .args(["add", "- looks like a list item"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("read differently"));
        in_dir()
            .args(["remove", "9"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("there is no question number 9"));
        in_dir().args(["remove", "0"]).assert().code(1);

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "One?\n");
    }

    #[test]
    fn listing_a_missing_bank_says_how_to_start_and_yaml_banks_are_not_edited() {
        let dir = tempfile::tempdir().unwrap();

        chatterg()
            .current_dir(dir.path())
            .arg("list")
            .assert()
            .code(1)
            .stderr(predicate::str::contains("there is no questions.txt yet"));

        write(dir.path(), "bank.yaml", "questions: [\"One?\"]\n");

        chatterg()
            .current_dir(dir.path())
            .args(["add", "Two?", "--file", "bank.yaml"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("YAML file"));

        chatterg()
            .current_dir(dir.path())
            .args(["list", "--file", "bank.yaml"])
            .assert()
            .success()
            .stdout(predicate::str::contains("bank.yaml: 1 question"));
    }

    #[test]
    fn the_bank_file_can_be_chosen() {
        let dir = tempfile::tempdir().unwrap();

        chatterg()
            .current_dir(dir.path())
            .args(["add", "One?", "--file", "mine/bank.txt"])
            .assert()
            .success();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("mine").join("bank.txt")).unwrap(),
            "One?\n"
        );
        assert!(!dir.path().join("questions.txt").exists());
    }
}
