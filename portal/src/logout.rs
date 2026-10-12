//! Logout & single sign-out (SSO / SLO) for the portal.
//!
//! `POST /api/logout` clears the portal session cookie and redirects the
//! browser to the IdP's OIDC end-session endpoint (RP-initiated logout,
//! [OIDC RP-Initiated Logout 1.0]). The IdP resolves the session from the
//! `id_token_hint` (the ID token the portal session carries) and terminates
//! it. Because every app on the stack — including OpenCloud — relies on that
//! shared IdP session, terminating it signs the user out everywhere: single
//! log-out from the portal reaches OpenCloud, no extra per-app calls needed.
//!
//! The portal session cookie is only ever written by the sign-in side; this
//! module owns its removal. When no IdP is configured (pure-local portal) the
//! handler still clears the local session and lands on `/logged-out`.

use axum::{
    extract::State,
    http::{
        header::{COOKIE, LOCATION, SET_COOKIE},
        HeaderMap, StatusCode,
    },
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::AppConfig;

/// Portal session cookie: holds the OIDC ID token minted for the signed-in
/// user (the value the IdP needs as `id_token_hint` to terminate the right
/// session). Cleared on logout.
pub const SESSION_COOKIE: &str = "OPENSMESESSION";

/// GET /logged-out — where the IdP sends the browser after end-session.
pub async fn handle_logged_out() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        r#"<!DOCTYPE html>
<html lang="de">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>Abgemeldet — openSME</title>
<style>
  body { font-family: system-ui, sans-serif; max-width: 30rem; margin: 4rem auto; padding: 0 1rem; line-height: 1.5; }
  h1 { font-size: 1.4rem; }
  a { color: #1a5fb4; }
</style>
</head>
<body>
  <main>
    <h1>Du bist abgemeldet.</h1>
    <p>Deine Sitzung wurde beendet — auch die anderen Dienste sind jetzt abgemeldet.</p>
    <p><a href="/">Zurück zum Portal</a></p>
  </main>
</body>
</html>"#,
    )
}

/// POST /api/logout — end the portal session and the shared IdP session.
pub async fn handle_logout(State(config): State<Arc<AppConfig>>, headers: HeaderMap) -> Response {
    let id_token = cookie_value(&headers, SESSION_COOKIE).filter(|v| !v.is_empty());
    let target = if config.idp_url.trim().is_empty() {
        // No IdP wired (local-only portal): still end the local session.
        "/logged-out".to_string()
    } else {
        logout_url(&config.idp_url, &config.portal_domain, id_token.as_deref())
    };

    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(LOCATION, target)
        .header(
            SET_COOKIE,
            format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=0"),
        )
        .body(axum::body::Body::empty())
        .expect("static logout response is always valid")
}

/// Build the IdP RP-initiated logout URL. The portal always names its own
/// `/logged-out` page as `post_logout_redirect_uri` — registered at the IdP
/// for the portal's OIDC client (see scripts/zitadel-client.sh).
fn logout_url(idp_url: &str, portal_domain: &str, id_token: Option<&str>) -> String {
    let base = idp_url.trim_end_matches('/');
    let mut out = format!("{base}/oidc/v1/end_session?");
    if let Some(tok) = id_token {
        out.push_str("id_token_hint=");
        out.push_str(&urlencode(tok));
        out.push('&');
    }
    out.push_str("post_logout_redirect_uri=");
    out.push_str(&urlencode(&format!("https://{portal_domain}/logged-out")));
    out
}

/// Percent-encode one query component. JWTs (id_token_hint) only contain
/// unreserved base64url chars and pass through untouched; URLs get their
/// `:/?#&` reserved chars escaped so the IdP sees one clean parameter.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Read a single cookie by name from the request's Cookie header.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(COOKIE)?.to_str().ok()?;
    for part in header.split(';') {
        let (key, value) = part.trim().split_once('=')?;
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn config(idp_url: &str, portal_domain: &str) -> AppConfig {
        AppConfig {
            idp_url: idp_url.into(),
            portal_domain: portal_domain.into(),
            ..crate::tests::cfg_with("", None, "")
        }
    }

    fn endpoint(config: &AppConfig, cookie: Option<&str>) -> Response {
        let mut builder = Request::builder().method("POST").uri("/api/logout");
        if let Some(c) = cookie {
            builder = builder.header(COOKIE, c);
        }
        let req = builder.body(Body::empty()).unwrap();
        let router = crate::build_router(Arc::new(config.clone()));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(router.oneshot(req))
            .unwrap()
    }

    #[test]
    fn urlencode_escapes_reserved_chars_only() {
        assert_eq!(urlencode("abcXYZ_-.~09"), "abcXYZ_-.~09");
        assert_eq!(
            urlencode("https://a/x?y=1&z=2"),
            "https%3A%2F%2Fa%2Fx%3Fy%3D1%26z%3D2"
        );
        assert_eq!(urlencode("+=ä"), "%2B%3D%C3%A4");
    }

    #[test]
    fn cookie_value_finds_named_cookie() {
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            "other=1; OPENSMESESSION=abc.def.ghi; foo=bar"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            cookie_value(&h, SESSION_COOKIE).as_deref(),
            Some("abc.def.ghi")
        );
        h.remove(COOKIE);
        assert_eq!(cookie_value(&h, SESSION_COOKIE), None);
    }

    #[test]
    fn logout_url_carries_token_and_portal_callback() {
        let url = logout_url(
            "https://auth.example",
            "portal.example",
            Some("eyJ.tok.sig"),
        );
        assert_eq!(
            url,
            "https://auth.example/oidc/v1/end_session?id_token_hint=eyJ.tok.sig&\
             post_logout_redirect_uri=https%3A%2F%2Fportal.example%2Flogged-out"
        );
    }

    #[test]
    fn logout_url_omits_hint_without_token() {
        let url = logout_url("https://auth.example/", "portal.example", None);
        assert_eq!(
            url,
            "https://auth.example/oidc/v1/end_session?\
             post_logout_redirect_uri=https%3A%2F%2Fportal.example%2Flogged-out"
        );
    }

    #[test]
    fn logout_clears_cookie_and_redirects_to_end_session() {
        let cfg = config("https://auth.example", "portal.example");
        let res = endpoint(&cfg, Some("OPENSMESESSION=eyJ.tok.sig"));
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let loc = res.headers().get(LOCATION).unwrap().to_str().unwrap();
        assert!(loc.starts_with("https://auth.example/oidc/v1/end_session?"));
        assert!(loc.contains("id_token_hint=eyJ.tok.sig"));
        assert!(loc.contains("post_logout_redirect_uri=https%3A%2F%2Fportal.example%2Flogged-out"));
        let set_cookie = res.headers().get(SET_COOKIE).unwrap().to_str().unwrap();
        assert!(set_cookie.starts_with("OPENSMESESSION=;"));
        assert!(set_cookie.contains("Max-Age=0"));
    }

    #[test]
    fn logout_without_idp_redirects_local() {
        let cfg = config("", "portal.example");
        let res = endpoint(&cfg, Some("OPENSMESESSION=tok"));
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers().get(LOCATION).unwrap(), "/logged-out");
        assert!(res
            .headers()
            .get(SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("OPENSMESESSION=;"));
    }

    #[test]
    fn logged_out_page_served() {
        let cfg = config("https://auth.example", "portal.example");
        let req = Request::builder()
            .method("GET")
            .uri("/logged-out")
            .body(Body::empty())
            .unwrap();
        let router = crate::build_router(Arc::new(cfg));
        let res = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(router.oneshot(req))
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
