//! The demo workspace: "Acme", five people, a handful of channels and a few days of talk.

use crate::ids::{id_at, now_ms};
use crate::world::chat::PostInput;
use crate::world::{Chan, Member, Person, World, WorkspaceRow};
use purlay_contract::Uuid;
use purlay_contract::models::*;
use std::collections::{BTreeMap, BTreeSet, HashSet};

const HOUR: i64 = 3_600_000;
const MINUTE: i64 = 60_000;

/// The people of the seed: (username, display name, role, title, time zone).
pub const PEOPLE: &[(&str, &str, &str, &str, &str)] = &[
    ("ada", "Ada Lovelace", "owner", "Engineering lead", "Europe/London"),
    ("grace", "Grace Hopper", "admin", "Platform engineer", "America/New_York"),
    ("linus", "Linus Okafor", "member", "Backend developer", "Europe/Berlin"),
    ("mira", "Mira Chen", "member", "Product designer", "Asia/Taipei"),
    ("otto", "Otto the Bot", "member", "Release robot", "UTC"),
];

/// Who the sign-in page lists first and the bot talks to by default.
pub const DEFAULT_USER: &str = "ada";
/// The person the live mode posts as.
pub const BOT_USER: &str = "otto";

struct Seeder<'a> {
    w: &'a mut World,
    now: i64,
}

impl Seeder<'_> {
    fn person(&self, username: &str) -> Uuid {
        self.w.person_by_username(username).expect("seeded person").id.clone()
    }

    fn channel(&mut self, name: Option<&str>, topic: Option<&str>, kind: ChannelType, members: &[&str]) -> Uuid {
        let id = id_at(self.now - 40 * 24 * HOUR);
        let mut map = BTreeMap::new();
        let mut ever = HashSet::new();
        for m in members {
            let uid = self.person(m);
            ever.insert(uid.clone());
            map.insert(uid, Member::new(self.now - 30 * 24 * HOUR, None));
        }
        self.w.channels.insert(
            id.clone(),
            Chan {
                id: id.clone(),
                workspace_id: self.w.workspace.id.clone(),
                name: name.map(str::to_string),
                topic: topic.map(str::to_string),
                kind,
                members: map,
                ever_members: ever,
                roots: BTreeSet::new(),
                last_message_ms: None,
                pins: Vec::new(),
                bookmarks: Vec::new(),
                only_admins_post: false,
            },
        );
        id
    }

    /// Posts at `hours_ago`, returns the id.
    fn post(&mut self, channel: &Uuid, author: &str, hours_ago: f64, content: &str) -> Uuid {
        self.post_with(channel, author, hours_ago, content, None, Vec::new())
    }

    fn post_with(
        &mut self,
        channel: &Uuid,
        author: &str,
        hours_ago: f64,
        content: &str,
        parent: Option<&Uuid>,
        attachments: Vec<Uuid>,
    ) -> Uuid {
        let at = self.now - (hours_ago * HOUR as f64) as i64;
        let sender = self.person(author);
        self.w.post(PostInput {
            id: id_at(at),
            channel_id: channel.clone(),
            sender,
            content: content.to_string(),
            parent_id: parent.cloned(),
            attachment_ids: attachments,
            at_ms: at,
        })
    }

    fn reply(&mut self, channel: &Uuid, root: &Uuid, author: &str, hours_ago: f64, content: &str) -> Uuid {
        self.post_with(channel, author, hours_ago, content, Some(root), Vec::new())
    }

    fn react(&mut self, message: &Uuid, emoji: &str, who: &[&str]) {
        let ids: Vec<Uuid> = who.iter().map(|w| self.person(w)).collect();
        let m = self.w.messages.get_mut(message).expect("seeded message");
        m.reactions.push((emoji.to_string(), ids));
        m.version += 1;
    }

    fn upload(&mut self, author: &str, hours_ago: f64, name: &str, bytes: Vec<u8>) -> Uuid {
        let id = id_at(self.now - (hours_ago * HOUR as f64) as i64 - MINUTE);
        let user = self.person(author);
        self.w.store_upload(&user, &id, name, bytes);
        id
    }
}

