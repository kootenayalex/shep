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
    let host = if host.contains(':') {
        host
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
    match render_qr(&payload) {
        Ok(image) => {
            println!("{image}");
        }
        Err(err) => eprintln!("(could not render QR: {err}; use the text values below)"),
    }
    println!("url: {url}");
    println!("token: {token}");
    println!("scan the QR in the companion app, or paste both into the pairing screen.");

    let code = generate_code()?;
    let path = code_path();
    let expires_at = now_unix() + CODE_TTL.as_secs();
    write_code_at(&path, &code, expires_at)?;
    println!();
    println!(
        "on the phone: enter {host} and the code {} (expires in {} min)",
        format_code(&code),
        CODE_TTL.as_secs() / 60,
    );
    if !wait {
        return Ok(0);
    }

    // Ctrl-C must not leave a claimable code behind for the rest of its TTL.
    let handler_path = path.clone();
    ctrlc::set_handler(move || {
        std::fs::remove_file(&handler_path).ok();
        std::process::exit(130);
    })
    .ok();

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

fn now_unix() -> u64 {
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
