//! Prometheus metrics for the portal.
//!
//! A single process-wide registry, scraped by Prometheus at `/metrics`
//! (see `handle_metrics` in main.rs). Everything is wired through
//! [`global()`] so handlers never construct or pass metrics around.

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::sync::OnceLock;

/// All portal metrics, registered exactly once per process.
pub struct Metrics {
    registry: Registry,
    http_requests_total: IntCounterVec,
    http_request_duration_seconds: HistogramVec,
    active_sessions: IntGauge,
    intercom_messages_total: IntCounter,
    sso_logins_total: IntCounter,
    sso_login_failures_total: IntCounter,
}

static GLOBAL: OnceLock<Metrics> = OnceLock::new();

/// Shared process-wide metrics (created lazily on first use).
pub fn global() -> &'static Metrics {
    GLOBAL.get_or_init(Metrics::new)
}

impl Metrics {
    fn new() -> Metrics {
        let registry = Registry::new();

        let http_requests_total = IntCounterVec::new(
            Opts::new(
                "http_requests_total",
                "Total number of HTTP requests handled by the portal.",
            ),
            &["method", "route", "status"],
        )
        .expect("metrics: build http_requests_total");

        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request latency in seconds.",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["method", "route"],
        )
        .expect("metrics: build http_request_duration_seconds");

        let active_sessions =
            IntGauge::new("active_sessions", "Number of currently active sessions.")
                .expect("metrics: build active_sessions");
        let intercom_messages_total = IntCounter::new(
            "intercom_messages_total",
            "Total number of intercom messages posted through the portal.",
        )
        .expect("metrics: build intercom_messages_total");
        let sso_logins_total =
            IntCounter::new("sso_logins_total", "Total number of successful SSO logins.")
                .expect("metrics: build sso_logins_total");
        let sso_login_failures_total = IntCounter::new(
            "sso_login_failures_total",
            "Total number of failed SSO logins.",
        )
        .expect("metrics: build sso_login_failures_total");

        let metrics = Metrics {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            active_sessions,
            intercom_messages_total,
            sso_logins_total,
            sso_login_failures_total,
        };
        metrics.register_all();
        metrics
    }

    fn register_all(&self) {
        for metric in [
            Box::new(self.http_requests_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(self.http_request_duration_seconds.clone()),
            Box::new(self.active_sessions.clone()),
            Box::new(self.intercom_messages_total.clone()),
            Box::new(self.sso_logins_total.clone()),
            Box::new(self.sso_login_failures_total.clone()),
        ] {
            self.registry
                .register(metric)
                .expect("metrics: register collector");
        }
    }

    /// Record one served HTTP request: a counter increment labelled by
    /// method / matched route / status code, plus a latency observation.
    pub fn record_request(&self, method: &str, route: &str, status: u16, duration_secs: f64) {
        let status = status.to_string();
        self.http_requests_total
            .with_label_values(&[method, route, &status])
            .inc();
        self.http_request_duration_seconds
            .with_label_values(&[method, route])
            .observe(duration_secs);
    }

    /// Count one intercom message persisted by the portal.
    pub fn inc_intercom_message(&self) {
        self.intercom_messages_total.inc();
    }

    // The portal itself does not observe SSO login outcomes (Zitadel does)
    // nor track sessions today, so these stay at zero until a hook exists.
    // Kept as public API so the future wiring is a one-line call.
    #[allow(dead_code)]
    pub fn set_active_sessions(&self, n: i64) {
        self.active_sessions.set(n);
    }

    #[allow(dead_code)]
    pub fn inc_sso_login(&self) {
        self.sso_logins_total.inc();
    }

    #[allow(dead_code)]
    pub fn inc_sso_login_failure(&self) {
        self.sso_login_failures_total.inc();
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render(&self) -> prometheus::Result<String> {
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        encoder.encode(&self.registry.gather(), &mut buf)?;
        Ok(String::from_utf8(buf).expect("prometheus text format is UTF-8"))
    }
}
