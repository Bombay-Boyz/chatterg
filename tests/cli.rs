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

// ---- Ctrl+C, --rate, Retry-After, the ETA ---------------------------------------------------

mod polite_and_stoppable {
    use super::{chatterg, json, predicate};
    use chatterg::storage::{Store, sqlite::SqliteStore};
    use mockito::{Server, ServerGuard};
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::mpsc::{self, Receiver},
        time::{Duration, Instant},
    };

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn reply(text: &str) -> String {
        json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
            {"kind": "text", "text": text}]}}})
        .to_string()
    }

    async fn card_for(server: &mut ServerGuard, rpc_url: &str) -> mockito::Mock {
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

    /// An agent whose replies take `delay`. The card is served by mockito; the
    /// replies come from a tiny hand-made server so the delay is real.
    struct SlowAgent {
        _card: mockito::Mock,
        server: ServerGuard,
    }

    impl SlowAgent {
        fn url(&self) -> String {
            self.server.url()
        }
    }

    /// The receiver gets a message the moment a request has reached the agent, so a
    /// test can send Ctrl+C when the question is genuinely in flight.
    async fn slow_agent(delay: Duration, answer: &'static str) -> (SlowAgent, Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (arrived, requests) = mpsc::channel();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let arrived = arrived.clone();
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 8192];
                    let _ = stream.read(&mut buffer);
                    let _ = arrived.send(());
                    std::thread::sleep(delay);
                    let body = reply(answer);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });

        let mut server = Server::new_async().await;
        let card = card_for(&mut server, &format!("http://127.0.0.1:{port}/rpc")).await;

        (SlowAgent { _card: card, server }, requests)
    }

    /// A running chatterg whose screen output can be watched line by line.
    struct Running {
        child: Child,
        lines: Receiver<String>,
        seen: Vec<String>,
    }

    impl Running {
        fn start(dir: &Path, agent_url: &str, questions: &Path, extra: &[&str]) -> Self {
            let program = chatterg().get_program().to_owned();

            let mut child = Command::new(program)
                .current_dir(dir)
                .arg(agent_url)
                .arg(questions)
                .args(["--cooldown", "0", "--store"])
                .arg(dir.join("c.db"))
                .args(extra)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();

            let (sender, lines) = mpsc::channel();
            let stderr = child.stderr.take().unwrap();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });

            Self { child, lines, seen: Vec::new() }
        }

        fn wait_for(&mut self, needle: &str, seconds: u64) {
            let deadline = Instant::now() + Duration::from_secs(seconds);

            while Instant::now() < deadline {
                if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(100)) {
                    let found = line.contains(needle);
                    self.seen.push(line);
                    if found {
                        return;
                    }
                }
            }

            let _ = self.child.kill();
            panic!("never saw {needle:?}; saw {:#?}", self.seen);
        }

        fn interrupt(&self) {
            let status =
                Command::new("kill").args(["-INT", &self.child.id().to_string()]).status().unwrap();
            assert!(status.success());
        }

        /// Waits for the process to end; fails the test if it takes longer than `seconds`.
        fn finish(&mut self, seconds: u64) -> (Option<i32>, String) {
            let deadline = Instant::now() + Duration::from_secs(seconds);

            let code = loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status.code();
                }
                if Instant::now() > deadline {
                    let _ = self.child.kill();
                    panic!("chatterg did not stop within {seconds}s; saw {:#?}", self.seen);
                }
                std::thread::sleep(Duration::from_millis(50));
            };

            std::thread::sleep(Duration::from_millis(100));
            while let Ok(line) = self.lines.try_recv() {
                self.seen.push(line);
            }

            (code, self.seen.join("\n"))
        }
    }

    async fn saved_position(dir: &Path) -> usize {
        SqliteStore::open_read_only(dir.join("c.db"))
            .unwrap()
            .load()
            .await
            .unwrap()
            .unwrap()
            .position
    }

    // ---- Ctrl+C ----------------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn the_first_ctrl_c_finishes_the_question_being_asked_then_stops() {
        let (agent, requests) = slow_agent(Duration::from_millis(1500), "slow answer").await;
        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\n");

        let mut run = Running::start(dir.path(), &agent.url(), &questions, &[]);
        run.wait_for("[1/2] q001", 20);
        requests.recv_timeout(Duration::from_secs(10)).expect("the request never arrived");
        run.interrupt();
        run.wait_for("finishing the question", 10);

        let started = Instant::now();
        let (code, screen) = run.finish(20);

        assert_eq!(code, Some(130), "{screen}");
        assert!(screen.contains("Stopped after 1 of 2 questions"), "{screen}");
        assert!(screen.contains("Run the same command again to carry on"), "{screen}");
        assert!(!screen.contains("error:"), "a stop is not an error:\n{screen}");
        assert!(!screen.contains("[2/2]"), "question 2 must not be started");
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "it waited for the in-flight answer"
        );

        assert_eq!(saved_position(dir.path()).await, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ctrl_c_during_the_pause_between_questions_stops_at_once() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = card_for(&mut server, &rpc_url).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\n");

        let mut run = Running::start(dir.path(), &server.url(), &questions, &["--delay", "300"]);
        run.wait_for("[1/2] q001", 20);
        std::thread::sleep(Duration::from_millis(700)); // question 1 is answered; now in the 300 s pause
        run.interrupt();

        let started = Instant::now();
        let (code, screen) = run.finish(10);

        assert_eq!(code, Some(130), "{screen}");
        assert!(screen.contains("Stopped after 1 of 2 questions"), "{screen}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(saved_position(dir.path()).await, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_second_ctrl_c_stops_even_while_waiting_for_a_slow_bot() {
        let (agent, requests) = slow_agent(Duration::from_secs(600), "never").await;
        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");

        let mut run = Running::start(dir.path(), &agent.url(), &questions, &[]);
        run.wait_for("[1/1] q001", 20);
        requests.recv_timeout(Duration::from_secs(10)).expect("the request never arrived");
        run.interrupt();
        run.wait_for("finishing the question", 10);
        run.interrupt();

        let (code, screen) = run.finish(10);

        assert_eq!(code, Some(130), "{screen}");
        assert!(screen.contains("Stopping now"), "{screen}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_polite_kill_is_treated_like_ctrl_c() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = card_for(&mut server, &rpc_url).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\n");

        let mut run = Running::start(dir.path(), &server.url(), &questions, &["--delay", "300"]);
        run.wait_for("[1/2] q001", 20);
        std::thread::sleep(Duration::from_millis(700));
        Command::new("kill").args(["-TERM", &run.child.id().to_string()]).status().unwrap();

        let (code, screen) = run.finish(10);

        assert_eq!(code, Some(130), "{screen}");
        assert_eq!(saved_position(dir.path()).await, 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_the_terminal_stops_a_run_cleanly_too() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = card_for(&mut server, &rpc_url).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\n");

        let mut run = Running::start(dir.path(), &server.url(), &questions, &["--delay", "300"]);
        run.wait_for("[1/2] q001", 20);
        std::thread::sleep(Duration::from_millis(700));
        Command::new("kill").args(["-HUP", &run.child.id().to_string()]).status().unwrap();

        let (code, screen) = run.finish(10);

        assert_eq!(code, Some(130), "{screen}");
        assert_eq!(saved_position(dir.path()).await, 1);
    }

    #[test]
    fn help_explains_stopping_and_the_new_switches() {
        chatterg()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("130"))
            .stdout(predicate::str::contains("Ctrl+C"))
            .stdout(predicate::str::contains("--rate <N/UNIT>"))
            .stdout(predicate::str::contains("--max-wait <SECONDS>"));

        chatterg()
            .args(["run", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--rate <N/UNIT>"))
            .stdout(predicate::str::contains("--max-wait <SECONDS>"));
    }

    // ---- --rate -------------------------------------------------------------------------------

    #[test]
    fn a_bad_rate_is_refused_with_an_example() {
        chatterg()
            .args(["http://localhost:1", "questions.yaml", "--rate", "fast"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("expected a rate like 10/minute, 30/hour or 100/day"));
    }

    #[tokio::test]
    async fn a_rate_spaces_the_requests() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = card_for(&mut server, &rpc_url).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(3)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\nThree?\n");

        let started = Instant::now();
        chatterg()
            .current_dir(dir.path())
            .arg(server.url())
            .arg(&questions)
            .args(["--rate", "4/second", "--store"])
            .arg(dir.path().join("c.db"))
            .assert()
            .success();

        // 4 per second = 250 ms apart, so three requests take at least half a second
        assert!(started.elapsed() >= Duration::from_millis(480), "{:?}", started.elapsed());
    }

    // ---- the server's own advice ------------------------------------------------------------------

    #[tokio::test]
    async fn retry_after_is_obeyed_and_announced() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = card_for(&mut server, &rpc_url).await;
        let _limited = server
            .mock("POST", "/rpc")
            .with_status(429)
            .with_header("retry-after", "1")
            .expect(1)
            .create_async()
            .await;
        let _ok = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(1)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\n");

        let started = Instant::now();
        chatterg()
            .current_dir(dir.path())
            .arg(server.url())
            .arg(&questions)
            .args(["--cooldown", "0", "--store"])
            .arg(dir.path().join("c.db"))
            .assert()
            .success()
            .stderr(predicate::str::contains("the server asks us to wait 1s"))
            .stderr(predicate::str::contains("pausing 1s"));

        assert!(started.elapsed() >= Duration::from_millis(950), "{:?}", started.elapsed());
    }

    // ---- progress -----------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_progress_line_shows_a_time_left_once_there_is_a_pace_to_measure() {
        let mut server = Server::new_async().await;
        let rpc_url = format!("{}/rpc", server.url());
        let _card = card_for(&mut server, &rpc_url).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(4)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\nThree?\nFour?\n");

        chatterg()
            .current_dir(dir.path())
            .arg(server.url())
            .arg(&questions)
            .args(["--store"])
            .arg(dir.path().join("c.db"))
            .assert()
            .success()
            .stderr(predicate::str::contains("[1/4] q001\n"))
            .stderr(predicate::str::contains("[3/4] q003  (about "))
            .stderr(predicate::str::contains(" left)"));
    }
}

