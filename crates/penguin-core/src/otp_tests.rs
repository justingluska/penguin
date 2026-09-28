//! Fixtures are fictional: invented services and codes that mimic the
//! formats real verification mail uses (see the module docs in otp.rs).

use super::*;
use crate::types::{Address, Message, OtpKind};

fn message(id: &str, subject: &str) -> Message {
    Message {
        account_id: "me@example.com".into(),
        id: id.into(),
        thread_id: id.into(),
        date: 0,
        from: Address {
            name: None,
            email: "no-reply@service.example".into(),
        },
        to: vec![],
        cc: vec![],
        bcc: vec![],
        reply_to: vec![],
        subject: subject.into(),
        snippet: String::new(),
        body_text: String::new(),
        body_html: None,
        label_ids: vec!["INBOX".into()],
        attachments: vec![],
        message_id_header: None,
        in_reply_to: None,
        references: vec![],
        list_unsubscribe: None,
        list_unsubscribe_post: None,
        sender_authenticated: true,
    }
}

/// (subject, body, expected code). "LINK" = a magic sign-in link.
const POSITIVE: &[(&str, &str, &str)] = &[
    ("0357", "A one-time Rydeo code has been created for you. \u{34f} \u{34f} \u{34f} Tap to continue.", "0357"),
    ("Your single-use code", "Hi fin@example.com, We received your request for a single-use code to use with your Northwind account. Your single-use code is: 0418273", "0418273"),
    ("Your OTP Code is 7206", "Hello, To complete your login, please enter the following One-Time Password (OTP): 7206 If you did not request this code, contact support.", "7206"),
    ("Your requested Lumen email verification code", "Hi Sam, 381904 is your Lumen email verification code. If you did not make this request, contact us.", "381904"),
    ("552017 Is Your Email Update Verification Code", "552017 Use this code to verify your identity. Once sent, this verification code is valid for 30 minutes.", "552017"),
    ("Your Chirp confirmation code is kd73jq2x", "We noticed an attempt to log in to your account that seems suspicious. Was this you?", "kd73jq2x"),
    ("Tallyhall confirmation code: Q7T-MWX", "Confirm your email address. Here's your confirmation code. You can copy it into the open browser window.", "Q7T-MWX"),
    ("Tallyhall confirmation code: RVB-KOZ", "Confirm your email address. Your confirmation code is below.", "RVB-KOZ"),
    ("Tallyhall confirmation code: HQX-508", "Confirm your email address.", "HQX-508"),
    ("Gamebox ID Login [614 208]", "614 208 - Use the verification code below to log in. Welcome back!", "614208"),
    ("08KD4M is your verification code", "You're nearly there! Your verification code is: 08KD4M Enter this code to continue.", "08KD4M"),
    ("Session Verification", "Log In Pat Doe, Here is the security code needed to log into your Serverly account: 9ac03f71 We sent you this message because a device logged in.", "9ac03f71"),
    ("New Login Alert (code: 480216)", "There was a new login attempt on your account from an unrecognized device.", "480216"),
    ("Your Orbital verification code", "Use code 902475 to verify your identity with Orbital. Do not share this code with anyone 902475", "902475"),
    ("Login security code", "Hi Pat, Your one time login code for logging into app.example.com is 731550. You are signing in from Springfield on macOS.", "731550"),
    ("264019 is your Budgetly login code", "264019 is your login code. Enter this code to log in. This code will expire in an hour.", "264019"),
    ("Verify your email address", "Your verification code is 617302. This code expires in 10 minutes. If you didn't sign up, ignore this email.", "617302"),
    ("Your code is: 305518", "THANKS FOR CREATING AN ACCOUNT WITH US. Please use the below code. YOUR CODE IS: 305518", "305518"),
    ("Verify it's you using this security code", "Hi Sam, We received your request for a one-time code to activate two-step authentication. 840127 The code expires in 60 minutes.", "840127"),
    ("One Time Passcode", "Hello, Please enter the following passcode on our website to continue with your login. One Time Passcode: 57093 Please note it expires.", "57093"),
    ("领事服务验证码提醒 Verification Code", "您好，请查看以下验证码并在服务页面中输入以完成验证。验证码：204518（切勿将验证码告知他人）。有效时间：5分钟。", "204518"),
    ("Cloudgate Access login code for app.example.com", "Your login code: 663012 Your Cloudgate Access code 663012 This code expires after 10 minutes.", "663012"),
    ("Your Mapview Verification Code", "We've received a request to access your Mapview account. Your Mapview verification code is: 118934 If you did not request this, ignore it.", "118934"),
    ("Your Studio Verification Code", "Welcome to Studio! Below is your code 772019 To verify your email address, please copy the code above.", "772019"),
    ("One-time security code", "Hello, We created a one-time security code for you to use. Your security code is 450982. Please enter it now.", "450982"),
    ("Here's your verification code", "Skyline Rewards: Your Skyline verification code is 239870. This code will expire in 5 minutes.", "239870"),
    ("Sign in to Jobly with code: 530917", "Your Jobly code is 530917.", "530917"),
    ("408816 is your Eventful sign-in code", "408816 is your Eventful sign-in code. If you didn't attempt to sign in, you can ignore this email.", "408816"),
    ("Please verify your email address", "Your verification code. To complete your sign-up, please enter the following verification code in the signup form: 7XKQ2MPD This code will expire.", "7XKQ2MPD"),
    ("Your one-time passcode is 603117", "The passcode you requested is enclosed. Your one-time, time-sensitive passcode is 603117.", "603117"),
    ("Snapgram Login Code: 048273", "Enter this code in your app to log into your account: 048273 Please don't share this code.", "048273"),
    ("Bookly - K4PQ72 is your verification code", "Verify email and create an account. You just requested a verification code.", "K4PQ72"),
    ("Your Account Access Code", "You recently tried to sign in to your account. Complete your sign-in with the security code below. This code will expire 30 minutes after you receive this email. Your security code: 390652", "390652"),
    ("Verify Your Tax Portal Account", "To verify, enter the One Time Access (OTA) code on the portal sign-in page. This code will expire in 20 minutes (Valid till 02/01/2026 01:53 PM EST) 551029 Thank you", "551029"),
    ("G-482913 is your Gmail verification code", "Use this code to verify it's you.", "G-482913"),
    ("Security code", "Please use the following security code for the Contoso account j***@example.com. Security code: 5617", "5617"),
    ("Your PIN", "Your temporary PIN: 8841. Enter it at the kiosk to sign in.", "8841"),
    ("Código de verificación", "Apreciado usuario: Para completar su registro se ha generado un código de verificación: Su código es: 4439", "4439"),
    ("Verify your PostHedge login", "We got a login attempt. To continue, please enter this verification code on the login page: 151924", "151924"),
    ("Your Northbank one-time passcode", "Do not share this code. Your one-time passcode is 40718325. It expires in 10 minutes.", "40718325"),
    ("Your code", "Use this code to continue. Rail Rewards Pat Doe # 9530271111 | My Account Dear Pat, Use this code below to complete the verification request. 297040 This code expires soon.", "297040"),
    ("Your SafePay Verification Code", "Do not share this code with anyone. Account ending: 02008 Below is your SafePay Verification Code: 718263", "718263"),
    ("Login Verification", "Hello Pat, Please verify your computer by entering the code shown below: Your verification code is: 9834 Thank you, Support Team 302 E Carson Avenue, STE 1061 Las Vegas, Nevada 89101", "9834"),
    ("123-456 is your Fastpay code", "Don't share it.", "123-456"),
    ("Sign in to Notable", "Click below to sign in. This link expires shortly and can only be used once. Sign in", "LINK"),
    ("Secure link to log in to Parrot.ai", "Let's get you signed in. Sign in with the secure link below. If you didn't request this email, you can safely ignore it.", "LINK"),
    ("Your login request to Quanta", "Click the button below to log into Quanta. Your link expires in 1 hour. Log in", "LINK"),
    ("Trying to sign in to Chatter? Use this password-free link.", "Instantly sign in with the button below. If you didn't request this, ignore it.", "LINK"),
];

