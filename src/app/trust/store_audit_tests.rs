// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::json;

fn physical_key(path: &Path) -> String {
    let physical = path.canonicalize().unwrap().to_string_lossy().into_owned();
    if cfg!(windows) {
        physical.strip_prefix(r"\\?\").unwrap_or(&physical).replace('\\', "/")
    } else {
        physical
    }
}

fn aliases(path: &Path) -> (String, String) {
    let key = physical_key(path);
    #[cfg(windows)]
    {
        (key.replace('/', "\\").to_ascii_lowercase(), format!("{key}/"))
    }
    #[cfg(unix)]
    {
        let link = path.with_file_name("project link");
        std::os::unix::fs::symlink(path, &link).unwrap();
        (link.to_string_lossy().into_owned(), format!("{key}/"))
    }
}

#[test]
fn lookup_requires_boolean_approval_for_the_actual_project() {
    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("Trust Case Project");
    std::fs::create_dir(&project).unwrap();
    let key = physical_key(&project);
    let (first, second) = aliases(&project);
    for approval in [json!(false), json!(null), json!("true"), json!(1), json!([]), json!({})] {
        let mut document = json!({"projects": {
            &first: {"hasTrustDialogAccepted": approval},
            &second: {"hasTrustDialogAccepted": false},
            "unrelated": {"hasTrustDialogAccepted": true}, "malformed-unrelated": 42
        }});
        let lookup = read_status(&document, &project);
        assert_eq!(lookup.project_key, key);
        assert!(!lookup.trusted, "{document}");
        document["projects"][&second]["hasTrustDialogAccepted"] = json!(true);
        assert!(read_status(&document, &project).trusted);
    }
    assert!(!read_status(&json!({"projects": {&first: {}}}), &project).trusted);
    assert!(!read_status(&json!({}), &project).trusted);
}

#[test]
fn serialized_keys_preserve_platform_roots_and_supported_aliases() {
    for (input, expected) in
        [("/Users/example/work/../project/", "/Users/example/project"), ("/../../", "/")]
    {
        assert_eq!(normalize_project_key_string(input), expected);
    }
    #[cfg(windows)]
    for (input, expected) in [
        (r"c:\work\project\", "C:/work/project"),
        (r"c:\work\demo\..\project\.\", "C:/work/project"),
        (r"c:\..\..\", "C:/"),
        (r"\\server\share\team\..\project\", "//server/share/project"),
        (r"\\server\share\..\..\", "//server/share"),
        (r"\\?\c:\work\project", "C:/work/project"),
        (r"\\?\UNC\server\share\project", "//server/share/project"),
    ] {
        assert_eq!(normalize_project_key_string(input), expected);
    }
    #[cfg(unix)]
    for key in ["/work/project ", r"/work/a\b", "/work/CaseProject"] {
        assert_eq!(normalize_project_key_string(key), key);
    }
}

#[test]
fn accepting_updates_all_matching_aliases_and_preserves_every_other_field() {
    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("Trust Case Project");
    std::fs::create_dir(&project).unwrap();
    let key = physical_key(&project);
    let (first, second) = aliases(&project);
    let original = json!({"account": {"token-reference":"keep"},"projects":{
        &first:{"allowedTools":["git"],"hasTrustDialogAccepted":false},
        &second:{"mcpServers":{"source":"second"}},
        "unrelated":{"hasTrustDialogAccepted":false,"keep":[1,2]},"malformed-unrelated":42
    }});
    let mut expected = original.clone();
    expected["projects"][&first]["hasTrustDialogAccepted"] = json!(true);
    expected["projects"][&second]["hasTrustDialogAccepted"] = json!(true);
    let mut actual = original.clone();
    assert_eq!(set_trusted(&mut actual, &project).unwrap(), key);
    assert_eq!(actual, expected);
    let path = fixture.path().join("preferences.json");
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(accept_at(&path, &project).unwrap(), key);
    assert_eq!(read_document(&path).unwrap(), expected);
    let mut fresh = json!({"sentinel":7});
    assert_eq!(set_trusted(&mut fresh, &project).unwrap(), key);
    assert_eq!(fresh, json!({"sentinel":7,"projects":{key:{"hasTrustDialogAccepted":true}}}));
}

#[test]
fn malformed_matching_alias_refuses_all_changes_and_recovers_after_repair() {
    let fixture = tempfile::tempdir().unwrap();
    let project = fixture.path().join("Trust Case Project");
    std::fs::create_dir(&project).unwrap();
    let key = physical_key(&project);
    let (a, b) = aliases(&project);
    let (first, second) = if a < b { (a, b) } else { (b, a) };
    let path = fixture.path().join("preferences.json");
    for malformed in [json!(null), json!(42), json!([])] {
        let original = json!({"account":"keep","projects":{
            &first:{"hasTrustDialogAccepted":false,"source":"first"},
            &second:malformed,"unrelated":42
        }});
        let mut actual = original.clone();
        assert!(set_trusted(&mut actual, &project).is_err());
        assert_eq!(actual, original);
        let bytes = serde_json::to_vec(&original).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(accept_at(&path, &project).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        actual["projects"][&second] = json!({"source":"repaired"});
        std::fs::write(&path, serde_json::to_vec(&actual).unwrap()).unwrap();
        let mut expected = actual;
        expected["projects"][&first]["hasTrustDialogAccepted"] = json!(true);
        expected["projects"][&second]["hasTrustDialogAccepted"] = json!(true);
        assert_eq!(accept_at(&path, &project).unwrap(), key);
        assert_eq!(read_document(&path).unwrap(), expected);
    }
}

#[cfg(unix)]
#[test]
fn distinct_real_unix_directories_never_inherit_or_overwrite_trust() {
    let fixture = tempfile::tempdir().unwrap();
    for (first, second) in [("project", "project "), ("a/b", r"a\b")] {
        let first = fixture.path().join(first);
        let second = fixture.path().join(second);
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        assert_ne!(first.canonicalize().unwrap(), second.canonicalize().unwrap());
        let first_key = physical_key(&first);
        let second_key = physical_key(&second);
        let mut document = json!({"projects":{
            &first_key:{"hasTrustDialogAccepted":true,"owner":"first"},
            &second_key:{"hasTrustDialogAccepted":false,"owner":"second"}
        }});
        let mut expected = document.clone();
        assert!(
            !read_status(&document, &second).trusted,
            "distinct directories share trust: {first_key:?}, {second_key:?}"
        );
        assert_eq!(set_trusted(&mut document, &second).unwrap(), second_key);
        expected["projects"][&second_key]["hasTrustDialogAccepted"] = json!(true);
        assert_eq!(document, expected);
    }
    // A parent component after a symlink follows the physical directory, not
    // the text spelling of the link's parent.
    let nested = fixture.path().join("physical/nested");
    let actual = fixture.path().join("physical/project");
    let lexical = fixture.path().join("project");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(&actual).unwrap();
    let link = fixture.path().join("linked");
    std::os::unix::fs::symlink(&nested, &link).unwrap();
    let stored = link.join("../project").to_string_lossy().into_owned();
    let mut document =
        json!({"projects":{&stored:{"hasTrustDialogAccepted":true,"owner":"physical"}}});
    assert!(read_status(&document, &actual).trusted);
    assert!(!read_status(&document, &lexical).trusted);
    let mut expected = document.clone();
    let key = physical_key(&lexical);
    expected["projects"][&key] = json!({"hasTrustDialogAccepted":true});
    assert_eq!(set_trusted(&mut document, &lexical).unwrap(), key);
    assert_eq!(document, expected);
}