// ---- backups, rollback, rewind, damaged notebooks, closed terminals ---------------------------

mod recovery_commands {
    use super::{chatterg, json, predicate};
    use mockito::{Mock, Server, ServerGuard};
    use std::{
        io::{BufRead, BufReader},
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

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

    fn reply(text: &str) -> String {
        json!({"jsonrpc": "2.0", "id": 1, "result": {"message": {"parts": [
            {"kind": "text", "text": text}]}}})
        .to_string()
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    /// The advanced form, keeping the notebook in `dir/c.db`.
    fn run(dir: &Path, server: &ServerGuard, questions: &Path) -> assert_cmd::Command {
        let mut command = chatterg();
        command
            .current_dir(dir)
            .arg(server.url())
            .arg(questions)
            .args(["--cooldown", "0", "--max-waits", "0", "--store"])
            .arg(dir.join("c.db"));
        command
    }

    fn store_cmd(dir: &Path, words: &[&str]) -> assert_cmd::Command {
        let mut command = chatterg();
        command.current_dir(dir).args(words).arg("--store").arg(dir.join("c.db"));
        command
    }

    /// A notebook with a run stopped after `answered` of the questions.
    async fn stopped_run(dir: &Path, answered: usize, total: usize) -> PathBuf {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _ok = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(answered)
            .create_async()
            .await;
        let _limited = server.mock("POST", "/rpc").with_status(429).create_async().await;

        let body: String = (1..=total).map(|n| format!("Question {n}?\n")).collect();
        let questions = write(dir, "q.txt", &body);

        run(dir, &server, &questions).assert().code(3);
        questions
    }

    fn snapshot_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir.join("c.db.backups"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn version_is_printed() {
        chatterg()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
    }

    #[tokio::test]
    async fn a_run_keeps_the_newest_automatic_backups_only() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(4)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\nThree?\nFour?\n");

        run(dir.path(), &server, &questions)
            .args(["--backup-every", "1", "--keep-backups", "2"])
            .assert()
            .success();

        let names = snapshot_names(dir.path());
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[0].ends_with("-q0003.db") && names[1].ends_with("-q0004.db"), "{names:?}");

        store_cmd(dir.path(), &["backups"])
            .assert()
            .success()
            .stdout(predicate::str::contains("auto"))
            .stdout(predicate::str::contains("q0004"));
    }

    #[tokio::test]
    async fn backups_can_be_switched_off() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(3)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\nThree?\n");

        run(dir.path(), &server, &questions).args(["--backup-every", "0"]).assert().success();

        assert!(!dir.path().join("c.db.backups").exists());
        store_cmd(dir.path(), &["backups"])
            .assert()
            .success()
            .stdout(predicate::str::contains("No backups yet"));
    }

