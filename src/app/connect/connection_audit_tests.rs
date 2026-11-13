// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::MessageBlock;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn isolated_app(cwd: &Path, home: &Path) -> App {
    use clap::Parser;
    let cli = Cli::try_parse_from([
        std::ffi::OsStr::new("claude-rs"),
        std::ffi::OsStr::new("--no-update-check"),
        std::ffi::OsStr::new("--dir"),
        cwd.as_os_str(),
    ])
    .expect("startup CLI");
    create_app_with_settings_and_home(
        &cli,
        settings::LoadedAppSettings {
            path: Some(home.join("app-settings.json")),
            settings: settings::AppSettings::default(),
        },
        Some(home.to_path_buf()),
    )
}
fn assert_directory(app: &App, raw: &Path, display: &str) {
    assert_eq!(Path::new(&app.cwd_raw), raw);
    assert_eq!(app.cwd, display);
    let welcome = app
        .transcript
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .find_map(|b| if let MessageBlock::Welcome(w) = b { Some(w) } else { None })
        .expect("welcome directory");
    assert_eq!(welcome.cwd, display);
}
#[test]
fn startup_resolves_parent_components_without_changing_profile_data() {
    let root = tempfile::Builder::new()
        .prefix(".cwd-audit-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let project = root.path().join("project");
    let home = root.path().join("profile");
    std::fs::create_dir_all(project.join("child")).unwrap();
    std::fs::create_dir(&home).unwrap();
    let profile = home.join(".claude.json");
    std::fs::write(&profile, b"{\"account\":\"retain\"}").unwrap();
    let relative = Path::new(root.path().file_name().unwrap()).join("project/child/..");
    let app = isolated_app(&relative, &home);
    assert_directory(&app, &project, &project.to_string_lossy());
    assert_eq!(std::fs::read(profile).unwrap(), b"{\"account\":\"retain\"}");
}
#[tokio::test]
async fn startup_and_replacement_abbreviate_only_home_directory_components() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("simon");
    let descendant = home.join("project");
    let sibling = root.path().join("simon-old/project");
    for path in [&home, &descendant, &sibling] {
        std::fs::create_dir_all(path).unwrap();
    }
    for (cwd, display) in [
        (&home, "~".to_owned()),
        (&descendant, Path::new("~").join("project").to_string_lossy().into_owned()),
        (&sibling, sibling.to_string_lossy().into_owned()),
    ] {
        let mut app = isolated_app(cwd, &home);
        assert_directory(&app, cwd, &display);
        // Start replacement from a different directory so a stale welcome cannot pass.
        app = isolated_app(root.path(), &home);
        let event=dispatch_test_event(json!({"event":"connected","session_id":"replaced","cwd":cwd,"current_model":{"requested_id":"default","resolved_id":"sonnet","display_name_short":"Sonnet","display_name_long":"Sonnet","supports_effort":false,"is_authoritative":true},"available_models":[],"fast_mode_state":"off","history_updates":[]})).await;
        crate::app::events::handle_client_event(&mut app, event);
        assert_directory(&app, cwd, &display);
    }
}

