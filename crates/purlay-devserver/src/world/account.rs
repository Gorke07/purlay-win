//! Sessions, native sign-in codes, the person, presence, Deep Work and the socket registry.

use super::*;
use crate::ids::{new_id, random_hex};
use base64::Engine;
use purlay_contract::constants::{DEEP_WORK_MAX_HOURS, close_codes};
use sha2::{Digest, Sha256};

/// Native codes live two minutes and burn after five wrong verifiers.
const NATIVE_CODE_TTL_MS: i64 = 120_000;
const NATIVE_CODE_WRONG_MAX: u32 = 5;
const NATIVE_CODES_PER_PERSON: usize = 5;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: Uuid,
    pub user: Uuid,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn matches_pattern(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl World {
    // --- sessions ------------------------------------------------------------------------

    /// Makes a session for a person and answers its cookie value.
    pub fn create_session(&mut self, user: &str, kind: DeviceSessionKind, name: Option<String>) -> String {
        let now = now_ms();
        let token = random_hex(32);
        let row = SessionRow {
            id: new_id(),
            token: token.clone(),
            user: Uuid::new(user),
            kind,
            name,
            created_ms: now,
            last_used_ms: now,
            expires_ms: now + SESSION_MAX_AGE_S * 1000,
        };
        self.session_by_token.insert(token.clone(), row.id.clone());
        self.sessions.insert(row.id.clone(), row);
        self.last_active_user = Some(Uuid::new(user));
        token
    }

    /// The live session a cookie value names.
    pub fn session_for_token(&mut self, token: &str) -> Option<SessionInfo> {
        let id = self.session_by_token.get(token)?.clone();
        let now = now_ms();
        let row = self.sessions.get_mut(&id)?;
        if row.expires_ms <= now {
            return None;
        }
        // Precise to the hour on the wire; kept exact here.
        row.last_used_ms = now;
        Some(SessionInfo { session_id: row.id.clone(), user: row.user.clone() })
    }

    /// Ends a session: its sockets are told `SESSION_ENDED` and closed with 4401.
    pub fn end_session(&mut self, session_id: &str, reason: SessionEndedFrameDataReason) {
        let Some(row) = self.sessions.remove(session_id) else { return };
        self.session_by_token.remove(&row.token);
        let frame = ServerFrame::SessionEnded(SessionEndedFrame {
            data: SessionEndedFrameData { reason },
            ts: now_ts(),
            ref_id: None,
        });
        let text = frame_text(&frame);
        let ids: Vec<u64> = self.sockets.iter().filter(|(_, s)| s.session_id.as_str() == session_id).map(|(k, _)| *k).collect();
        for id in ids {
            if let Some(s) = self.sockets.get(&id) {
                let _ = s.tx.send(Out::Text(text.clone()));
                let _ = s.tx.send(Out::Close(close_codes::SESSION_ENDED, "session ended".into()));
            }
            self.unregister_socket(id);
        }
    }

    pub fn end_all_sessions(&mut self, user: &str) {
        let ids: Vec<Uuid> = self.sessions.values().filter(|s| s.user.as_str() == user).map(|s| s.id.clone()).collect();
        for id in ids {
            self.end_session(&id, SessionEndedFrameDataReason::SignedOut);
        }
    }

    pub fn list_sessions(&self, info: &SessionInfo) -> DeviceSessionList {
        let now = now_ms();
        let mut rows: Vec<&SessionRow> =
            self.sessions.values().filter(|s| s.user == info.user && s.expires_ms > now).collect();
        rows.sort_by_key(|s| std::cmp::Reverse(s.last_used_ms));
        DeviceSessionList {
            data: rows
                .into_iter()
                .map(|s| DeviceSession {
                    id: s.id.clone(),
                    kind: s.kind.clone(),
                    name: s.name.clone(),
                    created_at: ts(s.created_ms),
                    last_used_at: ts(s.last_used_ms / 3_600_000 * 3_600_000),
                    expires_at: ts(s.expires_ms),
                    current: s.id == info.session_id,
                })
                .collect(),
        }
    }

    pub fn end_my_session(&mut self, info: &SessionInfo, session_id: &str) -> AppResult<()> {
        if !crate::ids::is_uuid(session_id) {
            return Err(AppError::field("sessionId", "is not a UUID"));
        }
        match self.sessions.get(session_id) {
            Some(s) if s.user == info.user => {
                self.end_session(session_id, SessionEndedFrameDataReason::SignedOut);
                Ok(())
            }
            _ => Err(AppError::new("SESSION_NOT_FOUND", "No such session.")),
        }
    }

    // --- native sign-in ------------------------------------------------------------------

    /// `POST /api/native/authorize`: a code for the signed-in person, bound to the challenge.
    pub fn native_authorize(&mut self, user: &str, challenge: &str, state: &str) -> AppResult<String> {
        if challenge.len() != 43 || !matches_pattern(challenge, 43, 43) {
            return Err(AppError::field("challenge", "is not a base64url S256 challenge"));
        }
        if !matches_pattern(state, 16, 128) {
            return Err(AppError::field("state", "is not 16 to 128 base64url characters"));
        }
        let now = now_ms();
        self.codes.retain(|_, c| now - c.created_ms < NATIVE_CODE_TTL_MS);
        let mine: Vec<(String, i64)> =
            self.codes.iter().filter(|(_, c)| c.user.as_str() == user).map(|(k, c)| (k.clone(), c.created_ms)).collect();
        if mine.len() >= NATIVE_CODES_PER_PERSON
            && let Some((oldest, _)) = mine.iter().min_by_key(|(_, at)| *at) {
                self.codes.remove(oldest);
            }
        let code = random_hex(32);
        self.codes.insert(
            code.clone(),
            NativeCode {
                challenge: challenge.to_string(),
                state: state.to_string(),
                user: Uuid::new(user),
                created_ms: now,
                wrong: 0,
            },
        );
        Ok(format!("{}?code={code}&state={state}", purlay_contract::constants::NATIVE_REDIRECT))
    }

    /// `POST /api/native/token`: the code and the verifier → a native session's cookie.
    pub fn native_token(&mut self, code: &str, verifier: &str, device_name: &str, client: Option<&str>) -> AppResult<String> {
        let refuse = || AppError::new("NATIVE_CODE_NOT_FOUND", "The sign-in code is not valid. Start again.");
        let now = now_ms();
        let Some(entry) = self.codes.get_mut(code) else { return Err(refuse()) };
        if now - entry.created_ms >= NATIVE_CODE_TTL_MS || !self.people.contains_key(&entry.user) {
            self.codes.remove(code);
            return Err(refuse());
        }
        let digest = b64url(&Sha256::digest(verifier.as_bytes()));
        if !matches_pattern(verifier, 43, 128) || digest != entry.challenge {
            entry.wrong += 1;
            if entry.wrong >= NATIVE_CODE_WRONG_MAX {
                self.codes.remove(code);
            }
            return Err(refuse());
        }
        let user = entry.user.clone();
        self.codes.remove(code);
        let name = device_name.trim();
        let bad_char = |c: char| {
            let u = c as u32;
            u <= 0x1f
                || (0x7f..=0x9f).contains(&u)
                || (0x200b..=0x200f).contains(&u)
                || (0x2028..=0x202e).contains(&u)
                || (0x2066..=0x2069).contains(&u)
                || u == 0xfeff
        };
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(bad_char) {
            return Err(AppError::field("device.name", "is not a device name"));
        }
        let version = client
            .and_then(|c| c.split(' ').next())
            .and_then(|p| p.split_once('/'))
            .map(|(_, v)| v.to_string())
            .unwrap_or_else(|| "unknown".into());
        let label = format!("Purlay for Windows/{version} ({name})");
        Ok(self.create_session(&user, DeviceSessionKind::Native, Some(label)))
    }

    // --- the person ----------------------------------------------------------------------

    pub fn me(&self, user: &str) -> AppResult<MeResponse> {
        let p = self.people.get(user).ok_or_else(AppError::unauthenticated)?;
        Ok(MeResponse {
            user: CurrentUser {
                id: p.id.clone(),
                username: p.username.clone(),
                display_name: p.display_name.clone(),
                avatar_url: None,
                email: p.email.clone(),
            },
            profile: Self::profile_view(p),
            memberships: self.membership_view(user).into_iter().collect(),
            may_create_workspace: false,
            is_server_admin: p.role == UserRole::Owner,
            name_from_git_hub: false,
            must_enrol_two_factor: false,
            scheduled_deletions: Vec::new(),
            modules: vec![ModuleId::Chat, ModuleId::Issues],
            away_chosen: self.away_chosen.contains(user),
        })
    }

    pub fn members(&self) -> WorkspaceMemberList {
        self.people
            .values()
            .map(|p| WorkspaceMember {
                user: self.user_view(&p.id),
                role: p.role.clone(),
                joined_at: ts(p.joined_ms),
                profile: Self::profile_view(p),
            })
            .collect()
    }

    // --- presence ------------------------------------------------------------------------

    fn is_online(&self, user: &str) -> bool {
        self.people.get(user).is_some_and(|p| p.always_online) || self.sockets.values().any(|s| s.user.as_str() == user)
    }

    /// Recomputes a person's presence and, when it changed, tells the workspace.
    pub fn refresh_presence(&mut self, user: &str, force: bool) {
        let online = self.is_online(user);
        let away = self.away_chosen.contains(user);
        let row = self.presence.entry(Uuid::new(user)).or_default();
        if !force && row.online == online && row.away == away && row.version > 0 {
            return;
        }
        self.presence_version += 1;
        row.version = self.presence_version;
        row.online = online;
        row.away = away;
        let change = PresenceChange { user_id: Uuid::new(user), online, away, version: row.version, chosen: None };
        let others = ServerFrame::Presence(PresenceFrame {
            data: PresenceFrameData { changes: vec![change.clone()] },
            ts: now_ts(),
            ref_id: None,
        });
        let own = ServerFrame::Presence(PresenceFrame {
            data: PresenceFrameData { changes: vec![PresenceChange { chosen: Some(away), ..change }] },
            ts: now_ts(),
            ref_id: None,
        });
        let (own_text, other_text) = (frame_text(&own), frame_text(&others));
        for s in self.sockets.values() {
            let _ = s.tx.send(Out::Text(if s.user.as_str() == user { own_text.clone() } else { other_text.clone() }));
        }
    }

    pub fn presence_snapshot(&self) -> PresenceSnapshot {
        let online = self
            .people
            .keys()
            .filter(|id| self.is_online(id))
            .map(|id| {
                let row = self.presence.get(id).cloned().unwrap_or_default();
                PresenceChange {
                    user_id: id.clone(),
                    online: true,
                    away: self.away_chosen.contains(id),
                    version: row.version,
                    chosen: None,
                }
            })
            .collect();
        PresenceSnapshot { online, version: self.presence_version }
    }

    pub fn set_away(&mut self, user: &str, away: bool) -> SetAwayResponse {
        if away {
            self.away_chosen.insert(Uuid::new(user));
        } else {
            self.away_chosen.remove(user);
        }
        self.refresh_presence(user, true);
        SetAwayResponse { away }
    }

    pub fn set_deep_work(&mut self, user: &str, req: &SetDeepWorkRequest) -> AppResult<DeepWorkResponse> {
        let until = match req {
            SetDeepWorkRequest::ActiveOn(on) => {
                let ms = on.until.unix_ms().ok_or_else(|| AppError::field("until", "is not a time"))?;
                let now = now_ms();
                if ms <= now || ms > now + DEEP_WORK_MAX_HOURS * 3_600_000 {
                    return Err(AppError::field("until", "must be ahead, at most 24 hours"));
                }
                self.deep_work.insert(Uuid::new(user), ms);
                Some(ms)
            }
            SetDeepWorkRequest::ActiveOff(_) => {
                self.deep_work.remove(user);
                None
            }
            SetDeepWorkRequest::Unknown(_) => return Err(AppError::field("active", "is required")),
        };
        let frame = ServerFrame::DeepWorkUpdated(DeepWorkUpdatedFrame {
            data: DeepWorkUpdatedFrameData { workspace_id: self.workspace.id.clone(), until: until.map(ts) },
            ts: now_ts(),
            ref_id: None,
        });
        self.publish_user(user, &frame);
        Ok(DeepWorkResponse { until: until.map(ts) })
    }

    // --- sockets -------------------------------------------------------------------------

    pub fn register_socket(&mut self, info: &SessionInfo, tx: UnboundedSender<Out>) -> u64 {
        let id = self.next_socket;
        self.next_socket += 1;
        self.sockets.insert(
            id,
            Socket { user: info.user.clone(), session_id: info.session_id.clone(), subs: HashSet::new(), tx },
        );
        self.last_active_user = Some(info.user.clone());
        self.refresh_presence(&info.user.clone(), false);
        id
    }

    pub fn unregister_socket(&mut self, id: u64) {
        if let Some(s) = self.sockets.remove(&id) {
            self.refresh_presence(&s.user, false);
        }
    }

    /// Whom the live bot talks to: the person last seen signing in or connecting.
    pub fn audience(&self) -> Uuid {
        self.last_active_user.clone().unwrap_or_else(|| self.default_user.clone())
    }
}
