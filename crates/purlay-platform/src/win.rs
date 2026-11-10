use anyhow::Context as _;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

/// The AppUserModelID toasts are shown under.
pub const AUMID: &str = "dev.purlay.windows";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Purlay";

fn exe() -> anyhow::Result<String> {
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

/// Registers `purlay:` for this user, pointing at the running executable. Safe to call on
/// every start: an install that moved is pointed at its new place.
pub fn register_protocol() -> anyhow::Result<()> {
    let exe = exe()?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(r"Software\Classes\purlay").context("create purlay key")?;
    key.set_value("", &"URL:Purlay")?;
    key.set_value("URL Protocol", &"")?;
    let (icon, _) = key.create_subkey("DefaultIcon")?;
    icon.set_value("", &format!("\"{exe}\",0"))?;
    let (cmd, _) = key.create_subkey(r"shell\open\command")?;
    let profile = std::env::var("PURLAY_PROFILE").ok().filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    match profile {
        Some(p) => cmd.set_value("", &format!("\"{exe}\" --profile {p} \"%1\""))?,
        None => cmd.set_value("", &format!("\"{exe}\" \"%1\""))?,
    }
    Ok(())
}

/// Gives toasts a name and an icon: an unpackaged app needs its AUMID registered.
pub fn register_app_identity(icon_png: Option<&std::path::Path>) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok((key, _)) = hkcu.create_subkey(format!(r"Software\Classes\AppUserModelId\{AUMID}")) {
        let _ = key.set_value("DisplayName", &"Purlay");
        if let Some(p) = icon_png {
            let _ = key.set_value("IconUri", &p.to_string_lossy().into_owned());
        }
    }
    unsafe {
        let id: Vec<u16> = AUMID.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(windows::core::PCWSTR(id.as_ptr()));
    }
}

pub fn set_launch_at_login(on: bool) -> anyhow::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)?;
    if on {
        key.set_value(RUN_VALUE, &format!("\"{}\" --background", exe()?))?;
    } else {
        let _ = key.delete_value(RUN_VALUE);
    }
    Ok(())
}

pub fn launch_at_login() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY).and_then(|k| k.get_value::<String, _>(RUN_VALUE)).is_ok()
}

/// A toast. `launch` is a `purlay:` URL opened (through the protocol) when it is clicked.
pub fn toast(title: &str, body: &str, launch: Option<&str>, silent: bool) {
    use tauri_winrt_notification::{Duration, Sound, Toast};
    let mut t = Toast::new(AUMID).title(title).text1(body).duration(Duration::Short);
    t = t.sound(if silent { None } else { Some(Sound::Default) });
    if let Some(url) = launch {
        let url = url.to_string();
        t = t.on_activated(move |_| {
            let _ = std::process::Command::new(std::env::current_exe().unwrap_or_default()).arg(&url).spawn();
            Ok(())
        });
    }
    if let Err(e) = t.show() {
        tracing::debug!("toast: {e}");
    }
}

/// Flashes the taskbar button until the window is brought forward (or stops it).
pub fn flash_window(hwnd: isize, on: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{FLASHW_STOP, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
    if hwnd == 0 {
        return;
    }
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(hwnd as *mut _),
        dwFlags: if on { FLASHW_TRAY | FLASHW_TIMERNOFG } else { FLASHW_STOP },
        uCount: 0,
        dwTimeout: 0,
    };
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}

/// Windows says now is not a time to interrupt: a full-screen game or video, a
/// presentation, or quiet hours. Purlay's own banners and sounds wait like toasts do.
pub fn busy() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_ACCEPTS_NOTIFICATIONS, QUNS_APP, QUNS_NOT_PRESENT};
    match unsafe { SHQueryUserNotificationState() } {
        Ok(s) => !(s == QUNS_ACCEPTS_NOTIFICATIONS || s == QUNS_APP || s == QUNS_NOT_PRESENT),
        Err(_) => false,
    }
}

/// Windows 11's Do Not Disturb (or Focus) is on: the shell's quiet-hours profile is
/// "priority only" (1) or "alarms only" (2). Windows publishes it only as a WNF state, which
/// `ntdll` reads; the registry values often suggested for this do not change with it.
pub fn do_not_disturb() -> bool {
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    // WNF_SHEL_QUIETHOURS_ACTIVE_PROFILE_CHANGED
    const STATE: u64 = 0x0D83_063E_A3BF_1C75;
    type Query = unsafe extern "system" fn(*const u64, *const core::ffi::c_void, *const core::ffi::c_void, *mut u32, *mut core::ffi::c_void, *mut u32) -> i32;
    unsafe {
        let Ok(ntdll) = GetModuleHandleW(w!("ntdll.dll")) else { return false };
        let Some(f) = GetProcAddress(ntdll, s!("NtQueryWnfStateData")) else { return false };
        let query: Query = std::mem::transmute(f);
        let name = STATE;
        let mut stamp = 0u32;
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = query(&name, std::ptr::null(), std::ptr::null(), &mut stamp, &mut value as *mut u32 as *mut _, &mut size);
        status >= 0 && size >= 4 && (value == 1 || value == 2)
    }
}

/// Windows' "Animation effects" (Settings > Accessibility > Visual effects) is on.
pub fn animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut on = windows::core::BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}