struct SlotGuard(Option<Rc<std::cell::RefCell<Option<ConnectionSlot>>>>);
impl Drop for SlotGuard {
    fn drop(&mut self) {
        CONN_SLOT.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}
fn seed_connections(
    app: &mut App,
) -> (SlotGuard, std::rc::Weak<AgentConnection>, std::rc::Weak<AgentConnection>) {
    let (active, _) = AgentConnection::test_channel();
    let active = Rc::new(active);
    let active_observer = Rc::downgrade(&active);
    app.session_runtime.conn = Some(active);
    let (pending, _) = AgentConnection::test_channel();
    let pending = Rc::new(pending);
    let pending_observer = Rc::downgrade(&pending);
    let previous = CONN_SLOT.with(|slot| {
        slot.replace(Some(Rc::new(std::cell::RefCell::new(Some(ConnectionSlot { conn: pending })))))
    });
    (SlotGuard(previous), active_observer, pending_observer)
}
#[tokio::test(flavor = "current_thread")]
async fn shutdown_releases_connections_but_waits_for_signalled_cleanup() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (_restore, active, pending) = seed_connections(&mut app);
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let (ack_tx, ack_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let join_handle = tokio::task::spawn_local(async move {
                shutdown_rx.await.expect("explicit shutdown signal");
                ack_tx.send(()).unwrap();
                release_rx.await.unwrap();
            });
            app.bridge_task = Some(BridgeTask { shutdown_tx, join_handle });
            let mut shutdown = begin_shutdown_connection(&mut app).expect("owned task");
            assert!(active.upgrade().is_none());
            assert!(pending.upgrade().is_none());
            assert!(take_connection_slot().is_none());
            assert!(app.session_runtime.conn.is_none());
            assert!(app.bridge_task.is_none());
            tokio::time::timeout(std::time::Duration::from_secs(3), ack_rx).await.unwrap().unwrap();
            let mut wait = std::pin::pin!(shutdown.wait());
            std::future::poll_fn(|cx| {
                assert!(
                    std::future::Future::poll(wait.as_mut(), cx).is_pending(),
                    "shutdown must await cleanup"
                );
                std::task::Poll::Ready(())
            })
            .await;
            release_tx.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), wait).await.unwrap();
            shutdown_connection(&mut app).await;
            assert!(begin_shutdown_connection(&mut app).is_none());
        })
        .await;
}
#[tokio::test(flavor = "current_thread")]
async fn forced_shutdown_waits_for_the_ready_tasks_resources_to_drop() {
    tokio::task::LocalSet::new()
        .run_until(async {
            struct Marker(Rc<std::cell::Cell<bool>>);
            impl Drop for Marker {
                fn drop(&mut self) {
                    self.0.set(true);
                }
            }
            let mut app = App::test_default();
            let (_restore, active, pending) = seed_connections(&mut app);
            let dropped = Rc::new(std::cell::Cell::new(false));
            let marker = Marker(dropped.clone());
            let (ready_tx, ready_rx) = oneshot::channel();
            let (ack_tx, ack_rx) = oneshot::channel();
            let (shutdown_tx, shutdown_rx) = oneshot::channel();
            let join_handle = tokio::task::spawn_local(async move {
                let _marker = marker;
                ready_tx.send(()).unwrap();
                shutdown_rx.await.expect("explicit shutdown signal");
                ack_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            });
            app.bridge_task = Some(BridgeTask { shutdown_tx, join_handle });
            tokio::time::timeout(std::time::Duration::from_secs(3), ready_rx)
                .await
                .unwrap()
                .unwrap();
            let shutdown = begin_shutdown_connection(&mut app).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), ack_rx).await.unwrap().unwrap();
            assert!(!dropped.get());
            assert!(active.upgrade().is_none());
            assert!(pending.upgrade().is_none());
            assert!(take_connection_slot().is_none());
            tokio::time::timeout(std::time::Duration::from_secs(3), shutdown.force())
                .await
                .unwrap();
            assert!(dropped.get());
            assert!(app.bridge_task.is_none());
            assert!(app.session_runtime.conn.is_none());
            assert!(begin_shutdown_connection(&mut app).is_none());
        })
        .await;
}
#[tokio::test]
async fn export_dispatch_preserves_neighboring_chunks_and_separate_loss_counts() {
    for (last, text, source, omitted) in
        [(false, "first body", "user-source", 0), (true, "last body", "assistant-source", 17)]
    {
        let event=dispatch_test_event(json!({"event":"session_history_export","request_id":"export-17","session_id":"history-session","last":last,"omitted_updates":omitted,"updates":[
            {"type":if last {"agent_message_chunk"}else{"user_message_chunk"},"source_message_uuid":source,"content":{"type":"text","text":text}},
            {"type":"user_message_chunk","content":{"type":"image","mime_type":"image/x-unsupported","data":"AAAA"}},
            {"type":"agent_message_chunk","content":{"type":"image","mime_type":"image/png","data":""}}
        ]})).await;
        let ClientEvent::SessionHistoryChunk { session_id, chunk } = event else {
            panic!("history chunk")
        };
        assert_eq!(session_id, "history-session");
        assert_eq!(chunk.request_id.as_deref(), Some("export-17"));
        assert_eq!(chunk.last, last);
        assert_eq!(chunk.omitted_updates, omitted);
        assert_eq!(chunk.unmapped_updates, 2);
        assert!(chunk.error.is_none());
        let [update] = chunk.updates.as_slice() else { panic!("only valid body remains") };
        let body = match update {
            crate::agent::model::SessionUpdate::UserMessageChunk(body) if !last => body,
            crate::agent::model::SessionUpdate::AgentMessageChunk(body) if last => body,
            _ => panic!("message role"),
        };
        assert_eq!(body.source_message_uuid.as_deref(), Some(source));
        assert_eq!(
            body.content,
            crate::agent::model::ContentBlock::Text(crate::agent::model::TextContent {
                text: text.into()
            })
        );
    }
}

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1sAAAAASUVORK5CYII=";
fn image_updates(role: &str) -> Vec<Value> {
    [
        json!({"type":"text","text":"before image"}),
        json!({"type":"image","mime_type":"image/x-unsupported","data":PNG}),
        json!({"type":"image","mime_type":"image/png"}),
        json!({"type":"image","mime_type":"image/png","data":""}),
        json!({"type":"image","mime_type":"image/png","uri":"https://example.invalid/image.png"}),
        json!({"type":"image","mime_type":"image/png","data":PNG}),
        json!({"type":"image","data":PNG}),
        json!({"type":"text","text":"after image"}),
    ]
    .into_iter()
    .map(|content| json!({"type":role,"source_message_uuid":"image-source","content":content}))
    .collect()
}
fn assert_image_sequence(updates: &[crate::agent::model::SessionUpdate], role: &str) {
    use crate::agent::model::{ContentBlock, SessionUpdate};
    let content: Vec<_> = updates
        .iter()
        .map(|update| {
            let chunk = match update {
                SessionUpdate::UserMessageChunk(c) if role == "user_message_chunk" => c,
                SessionUpdate::AgentMessageChunk(c) if role == "agent_message_chunk" => c,
                _ => panic!("role"),
            };
            assert_eq!(chunk.source_message_uuid.as_deref(), Some("image-source"));
            &chunk.content
        })
        .collect();
    let [
        ContentBlock::Text(before),
        ContentBlock::Image(explicit),
        ContentBlock::Image(default),
        ContentBlock::Text(after),
    ] = content.as_slice()
    else {
        panic!("valid neighbors and two supported images survive")
    };
    assert_eq!(before.text, "before image");
    assert_eq!(after.text, "after image");
    for image in [explicit, default] {
        assert_eq!(image.data, PNG);
        assert_eq!(image.mime_type, "image/png");
    }
}
#[tokio::test]
async fn live_and_history_dispatch_drop_unusable_images_without_losing_neighbors() {
    use crate::agent::model::SessionUpdate;
    for role in ["user_message_chunk", "agent_message_chunk"] {
        let updates = image_updates(role);
        let (connection, _) = AgentConnection::test_channel();
        let (tx, mut rx) = mpsc::channel(16);
        let mut connected = true;
        for update in &updates {
            event_dispatch::handle_bridge_event(
                &tx,
                &connection,
                &mut connected,
                false,
                serde_json::from_value(
                    json!({"event":"session_update","session_id":"images","update":update}),
                )
                .unwrap(),
            )
            .await;
        }
        let mut live = Vec::new();
        while let Ok(event) = rx.try_recv() {
            let ClientEvent::SessionUpdate { session_id, update } = event else {
                panic!("live update")
            };
            assert_eq!(session_id, "images");
            live.push(update);
        }
        assert_image_sequence(&live, role);
        for already_connected in [false, true] {
            let mut state = already_connected;
            event_dispatch::handle_bridge_event(&tx,&connection,&mut state,false,serde_json::from_value(json!({"event":"connected","session_id":"images","cwd":"/image-project","current_model":{"resolved_id":"sonnet","display_name_short":"Sonnet","display_name_long":"Sonnet","supports_effort":false,"is_authoritative":true},"available_models":[],"fast_mode_state":"off","history_updates":updates})).unwrap()).await;
            let history = match rx.try_recv().unwrap() {
                ClientEvent::Connected { history_updates, .. } if !already_connected => {
                    history_updates
                }
                ClientEvent::SessionReplaced { history_updates, .. } if already_connected => {
                    history_updates
                }
                _ => panic!("history destination"),
            };
            assert_image_sequence(&history, role);
        }
        let content: Vec<_> =
            updates.iter().map(|u| json!({"type":"content","content":u["content"]})).collect();
        for patch in [false, true] {
            let payload = if patch {
                json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"image-tool","fields":{"content":content}}})
            } else {
                json!({"type":"tool_call","tool_call":{"tool_call_id":"image-tool","title":"Images","kind":"read","status":"completed","content":content,"locations":[]}})
            };
            let mapped =
                type_converters::map_session_update(serde_json::from_value(payload).unwrap())
                    .unwrap();
            let content = match mapped {
                SessionUpdate::ToolCall(t) => t.content,
                SessionUpdate::ToolCallUpdate(t) => t.fields.content.unwrap(),
                _ => panic!("tool image content"),
            };
            let updates: Vec<_> = content
                .into_iter()
                .map(|c| {
                    let crate::agent::model::ToolCallContent::Content(c) = c else {
                        panic!("content")
                    };
                    let chunk = crate::agent::model::ContentChunk {
                        content: c.content,
                        source_message_uuid: Some("image-source".into()),
                    };
                    if role == "user_message_chunk" {
                        SessionUpdate::UserMessageChunk(chunk)
                    } else {
                        SessionUpdate::AgentMessageChunk(chunk)
                    }
                })
                .collect();
            assert_image_sequence(&updates, role);
        }
    }
}