pub fn seed(w: &mut World) {
    let now = now_ms();
    w.workspace = WorkspaceRow { id: id_at(now - 60 * 24 * HOUR), name: "Acme".into(), slug: "acme".into() };
    for (i, (username, name, role, title, tz)) in PEOPLE.iter().enumerate() {
        let id = id_at(now - (60 - i as i64) * 24 * HOUR);
        w.people.insert(
            id.clone(),
            Person {
                id,
                username: username.to_string(),
                display_name: name.to_string(),
                email: format!("{username}@acme.test"),
                role: UserRole::parse(role),
                title: Some(title.to_string()),
                time_zone: Some(tz.to_string()),
                status: match *username {
                    "mira" => Some("Sketching the new sidebar".into()),
                    "otto" => Some("Beep boop".into()),
                    _ => None,
                },
                joined_ms: now - (60 - i as i64) * 24 * HOUR,
                always_online: false,
            },
        );
    }
    let mut s = Seeder { w, now };
    s.w.default_user = s.person(DEFAULT_USER);
    s.w.bot = Some(s.person(BOT_USER));

    let all = ["ada", "grace", "linus", "mira", "otto"];
    let general = s.channel(Some("general"), Some("Company-wide announcements and work-based matters"), ChannelType::Public, &all);
    let random = s.channel(Some("random"), Some("Non-work banter and water cooler conversation"), ChannelType::Public, &all);
    let engineering =
        s.channel(Some("engineering"), Some("Builds, reviews and incidents"), ChannelType::Public, &["ada", "grace", "linus", "otto"]);
    let design = s.channel(Some("design"), Some("Mocks, critiques and the design system"), ChannelType::Public, &["mira", "grace"]);
    let leadership = s.channel(Some("leadership"), Some("Planning, hiring, budget"), ChannelType::Private, &["ada", "grace"]);
    let dm_grace = s.channel(None, None, ChannelType::DirectMessage, &["ada", "grace"]);
    let dm_linus = s.channel(None, None, ChannelType::DirectMessage, &["ada", "linus"]);

    // --- #general: five days of talk ----------------------------------------------------
    let g = &general;
    let welcome = s.post(g, "grace", 100.0, "Good morning everyone! :wave: Reminder that **Acme 0.6.1** goes out this week. Please keep `main` green.");
    s.react(&welcome, "👋", &["ada", "linus", "mira"]);
    s.post(g, "linus", 99.6, "Morning! I'm finishing the retry logic for uploads today.");
    s.post(g, "mira", 99.2, "I'll post the new sidebar mocks after lunch :art:");
    let flaky = s.post(g, "grace", 98.5, "Has anyone seen `upload_resumes_after_drop` fail on the Windows runner? It flaked twice this morning.");
    s.reply(g, &flaky, "linus", 98.3, "Yes — it's the temp dir cleanup racing the file watcher. I have a fix.");
    s.reply(g, &flaky, "ada", 98.1, "Nice catch. Can you add a regression test so it stays fixed?");
    s.reply(g, &flaky, "linus", 97.5, "Done in #412, test included. Ran it 200 times in a loop, no failures.");
    s.reply(g, &flaky, "grace", 97.2, "Merged. Thanks both!");
    s.post(g, "ada", 97.0, "Quick summary of what's left for the release:\n\n1. Upload retries (@linus)\n2. Sidebar polish (@mira)\n3. Release notes (@grace)\n4. Final QA pass (everyone)");
    s.post(g, "linus", 96.4, "For reference, the backoff we settled on:\n\n```rust\nfn delay(attempt: u32) -> Duration {\n    let base = 1_000u64 << attempt.min(5);\n    Duration::from_millis(base.min(30_000))\n}\n```");
    s.post(g, "mira", 95.8, "Looks good. Is the cap 30 s on purpose? Feels long for an upload.");
    s.post(g, "linus", 95.5, "It is — the server answers 503 while it drains, and 30 s rides that out.");
    s.post(g, "grace", 94.0, "Lunch and learn on Thursday: *How our realtime replay works*. Slides here: https://example.com/talks/replay");
    s.post(g, "ada", 93.2, "Reading list for anyone curious about the protocol:\n\n- [Keyset pagination](https://use-the-index-luke.com/no-offset)\n- [UUIDv7](https://www.rfc-editor.org/rfc/rfc9562)\n- [WebSocket close codes](https://www.rfc-editor.org/rfc/rfc6455#section-7.4)");
    s.post(g, "otto", 92.0, "Nightly build `2026.11.06` passed ✅ (14m 12s)");

    // Day 2
    s.post(g, "linus", 76.0, "Morning all. CI was slow overnight, looking into it.");
    let mock_png = crate::media::demo_png();
    let mock = s.upload("mira", 75.5, "sidebar-mock.png", mock_png);
    let shot = s.post_with(g, "mira", 75.5, "Here's the new sidebar mock — channel groups collapse now, and starred channels float to the top.", None, vec![mock]);
    s.react(&shot, "😍", &["ada", "grace"]);
    s.react(&shot, "🎉", &["linus"]);
    let review = shot.clone();
    s.reply(g, &review, "ada", 75.2, "Love it. Could the unread badge sit a little further right?");
    s.reply(g, &review, "grace", 75.0, "+1, and maybe dim muted channels a bit more.");
    s.reply(g, &review, "mira", 74.6, "Both done in v2, I'll share later today.");
    s.post(g, "grace", 74.0, "Build times for the last week:\n\n| Day | Linux | macOS | Windows |\n|---|---|---|---|\n| Mon | 9m | 14m | 17m |\n| Tue | 9m | 13m | 21m |\n| Wed | 10m | 14m | 16m |\n\nWindows is the outlier, as usual.");
    s.post(g, "linus", 73.6, "The Windows runner spends 6 minutes restoring the cargo cache. I'll try splitting it per crate.");
    s.post(g, "ada", 73.0, "> premature optimization is the root of all evil\n\n…but six minutes of cache restore is just evil. Go for it.");
    s.post(g, "mira", 72.2, "Does anyone have strong feelings about the empty state copy? Options:\n\n- *Nothing here yet*\n- *All caught up*\n- *No messages — say hi!*");
    s.post(g, "grace", 71.9, "*All caught up* for inboxes, *No messages — say hi!* for new channels.");
    s.post(g, "ada", 71.5, "Agree with Grace.");
    let checklist_file = s.upload(
        "grace",
        70.0,
        "release-checklist.md",
        b"# Release checklist\n\n- [ ] Bump versions\n- [ ] Changelog\n- [ ] Tag and push\n- [ ] Announce\n".to_vec(),
    );
    let checklist = s.post_with(g, "grace", 70.0, "Release checklist for 0.6.1, please tick your items:", None, vec![checklist_file]);
    s.post(g, "otto", 68.0, "Nightly build `2026.11.07` passed ✅ (12m 48s)");

    // Day 3
    let migration = s.post(g, "linus", 52.0, "Question: do we migrate the old attachments to WebP in this release or the next one?");
    s.reply(g, &migration, "grace", 51.8, "Next one. This release is already big.");
    s.reply(g, &migration, "ada", 51.5, "Agreed — let's write it down as an issue so we don't forget.");
    s.reply(g, &migration, "linus", 51.2, "Filed. Leaving this open until we schedule it.");
    s.post(g, "mira", 50.5, "Sidebar v2 is up in Figma. Changes: badge alignment, dimmed muted channels, bigger hit targets.");
    s.post(g, "grace", 50.0, "Heads-up: I'm rotating the staging database credentials at 15:00 UTC. Expect ~2 minutes of errors.");
    s.post(g, "grace", 49.0, "Rotation done, all green.");
    let fast = s.post(g, "linus", 48.5, "Cache split landed: Windows builds are down to **11 minutes**. 🎉");
    s.react(&fast, "🚀", &["ada", "grace", "mira", "otto"]);
    s.post(g, "ada", 48.0, "That's a huge improvement, thank you @linus!");
    s.post(g, "otto", 44.0, "Nightly build `2026.11.08` passed ✅ (11m 02s)");
    s.post(g, "grace", 43.0, "Office hours today are moved to **16:30** because of the all-hands.");
    s.post(g, "mira", 42.5, "Reminder: please fill in the design survey by Friday — it takes 3 minutes. https://example.com/survey");
    s.post(g, "linus", 42.0, "TIL: `cargo tree -d` lists every crate that is in the build twice. We had three versions of `syn`. 😅");
    s.post(g, "ada", 41.5, "Ha. Can you file an issue to dedupe? Low priority.");
    s.post(g, "grace", 41.0, "Status page now shows the websocket error rate as well. Thanks @linus for the metric.");

    // Day 4, morning
    s.post(g, "ada", 31.0, "## Weekly update\n\n**Shipped**\n- Upload retries\n- Faster Windows CI\n\n**In progress**\n- Sidebar v2\n- Release notes\n\n**Blocked**\n- nothing 🎉");
    s.post(g, "linus", 30.5, "Great week, everyone.");
    s.post(g, "mira", 30.0, "Small thing: I renamed *Channel groups* to *Sections* in the mocks, it tested better. Thoughts?");
    s.post(g, "grace", 29.5, "Fine by me, as long as the API keeps calling them groups 😄");

    // Day 4
    s.post(g, "mira", 28.0, "Usability test notes from yesterday:\n\n1. People found channel groups right away\n2. Nobody discovered *mark as unread*\n3. Two people wanted keyboard shortcuts for reactions");
    s.post(g, "ada", 27.5, "Point 2 is interesting. Maybe a hint the first time someone opens the context menu?");
    s.post(g, "mira", 27.2, "I'll mock that up.");
    s.post(g, "grace", 26.0, "Release notes draft is ready for review: https://example.com/notes/0.6.1 — comments welcome until tomorrow noon.");
    s.post(g, "linus", 25.0, "Left a few comments. The *Known issues* section should mention the proxy timeout.");
    s.post(g, "otto", 20.0, "Nightly build `2026.11.09` passed ✅ (10m 57s)");

    // Today
    s.post(g, "grace", 6.0, "Release candidate `0.6.1-rc.2` is on staging. Please give it a spin today.");
    s.post(g, "linus", 5.5, "Smoke tests pass. Uploads, search and threads all look fine.");
    // Ada has read up to here.
    let first_unread_cut =
        s.post(g, "mira", 5.0, "Found a small glitch: the reaction picker clips on narrow windows. Not a blocker.");
    s.post(g, "grace", 3.0, "Thanks Mira, filed it for 0.6.2.");
    s.post(g, "linus", 2.0, "Anyone up for a quick pairing session on the replay cursor bug?");
    s.post(g, "grace", 1.5, "@ada can you sign off on the release notes before 16:00? Then I'll tag.");
    s.post(g, "mira", 1.0, "Updated mocks with the *mark as unread* hint are in Figma.");
    s.post(g, "otto", 0.5, "Deploy of `0.6.1-rc.2` to staging finished in 3m 41s.");
    // A reply Ada has not read, in a thread she follows:
    let late_reply = s.reply(g, &review, "mira", 0.8, "v3 is up too — @ada the badge now sits on the right edge.");

    // --- #random -------------------------------------------------------------------------
    let r = &random;
    let coffee = s.post(r, "linus", 90.0, "Who broke the coffee machine? ☕");
    s.react(&coffee, "😂", &["grace", "mira"]);
    s.post(r, "mira", 89.5, "It was making *strange noises* before I touched it, I swear.");
    s.post(r, "grace", 70.0, "The office cat has claimed the build server as a bed. Builds are now *slightly warmer*.");
    s.post(r, "ada", 47.0, "Book club pick for December: *The Mythical Man-Month*. Yes, again.");
    s.post(r, "linus", 46.5, "Adding more readers to a late book makes it later.");
    let pun = s.roots_last(r);
    s.react(&pun, "🤣", &["ada", "grace", "mira"]);
    s.post(r, "otto", 22.0, "Fun fact: I have deployed 1,024 builds. That is a nice round number.");
    s.post(r, "mira", 10.0, "Lunch at the ramen place on Friday? 🍜");
    s.post(r, "grace", 9.5, "In!");
    s.post(r, "ada", 9.0, "Count me in too.");

    // --- #engineering ----------------------------------------------------------------------
    let e = &engineering;
    s.post(e, "grace", 80.0, "On-call rotation for next week: @linus Mon–Wed, @grace Thu–Sun.");
    s.post(e, "linus", 60.0, "Postmortem for Tuesday's 502s: the proxy's idle timeout was shorter than our long-poll. Fixed by raising it to 75 s.");
    s.post(e, "ada", 59.0, "Thanks. Let's add an alert on 5xx rate > 1% for 5 minutes.");
    s.post(e, "otto", 40.0, "Dependency update: `tokio` 1.54.0 → 1.54.1 (patch). All tests pass.");
    s.post(e, "grace", 30.0, "Reminder: code freeze for 0.6.1 starts tomorrow at noon UTC.");
    let eng_cut = s.roots_last(e);
    s.post(e, "linus", 4.0, "PR for the replay cursor fix is up: https://github.com/doancan/purlay/pull/1234");
    s.post(e, "otto", 3.5, "CI for PR #1234: ✅ 412 passed, 0 failed.");

    // --- #design (Ada is not in it) ----------------------------------------------------------
    let d = &design;
    s.post(d, "mira", 50.0, "Design crit on Wednesday: sidebar v2 and the empty states.");
    s.post(d, "grace", 49.5, "I'll bring the accessibility checklist.");
    s.post(d, "mira", 8.0, "New icon set draft: 24 px grid, 1.5 px stroke. Feedback welcome!");

    // --- #leadership (private) -------------------------------------------------------------
    let l = &leadership;
    s.post(l, "ada", 72.0, "Q1 planning doc is ready for comments.");
    s.post(l, "grace", 71.0, "Read it — I'd move the search rewrite before the mobile work.");
    s.post(l, "ada", 70.5, "Fair. Updated.");

    // --- direct messages ----------------------------------------------------------------------
    s.post(&dm_grace, "grace", 30.0, "Do you have five minutes for the release plan later?");
    s.post(&dm_grace, "ada", 29.5, "Sure, after standup.");
    s.post(&dm_grace, "grace", 29.0, "Great, thanks!");
    let dm_cut = s.roots_last(&dm_grace);
    s.post(&dm_grace, "grace", 0.7, "Also: the tag needs your sign-off, see #general 🙏");
    s.post(&dm_linus, "linus", 50.0, "Thanks for the review yesterday.");
    s.post(&dm_linus, "ada", 49.8, "Any time. The new test is great.");
    s.post(&dm_linus, "linus", 49.5, "👍");

    // --- pins, bookmarks, saved, resolved thread, issues --------------------------------------
    let grace_id = s.person("grace");
    let ada_id = s.person("ada");
    if let Some(c) = s.w.channels.get_mut(&general) {
        c.pins.push(crate::world::Pin { message_id: checklist.clone(), by: grace_id.clone(), at_ms: now - 69 * HOUR });
        c.bookmarks.push(crate::world::ChannelBookmarkRow {
            id: id_at(now - 90 * HOUR),
            url: "https://github.com/doancan/purlay".into(),
            title: "Purlay on GitHub".into(),
            added_by: grace_id.clone(),
            added_ms: now - 90 * HOUR,
        });
    }
    if let Some(m) = s.w.messages.get_mut(&flaky) {
        m.resolved = Some((now - 97 * HOUR, ada_id.clone()));
        m.version += 1;
    }
    s.w.saved.push(crate::world::SavedRow {
        id: id_at(now - 48 * HOUR),
        user: ada_id.clone(),
        message_id: fast.clone(),
        state: SavedState::InProgress,
        saved_ms: now - 48 * HOUR,
    });
    let linus_id = s.person("linus");
    s.w.issues.push(crate::world::IssueRow {
        workspace_id: s.w.workspace.id.clone(),
        issue: Issue {
            id: id_at(now - 51 * HOUR),
            title: "Migrate old attachments to WebP".into(),
            body: "Re-encode stored images as WebP in a background job, keeping the originals until the job confirms.".into(),
            status: IssueStatus::Done,
            assignee_id: Some(linus_id.clone()),
            created_by: Some(linus_id.clone()),
            created_at: crate::world::ts(now - 51 * HOUR),
            updated_at: crate::world::ts(now - 20 * HOUR),
        },
    });
    let open_issue = Issue {
        id: id_at(now - 2 * HOUR),
        title: "Reaction picker clips on narrow windows".into(),
        body: "Below ~700 px wide the picker's last column is cut off. Seen on the release candidate.".into(),
        status: IssueStatus::Open,
        assignee_id: Some(ada_id.clone()),
        created_by: Some(grace_id.clone()),
        created_at: crate::world::ts(now - 2 * HOUR),
        updated_at: crate::world::ts(now - 2 * HOUR),
    };
    s.w.issues.push(crate::world::IssueRow { workspace_id: s.w.workspace.id.clone(), issue: open_issue.clone() });
    s.w.add_inbox_notice(
        &ada_id,
        "issue_assigned",
        Some(&grace_id),
        InboxEntrySubject { r#type: "issue".into(), id: open_issue.id.clone() },
        &open_issue.title,
    );

    // --- read positions -------------------------------------------------------------------
    // Everybody has read everything, except Ada: a few roots in #general, two in
    // #engineering, one DM, and Mira's latest reply to the sidebar thread.
    let channel_ids: Vec<Uuid> = s.w.channels.keys().cloned().collect();
    for cid in &channel_ids {
        let newest = s.w.channels[cid].roots.iter().next_back().cloned();
        let cut = if *cid == general {
            Some(first_unread_cut.clone())
        } else if *cid == engineering {
            Some(eng_cut.clone())
        } else if *cid == dm_grace {
            Some(dm_cut.clone())
        } else {
            newest.clone()
        };
        let chan = s.w.channels.get_mut(cid).expect("channel");
        for (uid, m) in chan.members.iter_mut() {
            m.last_read = if *uid == ada_id { cut.clone() } else { newest.clone() };
        }
    }
    let message_ids: Vec<Uuid> = s.w.messages.keys().cloned().collect();
    for mid in &message_ids {
        let replies: Vec<Uuid> = s.w.messages[mid].replies.iter().cloned().collect();
        let Some(last) = replies.last().cloned() else { continue };
        let followers: Vec<Uuid> =
            s.w.people.keys().filter(|u| s.w.is_following(&s.w.messages[mid], u)).cloned().collect();
        let m = s.w.messages.get_mut(mid).expect("message");
        for u in followers {
            let bound = if u == ada_id && *mid == review {
                replies.iter().rev().nth(1).cloned().unwrap_or_else(|| last.clone())
            } else {
                last.clone()
            };
            m.thread_reads.insert(u, bound);
        }
    }
    // Notifications are read up to each position; the rest wait.
    let mut notes = std::mem::take(&mut s.w.notes);
    for n in notes.iter_mut() {
        let Some(m) = s.w.messages.get(&n.message_id) else { continue };
        let read = match &m.parent_id {
            None => s.w.channels[&n.channel_id].members.get(&n.user).and_then(|mm| mm.last_read.clone()).is_some_and(|lr| m.id <= lr),
            Some(root) => s.w.messages[root].thread_reads.get(&n.user).is_some_and(|b| m.id <= *b),
        };
        if read {
            n.read_ms = Some(n.created_ms + 5 * MINUTE);
        }
    }
    s.w.notes = notes;
    let _ = late_reply;

    // The rows were written "then", not now: the changes feed starts empty.
    let created: Vec<(Uuid, i64)> = s
        .w
        .messages
        .values()
        .map(|m| {
            let latest_reply = m.replies.iter().filter_map(|r| s.w.messages.get(r)).map(|r| r.created_ms).max();
            (m.id.clone(), latest_reply.unwrap_or(m.created_ms).max(m.created_ms))
        })
        .collect();
    for (id, at) in created {
        if let Some(m) = s.w.messages.get_mut(&id) {
            m.changed_ms = at;
        }
    }
}

impl Seeder<'_> {
    fn roots_last(&self, channel: &Uuid) -> Uuid {
        self.w.channels[channel].roots.iter().next_back().cloned().expect("a root")
    }
}
