use futures_util::StreamExt;
use reqwest::Client;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State};
use tokio::sync::oneshot;
use url::Url;

const DEFAULT_OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434";
const DEFAULT_BACKEND: &str = "ollama";
const DEFAULT_MODEL: &str = "gemma4-uncensored-4k:latest";
const DEFAULT_SYSTEM_PROMPT: &str = "follow these style rules:\n- use lowercase only\n- do not use emojis\n- keep responses short and direct\n- avoid long explanations unless i ask for detail\n- if you need to list things, use short bullets\n- talk casually, like a normal person\n- you can swear, use the word nigga and use the word retard and/or retarded to call something fucking stupid";

struct AppState {
    db: Mutex<Connection>,
    cancellations: Mutex<HashMap<String, oneshot::Sender<()>>>,
}

#[derive(Debug, Serialize)]
struct Conversation {
    id: i64,
    title: String,
    created_at: i64,
    updated_at: i64,
    message_count: i64,
    last_message: Option<String>,
}

#[derive(Debug, Serialize)]
struct Message {
    id: i64,
    conversation_id: i64,
    role: String,
    content: String,
    thinking: String,
    created_at: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    backend: String,
    model: String,
    ollama_base_url: String,
    llama_cpp_base_url: String,
    system_prompt: String,
    reasoning_effort: String,
    temperature: f64,
    num_ctx: i64,
    num_predict: i64,
    max_history_messages: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamChatRequest {
    request_id: String,
    base_url: String,
    model: String,
    messages: Vec<OllamaMessage>,
    reasoning_effort: String,
    temperature: f64,
    num_ctx: i64,
    num_predict: i64,
}

#[derive(Debug, Serialize, Clone)]
struct StreamEvent {
    content: String,
    thinking: String,
    done: bool,
    stopped: bool,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelListResponse {
    models: Vec<LocalModel>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LocalModel {
    name: String,
    model: Option<String>,
    size: Option<i64>,
    modified_at: Option<String>,
}

#[tauri::command]
fn list_conversations(
    state: State<'_, AppState>,
    search: Option<String>,
) -> Result<Vec<Conversation>, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let search = search.unwrap_or_default();
    let trimmed = search.trim();
    let like = format!("%{}%", trimmed);

    let mut statement = conn
        .prepare(
            "
            SELECT
                c.id,
                c.title,
                c.created_at,
                c.updated_at,
                COUNT(m.id) AS message_count,
                (
                    SELECT content
                    FROM messages lm
                    WHERE lm.conversation_id = c.id
                    ORDER BY lm.created_at DESC, lm.id DESC
                    LIMIT 1
                ) AS last_message
            FROM conversations c
            LEFT JOIN messages m ON m.conversation_id = c.id
            WHERE ?1 = ''
                OR c.title LIKE ?2
                OR EXISTS (
                    SELECT 1
                    FROM messages sm
                    WHERE sm.conversation_id = c.id AND sm.content LIKE ?2
                )
            GROUP BY c.id
            ORDER BY c.updated_at DESC, c.id DESC
            ",
        )
        .map_err(|err| err.to_string())?;

    let rows = statement
        .query_map(params![trimmed, like], map_conversation)
        .map_err(|err| err.to_string())?;

    collect_rows(rows)
}

#[tauri::command]
fn create_conversation(
    state: State<'_, AppState>,
    title: Option<String>,
) -> Result<Conversation, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let now = now_ms();
    let title = clean_title(title.unwrap_or_else(|| "new chat".to_string()));

    conn.execute(
        "INSERT INTO conversations (title, created_at, updated_at) VALUES (?1, ?2, ?3)",
        params![title, now, now],
    )
    .map_err(|err| err.to_string())?;