/// (subject, body) that must yield nothing.
const NEGATIVE: &[(&str, &str)] = &[
    ("Your order #48213 has shipped", "Tracking number 1Z999AA10123456784. Order 482131 will arrive Tuesday.", ),
    ("Invoice 20931 from Acme", "Invoice number: 20931. Amount due $1,250.00 by 10/15/2026. Pay online.", ),
    ("Welcome to Encore! Enjoy 15% off", "Get 15% off reg. price styles with your code SAVE15NOW at checkout.", ),
    ("Use Promo Code NTD2026 For 15% Off", "Tomorrow is National Train Day! Use Promo Code NTD2026 for 15% off everything.", ),
    ("Here's your presale code for The Echoes!", "The presale starts Wednesday, June 17, 2026 at 10 AM. Your code doesn't guarantee tickets.", ),
    ("Login from a new device detected on 24 September 2026, 05:40 EDT", "Communication code: MOONFISH. Your account for the world's money. We noticed a login from a new device.", ),
    ("New login attempt from Portugal", "Communication code: MOONFISH. If this was you, no action is needed.", ),
    ("Did you sign in from a new device?", "Help us protect your account. We noticed a sign-in on Chrome, macOS at 10:42 AM.", ),
    ("[Codehub] Please review this sign in", "Your account was signed in to but we did not recognize the location of the sign in.", ),
    ("New Device Login", "We noticed a login from a new device or browser. If this wasn't you, reset your password.", ),
    ("Re: Your verification code 230941", "Hello, I still can't get in. On Wed, Jan 7, 2026 someone wrote: Your code is 230941", ),
    ("Fwd: Confirm your sign-in", "---------- Forwarded message ---------- Your code is 551122", ),
    ("RE: Verification Code", "Hi team, I am having trouble receiving the verification code in my email. Please assist.", ),
    ("Verification code difficulties", "Good day, I am having a problem with the verification code process. Please help.", ),
    ("2-Step Verification turned on", "Your account pat@example.com is now protected with 2-Step Verification.", ),
    ("[Codehub] Your two-factor authentication recovery codes were viewed", "Your two-factor recovery codes were viewed on January 28, 2026 at 21:11 UTC.", ),
    ("Account verification lost for the site example.com", "You have lost the ownership status of https://example.com/ because we were unable to verify it.", ),
    ("Account Submitted for Verification", "Hi! Thank you for the feedback. I already changed the necessary adjustments on my profile.", ),
    ("Your flight confirmation code: KX7P2Q", "Thanks for booking your flight. Your confirmation code is KX7P2Q. Check in 24 hours before departure.", ),
    ("Reservation confirmed", "Your hotel reservation number is 88213. Check-in 3:00 PM on 10/12/2026.", ),
    ("Meeting at 1400", "Let's meet at 1400 in room 2041 to go over the Q3 plan.", ),
    ("Call me", "My number is (954) 555-0142, or +1 954 555 0142. Code review later?", ),
    ("Your receipt from Beanery", "Total $12.50. Card ending in 4242. Transaction 509182. Use code COFFEE10 next time for 10% off.", ),
    ("Your package is ready: get your pickup code", "Pick up codes are valid for 60 minutes. Click here when you're at the location to generate a pickup code.", ),
    ("Claude Code tips for your team", "Code review in 2026: here are 5 tips. Version 2.1.0 ships 30000 new tests.", ),
    ("Quarterly report 2026", "Revenue grew 12% to $1,200,000 in 2026. See page 4 for the 2025 comparison.", ),
    ("Password reset", "We received a request to reset your password. Click the link below to choose a new one.", ),
    ("Your Gamecenter account login details", "Username: pat. To set your password, visit https://example.com/wp-login.php?login=pat&key=x5P9 1234", ),
    ("Join our webinar on 2FA", "Two-factor authentication matters. Join us September 30 at 11 AM ET. Register now, seats are limited to 500 people.", ),
    ("Shipping update", "Your zip code 10001 is eligible for same-day delivery. Order 772910 is out for delivery.", ),
    ("Your Lifeflights number", "Your Lifeflights number: 59982735201. There has been a recent login on your account.", ),
    ("Your client number", "Your client number: K0534410125. Please log in to review your invoice 20931.", ),
    ("The Q3 PIN pad rollout", "The new PIN pads ship on 10/02. Store 4412 goes first, then store 5520.", ),
    ("Error code 5003 on checkout", "Customers see error code 5003 when paying. Can you look into it?", ),
    ("Your Nimbus bill is ready", "Account 88213377 balance $54.20 due 10/01. Sign in to view your statement.", ),
    ("Verify your contact info", "If your domain contact information is up to date, you're good to go. Open this email to get started.", ),
    ("Sign in from new device", "Sign in from new device \u{34f} \u{34f} \u{34f} We noticed a sign-in from Safari on iOS.", ),
    ("New Casewise contact message — Deploy verification", "Someone reached out through the contact form. Name: Pat Doe. Message: testing 1234.", ),
    ("Order confirmation code ABC-123", "Your order confirmation code ABC-123 is ready. Thanks for shopping!", ),
    ("Login to Access the Community launch", "Hi Pat, thanks for registering! Use the link below to manage your registration.", ),
    ("Here's your pass to sign in at Lobby 5", "Sign in when you arrive. Here's your pass to visit on Thursday, July 16 at 5:00 pm.", ),
    ("brainyskills code", "Brainyskills code #GJ^6:X/A(C](8T! Sincerely, Pat", ),
    ("Your recent login to Tradewell", "We're confirming your recent login to Tradewell on September 23, 2026.", ),
    ("Room 1204 door code", "Hi! See you Friday. The venue opens at 1800.", ),
    ("Board meeting minutes", "Motion 2231 passed 5-2. Next meeting on 10/14/2026 at 7:30 PM, room 1204.", ),
    ("Thanks! Your booking is confirmed at Harbor Inn", "Confirmation: 4410982231 PIN: 7312 (Confidential) Your booking in Lisbon is confirmed.", ),
    ("Italia Rail - Booking Confirmation", "Booking contact: Pat Doe. Ticket code: KX44PQ Departure 10/12 Time 08:15", ),
    ("Tonight only: the comedy special", "Join us for the new special. Use code LAUGH24 at checkout for special pricing.", ),
    ("[pat/standup-app] Dev to main (PR #12)", "Commit Summary 4f2a91c Worked on settings code 3e9b7d2 Worked on time tracking", ),
    ("Invitation: Pat <> Sam @ Wed Jul 8", "Join Zoom Meeting https://zoom.example/j/123 Meeting ID: 812 4410 9921 Passcode: 551902", ),
    ("You've set your Debit Card PIN", "Retain this PIN. Account ending: 02008 Your Business Debit Card PIN has been set up.", ),
    ("A record-breaking year for Code.club", "Hour of Code 2025 is a wrap! Sign in to see your impact.", ),
    ("A record-breaking year", "Hour of Code 2025 is a wrap! Thanks to everyone who held an Hour of Code.", ),
    ("4471920 - Pat Doe - Employment Verification", "Please see the attached request form to verify employment dates for the candidate.", ),
    ("[FOR-412] Resolved: workflow with built-in verification", "The issue has been resolved and verified in production.", ),
    ("Accepted: Pat <> Sam sync @ Fri Feb 6", "Sam has accepted this invitation. Join with Google Meet meet.google.com/abc-defg-hij Join by phone +1 555-010-2000 PIN: 440192831#", ),
    ("Your SafePay Verification Code", "Do not share this code with anyone PAT DOE Account ending: 71003 Below is your SafePay Verification Code", ),
    ("Updated pricing", "Plans now start at $1299 per year, or 1499 EUR. Contact sales.", ),
];

