//! `shep bridge pair` — hand this phone the bridge's URL and token.
//!
//! Two ways in, one credential. The QR carries `shep://pair?url=&token=` and
//! is the fast path when a camera is at hand. The **claim code** is the path
//! for everything else: eight characters read off the screen and typed into
//! the phone, which then fetches the real token over the same WebSocket.
//!
//! The code is deliberately weak on its own — 8 characters of a 32-symbol
//! alphabet is 40 bits — and is made safe by everything around it: it exists
//! only while this command is running, expires after
//! [`CODE_TTL`], works exactly once, and a wrong guess costs the same
//! growing per-address backoff as a wrong token. A guessed *code* is also
//! worth strictly less than a guessed token: claiming ends the window, so the
//! real owner's pairing fails loudly instead of silently sharing access.
//!
//! A wrong code never deletes the file. Otherwise anyone who can reach the
//! port could cancel a pairing window they cannot use.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{constant_time_eq, load_or_create_token, DEFAULT_BIND, USAGE};

/// No `0`/`O`/`1`/`I`: the code is read off one screen and typed into another.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const CODE_LEN: usize = 8;
/// How long a printed code stays claimable.
const CODE_TTL: Duration = Duration::from_secs(300);
/// How often `pair` looks to see whether the phone has claimed the code.
const POLL: Duration = Duration::from_millis(500);

pub(super) fn code_path() -> PathBuf {
    crate::config::config_dir().join("bridge-pair-code")
}

/// Everything a pairing screen shows, and the armed claim window behind it.
///
/// Built once here so the CLI and the TUI arm pairing identically: the same
/// token, the same single-use code in the same file the bridge reads, the same
/// five-minute window. A second implementation of this would be a second
/// security model.
pub(crate) struct PairingOffer {
    /// `ws://host:port/` — what the phone dials.
    pub url: String,
    /// The QR image as terminal rows, or `None` when it could not be rendered.
    pub qr: Option<String>,
    /// The claim code, grouped for reading aloud: `7K4M-9QP2`.
    pub code: String,
    /// Unix seconds at which the code stops working.
    pub expires_at: u64,
}

impl PairingOffer {
    /// Disarm the window. Leaving a claimable code behind for the rest of its
    /// TTL is the thing this exists to prevent — see the Ctrl-C handler in
    /// [`pair`], which does the same job for the CLI.
    pub(crate) fn cancel(&self) {
        std::fs::remove_file(code_path()).ok();
    }

    /// Whether the code is still good, so a screen showing it can stop.
    pub(crate) fn is_live(&self, now: u64) -> bool {
        now < self.expires_at
    }

    /// Whole minutes left, rounded up, for a screen to show. Zero means the
    /// window has closed.
    pub(crate) fn minutes_left(&self, now: u64) -> u64 {
        self.expires_at.saturating_sub(now).div_ceil(60)
    }

    /// Whether the phone has taken the code. `claim_at` deletes the file on a
    /// successful claim, so its absence is the signal.
    pub(crate) fn is_claimed(&self) -> bool {
        !code_path().exists()
    }
}

/// Mint a token if there is not one, render the QR, and arm a fresh claim code.
pub(crate) fn arm(host: &str) -> std::io::Result<PairingOffer> {
    let host = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:7431")
    };
    let token = load_or_create_token()?;
    let url = format!("ws://{host}/");
    let payload = format!(
        "shep://pair?url={}&token={}",
        percent_encode(&url),
        percent_encode(&token),
    );
    let qr = render_qr(&payload).ok();
    let code = generate_code()?;
    let expires_at = now_unix() + CODE_TTL.as_secs();
    write_code_at(&code_path(), &code, expires_at)?;
    Ok(PairingOffer {
        url,
        qr,
        code: format_code(&code),
        expires_at,
    })
}

