//! The browser half of native sign-in (D-191), made up for development: instead of a real
//! sign-in, a page lists the seeded people and signs in as whoever is picked, then does
//! what `POST /api/native/authorize` does and sends the browser to `purlay://auth?…`.

use crate::http::Shared;
use crate::world::World;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use purlay_contract::models::DeviceSessionKind;
use serde::Deserialize;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

const STYLE: &str = r#"
<style>
  body { font: 15px/1.5 "Segoe UI", system-ui, sans-serif; background: #f4f5f8; color: #1d2230; margin: 0; }
  main { max-width: 440px; margin: 64px auto; background: #fff; border-radius: 12px; padding: 28px 32px;
         box-shadow: 0 8px 30px rgba(20, 30, 60, .08); }
  h1 { font-size: 20px; margin: 0 0 4px; }
  p.muted { color: #667085; margin: 0 0 20px; }
  form { margin: 0 0 10px; }
  button { width: 100%; display: flex; align-items: center; gap: 12px; padding: 10px 14px; font: inherit;
           border: 1px solid #d9dde7; border-radius: 8px; background: #fff; cursor: pointer; text-align: left; }
  button:hover { background: #f0f3ff; border-color: #8ea0ff; }
  .avatar { width: 32px; height: 32px; border-radius: 50%; background: #4f5bd5; color: #fff; display: grid;
            place-items: center; font-weight: 600; flex: none; }
  .who small { display: block; color: #667085; }
  code { background: #f0f2f6; padding: 2px 6px; border-radius: 4px; word-break: break-all; }
  a.open { display: inline-block; margin-top: 12px; padding: 10px 16px; border-radius: 8px; background: #4f5bd5;
           color: #fff; text-decoration: none; }
</style>
"#;

fn page_html(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{}</title>{STYLE}</head><body><main>{body}</main></body></html>",
        esc(title)
    ))
}

fn valid(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Deserialize)]
pub struct NativeParams {
    #[serde(default)]
    challenge: String,
    #[serde(default)]
    state: String,
}

fn people_buttons(w: &World, challenge: &str, state: &str) -> String {
    let mut people: Vec<_> = w.people.values().collect();
    // The default person first, the bot last.
    people.sort_by_key(|p| (p.id != w.default_user, Some(&p.id) == w.bot.as_ref(), p.display_name.clone()));
    people
        .iter()
        .map(|p| {
            let initial = p.display_name.chars().next().unwrap_or('?');
            format!(
                r#"<form method="post" action="/sign-in/native/choose">
  <input type="hidden" name="challenge" value="{c}"><input type="hidden" name="state" value="{s}">
  <input type="hidden" name="user" value="{u}">
  <button type="submit"><span class="avatar">{i}</span><span class="who">Sign in as {n}<small>@{u} · {t}</small></span></button>
</form>"#,
                c = esc(challenge),
                s = esc(state),
                u = esc(&p.username),
                i = esc(&initial.to_string()),
                n = esc(&p.display_name),
                t = esc(p.title.as_deref().unwrap_or("")),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `GET /sign-in/native?challenge=…&state=…`
pub async fn page(State(app): State<Shared>, Query(q): Query<NativeParams>) -> Response {
    if !valid(&q.challenge, 43, 43) || !valid(&q.state, 16, 128) {
        let body = page_html(
            "Purlay dev server",
            "<h1>This sign-in link is not complete</h1><p class=\"muted\">Start signing in again from the Purlay app.</p>",
        );
        return (StatusCode::BAD_REQUEST, body).into_response();
    }
    if let Some(user) = app.auto_sign_in.clone() {
        let form = serde_urlencoded::to_string([("challenge", q.challenge.as_str()), ("state", q.state.as_str()), ("user", user.as_str())]).unwrap_or_default();
        return choose(State(app), Bytes::from(form)).await;
    }
    let w = app.world.lock();
    let body = format!(
        "<h1>Sign in to {}</h1><p class=\"muted\">Purlay dev server — pick who you are. No password needed.</p>{}",
        esc(&w.workspace.name),
        people_buttons(&w, &q.challenge, &q.state)
    );
    page_html("Sign in — Purlay dev server", &body).into_response()
}

#[derive(Deserialize)]
struct Choice {
    challenge: String,
    state: String,
    user: String,
}

/// `POST /sign-in/native/choose`: signs the browser in as the person picked, mints the
/// native code exactly as `POST /api/native/authorize` does, and opens `purlay://auth`.
pub async fn choose(State(app): State<Shared>, body: Bytes) -> Response {
    let Ok(choice) = serde_urlencoded::from_bytes::<Choice>(&body) else {
        return (StatusCode::BAD_REQUEST, page_html("Purlay dev server", "<h1>That did not work</h1>")).into_response();
    };
    let mut w = app.world.lock();
    let Some(person) = w.person_by_username(&choice.user).cloned() else {
        return (StatusCode::NOT_FOUND, page_html("Purlay dev server", "<h1>No such person</h1>")).into_response();
    };
    // The browser's own session, as a real web sign-in would make.
    let browser_cookie = w.create_session(&person.id, DeviceSessionKind::Browser, None);
    let redirect = match w.native_authorize(&person.id, &choice.challenge, &choice.state) {
        Ok(r) => r,
        Err(e) => {
            let body = format!("<h1>That did not work</h1><p class=\"muted\">{}</p>", esc(&e.message));
            return (StatusCode::BAD_REQUEST, page_html("Purlay dev server", &body)).into_response();
        }
    };
    drop(w);
    tracing::info!("native sign-in as @{}", person.username);
    let body = format!(
        r#"<h1>Signed in as {name}</h1>
<p class="muted">Your browser should now offer to open Purlay. If nothing happens, use the button.</p>
<a class="open" href="{href}">Open Purlay</a>
<p class="muted" style="margin-top:20px">Or paste this address into the app: <code>{href}</code></p>
<script>window.location.href = {js};</script>"#,
        name = esc(&person.display_name),
        href = esc(&redirect),
        js = serde_json::to_string(&redirect).unwrap_or_default(),
    );
    let mut res = page_html("Signed in — Purlay dev server", &body).into_response();
    // Not `__Host-`: browsers keep those only over https.
    if let Ok(v) = format!("purlay_dev_session={browser_cookie}; Path=/; HttpOnly; SameSite=Lax").parse() {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

/// `GET /`: what this is and how to use it.
pub async fn index(State(app): State<Shared>) -> Response {
    let w = app.world.lock();
    let people: String = w
        .people
        .values()
        .map(|p| format!("<li><b>{}</b> @{} — {}</li>", esc(&p.display_name), esc(&p.username), esc(p.role.as_str())))
        .collect();
    let body = format!(
        r#"<h1>Purlay dev server</h1>
<p class="muted">An in-memory fake of the Purlay server (contract 0.6.1). Point the app at <code>{origin}</code>.</p>
<p>Workspace <b>{ws}</b> (<code>{slug}</code>), people:</p><ul>{people}</ul>"#,
        origin = esc(&app.origin),
        ws = esc(&w.workspace.name),
        slug = esc(&w.workspace.slug),
    );
    page_html("Purlay dev server", &body).into_response()
}
