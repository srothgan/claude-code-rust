#![cfg(feature = "serde")]

use tui_textarea::{
    AtomicCursorBias, AtomicDeleteDirection, AtomicRange, AtomicRangeError,
    AtomicRangeRejectReason, CursorMove, Input, Key, RejectedAtomicRange, Scrolling,
};

#[test]
fn test_serde_key() {
    let k = Key::Char('a');
    let s = serde_json::to_string(&k).unwrap();
    assert_eq!(s, r#"{"Char":"a"}"#);
    let d: Key = serde_json::from_str(&s).unwrap();
    assert_eq!(d, k);
}

#[test]
fn test_serde_input() {
    let i = Input {
        key: Key::Char('a'),
        ctrl: true,
        alt: false,
        shift: true,
    };
    let s = serde_json::to_string(&i).unwrap();
    assert_eq!(
        s,
        r#"{"key":{"Char":"a"},"ctrl":true,"alt":false,"shift":true}"#,
    );
    let d: Input = serde_json::from_str(&s).unwrap();
    assert_eq!(d, i);
}

#[test]
fn test_serde_scrolling() {
    let scroll = Scrolling::Delta { rows: 1, cols: 2 };
    let s = serde_json::to_string(&scroll).unwrap();
    assert_eq!(s, r#"{"Delta":{"rows":1,"cols":2}}"#);
    let d: Scrolling = serde_json::from_str(&s).unwrap();
    assert_eq!(d, scroll);
}

#[test]
fn test_serde_cursor_move() {
    let c = CursorMove::Forward;
    let s = serde_json::to_string(&c).unwrap();
    assert_eq!(s, r#""Forward""#);
    let d: CursorMove = serde_json::from_str(&s).unwrap();
    assert_eq!(d, c);
}

#[test]
fn test_serde_atomic_types() {
    let range = AtomicRange {
        row: 1,
        start_col: 2,
        end_col: 5,
    };
    let s = serde_json::to_string(&range).unwrap();
    assert_eq!(s, r#"{"row":1,"start_col":2,"end_col":5}"#);
    let d: AtomicRange = serde_json::from_str(&s).unwrap();
    assert_eq!(d, range);

    let error = AtomicRangeError {
        rejected: vec![RejectedAtomicRange {
            range,
            reason: AtomicRangeRejectReason::OverlapsPrevious,
        }],
    };
    let s = serde_json::to_string(&error).unwrap();
    let d: AtomicRangeError = serde_json::from_str(&s).unwrap();
    assert_eq!(d, error);

    assert_eq!(
        serde_json::to_string(&AtomicCursorBias::Forward).unwrap(),
        r#""Forward""#
    );
    assert_eq!(
        serde_json::to_string(&AtomicDeleteDirection::Backward).unwrap(),
        r#""Backward""#
    );
}
