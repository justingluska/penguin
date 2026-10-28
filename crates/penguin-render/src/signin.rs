//! Finds the button in a one-time sign-in email ("Sign in to Rydeo",
//! "Your login link") so the app can offer it as one tap. penguin-core's
//! `otp` decides whether a message *is* such an email (subject, body, the
//! sender's authentication); this finds *which* link it means.
//!
//! Input is our own `render_html` / `render_text` output (see
//! unsubscribe.rs for why a small scanner is enough). A link counts when,
//! best first:
//! 1. its own text says sign in ("Sign in", "Log in to Rydeo", "Magic
//!    link", "Iniciar sesión");
//! 2. its own text is a generic action ("Verify", "Confirm", "Continue",
//!    "Click here") and the text around it says sign in;
//! 3. it is the only link left once boilerplate (unsubscribe, privacy,
//!    terms, help, the sender's home page) is set aside.
//!
//! Within a rank the first link wins: the button sits above the footer.
//! Only https links are returned. The caller maps the result back to the
//! link as the sender wrote it (`RenderedHtml::original_link`): a magic
//! link's token must survive tracking-parameter stripping.

use crate::unsubscribe::{anchors, contains_phrase, is_web_url, normalize, UNSUBSCRIBE_PHRASES};

/// Link text that means "sign in", lowercase, hyphens as spaces.
const SIGN_IN: &[&str] = &[
    "sign in",
    "signin",
    "sign me in",
    "log in",
    "login",
    "log me in",
    "magic link",
    "sign in link",
    "login link",
    "iniciar sesión",
    "inicia sesión",
    "iniciar sesion",
    "inicia sesion",
    "se connecter",
    "anmelden",
    "einloggen",
    "accedi",
    "entrar",
];

/// Generic button text: counts only next to sign-in wording.
const ACTION: &[&str] = &[
    "verify",
    "confirm",
    "continue",
    "click here",
    "open",
    "access",
    "get started",
    "go to",
    "here",
    "acceder",
    "confirmar",
    "verificar",
];

/// Links that are never the button.
const BOILERPLATE: &[&str] = &[
    "privacy",
    "terms",
    "help",
    "support",
    "contact",
    "faq",
    "learn more",
    "security",
    "report",
    "view in browser",
    "view online",
    "download",
    "app store",
    "google play",
    "privacidad",
    "ayuda",
];

/// Link text longer than this is a sentence, not a button.
const BUTTON_WORDS: usize = 8;

fn any(text: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| contains_phrase(text, p))
}

fn is_https(href: &str) -> bool {
    is_web_url(href)
        && href
            .trim()
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
}

fn is_unsubscribe(text: &str) -> bool {
    UNSUBSCRIBE_PHRASES
        .iter()
        .any(|(p, _)| contains_phrase(text, p))
}

/// The sign-in button's link in a rendered body, if one stands out.
pub fn find_sign_in_link(rendered: &str) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    let mut plain: Vec<String> = Vec::new();
    for a in anchors(rendered) {
        if !is_https(&a.href) {
            continue;
        }
        let text = normalize(&a.text);
        if is_unsubscribe(&text) || any(&text, BOILERPLATE) {
            continue;
        }
        let words = text.split(' ').filter(|w| !w.is_empty()).count();
        let around = format!("{} {}", normalize(&a.before), normalize(&a.after));
        let rank = if words <= BUTTON_WORDS && any(&text, SIGN_IN) {
            0
        } else if words <= BUTTON_WORDS && any(&text, ACTION) && any(&around, SIGN_IN) {
            1
        } else {
            if !plain.contains(&a.href) {
                plain.push(a.href.clone());
            }
            continue;
        };
        // `<`: the first link of a rank wins (the button, not the footer).
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, a.href));
        }
    }
    best.map(|(_, href)| href).or_else(|| {
        // A plain-text email: the one link that's left is the button.
        (plain.len() == 1).then(|| plain.remove(0))
    })
}

#[cfg(test)]
#[path = "signin_tests.rs"]
mod tests;