/// Print the pairing info the companion app asks for (URL + token + code).
pub(super) fn pair(args: &[String]) -> std::io::Result<i32> {
    let mut host = None;
    let mut wait = true;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--host" => host = iter.next().cloned(),
            "--no-wait" => wait = false,
            _ => {
                eprintln!("{USAGE}");
                return Ok(2);
            }
        }
    }
    let host = host.unwrap_or_else(|| DEFAULT_BIND.to_string());
    let offer = arm(&host)?;
    match &offer.qr {
        Some(image) => println!("{image}"),
        None => eprintln!("(could not render QR; use the text values below)"),
    }
    println!("url: {}", offer.url);
    println!("token: {}", load_or_create_token()?);
    println!("scan the QR in the companion app, or paste both into the pairing screen.");
    println!();
    println!(
        "on the phone: enter {host} and the code {} (expires in {} min)",
        offer.code,
        CODE_TTL.as_secs() / 60,
    );
    if !wait {
        return Ok(0);
    }

    // Ctrl-C must not leave a claimable code behind for the rest of its TTL.
    ctrlc::set_handler(move || {
        std::fs::remove_file(code_path()).ok();
        std::process::exit(130);
    })
    .ok();

    let path = code_path();
    loop {
        std::thread::sleep(POLL);
        if !path.exists() {
            println!("paired");
            return Ok(0);
        }
        if read_code_at(&path, now_unix()).is_none() {
            std::fs::remove_file(&path).ok();
            eprintln!("code expired — run it again");
            return Ok(1);
        }
    }
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Eight characters from [`CODE_ALPHABET`], uniformly.
///
/// The alphabet has 32 symbols and 256 is a whole multiple of 32, so masking
/// each random byte to its low 5 bits is unbiased — no rejection loop needed.
fn generate_code() -> std::io::Result<String> {
    let mut bytes = [0u8; CODE_LEN];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes
        .iter()
        .map(|b| CODE_ALPHABET[(b & 0x1f) as usize] as char)
        .collect())
}

/// `7K4M9QP2` → `7K4M-9QP2`, which is what a person can read aloud.
fn format_code(code: &str) -> String {
    if code.len() != CODE_LEN {
        return code.to_string();
    }
    format!("{}-{}", &code[..4], &code[4..])
}

/// Anything a person might type back — dashes, spaces, lower case — reduced
/// to what [`claim_at`] compares.
pub(super) fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn write_code_at(path: &Path, code: &str, expires_at: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{code}\n{expires_at}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The live code at `path`, or `None` if there is none, it is malformed, or
/// it has expired. A file left behind by a killed `pair` is inert, not a
/// permanent back door.
fn read_code_at(path: &Path, now: u64) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let code = lines.next()?.trim().to_string();
    let expires_at: u64 = lines.next()?.trim().parse().ok()?;
    if code.len() != CODE_LEN || now >= expires_at {
        return None;
    }
    Some(code)
}

/// Whether `presented` claims the live code, consuming it if so.
///
/// A wrong code is refused and the window stays open — the guesser must not
/// be able to cancel a pairing they cannot complete.
pub(super) fn claim_at(path: &Path, presented: &str, now: u64) -> bool {
    let Some(code) = read_code_at(path, now) else {
        return false;
    };
    if !constant_time_eq(presented.as_bytes(), code.as_bytes()) {
        return false;
    }
    std::fs::remove_file(path).ok();
    true
}

/// Claim the live code in the config dir, if `presented` matches it.
pub(super) fn claim(presented: &str) -> bool {
    claim_at(&code_path(), presented, now_unix())
}

