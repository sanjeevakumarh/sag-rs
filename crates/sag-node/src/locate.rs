//! Controller discovery + leader election over a known peer list.
//!
//! A node (and the client) probes every configured `controller.peers` address for
//! a live controller, then picks the **leader**: `min` by `(start_epoch, latency)`
//! with the URL as a final stable tiebreak so the choice always converges. If no
//! controller answers, an eligible node self-promotes (see `main.rs`).

use std::time::{Duration, Instant};

use sag_proto::StatusResponse;
use tokio::task::JoinSet;

/// A reachable controller and what we learned probing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Normalized base URL (no trailing slash).
    pub url: String,
    pub status: StatusResponse,
    /// Round-trip latency of the `/status` probe, in milliseconds.
    pub latency_ms: u64,
}

/// Probe every peer concurrently and return the elected leader, or `None` if no
/// controller is reachable.
pub async fn discover_leader(http: &reqwest::Client, peers: &[String]) -> Option<Candidate> {
    let mut set = JoinSet::new();
    for peer in peers {
        let http = http.clone();
        let url = peer.trim_end_matches('/').to_string();
        set.spawn(async move { probe(&http, url).await });
    }
    let mut candidates = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some(candidate)) = joined {
            candidates.push(candidate);
        }
    }
    elect(candidates)
}

/// Probe one peer's `/status`, timing the round trip. `None` if it doesn't answer.
async fn probe(http: &reqwest::Client, url: String) -> Option<Candidate> {
    let started = Instant::now();
    let resp = http
        .get(format!("{url}/status"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let status: StatusResponse = resp.json().await.ok()?;
    Some(Candidate {
        url,
        status,
        latency_ms,
    })
}

/// Pick the leader from reachable candidates: smallest `(start_epoch, latency)`,
/// then smallest URL as a deterministic final tiebreak. Pure — unit-tested.
pub fn elect(mut candidates: Vec<Candidate>) -> Option<Candidate> {
    candidates.sort_by(|a, b| {
        a.status
            .election_key(a.latency_ms)
            .cmp(&b.status.election_key(b.latency_ms))
            .then_with(|| a.url.cmp(&b.url))
    });
    candidates.into_iter().next()
}

/// Should a self-promoted controller (`my_epoch` / `my_url`) step down for
/// `remote`? Compared by `(start_epoch, url)` only — **not** latency: an observer
/// measures its own latency as ~0, which would make every controller think itself
/// best and split-brain forever. The stable `(epoch, url)` order guarantees every
/// controller independently agrees on the single winner and the losers step down.
pub fn remote_outranks_self(remote: &Candidate, my_epoch: u64, my_url: &str) -> bool {
    (remote.status.start_epoch, remote.url.as_str()) < (my_epoch, my_url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sag_proto::PROTOCOL_VERSION;

    fn candidate(url: &str, start_epoch: u64, latency_ms: u64) -> Candidate {
        Candidate {
            url: url.into(),
            status: StatusResponse {
                protocol_version: PROTOCOL_VERSION,
                start_epoch,
            },
            latency_ms,
        }
    }

    #[test]
    fn elect_none_when_empty() {
        assert!(elect(vec![]).is_none());
    }

    #[test]
    fn elect_prefers_earliest_start() {
        let winner = elect(vec![
            candidate("http://b", 200, 1),
            candidate("http://a", 100, 500), // earlier start, worse latency — still wins
        ])
        .unwrap();
        assert_eq!(winner.url, "http://a");
    }

    #[test]
    fn elect_breaks_start_ties_by_latency_then_url() {
        let winner = elect(vec![
            candidate("http://z", 100, 5),
            candidate("http://a", 100, 5), // same start+latency → URL tiebreak
            candidate("http://m", 100, 50),
        ])
        .unwrap();
        assert_eq!(winner.url, "http://a");
    }

    #[test]
    fn step_down_only_for_strictly_better_controller() {
        // Older remote → step down.
        assert!(remote_outranks_self(
            &candidate("http://a", 100, 9),
            200,
            "http://me"
        ));
        // Newer remote → keep leadership.
        assert!(!remote_outranks_self(
            &candidate("http://a", 300, 0),
            200,
            "http://me"
        ));
        // Same epoch → URL breaks it, and it is symmetric: exactly one side yields.
        assert!(remote_outranks_self(
            &candidate("http://a", 200, 5),
            200,
            "http://b"
        ));
        assert!(!remote_outranks_self(
            &candidate("http://b", 200, 5),
            200,
            "http://a"
        ));
    }
}
