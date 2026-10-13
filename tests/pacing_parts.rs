//! The small pieces behind polite, stoppable runs: rate parsing, the ETA, the
//! stop signal and reading `Retry-After`.

use std::time::{Duration, Instant};

use chatterg::{
    application::parse_rate,
    cancel::Cancel,
    output::progress::{Eta, approximately},
    transport::a2a::wait_hint,
};
use chrono::{TimeZone, Utc};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

// ---- --rate ---------------------------------------------------------------------------

#[test]
fn rates_become_gaps_between_requests() {
    assert_eq!(parse_rate("30/hour").unwrap(), Duration::from_secs(120));
    assert_eq!(parse_rate("10/minute").unwrap(), Duration::from_secs(6));
    assert_eq!(parse_rate("1/second").unwrap(), Duration::from_secs(1));
    assert_eq!(parse_rate("48/day").unwrap(), Duration::from_secs(1800));
    assert_eq!(parse_rate("10/day").unwrap(), Duration::from_secs(8640));
}

#[test]
fn units_can_be_written_short_or_long_in_any_case() {
    for text in ["30/h", "30/hr", "30/hour", "30/hours", "30/HOUR", " 30 / Hour "] {
        assert_eq!(parse_rate(text).unwrap(), Duration::from_secs(120), "{text:?}");
    }
    for text in ["2/s", "2/sec", "2/seconds"] {
        assert_eq!(parse_rate(text).unwrap(), Duration::from_millis(500), "{text:?}");
    }
    assert_eq!(parse_rate("2/m").unwrap(), Duration::from_secs(30));
    assert_eq!(parse_rate("2/d").unwrap(), Duration::from_secs(43_200));
}

#[test]
fn rates_that_do_not_divide_evenly_keep_their_precision() {
    let gap = parse_rate("7/day").unwrap();

    assert!(gap > Duration::from_secs(12_342) && gap < Duration::from_secs(12_343), "{gap:?}");
}

#[test]
fn nonsense_rates_are_refused_with_an_example() {
    for text in
        ["", "30", "/hour", "0/hour", "-5/hour", "ten/hour", "30/fortnight", "30/", "1.5/hour"]
    {
        let error = parse_rate(text).unwrap_err();

        assert!(error.contains("10/minute"), "{text:?} -> {error}");
    }
}

// ---- the ETA --------------------------------------------------------------------------

fn at(base: Instant, seconds: u64) -> Instant {
    base + Duration::from_secs(seconds)
}

#[test]
fn no_estimate_until_a_couple_of_questions_have_finished() {
    let base = Instant::now();
    let mut eta = Eta::new();

    assert_eq!(eta.observe(1, 100, at(base, 0)), None);
    assert_eq!(eta.observe(2, 100, at(base, 60)), None, "one data point is too few");
    assert!(eta.observe(3, 100, at(base, 120)).is_some());
}

#[test]
fn the_estimate_is_pace_times_questions_left_including_the_current_one() {
    let base = Instant::now();
    let mut eta = Eta::new();

    eta.observe(1, 10, at(base, 0));
    eta.observe(2, 10, at(base, 60));

    // two questions finished in 120 s: 60 s each; questions 3..=10 remain = 8
    assert_eq!(eta.observe(3, 10, at(base, 120)), Some(Duration::from_secs(60 * 8)));
}

#[test]
fn a_resumed_run_is_measured_from_where_this_session_started() {
    let base = Instant::now();
    let mut eta = Eta::new();

    // resuming at question 400 of 701
    eta.observe(400, 701, at(base, 0));
    eta.observe(401, 701, at(base, 30));

    assert_eq!(eta.observe(402, 701, at(base, 60)), Some(Duration::from_secs(30 * 300)));
}

#[test]
fn a_retry_of_the_same_question_does_not_break_the_estimate() {
    let base = Instant::now();
    let mut eta = Eta::new();

    eta.observe(1, 10, at(base, 0));
    eta.observe(2, 10, at(base, 60));
    eta.observe(3, 10, at(base, 120));
    let after_retry = eta.observe(3, 10, at(base, 180));

    // the retry cost a minute, so the pace per finished question got slower
    assert_eq!(after_retry, Some(Duration::from_secs(90 * 8)));
}

#[test]
fn the_last_question_has_one_pace_left() {
    let base = Instant::now();
    let mut eta = Eta::new();

    eta.observe(1, 3, at(base, 0));
    eta.observe(2, 3, at(base, 10));

    assert_eq!(eta.observe(3, 3, at(base, 20)), Some(Duration::from_secs(10)));
}