/// Render `payload` as a terminal QR (light modules on the dark background, an
/// inverted QR that scanners read fine). Kept dependency-light: the `qrcode`
/// crate's unicode renderer, no image backend.
fn render_qr(payload: &str) -> Result<String, qrcode::types::QrError> {
    use qrcode::render::unicode;
    let code = qrcode::QrCode::new(payload.as_bytes())?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// Percent-encode a query-parameter value (RFC 3986 unreserved set passes
/// through). The companion app decodes via `Uri.getQueryParameter`.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("shep-pair-{}-{name}", std::process::id()))
    }

    #[test]
    fn generated_codes_use_the_readable_alphabet() {
        for _ in 0..200 {
            let code = generate_code().expect("urandom");
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)), "{code}");
        }
    }

    #[test]
    fn a_code_survives_the_trip_through_a_person() {
        assert_eq!(format_code("7K4M9QP2"), "7K4M-9QP2");
        assert_eq!(normalize_code("7k4m-9qp2"), "7K4M9QP2");
        assert_eq!(normalize_code(" 7K4M 9QP2 "), "7K4M9QP2");
    }

    #[test]
    fn a_written_code_is_private_and_reads_back() {
        let path = temp_path("write");
        write_code_at(&path, "7K4M9QP2", 4_000_000_000).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(read_code_at(&path, 1).as_deref(), Some("7K4M9QP2"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_expired_code_is_inert() {
        let path = temp_path("expired");
        write_code_at(&path, "7K4M9QP2", 100).expect("write");
        assert!(read_code_at(&path, 100).is_none());
        assert!(!claim_at(&path, "7K4M9QP2", 101));
        // Still there — expiry is `pair`'s to clean up, not a claimant's.
        assert!(path.exists());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_code_works_exactly_once() {
        let path = temp_path("once");
        write_code_at(&path, "7K4M9QP2", 4_000_000_000).expect("write");
        assert!(claim_at(&path, "7K4M9QP2", 1));
        assert!(!claim_at(&path, "7K4M9QP2", 1));
        assert!(!path.exists());
    }

    #[test]
    fn a_wrong_code_leaves_the_window_open() {
        let path = temp_path("wrong");
        write_code_at(&path, "7K4M9QP2", 4_000_000_000).expect("write");
        assert!(!claim_at(&path, "AAAAAAAA", 1));
        assert!(path.exists());
        assert!(claim_at(&path, "7K4M9QP2", 1));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_code_is_not_claimable() {
        assert!(!claim_at(&temp_path("absent"), "7K4M9QP2", 1));
    }
}

#[cfg(test)]
mod offer_tests {
    use super::*;

    /// Point `config_dir()` at a fresh directory for the duration of a test.
    ///
    /// `arm` deliberately writes into the real config dir, because that file is
    /// the entire interface between the pairing screen and the bridge. That
    /// makes it shared mutable state between tests, so each one gets its own
    /// config home — nextest runs a process per test, so the env var is not
    /// racing anything.
    struct IsolatedConfig(std::path::PathBuf);

    impl IsolatedConfig {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "shep-pair-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("config home");
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            Self(dir)
        }
    }

    impl Drop for IsolatedConfig {
        fn drop(&mut self) {
            std::env::remove_var("XDG_CONFIG_HOME");
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn offer_expiring_at(expires_at: u64) -> PairingOffer {
        PairingOffer {
            url: "ws://100.83.179.75:7431/".to_string(),
            qr: None,
            code: "7K4M-9QP2".to_string(),
            expires_at,
        }
    }

    #[test]
    fn minutes_left_rounds_up_so_a_live_code_never_reads_as_zero() {
        let offer = offer_expiring_at(100);
        assert_eq!(
            offer.minutes_left(40),
            1,
            "one second left is still a minute"
        );
        assert_eq!(offer.minutes_left(0), 2);
        // And a dead code says zero rather than underflowing into a huge number.
        assert_eq!(offer.minutes_left(999), 0);
        assert!(!offer.is_live(999));
    }

    /// The code on screen must be the code the bridge would take. They meet
    /// only through a file, and the screen shows a grouped form (`7K4M-9QP2`)
    /// that the bridge never sees — so this is exactly where the two could
    /// drift apart without anything failing to compile.
    #[test]
    fn the_code_on_screen_is_the_code_the_bridge_accepts() {
        let _config = IsolatedConfig::new("claim");
        let path = code_path();

        let offer = arm("127.0.0.1").expect("arm");
        assert!(path.exists(), "arming did not leave a code for the bridge");
        assert!(
            claim_at(&path, &normalize_code(&offer.code), now_unix()),
            "the code shown on screen is not the one the bridge would accept"
        );
        // Claiming consumes it, which is what the screen watches for.
        assert!(offer.is_claimed());
    }

    #[test]
    fn cancelling_disarms_the_window() {
        let _config = IsolatedConfig::new("cancel");
        let offer = arm("127.0.0.1").expect("arm");
        assert!(code_path().exists());
        offer.cancel();
        assert!(
            !code_path().exists(),
            "closing the screen left a claimable code behind"
        );
        assert!(offer.is_claimed(), "a disarmed offer reads as finished");
    }

    #[test]
    fn the_offer_carries_a_dialable_url() {
        let _config = IsolatedConfig::new("url");
        // The port is implied when the host does not carry one, because the
        // phone's own default is 7431 and a mismatch here pairs nothing.
        assert_eq!(
            arm("100.83.179.75").expect("arm").url,
            "ws://100.83.179.75:7431/"
        );
        assert_eq!(
            arm("10.0.0.27:9999").expect("arm").url,
            "ws://10.0.0.27:9999/"
        );
    }

    /// The QR is the fast path, and it is a fixed size because the payload is:
    /// a URL plus a 43-character token. The overlay lays itself out around
    /// those numbers, so a change in either has to be noticed here.
    #[test]
    fn the_qr_is_the_size_the_pairing_screen_reserves() {
        let _config = IsolatedConfig::new("qr");
        let offer = arm("100.83.179.75").expect("arm");
        let image = offer.qr.clone().expect("a QR renders for a normal payload");
        let rows: Vec<&str> = image.lines().collect();
        let cols = rows.first().map(|row| row.chars().count()).unwrap_or(0);
        assert_eq!(
            (cols, rows.len()),
            (
                crate::ui::PAIR_QR_COLS as usize,
                crate::ui::PAIR_QR_ROWS as usize
            ),
            "the QR changed size; the pairing overlay reserves a fixed block for it"
        );
        offer.cancel();
    }
}
