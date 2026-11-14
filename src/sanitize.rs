//! Cleaning what a service said before the app shows it: a service may echo
//! the request's credentials, or carry tokens of its own.

/// The longest message kept, in characters.
const LONGEST: usize = 300;

const REDACTED: &str = "[redacted]";

/// `message` with every key-shaped thing replaced by `[redacted]`: each of
/// `secrets` (of 4 characters or more), a credential-looking word after
/// `Key`, `Bearer`, `Token` or `Basic`, Replicate tokens (`r8_…`), and long runs of letters
/// and digits such as hex or base64 (32 characters or more, or 16 or more
/// mixing letters and digits). Control characters become spaces, runs of
/// whitespace one space, and the result is at most 300 characters.
pub fn sanitize(message: &str, secrets: &[&str]) -> String {
    let mut text = message.to_string();
    for secret in secrets {
        let secret = secret.trim();
        if secret.chars().count() >= 4 {
            text = text.replace(secret, REDACTED);
        }
    }
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut after_scheme = false;
    for word in text.split_whitespace() {
        let mut cleaned = String::new();
        let mut token = String::new();
        let flush = |token: &mut String, cleaned: &mut String, after_scheme: bool| {
            if !token.is_empty() {
                let credential = after_scheme && after_scheme_shaped(token);
                let keep = token == "redacted" || !(credential || key_shaped(token));
                cleaned.push_str(if !keep { REDACTED } else { token });
                token.clear();
            }
        };
        for c in word.chars() {
            if is_token_char(c) {
                token.push(c);
            } else {
                flush(&mut token, &mut cleaned, after_scheme);
                cleaned.push(c);
            }
        }
        flush(&mut token, &mut cleaned, after_scheme);
        let bare = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        after_scheme = ["key", "bearer", "token", "basic"]
            .iter()
            .any(|s| bare.eq_ignore_ascii_case(s));
        out.push(cleaned);
    }
    let joined = out.join(" ");
    if joined.chars().count() <= LONGEST {
        joined
    } else {
        let mut cut: String = joined.chars().take(LONGEST - 1).collect();
        cut.push('…');
        cut
    }
}

/// The characters keys and tokens are made of.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '/' | '=' | ':' | '.')
}

/// Whether a token after `Key`, `Bearer`, … is the credential rather than
/// the next word of a sentence ("the key is invalid").
fn after_scheme_shaped(token: &str) -> bool {
    token.chars().any(|c| !c.is_ascii_alphabetic()) || token.chars().count() >= 20
}

fn key_shaped(token: &str) -> bool {
    let length = token.chars().count();
    let digits = token.chars().any(|c| c.is_ascii_digit());
    let letters = token.chars().any(|c| c.is_ascii_alphabetic());
    token.starts_with("r8_")
        || token.starts_with("sk-")
        || length >= 32
        || (length >= 16 && digits && letters)
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn keys_and_key_shaped_runs_go() {
        let key = "sk-SENTINEL-1234567890abcdef";
        let said = format!("bad key {key} in header Key {key}");
        let clean = sanitize(&said, &[key]);
        assert!(!clean.contains("SENTINEL"), "{clean}");
        assert_eq!(clean, "bad key [redacted] in header Key [redacted]");
        assert_eq!(
            sanitize("Authorization: Bearer abc.def", &[]),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(
            sanitize("token r8_abcdef is unknown", &[]),
            "token [redacted] is unknown"
        );
        let fal = "3f2a1b4c-1111-2222-3333-444455556666:0123456789abcdef0123456789abcdef";
        assert_eq!(sanitize(&format!("\"{fal}\""), &[]), "\"[redacted]\"");
        assert_eq!(
            sanitize(
                "Request was throttled.\nExpected available in 1 second.",
                &[]
            ),
            "Request was throttled. Expected available in 1 second."
        );
        assert_eq!(
            sanitize("The API key is invalid", &[]),
            "The API key is invalid"
        );
        assert_eq!(sanitize(&"a ".repeat(400), &[]).chars().count(), 300);
    }
}