#[test]
fn positives() {
    let mut misses = Vec::new();
    for (subject, body, want) in POSITIVE {
        let got = detect(subject, body, true, 0);
        let ok = match (&got, *want) {
            (Some(o), "LINK") => o.kind == OtpKind::Link,
            (Some(o), w) => o.kind == OtpKind::Code && o.code.as_deref() == Some(w),
            (None, _) => false,
        };
        if !ok {
            misses.push(format!("{subject:?} -> {got:?}, want {want}"));
        }
    }
    assert!(
        misses.is_empty(),
        "{} of {} missed:\n{}",
        misses.len(),
        POSITIVE.len(),
        misses.join("\n")
    );
}

#[test]
fn negatives() {
    let mut hits = Vec::new();
    for (subject, body) in NEGATIVE {
        if let Some(o) = detect(subject, body, true, 0) {
            hits.push(format!("{subject:?} -> {o:?}"));
        }
    }
    assert!(
        hits.is_empty(),
        "{} of {} false positives:\n{}",
        hits.len(),
        NEGATIVE.len(),
        hits.join("\n")
    );
}

#[test]
fn fixture_counts() {
    assert!(POSITIVE.len() >= 40 && NEGATIVE.len() >= 40);
}

#[test]
fn verified_follows_sender_authentication() {
    let o = detect("Your code", "Your verification code is 482913.", false, 7).unwrap();
    assert!(!o.verified);
    assert_eq!(o.date, 7);
    assert!(
        detect("Your code", "Your verification code is 482913.", true, 7)
            .unwrap()
            .verified
    );
}

