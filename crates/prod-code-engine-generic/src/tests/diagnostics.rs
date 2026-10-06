/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::{Duration, Instant};

use crate::diagnostics::{DiagnosticsUnavailable, Published, Sent, Unavailable, gap};

#[test]
fn a_publication_answers_for_the_last_text_only_when_it_covers_it() {
    let now = Instant::now();
    let later = now + Duration::from_millis(5);
    let published = |version, at| Published {
        version,
        at,
        items: Vec::new(),
    };
    let sent = |version, at| Sent { version, at };
    let covers = |p: &Published, s: Option<&Sent>, versioned| gap(p, s, versioned).is_none();
    assert_eq!(
        gap(&published(Some(1), later), Some(&sent(Some(2), now)), true),
        Some(Unavailable::OlderVersion {
            published: 1,
            sent: 2
        })
    );
    assert_eq!(
        gap(&published(None, later), None, true),
        Some(Unavailable::Unversioned { sent: None }),
        "from clangd, an unversioned publication with nothing open is a closed document's"
    );
    assert!(covers(&published(None, later), None, false));
    assert_eq!(
        gap(&published(None, now), Some(&sent(Some(2), later)), false),
        Some(Unavailable::Earlier { sent: Some(2) }),
        "without a version, only the order of arrival is known"
    );
    for versioned in [false, true] {
        assert!(
            covers(&published(Some(1), now), None, versioned),
            "nothing sent"
        );
        assert!(
            !covers(
                &published(Some(1), later),
                Some(&sent(Some(2), now)),
                versioned
            ),
            "the text before the change, even when it arrives after it"
        );
        assert!(covers(
            &published(Some(2), later),
            Some(&sent(Some(2), now)),
            versioned
        ));
        assert!(
            !covers(
                &published(Some(3), later),
                Some(&sent(Some(2), now)),
                versioned
            ),
            "a late version from a closed opening cannot cover the reopened text"
        );
        assert!(
            !covers(
                &published(None, now),
                Some(&sent(Some(2), later)),
                versioned
            ),
            "without a version, one from before the text was sent does not"
        );
        assert!(covers(
            &published(Some(1), later),
            Some(&sent(None, now)),
            versioned
        ));
    }
    assert!(
        covers(&published(None, later), Some(&sent(Some(2), now)), false),
        "from a server without versions, one that arrived after the text covers it"
    );
    assert!(
        !covers(&published(None, later), Some(&sent(Some(2), now)), true),
        "from clangd, a publication without a version is a closed document's"
    );
}

/// Every reason says which document has no report and why, and none claims a version the
/// server did not give.
#[test]
fn a_missing_report_says_why_for_which_document() {
    let said = |kind| {
        DiagnosticsUnavailable {
            uri: "file:///w/a.c".to_string(),
            waited: Duration::from_millis(20),
            kind,
        }
        .to_string()
    };
    for (kind, reason) in [
        (Unavailable::NeverPublished, "no text of it was sent"),
        (
            Unavailable::NotPublished { sent: Some(1) },
            "published nothing for the text last sent (version 1) within 20 ms",
        ),
        (
            Unavailable::OlderVersion {
                published: 1,
                sent: 2,
            },
            "last publication is for version 1, older than version 2 last sent",
        ),
        (
            Unavailable::Unversioned { sent: None },
            "carries no version, as one for a closed document does; no text of it is open",
        ),
        (
            Unavailable::Unversioned { sent: Some(3) },
            "none for the text last sent (version 3) came within 20 ms",
        ),
        (
            Unavailable::Earlier { sent: None },
            "arrived before the text last sent, and none came after it within 20 ms",
        ),
        (
            Unavailable::UnexpectedVersion {
                published: 3,
                sent: 1,
            },
            "published version 3, but the current text is version 1",
        ),
        (Unavailable::ServerExited, "exited before it published"),
    ] {
        let text = said(kind);
        assert!(
            text.starts_with("no current diagnostics for file:///w/a.c: "),
            "{text}"
        );
        assert!(text.contains(reason), "{reason}: {text}");
        assert!(
            text.ends_with("does not mean the document has no errors"),
            "{text}"
        );
    }
    assert!(!said(Unavailable::Earlier { sent: Some(2) }).contains("version 1"));
}