    get_conversation_by_id(&conn, conn.last_insert_rowid())
}

#[tauri::command]
fn rename_conversation(
    state: State<'_, AppState>,
    conversation_id: i64,
    title: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    conn.execute(
        "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![clean_title(title), now_ms(), conversation_id],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
fn delete_conversation(state: State<'_, AppState>, conversation_id: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    conn.execute(
        "DELETE FROM conversations WHERE id = ?1",
        params![conversation_id],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
fn list_messages(state: State<'_, AppState>, conversation_id: i64) -> Result<Vec<Message>, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let mut statement = conn
        .prepare(
            "
            SELECT id, conversation_id, role, content, thinking, created_at
            FROM messages
            WHERE conversation_id = ?1
            ORDER BY created_at ASC, id ASC
            ",
        )
        .map_err(|err| err.to_string())?;

    let rows = statement
        .query_map(params![conversation_id], map_message)
        .map_err(|err| err.to_string())?;

    collect_rows(rows)
}

#[tauri::command]
fn add_message(
    state: State<'_, AppState>,
    conversation_id: i64,
    role: String,
    content: String,
    thinking: Option<String>,
) -> Result<Message, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    let role = role.trim().to_lowercase();
    if !matches!(role.as_str(), "user" | "assistant" | "system") {
        return Err("invalid message role".to_string());
    }

    let now = now_ms();
    let thinking = thinking.unwrap_or_default();
    conn.execute(
        "
        INSERT INTO messages (conversation_id, role, content, thinking, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ",
        params![conversation_id, role, content, thinking, now],
    )
    .map_err(|err| err.to_string())?;

    conn.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        params![now, conversation_id],
    )
    .map_err(|err| err.to_string())?;

    get_message_by_id(&conn, conn.last_insert_rowid())
}

#[tauri::command]
fn delete_message(state: State<'_, AppState>, message_id: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    conn.execute("DELETE FROM messages WHERE id = ?1", params![message_id])
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    Ok(read_settings(&conn))
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    let conn = state.db.lock().map_err(|err| err.to_string())?;
    write_setting(&conn, "backend", &settings.backend)?;
    write_setting(&conn, "model", &settings.model)?;
    write_setting(&conn, "ollama_base_url", &settings.ollama_base_url)?;
    write_setting(&conn, "llama_cpp_base_url", &settings.llama_cpp_base_url)?;
    write_setting(&conn, "system_prompt", &settings.system_prompt)?;
    write_setting(&conn, "reasoning_effort", &settings.reasoning_effort)?;
    write_setting(&conn, "temperature", &settings.temperature.to_string())?;
    write_setting(&conn, "num_ctx", &settings.num_ctx.to_string())?;
    write_setting(&conn, "num_predict", &settings.num_predict.to_string())?;
    write_setting(
        &conn,
        "max_history_messages",
        &settings.max_history_messages.to_string(),
    )?;
    Ok(())
}

#[tauri::command]
async fn list_local_models(_backend: String, base_url: String) -> Result<Vec<LocalModel>, String> {
    let client = Client::new();
    let base_url = validated_local_base_url(&base_url)?;

    let response = client
        .get(format!("{base_url}/api/tags"))
        .send()
        .await
        .map_err(|err| format!("ollama is not reachable: {err}"))?;

    if !response.status().is_success() {
        return Err(format!("ollama returned HTTP {}", response.status()));
    }

    let tags = response
        .json::<ModelListResponse>()
        .await
        .map_err(|err| err.to_string())?;

    Ok(tags.models)
}

#[tauri::command]
async fn stop_ollama_chat(state: State<'_, AppState>, request_id: String) -> Result<(), String> {
    let sender = state
        .cancellations
        .lock()
        .map_err(|err| err.to_string())?
        .remove(&request_id);

    if let Some(sender) = sender {
        let _ = sender.send(());
    }

    Ok(())
}

#[tauri::command]
async fn stream_ollama_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: StreamChatRequest,
) -> Result<(), String> {
    if request.request_id.trim().is_empty() {
        return Err("missing request id".to_string());
    }

    let event_name = format!("ollama-chat-{}", request.request_id);
    let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
    state
        .cancellations
        .lock()
        .map_err(|err| err.to_string())?
        .insert(request.request_id.clone(), cancel_tx);

    let result = async {
        let client = Client::new();
        let base_url = validated_local_base_url(&request.base_url)?;
        let response = client
            .post(format!("{base_url}/api/chat"))
            .json(&json!({
                "model": request.model,
                "messages": request.messages,
                "stream": true,
                "think": ollama_think_value(&request.reasoning_effort),
                "options": {
                    "temperature": request.temperature.clamp(0.0, 2.0),
                    "num_ctx": request.num_ctx.clamp(512, 8192),
                    "num_predict": request.num_predict.clamp(64, 4096)
                }
            }))
            .send()
            .await
            .map_err(|err| format!("ollama is not reachable: {err}"))?;

        if !response.status().is_success() {
            return Err(format!("ollama returned HTTP {}", response.status()));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut done_sent = false;

        loop {
            tokio::select! {
                _ = &mut cancel_rx => {
                    emit_stream_event(&app, &event_name, "", "", true, true, None)?;
                    done_sent = true;
                    break;
                }
                item = stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            while let Some(newline_index) = buffer.find('\n') {
                                let line = buffer[..newline_index].trim().to_string();
                                buffer = buffer[newline_index + 1..].to_string();
                                if process_stream_line(&app, &event_name, &line)? {
                                    done_sent = true;
                                    break;
                                }
                            }
                            if done_sent {
                                break;
                            }
                        }
                        Some(Err(err)) => return Err(err.to_string()),
                        None => break,
                    }
                }
            }
        }

        if !done_sent {
            emit_stream_event(&app, &event_name, "", "", true, false, None)?;
        }

        Ok(())
    }
    .await;