#[test]
fn quoted_history_and_sent_mail_are_ignored() {
    let mut m = message("m1", "Thanks!");
    m.body_text =
        "Got it, thanks!\n\nOn Tue, Pat wrote:\n> Your verification code is 482913".into();
    assert_eq!(detect_message(&m), None);
    m.body_text = "Your verification code is 482913".into();
    assert!(detect_message(&m).is_some());
    m.label_ids = vec!["SENT".into()];
    assert_eq!(detect_message(&m), None);
}

#[test]
fn headers_only_mail_uses_the_snippet() {
    let mut m = message("m1", "Your login code");
    m.body_text = String::new();
    m.snippet = "Enter 604417 to sign in. It expires in 10 minutes.".into();
    // "Enter 604417 to sign in" has no label next to the code but the
    // trigger "code" is in the subject only, so this stays undetected…
    assert_eq!(detect_message(&m), None);
    m.snippet = "Your login code is 604417. It expires in 10 minutes.".into();
    assert_eq!(
        detect_message(&m).and_then(|o| o.code),
        Some("604417".into())
    );
}

#[test]
fn loose_shapes_need_an_explicit_label() {
    let code = |s: &str, b: &str| detect(s, b, true, 0).and_then(|o| o.code);
    assert_eq!(
        code(
            "Your Fable verification code is QWEX",
            "Are you trying to sign in?"
        ),
        Some("QWEX".into())
    );
    assert_eq!(
        code(
            "Your temporary Noted login code is amble-tovi-ranok-sute",
            ""
        ),
        Some("amble-tovi-ranok-sute".into())
    );
    assert_eq!(
        code(
            "Your Lynx confirmation code",
            "Here's your Lynx confirmation code: DtUy7k"
        ),
        Some("DtUy7k".into())
    );
    assert_eq!(code("Your Mailbird verification code", "If you are attempting to log in, here is your verification code: 4410928312 This code will expire in 1 hour."), Some("4410928312".into()));
    assert_eq!(
        code(
            "Your requested One Time Passcode",
            "To proceed, please enter your One-Time Password abcd-551902, before it expires."
        ),
        Some("abcd-551902".into())
    );
    assert_eq!(code("Email OTP", "Here is the One Time Passcode (OTP) for your access request OTP:551902 This one-time code is time sensitive."), Some("551902".into()));
    assert_eq!(
        code(
            "Your confirmation code from Fintap",
            "Enter the code below online or in the app 551-902 Use the above code to proceed."
        ),
        Some("551-902".into())
    );
    assert_eq!(
        code(
            "Your account verification code",
            "Your safety comes first. SKYAIR 551902 Use this code to verify your account."
        ),
        Some("551902".into())
    );
    // Unlabeled or merely "code X": no.
    assert_eq!(code("Your code", "USE THE CODE BELOW TO SIGN IN"), None);
    assert_eq!(
        code("Sign in", "Your sign-in code QWEX was sent to your phone."),
        None
    );
}