#[test]
fn durations_are_shown_roughly() {
    assert_eq!(approximately(Duration::from_secs(40)), "40s");
    assert_eq!(approximately(Duration::from_secs(59)), "59s");
    assert_eq!(approximately(Duration::from_secs(60)), "1m");
    assert_eq!(approximately(Duration::from_secs(35 * 60 + 50)), "35m");
    assert_eq!(approximately(Duration::from_secs(3600)), "1h 00m");
    assert_eq!(approximately(Duration::from_secs(11 * 3600 + 41 * 60 + 9)), "11h 41m");
}

// ---- the stop signal --------------------------------------------------------------------

#[tokio::test]
async fn cancelling_wakes_everyone_waiting() {
    let cancel = Cancel::default();
    assert!(!cancel.is_cancelled());

    let waiting = cancel.clone();
    let waiter = tokio::spawn(async move {
        waiting.cancelled().await;
    });

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!waiter.is_finished(), "must keep waiting until cancelled");

    cancel.cancel();

    tokio::time::timeout(Duration::from_secs(2), waiter).await.unwrap().unwrap();
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn an_already_cancelled_signal_completes_at_once_and_can_be_repeated() {
    let cancel = Cancel::default();
    cancel.cancel();
    cancel.cancel();

    tokio::time::timeout(Duration::from_millis(500), cancel.cancelled()).await.unwrap();
    tokio::time::timeout(Duration::from_millis(500), cancel.cancelled()).await.unwrap();
}

// ---- Retry-After and friends -------------------------------------------------------------

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    map
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 21, 7, 28, 0).unwrap()
}

#[test]
fn retry_after_in_seconds() {
    assert_eq!(
        wait_hint(&headers(&[("retry-after", "120")]), now()),
        Some(Duration::from_secs(120))
    );
    assert_eq!(wait_hint(&headers(&[("Retry-After", " 7 ")]), now()), Some(Duration::from_secs(7)));
    assert_eq!(wait_hint(&headers(&[("retry-after", "0")]), now()), Some(Duration::ZERO));
}

#[test]
fn retry_after_as_an_http_date() {
    let in_90_seconds = headers(&[("retry-after", "Wed, 21 Oct 2026 07:29:30 GMT")]);
    assert_eq!(wait_hint(&in_90_seconds, now()), Some(Duration::from_secs(90)));

    let already_past = headers(&[("retry-after", "Wed, 21 Oct 2026 07:00:00 GMT")]);
    assert_eq!(wait_hint(&already_past, now()), Some(Duration::ZERO));
}

#[test]
fn ratelimit_reset_headers() {
    assert_eq!(
        wait_hint(&headers(&[("ratelimit-reset", "45")]), now()),
        Some(Duration::from_secs(45))
    );

    // a small number is "seconds from now" ...
    assert_eq!(
        wait_hint(&headers(&[("x-ratelimit-reset", "30")]), now()),
        Some(Duration::from_secs(30))
    );

    // ... a ten-digit one is a Unix timestamp
    let at = now().timestamp() + 600;
    assert_eq!(
        wait_hint(&headers(&[("x-ratelimit-reset", &at.to_string())]), now()),
        Some(Duration::from_secs(600))
    );
    let stale = now().timestamp() - 600;
    assert_eq!(
        wait_hint(&headers(&[("x-ratelimit-reset", &stale.to_string())]), now()),
        Some(Duration::ZERO)
    );
}

#[test]
fn retry_after_wins_over_the_other_headers() {
    let both =
        headers(&[("retry-after", "10"), ("ratelimit-reset", "99"), ("x-ratelimit-reset", "50")]);

    assert_eq!(wait_hint(&both, now()), Some(Duration::from_secs(10)));
}

#[test]
fn unreadable_hints_are_ignored() {
    assert_eq!(wait_hint(&HeaderMap::new(), now()), None);
    assert_eq!(wait_hint(&headers(&[("retry-after", "soon")]), now()), None);
    assert_eq!(wait_hint(&headers(&[("retry-after", "-5")]), now()), None);
    assert_eq!(wait_hint(&headers(&[("ratelimit-reset", "never")]), now()), None);

    // an unreadable Retry-After does not hide a good fallback
    let fallback = headers(&[("retry-after", "soon"), ("ratelimit-reset", "20")]);
    assert_eq!(wait_hint(&fallback, now()), Some(Duration::from_secs(20)));
}