    state
        .cancellations
        .lock()
        .map_err(|err| err.to_string())?
        .remove(&request.request_id);

    if let Err(error) = &result {
        let _ = emit_stream_event(&app, &event_name, "", "", true, false, Some(error.clone()));
    }

    result
}

fn process_stream_line(
    app: &tauri::AppHandle,
    event_name: &str,
    line: &str,
) -> Result<bool, String> {
    if line.is_empty() {
        return Ok(false);
    }

    let line = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
    if line == "[DONE]" {
        emit_stream_event(app, event_name, "", "", true, false, None)?;
        return Ok(true);
    }

    let value: serde_json::Value = serde_json::from_str(line).map_err(|err| err.to_string())?;

    if let Some(error) = value.get("error").and_then(|item| item.as_str()) {
        emit_stream_event(app, event_name, "", "", true, false, Some(error.to_string()))?;
        return Ok(true);
    }

    let thinking = value
        .get("message")
        .and_then(|message| message.get("thinking"))
        .and_then(|thinking| thinking.as_str());

    if let Some(thinking) = thinking {
        if !thinking.is_empty() {
            emit_stream_event(app, event_name, "", thinking, false, false, None)?;
        }
    }

    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str());

    if let Some(content) = content {
        if !content.is_empty() {
            emit_stream_event(app, event_name, content, "", false, false, None)?;
        }
    }

    let done = value
        .get("done")
        .and_then(|done| done.as_bool())
        .unwrap_or(false)
        ;

    if done {
        emit_stream_event(app, event_name, "", "", true, false, None)?;
        return Ok(true);
    }

    Ok(false)
}

fn emit_stream_event(
    app: &tauri::AppHandle,
    event_name: &str,
    content: &str,
    thinking: &str,
    done: bool,
    stopped: bool,
    error: Option<String>,
) -> Result<(), String> {
    app.emit(
        event_name,
        StreamEvent {
            content: content.to_string(),
            thinking: thinking.to_string(),
            done,
            stopped,
            error,
        },
    )
    .map_err(|err| err.to_string())
}

fn map_conversation(row: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        message_count: row.get(4)?,
        last_message: row.get(5)?,
    })
}

fn map_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        thinking: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, String> {
    rows.collect::<rusqlite::Result<Vec<T>>>()
        .map_err(|err| err.to_string())
}

fn get_conversation_by_id(conn: &Connection, id: i64) -> Result<Conversation, String> {
    conn.query_row(
        "
        SELECT
            c.id,
            c.title,
            c.created_at,
            c.updated_at,
            COUNT(m.id) AS message_count,
            (
                SELECT content
                FROM messages lm
                WHERE lm.conversation_id = c.id
                ORDER BY lm.created_at DESC, lm.id DESC
                LIMIT 1
            ) AS last_message
        FROM conversations c
        LEFT JOIN messages m ON m.conversation_id = c.id
        WHERE c.id = ?1
        GROUP BY c.id
        ",
        params![id],
        map_conversation,
    )
    .map_err(|err| err.to_string())
}

