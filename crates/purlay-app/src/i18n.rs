//! English and Turkish. Turkish addresses the person as "sen", like the web's catalogue.
//! Every key has both values; `cargo test` checks it.

use std::sync::atomic::{AtomicBool, Ordering};

static TURKISH: AtomicBool = AtomicBool::new(false);

pub fn set_language(code: &str) {
    let tr = match code {
        "tr" => true,
        "en" => false,
        _ => system_is_turkish(),
    };
    TURKISH.store(tr, Ordering::Relaxed);
}

pub fn is_turkish() -> bool {
    TURKISH.load(Ordering::Relaxed)
}

fn system_is_turkish() -> bool {
    for var in ["PURLAY_LANG", "LANG", "LC_ALL"] {
        if let Ok(v) = std::env::var(var) {
            return v.to_ascii_lowercase().starts_with("tr");
        }
    }
    #[cfg(windows)]
    {
        // The user's UI language, e.g. `tr-TR`.
        if let Ok(out) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "(Get-UICulture).Name"])
            .creation_flags_no_window()
            .output()
        {
            return String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase().starts_with("tr");
        }
    }
    false
}

trait NoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000);
        }
        self
    }
}

/// The text for `key` in the current language; the key itself when missing.
pub fn t(key: &'static str) -> &'static str {
    let tr = is_turkish();
    for (k, en, trv) in STRINGS {
        if *k == key {
            return if tr { trv } else { en };
        }
    }
    key
}

/// `t` with `{}` replaced in order.
pub fn tf(key: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut rest = t(key);
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        if let Some(a) = it.next() {
            out.push_str(&a.to_string());
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// (key, English, Turkish)
pub const STRINGS: &[(&str, &str, &str)] = &[
    // App & connection
    ("app.name", "Purlay", "Purlay"),
    ("connect.title", "Connect to your team", "Ekibine bağlan"),
    ("connect.subtitle", "Enter your Purlay server's address.", "Purlay sunucunun adresini yaz."),
    ("connect.placeholder", "chat.example.com", "chat.example.com"),
    ("connect.continue", "Continue", "Devam"),
    ("connect.checking", "Checking the server…", "Sunucu kontrol ediliyor…"),
    ("connect.recent", "Recent servers", "Son sunucular"),
    ("connect.error.empty", "Enter an address.", "Bir adres yaz."),
    ("connect.error.address", "That is not a server address. Use only the server's name, like chat.example.com.", "Bu bir sunucu adresi değil. Yalnızca sunucunun adını yaz, örneğin chat.example.com."),
    ("connect.error.unreachable", "Could not reach the server. Check the address and your connection.", "Sunucuya ulaşılamadı. Adresi ve bağlantını kontrol et."),
    ("connect.error.notPurlay", "That address does not answer like a Purlay server.", "Bu adres bir Purlay sunucusu gibi yanıt vermiyor."),
    ("connect.error.serverOld", "This server is too old for this app (version {}). Ask its administrator to update it.", "Bu sunucu bu uygulama için çok eski (sürüm {}). Yöneticisinden güncellemesini iste."),
    ("connect.error.appOld", "This app is too old for the server. Update Purlay.", "Bu uygulama sunucu için çok eski. Purlay'ı güncelle."),
    ("connect.error.origin", "The server's own address is not usable. Ask its administrator to check APP_URL.", "Sunucunun kendi adresi kullanılamıyor. Yöneticisinden APP_URL'yi kontrol etmesini iste."),
    ("signin.title", "Sign in to {}", "{} sunucusuna giriş yap"),
    ("signin.body", "Your browser opens to sign in. Approve Purlay there, and this window continues on its own.", "Giriş için tarayıcın açılacak. Orada Purlay'ı onayla; bu pencere kendiliğinden devam eder."),
    ("signin.open", "Open browser to sign in", "Giriş için tarayıcıyı aç"),
    ("signin.waiting", "Waiting for your browser…", "Tarayıcın bekleniyor…"),
    ("signin.again", "Open the browser again", "Tarayıcıyı yeniden aç"),
    ("signin.other", "Use another server", "Başka bir sunucu kullan"),
    ("signin.error.state", "That sign-in answer belongs to an older attempt. Start again.", "Bu giriş yanıtı eski bir denemeye ait. Yeniden başla."),
    ("signin.error.code", "The sign-in code expired. Start again.", "Giriş kodunun süresi doldu. Yeniden başla."),
    ("signin.error.rate", "Too many attempts. Wait a moment and try again.", "Çok fazla deneme. Biraz bekleyip yeniden dene."),
    ("signin.error.generic", "Signing in did not finish: {}", "Giriş tamamlanmadı: {}"),
    ("signin.paste", "Browser did not return? Paste the purlay:// link here.", "Tarayıcı geri dönmedi mi? purlay:// bağlantısını buraya yapıştır."),
    ("loading", "Loading…", "Yükleniyor…"),
    ("retry", "Try Again", "Yeniden dene"),
    ("cancel", "Cancel", "Vazgeç"),
    ("close", "Close", "Kapat"),
    ("save", "Save", "Kaydet"),
    ("delete", "Delete", "Sil"),
    ("done", "Done", "Tamam"),
    ("copy", "Copy", "Kopyala"),
    ("copied", "Copied", "Kopyalandı"),
    ("open", "Open", "Aç"),
    ("error.load", "Could not load this.", "Bu yüklenemedi."),
    ("error.action", "That did not work: {}", "Bu işe yaramadı: {}"),
    ("session.ended", "You were signed out. Sign in again to continue.", "Oturumun kapandı. Devam etmek için yeniden giriş yap."),
    // Connection bar
    ("conn.reconnecting", "Reconnecting…", "Yeniden bağlanılıyor…"),
    ("conn.offline", "Offline. Messages you send go out when the connection is back.", "Çevrimdışı. Gönderdiğin mesajlar bağlantı gelince gider."),
    ("conn.degraded", "Some channels are not updating live.", "Bazı kanallar canlı güncellenmiyor."),
    ("conn.lost", "{} unsent messages were removed with a channel you no longer have.", "Artık erişemediğin bir kanalla birlikte {} gönderilmemiş mesaj kaldırıldı."),
    // Navigation
    ("nav.inbox", "Inbox", "Gelen kutusu"),
    ("nav.search", "Search", "Ara"),
    ("nav.saved", "Saved", "Kaydedilenler"),
    ("nav.openTopics", "Open Topics", "Açık konular"),
    ("nav.issues", "Issues", "Konular"),
    ("nav.drafts", "Drafts", "Taslaklar"),
    ("nav.scheduled", "Scheduled", "Zamanlanmış"),
    ("nav.reminders", "Reminders", "Hatırlatıcılar"),
    ("nav.settings", "Settings", "Ayarlar"),
    ("nav.jump", "Jump to…", "Git…"),
    ("sidebar.starred", "Starred", "Yıldızlı"),
    ("sidebar.channels", "Channels", "Kanallar"),
    ("sidebar.conversations", "Direct Messages", "Doğrudan mesajlar"),
    ("sidebar.newChannel", "New Channel", "Yeni kanal"),
    ("sidebar.newConversation", "New Conversation", "Yeni konuşma"),
    ("sidebar.markAllRead", "Mark All Read", "Tümünü okundu say"),
    ("sidebar.browse", "Browse Channels", "Kanallara göz at"),
    ("sidebar.empty", "No channels yet.", "Henüz kanal yok."),
    ("sidebar.noChat", "Chat is not available in this workspace.", "Bu çalışma alanında sohbet kullanılamıyor."),
    // Channel
    ("channel.join", "Join Channel", "Kanala katıl"),
    ("channel.leave", "Leave Channel", "Kanaldan ayrıl"),
    ("channel.star", "Star", "Yıldızla"),
    ("channel.unstar", "Unstar", "Yıldızı kaldır"),
    ("channel.members", "{} members", "{} üye"),
    ("channel.files", "Files", "Dosyalar"),
    ("channel.pins", "Pinned", "Sabitlenenler"),
    ("channel.bookmarks", "Bookmarks", "Yer imleri"),
    ("channel.details", "Details", "Ayrıntılar"),
    ("channel.notify", "Notifications", "Bildirimler"),
    ("channel.notify.all", "All messages", "Tüm mesajlar"),
    ("channel.notify.mentions", "Mentions only", "Yalnızca bahsetmeler"),
    ("channel.notify.none", "Nothing", "Hiçbiri"),
    ("channel.markUnread", "Mark Unread", "Okunmadı say"),
    ("channel.empty", "This is the very beginning of {}.", "{} burada başlıyor."),
    ("channel.emptyDm", "This is the start of your conversation.", "Konuşmanızın başlangıcı."),
    ("channel.gone.archived", "This channel was archived.", "Bu kanal arşivlendi."),
    ("channel.gone.left", "You left this channel.", "Bu kanaldan ayrıldın."),
    ("channel.gone.removed", "You were removed from this channel.", "Bu kanaldan çıkarıldın."),
    ("channel.gone", "This channel is no longer available.", "Bu kanal artık kullanılamıyor."),
    ("channel.onlyAdmins", "Only admins can post in this channel.", "Bu kanala yalnızca yöneticiler yazabilir."),
    ("channel.notMember", "You are not in this channel.", "Bu kanalda değilsin."),
    ("channel.newMessages", "{} new messages", "{} yeni mesaj"),
    ("channel.jumpRecent", "Jump to recent", "En yeniye git"),
    ("channel.new", "New", "Yeni"),
    ("channel.loadOlder", "Loading earlier messages…", "Önceki mesajlar yükleniyor…"),
    ("channel.create.title", "New Channel", "Yeni kanal"),
    ("channel.create.name", "Name", "Ad"),
    ("channel.create.topic", "Topic (optional)", "Konu (isteğe bağlı)"),
    ("channel.create.private", "Private: only invited people can see it", "Özel: yalnızca davet edilenler görebilir"),
    ("channel.create.create", "Create", "Oluştur"),
    ("dm.title", "New Conversation", "Yeni konuşma"),
    ("dm.placeholder", "Find people", "Kişi bul"),
    ("dm.start", "Start", "Başlat"),
    ("dm.limit", "A conversation has at most {} people.", "Bir konuşmada en fazla {} kişi olur."),
    // Messages
    ("msg.edited", "(edited)", "(düzenlendi)"),
    ("msg.deleted", "This message was deleted.", "Bu mesaj silindi."),
    ("msg.sending", "Sending", "Gönderiliyor"),
    ("msg.failed", "Not sent", "Gönderilmedi"),
    ("msg.retry", "Retry", "Yeniden dene"),
    ("msg.discard", "Delete", "Sil"),
    ("msg.reply", "Reply in Thread", "Başlıkta yanıtla"),
    ("msg.react", "Add Reaction", "Tepki ekle"),
    ("msg.edit", "Edit", "Düzenle"),
    ("msg.delete", "Delete for Everyone", "Herkes için sil"),
    ("msg.hide", "Delete for Me", "Benim için sil"),
    ("msg.deleteConfirm", "Delete this message?", "Bu mesaj silinsin mi?"),
    ("msg.deleteConfirmBody", "Deleting for everyone removes it from the conversation for all members.", "Herkes için silmek, mesajı tüm üyeler için konuşmadan kaldırır."),
    ("msg.copyText", "Copy Text", "Metni kopyala"),
    ("msg.copyLink", "Copy Link", "Bağlantıyı kopyala"),
    ("msg.pin", "Pin to Channel", "Kanala sabitle"),
    ("msg.unpin", "Unpin", "Sabitlemeyi kaldır"),
    ("msg.save", "Save for Later", "Sonrası için kaydet"),
    ("msg.unsave", "Remove from Saved", "Kaydedilenlerden çıkar"),
    ("msg.remind", "Remind Me", "Bana hatırlat"),
    ("msg.remind.20m", "In 20 minutes", "20 dakika sonra"),
    ("msg.remind.1h", "In 1 hour", "1 saat sonra"),
    ("msg.remind.3h", "In 3 hours", "3 saat sonra"),
    ("msg.remind.tomorrow", "Tomorrow at 9:00", "Yarın 09:00'da"),
    ("msg.remind.set", "Reminder set.", "Hatırlatıcı kuruldu."),
    ("msg.markUnread", "Mark Unread from Here", "Buradan itibaren okunmadı say"),
    ("msg.history", "Edit History", "Düzenleme geçmişi"),
    ("msg.reactionLimit", "You can add at most {} reactions to a message.", "Bir mesaja en fazla {} tepki ekleyebilirsin."),
    ("msg.webhook", "via webhook", "webhook ile"),
    ("msg.today", "Today", "Bugün"),
    ("msg.yesterday", "Yesterday", "Dün"),
    // Threads
    ("thread.title", "Thread", "Başlık"),
    ("thread.replies", "{} replies", "{} yanıt"),
    ("thread.reply1", "1 reply", "1 yanıt"),
    ("thread.last", "last {}", "son {}"),
    ("thread.open", "Thread open", "Başlık açık"),
    ("thread.openState", "Open", "Açık"),
    ("thread.resolved", "Resolved", "Çözüldü"),
    ("thread.resolve", "Resolve", "Çözüldü say"),
    ("thread.reopen", "Reopen", "Yeniden aç"),
    ("thread.follow", "Follow", "Takip et"),
    ("thread.unfollow", "Unfollow", "Takibi bırak"),
    ("thread.replyPlaceholder", "Reply…", "Yanıtla…"),
    ("thread.question", "Question", "Soru"),
    ("thread.markQuestion", "Mark as Question", "Soru olarak işaretle"),
    ("thread.unmarkQuestion", "Not a Question", "Soru değil"),
    ("thread.accept", "Accept as Answer", "Yanıt olarak kabul et"),
    ("thread.accepted", "Accepted answer", "Kabul edilen yanıt"),
    ("thread.decision", "Decision", "Karar"),
    // Composer
    ("composer.placeholder", "Message {}", "{} kanalına yaz"),
    ("composer.placeholderDm", "Message {}", "{} kişisine yaz"),
    ("composer.send", "Send", "Gönder"),
    ("composer.attach", "Attach Files", "Dosya ekle"),
    ("composer.emoji", "Emoji", "Emoji"),
    ("composer.mention", "Mention someone", "Birinden bahset"),
    ("chrome.sidePanel", "Side panel", "Yan panel"),
    ("chrome.showSidebar", "Show sidebar", "Kenar çubuğunu göster"),
    ("chrome.hideSidebar", "Hide sidebar", "Kenar çubuğunu gizle"),
    ("chrome.workspace", "Workspace", "Çalışma alanı"),
    ("chrome.search", "Search Purlay", "Purlay'de ara"),
    ("chrome.more", "More", "Diğer"),
    ("composer.uploading", "Uploading…", "Yükleniyor…"),
    ("composer.uploadFailed", "Upload failed: {}", "Yükleme başarısız: {}"),
    ("composer.tooLarge", "{} is larger than 25 MB.", "{} 25 MB'tan büyük."),
    ("composer.tooMany", "A message can carry at most {} files.", "Bir mesaj en fazla {} dosya taşıyabilir."),
    ("composer.typing1", "{} is typing…", "{} yazıyor…"),
    ("composer.typing2", "{} and {} are typing…", "{} ve {} yazıyor…"),
    ("composer.typingMany", "Several people are typing…", "Birkaç kişi yazıyor…"),
    ("composer.schedule", "Schedule for Later", "Sonraya zamanla"),
    ("composer.editing", "Editing message", "Mesaj düzenleniyor"),
    ("composer.hint", "Enter to send · Shift+Enter for a new line", "Göndermek için Enter · Yeni satır için Shift+Enter"),
    ("composer.preview", "Preview", "Önizleme"),
    // Inbox
    ("inbox.all", "All", "Tümü"),
    ("inbox.mentions", "Mentions", "Bahsetmeler"),
    ("inbox.direct", "Direct", "Doğrudan"),
    ("inbox.threads", "Threads", "Başlıklar"),
    ("inbox.updates", "Updates", "Güncellemeler"),
    ("inbox.unread", "Unread", "Okunmamış"),
    ("inbox.markAllRead", "Mark All Read", "Tümünü okundu say"),
    ("inbox.empty", "Nothing needs you right now.", "Şu an seni bekleyen bir şey yok."),
    ("inbox.earlier", "Earlier", "Daha önce"),
    ("inbox.mentioned", "mentioned you in {}", "{} içinde senden bahsetti"),
    ("inbox.dm", "sent you a message", "sana mesaj gönderdi"),
    ("inbox.replied", "replied in a thread in {}", "{} içindeki bir başlıkta yanıtladı"),
    ("inbox.resolved", "resolved a thread in {}", "{} içindeki bir başlığı çözdü"),
    ("inbox.channelMessage", "posted in {}", "{} kanalına yazdı"),
    ("inbox.reminder", "Reminder", "Hatırlatıcı"),
    // Search
    ("search.placeholder", "Search messages", "Mesajlarda ara"),
    ("search.min", "Type at least {} characters.", "En az {} karakter yaz."),
    ("search.none", "No messages match.", "Eşleşen mesaj yok."),
    ("search.recent", "Recent searches", "Son aramalar"),
    ("search.in", "In channel", "Kanalda"),
    ("search.from", "From", "Gönderen"),
    ("search.anyChannel", "Any channel", "Tüm kanallar"),
    ("search.anyone", "Anyone", "Herkes"),
    ("search.questions", "Questions", "Sorular"),
    ("search.code", "Code", "Kod"),
    ("search.files", "Files", "Dosyalar"),
    ("search.other", "Elsewhere", "Başka yerlerde"),
    ("search.save", "Save Search", "Aramayı kaydet"),
    ("search.saved", "Saved searches", "Kayıtlı aramalar"),
    // Saved
    ("saved.inProgress", "In Progress", "Devam eden"),
    ("saved.completed", "Completed", "Tamamlanan"),
    ("saved.archived", "Archived", "Arşivlenen"),
    ("saved.empty", "Nothing saved here.", "Burada kayıtlı bir şey yok."),
    ("saved.complete", "Mark Completed", "Tamamlandı olarak işaretle"),
    ("saved.archive", "Archive", "Arşivle"),
    ("saved.restore", "Move to In Progress", "Devam edenlere taşı"),
    // Open topics
    ("topics.empty", "No open topics. Every question has an answer.", "Açık konu yok. Her sorunun bir yanıtı var."),
    ("topics.unanswered", "Unanswered only", "Yalnızca yanıtsız"),
    // Issues
    ("issues.open", "Open", "Açık"),
    ("issues.done", "Done", "Bitti"),
    ("issues.new", "New Issue", "Yeni konu"),
    ("issues.title", "Title", "Başlık"),
    ("issues.body", "Description", "Açıklama"),
    ("issues.assignee", "Assignee", "Atanan"),
    ("issues.unassigned", "Unassigned", "Atanmamış"),
    ("issues.markDone", "Mark Done", "Bitti olarak işaretle"),
    ("issues.reopen", "Reopen", "Yeniden aç"),
    ("issues.empty", "No issues here.", "Burada konu yok."),
    ("issues.create", "Create Issue", "Konu oluştur"),
    ("issues.off", "Issues are not available in this workspace.", "Bu çalışma alanında konular kullanılamıyor."),
    // Drafts, scheduled, reminders
    ("drafts.empty", "No drafts.", "Taslak yok."),
    ("scheduled.empty", "No scheduled messages.", "Zamanlanmış mesaj yok."),
    ("scheduled.at", "Sends {}", "{} gönderilecek"),
    ("scheduled.cancel", "Cancel Sending", "Göndermeyi iptal et"),
    ("scheduled.pick", "Send at", "Gönderim zamanı"),
    ("reminders.empty", "No reminders.", "Hatırlatıcı yok."),
    ("reminders.at", "Reminds {}", "{} hatırlatacak"),
    // Deep work & away
    ("deep.title", "Deep Work", "Derin çalışma"),
    ("deep.on", "Deep Work until {}", "{} saatine kadar derin çalışma"),
    ("deep.start", "Start Deep Work", "Derin çalışmayı başlat"),
    ("deep.end", "End Deep Work", "Derin çalışmayı bitir"),
    ("deep.30m", "30 minutes", "30 dakika"),
    ("deep.1h", "1 hour", "1 saat"),
    ("deep.2h", "2 hours", "2 saat"),
    ("deep.tomorrow", "Until tomorrow 9:00", "Yarın 09:00'a kadar"),
    ("deep.body", "Notifications are held until you finish.", "Bitirene kadar bildirimler bekletilir."),
    ("away.set", "Set Yourself Away", "Kendini uzakta göster"),
    ("away.clear", "Set Yourself Active", "Kendini etkin göster"),
    // Palette
    ("palette.placeholder", "Jump to a channel or person", "Bir kanala ya da kişiye git"),
    ("palette.none", "No matches.", "Eşleşme yok."),
    // Emoji
    ("emoji.search", "Search emoji", "Emoji ara"),
    ("emoji.recent", "Recently used", "Son kullanılanlar"),
    ("emoji.all", "Emoji", "Emoji"),
    // Settings
    ("settings.general", "General", "Genel"),
    ("settings.appearance", "Appearance", "Görünüm"),
    ("settings.notifications", "Notifications", "Bildirimler"),
    ("settings.account", "Account", "Hesap"),
    ("settings.workspace", "Workspace", "Çalışma alanı"),
    ("settings.server", "Server", "Sunucu"),
    ("settings.theme", "Theme", "Tema"),
    ("settings.theme.system", "System", "Sistem"),
    ("settings.theme.light", "Light", "Açık"),
    ("settings.theme.dark", "Dark", "Koyu"),
    ("settings.density", "Message density", "Mesaj yoğunluğu"),
    ("settings.density.comfortable", "Comfortable", "Rahat"),
    ("settings.density.compact", "Compact", "Sıkı"),
    ("settings.language", "Language", "Dil"),
    ("settings.language.system", "System", "Sistem"),
    ("settings.launch", "Open Purlay when you sign in to Windows", "Windows'a girişte Purlay'ı aç"),
    ("settings.notify.enabled", "Show desktop notifications", "Masaüstü bildirimlerini göster"),
    ("settings.notify.sound", "Play a sound", "Ses çal"),
    ("settings.notify.body", "Purlay notifies you about direct messages, mentions and replies in threads you follow, while it is running. Nothing interrupts you in Deep Work.", "Purlay çalışırken doğrudan mesajlar, bahsetmeler ve takip ettiğin başlıklardaki yanıtlar için bildirim gösterir. Derin çalışmada hiçbir şey seni bölmez."),
    ("settings.profile", "Edit Profile", "Profili düzenle"),
    ("settings.signOut", "Sign Out", "Çıkış yap"),
    ("settings.signOutAll", "Sign Out Everywhere", "Her yerden çıkış yap"),
    ("settings.sessions", "Signed-in devices", "Oturum açık cihazlar"),
    ("settings.thisDevice", "This device", "Bu cihaz"),
    ("settings.endSession", "Sign Out", "Çıkış yap"),
    ("settings.members", "Members", "Üyeler"),
    ("settings.invitations", "Invitations", "Davetler"),
    ("settings.integrations", "Integrations", "Entegrasyonlar"),
    ("settings.audit", "Audit Log", "Denetim kaydı"),
    ("settings.data", "Data and History", "Veri ve geçmiş"),
    ("settings.workspaceGeneral", "Workspace Settings", "Çalışma alanı ayarları"),
    ("settings.serverSettings", "Server Settings", "Sunucu ayarları"),
    ("settings.inBrowser", "Opens in your browser", "Tarayıcında açılır"),
    ("settings.version", "Version {} · server {}", "Sürüm {} · sunucu {}"),
    ("settings.role.owner", "Owner", "Sahip"),
    ("settings.role.admin", "Admin", "Yönetici"),
    ("settings.role.member", "Member", "Üye"),
    ("settings.servers", "Servers", "Sunucular"),
    ("settings.addServer", "Add Server", "Sunucu ekle"),
    ("settings.shortcuts", "Keyboard Shortcuts", "Klavye kısayolları"),
    // Presence
    ("presence.online", "Online", "Çevrimiçi"),
    ("presence.away", "Away", "Uzakta"),
    ("presence.offline", "Offline", "Çevrimdışı"),
    // Notifications
    ("notify.dm", "{} (direct message)", "{} (doğrudan mesaj)"),
    ("notify.mention", "{} in {}", "{} · {}"),
    ("notify.thread", "{} replied in a thread", "{} bir başlıkta yanıtladı"),
    // Files
    ("files.empty", "No files in this conversation yet.", "Bu konuşmada henüz dosya yok."),
    ("files.download", "Save As…", "Farklı kaydet…"),
    ("pins.empty", "Nothing is pinned.", "Sabitlenmiş bir şey yok."),
    ("bookmarks.empty", "No bookmarks.", "Yer imi yok."),
    ("bookmarks.add", "Add Bookmark", "Yer imi ekle"),
    ("members.empty", "No members.", "Üye yok."),
    // Shortcuts
    ("shortcut.palette", "Jump to a channel or person", "Bir kanala ya da kişiye git"),
    ("shortcut.search", "Search messages", "Mesajlarda ara"),
    ("shortcut.inbox", "Open Inbox", "Gelen kutusunu aç"),
    ("shortcut.deepWork", "Toggle Deep Work", "Derin çalışmayı aç/kapat"),
    ("shortcut.markAllRead", "Mark everything read", "Her şeyi okundu say"),
    ("shortcut.nextUnread", "Next unread channel", "Sonraki okunmamış kanal"),
    ("shortcut.prevUnread", "Previous unread channel", "Önceki okunmamış kanal"),
    ("shortcut.newConversation", "New conversation", "Yeni konuşma"),
    ("shortcut.settings", "Settings", "Ayarlar"),
    ("shortcut.shortcuts", "Show all shortcuts", "Tüm kısayolları göster"),
    ("shortcut.close", "Close panel", "Paneli kapat"),
    ("shortcut.focusComposer", "Focus the message box", "Mesaj kutusuna odaklan"),
    ("shortcut.editLast", "Edit your last message", "Son mesajını düzenle"),
    // Workspace
    ("workspace.switch", "Switch Workspace", "Çalışma alanı değiştir"),
    ("workspace.none", "You are not in a workspace yet. Accept an invitation in your browser, then come back.", "Henüz bir çalışma alanında değilsin. Tarayıcında bir daveti kabul et, sonra geri dön."),
    ("workspace.twoFactor", "This server asks you to turn on two-step verification. Do it in your browser to continue.", "Bu sunucu iki adımlı doğrulamayı açmanı istiyor. Devam etmek için tarayıcında aç."),
    ("workspace.openBrowser", "Open in Browser", "Tarayıcıda aç"),
    ("time.now", "now", "şimdi"),
    ("time.minutes", "{}m", "{}dk"),
    ("time.hours", "{}h", "{}sa"),
    ("time.days", "{}d", "{}g"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_both_languages_and_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for (k, en, tr) in STRINGS {
            assert!(seen.insert(*k), "duplicate key {k}");
            assert!(!en.is_empty() && !tr.is_empty(), "{k} is missing a value");
            assert_eq!(en.matches("{}").count(), tr.matches("{}").count(), "{k}: placeholders differ");
        }
    }

    #[test]
    fn formats() {
        set_language("en");
        assert_eq!(tf("thread.replies", &[&3]), "3 replies");
        set_language("tr");
        assert_eq!(tf("thread.replies", &[&3]), "3 yanıt");
        set_language("en");
    }
}
