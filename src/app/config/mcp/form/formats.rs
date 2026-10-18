// SPDX-License-Identifier: Apache-2.0

/// One dispatch table defines both supported format names and their validators.
pub(super) fn validator(format: &str) -> Option<fn(&str) -> bool> {
    match format {
        "date" => Some(|text| {
            text.len() == 10 && chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok()
        }),
        "date-time" => Some(|text| chrono::DateTime::parse_from_rfc3339(text).is_ok()),
        "uri" => Some(uri),
        "email" => Some(email),
        _ => None,
    }
}

fn uri(text: &str) -> bool {
    // Url uses browser normalization. A form answer must already be URI text,
    // including valid escapes, rather than silently becoming a different URI.
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            if !bytes.next().is_some_and(|b| b.is_ascii_hexdigit())
                || !bytes.next().is_some_and(|b| b.is_ascii_hexdigit())
            {
                return false;
            }
        } else if !(byte.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=".contains(&byte)) {
            return false;
        }
    }
    reqwest::Url::parse(text).is_ok()
}

fn email(text: &str) -> bool {
    // MCP's email format is an ASCII mailbox, including quoted local parts and
    // address literals. Internationalized mailboxes require a different format.
    if !text.is_ascii() || text.len() > 254 {
        return false;
    }
    let Some((local, domain)) = text.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.is_empty() {
        return false;
    }
    let local_valid =
        if let Some(quoted) = local.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            let mut bytes = quoted.bytes();
            let mut valid = true;
            while let Some(byte) = bytes.next() {
                if byte == b'\\' {
                    if !bytes.next().is_some_and(|b| (32..=126).contains(&b)) {
                        valid = false;
                        break;
                    }
                } else if byte == b'"' || !(32..=126).contains(&byte) {
                    valid = false;
                    break;
                }
            }
            valid
        } else {
            local.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~".contains(&b))
            })
        };
    let domain_valid =
        if let Some(literal) = domain.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if literal.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("IPv6:")) {
                literal[5..].parse::<std::net::Ipv6Addr>().is_ok()
            } else {
                literal.parse::<std::net::Ipv4Addr>().is_ok()
            }
        } else {
            domain.split('.').all(|part| {
                !part.is_empty()
                    && part.len() <= 63
                    && part.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                    && part.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        };
    local_valid && domain_valid
}