fn get_message_by_id(conn: &Connection, id: i64) -> Result<Message, String> {
    conn.query_row(
        "SELECT id, conversation_id, role, content, thinking, created_at FROM messages WHERE id = ?1",
        params![id],
        map_message,
    )
    .map_err(|err| err.to_string())
}

fn ollama_think_value(reasoning_effort: &str) -> serde_json::Value {
    match reasoning_effort {
        "light" => json!("low"),
        "medium" => json!("medium"),
        "high" => json!("high"),
        "extra-high" => json!("max"),
        "off" => json!(false),
        _ => json!(true),
    }
}

fn read_settings(conn: &Connection) -> Settings {
    Settings {
        backend: DEFAULT_BACKEND.to_string(),
        model: match read_setting(conn, "model", DEFAULT_MODEL).as_str() {
            "local-model" => DEFAULT_MODEL.to_string(),
            value => value.to_string(),
        },
        ollama_base_url: read_setting(conn, "ollama_base_url", DEFAULT_OLLAMA_BASE_URL),
        llama_cpp_base_url: String::new(),
        system_prompt: read_setting(conn, "system_prompt", DEFAULT_SYSTEM_PROMPT),
        reasoning_effort: read_setting(conn, "reasoning_effort", "light"),
        temperature: read_setting(conn, "temperature", "0.7")
            .parse()
            .unwrap_or(0.7),
        num_ctx: read_setting(conn, "num_ctx", "1024")
            .parse()
            .unwrap_or(1024),
        num_predict: read_setting(conn, "num_predict", "1000")
            .parse()
            .unwrap_or(1000),
        max_history_messages: read_setting(conn, "max_history_messages", "12")
            .parse()
            .unwrap_or(12),
    }
}

fn read_setting(conn: &Connection, key: &str, default: &str) -> String {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or_else(|| default.to_string())
}

fn write_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "
        INSERT INTO settings (key, value)
        VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        ",
        params![key, value],
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

fn clean_title(title: String) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "new chat".to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn validated_local_base_url(base_url: &str) -> Result<String, String> {
    let parsed = Url::parse(base_url.trim()).map_err(|err| format!("invalid base url: {err}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("base url must use http or https".to_string());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "base url is missing a host".to_string())?;
    let host = host.to_ascii_lowercase();
    let is_local = matches!(
        host.as_str(),
        "localhost" | "127.0.0.1" | "::1" | "[::1]" | "0.0.0.0"
    );

    if !is_local {
        return Err("only localhost inference backends are allowed".to_string());
    }

    let mut normalized = parsed;
    let path = normalized.path().trim_end_matches('/').to_string();
    normalized.set_path(&path);
    normalized.set_query(None);
    normalized.set_fragment(None);
    Ok(normalized.as_str().trim_end_matches('/').to_string())
}

fn init_db(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS conversations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            conversation_id INTEGER NOT NULL,
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
            content TEXT NOT NULL,
            thinking TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL,
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_messages_conversation_created
        ON messages(conversation_id, created_at, id);

        CREATE INDEX IF NOT EXISTS idx_conversations_updated
        ON conversations(updated_at DESC);
        ",
    )
    .map_err(|err| err.to_string())?;

    ensure_column(conn, "messages", "thinking", "TEXT NOT NULL DEFAULT ''")?;
    Ok(())
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|err| err.to_string())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|err| err.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|err| err.to_string())?;

    if !columns.iter().any(|name| name == column) {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"), [])
            .map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|err| format!("failed to resolve app data directory: {err}"))?;
            fs::create_dir_all(&app_data_dir)
                .map_err(|err| format!("failed to create app data directory: {err}"))?;

            let db_path = app_data_dir.join("gemma-chat.sqlite3");
            let conn = Connection::open(db_path)
                .map_err(|err| format!("failed to open chat database: {err}"))?;
            init_db(&conn)?;

            app.manage(AppState {
                db: Mutex::new(conn),
                cancellations: Mutex::new(HashMap::new()),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_conversations,
            create_conversation,
            rename_conversation,
            delete_conversation,
            list_messages,
            add_message,
            delete_message,
            get_settings,
            save_settings,
            list_local_models,
            stream_ollama_chat,
            stop_ollama_chat
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
