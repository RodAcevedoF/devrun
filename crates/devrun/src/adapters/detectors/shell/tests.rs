#![allow(clippy::unwrap_used)]

use std::process::Command;

use super::{join, quote};

const HOSTILE: [&str; 27] = [
    "$0",
    "a;b",
    "a&b",
    "a|b",
    "a>b",
    "a<b",
    "(x)",
    "a`b",
    "a\\b",
    "{a,b}",
    "#x",
    "?",
    "[ab]",
    "$(touch pwned)",
    "`touch pwned`",
    "a;touch pwned",
    "x && touch pwned",
    "x|touch pwned",
    "it's \"q\" $HOME",
    "name with space",
    "*",
    "~root",
    "ünïcødé✓",
    "a'b'$(touch pwned)'",
    "--filter=web$(touch pwned)'x^...",
    "-leading",
    "",
];

#[test]
fn hostile_words_reach_the_program_verbatim_through_sh() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b"), "").unwrap();
    let line = join(["printf", "%s\\n"].into_iter().chain(HOSTILE)).unwrap();
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(&line)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{line}");
    let printed = String::from_utf8(output.stdout).unwrap();
    assert_eq!(printed.lines().collect::<Vec<_>>(), HOSTILE, "{line}");
    assert!(!dir.path().join("pwned").exists(), "{line}");
}

#[test]
fn newline_carriage_return_and_nul_are_rejected() {
    for word in ["a\nb", "a\rb", "a\0b"] {
        assert!(quote(word).is_err(), "{word:?}");
    }
}