    #[tokio::test]
    async fn backup_rewind_and_rollback_work_together() {
        let dir = tempfile::tempdir().unwrap();
        stopped_run(dir.path(), 3, 5).await;

        store_cmd(dir.path(), &["backup"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Backup saved:"))
            .stdout(predicate::str::contains("after 3 questions"));

        store_cmd(dir.path(), &["rewind", "--to", "2"])
            .assert()
            .success()
            .stdout(predicate::str::contains(
                "Rewound to question 2: 2 answers will be asked again",
            ))
            .stdout(predicate::str::contains("kept as"));

        store_cmd(dir.path(), &["backups"])
            .assert()
            .success()
            .stdout(predicate::str::contains("safety"))
            .stdout(predicate::str::contains("manual"));

        store_cmd(dir.path(), &["rollback"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rolled back to the backup manual-"))
            .stdout(predicate::str::contains("3 questions finished of 5"));

        // and the notebook really is back at question 4 of 5
        store_cmd(dir.path(), &["report", "--format", "json"])
            .arg("--out")
            .arg(dir.path().join("r.json"))
            .assert()
            .success();
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("r.json")).unwrap())
                .unwrap();
        assert_eq!(report["summary"]["answered"], 3);
    }

    #[tokio::test]
    async fn the_question_can_be_given_as_q040_and_wrong_numbers_are_explained() {
        let dir = tempfile::tempdir().unwrap();
        stopped_run(dir.path(), 2, 4).await;

        store_cmd(dir.path(), &["rewind", "--to", "9"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("only questions 1 to 2 have been finished"));

        store_cmd(dir.path(), &["rewind", "--to", "abc"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("expected a question number like 40 or q040"));

        store_cmd(dir.path(), &["rewind", "--to", "q002"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Rewound to question 2"));
    }

    #[test]
    fn rollback_without_any_backup_says_so() {
        let dir = tempfile::tempdir().unwrap();

        store_cmd(dir.path(), &["rollback"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("there are no backups for this notebook yet"));
    }

    #[tokio::test]
    async fn a_damaged_notebook_is_explained_and_can_be_put_right_from_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let questions = stopped_run(dir.path(), 2, 4).await;

        // no backup yet: the message says how to start fresh
        let garbage = "garbage, long enough that it cannot be mistaken for a database header";
        std::fs::write(dir.path().join("c.db"), garbage).unwrap();
        let _ = std::fs::remove_file(dir.path().join("c.db-wal"));
        let _ = std::fs::remove_file(dir.path().join("c.db-shm"));
        let server = Server::new_async().await;
        run(dir.path(), &server, &questions)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("is damaged"))
            .stderr(predicate::str::contains("There are no backups of it"));

        // restore a good notebook, back it up, damage it again
        std::fs::remove_file(dir.path().join("c.db")).unwrap();
        stopped_run(dir.path(), 2, 4).await;
        store_cmd(dir.path(), &["backup"]).assert().success();
        std::fs::write(dir.path().join("c.db"), garbage).unwrap();
        let _ = std::fs::remove_file(dir.path().join("c.db-wal"));
        let _ = std::fs::remove_file(dir.path().join("c.db-shm"));

        let server2 = Server::new_async().await;
        run(dir.path(), &server2, &questions)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("There is a backup: manual-"))
            .stderr(predicate::str::contains("chatterg rollback --store"));

        store_cmd(dir.path(), &["rollback"])
            .assert()
            .success()
            .stdout(predicate::str::contains("damaged notebook was moved aside"))
            .stdout(predicate::str::contains("2 questions finished"));

        let moved: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".damaged-"))
            .collect();
        assert_eq!(moved.len(), 1, "the damaged notebook is kept, never deleted");
        assert!(std::fs::read_to_string(moved[0].path()).unwrap().starts_with("garbage"));
    }

    // ---- closed terminals and broken pipes --------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn a_broken_pipe_does_not_crash_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let body: String =
            (1..=5000).map(|n| format!("Question number {n} about zeolite membranes?\n")).collect();
        let bank = write(dir.path(), "big.txt", &body);

        let program = chatterg().get_program().to_owned();
        let mut child = Command::new(program)
            .args(["list", "--file"])
            .arg(&bank)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        // like `chatterg list | head -1`: read one line, then hang up
        let stdout = child.stdout.take().unwrap();
        let mut first = String::new();
        BufReader::new(stdout).read_line(&mut first).unwrap();
        assert!(first.contains("5000 questions"), "{first}");

        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("panicked") && !stderr.contains("failed printing"), "{stderr}");
        assert_eq!(output.status.code(), Some(0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_closed_terminal_does_not_crash_a_run() {
        let mut server = Server::new_async().await;
        let _card = card(&mut server).await;
        let _rpc = server
            .mock("POST", "/rpc")
            .with_status(200)
            .with_body(reply("Acme"))
            .expect(2)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let questions = write(dir.path(), "q.txt", "One?\nTwo?\n");
        let program = chatterg().get_program().to_owned();

        // `2>&-` closes the error output, as a vanished terminal would
        let status = Command::new("sh")
            .current_dir(dir.path())
            .arg("-c")
            .arg("exec \"$0\" \"$@\" 2>&-")
            .arg(program)
            .arg(server.url())
            .arg(&questions)
            .args(["--store"])
            .arg(dir.path().join("c.db"))
            .stdout(Stdio::null())
            .status()
            .unwrap();

        assert_eq!(
            status.code(),
            Some(0),
            "a run must finish even if nobody can read its messages"
        );
    }
}
