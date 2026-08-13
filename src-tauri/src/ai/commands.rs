//! What the chat page's messages turn into.
//!
//! `session.rs` is the loop and `bridge.rs` is the host it runs on; this is the
//! outermost layer, and the only one that knows Tauri exists. Every command here
//! is short on purpose -- it finds the panel, calls one thing, and hands back
//! what the page reads.
//!
//! Two rules run through the lot. Anything that changes a setting reloads it into
//! every open panel, because a model or mode switched in one is the truth
//! everywhere a moment later. And nothing here composes a sentence for the user:
//! the page has the string table, so what crosses this boundary is keys, ids and
//! outcomes.

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::State;

use crate::config::AppConfig;
use crate::secrets::Store;
use crate::{dial_plan, load, reveal, ssh, transfer};

use super::bridge::{AiStores, PanelBootstrap, Panels};
use super::settings::{model_secret_key, AgentMode, AiSettings, ThinkingEffort};
use super::store::memory::{EditOutcome, RemoveOutcome};
use super::store::trust::TrustWrite;
use super::types::MemoryScope;

type SharedPanels = Arc<Panels>;
type SharedStores = Arc<AiStores>;

/// Re-reads the config into every open panel.
fn refresh_panels(panels: &Panels, store: &Store) -> Result<AppConfig, String> {
    let config = load()?;
    for panel in panels.all() {
        panel.refresh(&config, store);
    }
    Ok(config)
}

/// Loads, mutates, saves, and tells every panel. The shape every AI setting
/// command ends in.
fn update_ai<T>(
    panels: &Panels,
    store: &Store,
    change: impl FnOnce(&mut AiSettings) -> T,
) -> Result<T, String> {
    let mut config = load()?;
    let answer = change(&mut config.settings.ai);
    config.normalize();
    crate::config::save(&config)?;
    for panel in panels.all() {
        panel.refresh(&config, store);
    }
    Ok(answer)
}

fn scope_of(value: &str) -> MemoryScope {
    if value == "global" {
        MemoryScope::Global
    } else {
        MemoryScope::Server
    }
}

fn removal_tag(outcome: RemoveOutcome) -> String {
    match outcome {
        RemoveOutcome::Ok => "ok",
        RemoveOutcome::Missing => "missing",
        RemoveOutcome::Ambiguous => "ambiguous",
        RemoveOutcome::Failed => "failed",
    }
    .to_string()
}

// ------------------------------------------------------------------ the panel ---

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ai_open(
    pane: String,
    terminal: String,
    group_id: String,
    server_id: String,
    out: Channel<Value>,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
    sessions: State<'_, Arc<ssh::Sessions>>,
    transfers: State<'_, Arc<transfer::Transfers>>,
    store: State<'_, Store>,
) -> Result<PanelBootstrap, String> {
    let config = load()?;
    let server = config.find_server(&group_id, &server_id).ok_or("Server not found.")?;
    let name = if server.name.trim().is_empty() {
        server.host.clone()
    } else {
        server.name.clone()
    };
    // The terminal's own resolved credentials, so the assistant's SFTP channel
    // never asks for a password the user has already given.
    let (target, encoding) = dial_plan(&group_id, &server_id, &store)?;

    let (panel, bootstrap) = super::bridge::open_panel(
        pane.clone(),
        terminal,
        server_id,
        name,
        encoding,
        target,
        Arc::clone(&sessions),
        Arc::clone(&transfers),
        Arc::clone(&stores),
        &store,
        &config,
        out,
    );
    panels.insert(&pane, panel);
    Ok(bootstrap)
}

/// One message, run to completion.
///
/// Deliberately not awaited by the page: the reply arrives on the channel as it
/// happens, and a task can run for minutes. What this returns is only whether the
/// panel was there to take it.
#[tauri::command]
pub async fn ai_send(
    pane: String,
    text: String,
    panels: State<'_, SharedPanels>,
) -> Result<(), String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    tauri::async_runtime::spawn(async move { panel.send(text).await });
    Ok(())
}

#[tauri::command]
pub fn ai_stop(pane: String, panels: State<'_, SharedPanels>) {
    if let Some(panel) = panels.get(&pane) {
        panel.session.stop();
    }
}

/// The answer to a confirmation. `id` is the one the question carried.
#[tauri::command]
pub fn ai_answer(pane: String, id: u64, ok: bool, panels: State<'_, SharedPanels>) {
    if let Some(panel) = panels.get(&pane) {
        panel.host.answer(id, ok);
    }
}

#[tauri::command]
pub fn ai_close(pane: String, panels: State<'_, SharedPanels>) {
    if let Some(panel) = panels.remove(&pane) {
        panel.close();
    }
}

// ---------------------------------------------------------------- the history ---

#[tauri::command]
pub fn ai_history(stores: State<'_, SharedStores>) -> Value {
    json!({ "items": stores.chats.list() })
}

#[tauri::command]
pub fn ai_load_chat(
    pane: String,
    id: String,
    panels: State<'_, SharedPanels>,
) -> Result<Value, String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    panel.load(&id).ok_or_else(|| "That conversation is no longer there.".to_string())
}

#[tauri::command]
pub fn ai_new_chat(pane: String, panels: State<'_, SharedPanels>) -> Result<(), String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    panel.new_chat();
    Ok(())
}

#[tauri::command]
pub fn ai_delete_chat(
    pane: String,
    id: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) {
    stores.chats.remove(&id);
    // Deleting the one on screen would leave the panel showing a conversation
    // that no longer exists, so it starts a fresh one rather than pretending.
    if let Some(panel) = panels.get(&pane) {
        if panel.record_id() == id {
            panel.new_chat();
        }
    }
}

// ----------------------------------------------------------------- the memory ---

#[tauri::command]
pub fn ai_memory_list(
    pane: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<Value, String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    let server = panel.server_id();
    Ok(json!({
        "global": stores.memory.lines(MemoryScope::Global, None),
        "server": stores.memory.lines(MemoryScope::Server, Some(&server)),
    }))
}

#[tauri::command]
pub fn ai_memory_edit(
    pane: String,
    scope: String,
    index: usize,
    was: String,
    text: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<String, String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    let scope = scope_of(&scope);
    let settings = load()?.settings.ai;
    let budget = match scope {
        MemoryScope::Global => settings.memory.global_budget,
        MemoryScope::Server => settings.memory.server_budget,
    };
    let server = panel.server_id();
    let id = (scope == MemoryScope::Server).then_some(server.as_str());
    Ok(match stores.memory.replace(scope, id, index, &was, &text, budget) {
        EditOutcome::Ok => "ok",
        EditOutcome::Full => "full",
        EditOutcome::Missing => "missing",
        EditOutcome::Failed => "failed",
    }
    .to_string())
}

#[tauri::command]
pub fn ai_memory_delete(
    pane: String,
    scope: String,
    index: usize,
    was: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<String, String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    let scope = scope_of(&scope);
    let server = panel.server_id();
    let id = (scope == MemoryScope::Server).then_some(server.as_str());
    Ok(removal_tag(stores.memory.remove_at(scope, id, index, &was)))
}

#[tauri::command]
pub fn ai_memory_undo(token: String, stores: State<'_, SharedStores>) -> bool {
    stores.memory.undo(&token) == RemoveOutcome::Ok
}

// ------------------------------------------------------------------ the trust ---

#[tauri::command]
pub fn ai_trust_list(
    pane: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<Value, String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    let config = load()?;
    Ok(json!({
        "dirs": stores.trust.list(&panel.server_id()),
        "mode": config.settings.ai.agent.mode.tag(),
    }))
}

/// Trusting a directory from the file dialog's own button.
#[tauri::command]
pub fn ai_trust_add(
    pane: String,
    dir: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<(), String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    if stores.trust.add(&panel.server_id(), &dir) == TrustWrite::Ok {
        panel.note_trusted(&dir);
    }
    Ok(())
}

#[tauri::command]
pub fn ai_trust_remove(
    pane: String,
    dir: String,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<(), String> {
    let panel = panels.get(&pane).ok_or("That panel is not open.")?;
    stores.trust.remove(&panel.server_id(), &dir);
    Ok(())
}

// ----------------------------------------------------------------- the skills ---

#[tauri::command]
pub fn ai_skill_list(stores: State<'_, SharedStores>) -> Result<Value, String> {
    let config = load()?;
    let skills = stores.skills.list(&config.settings.ai.skills.disabled);
    Ok(json!({
        "items": skills.iter().map(|skill| json!({
            "id": skill.id,
            "name": skill.name,
            "description": skill.description,
            "disabled": skill.disabled,
        })).collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub fn ai_skill_toggle(
    id: String,
    on: bool,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<(), String> {
    update_ai(&panels, &store, |settings| {
        settings.skills.disabled.retain(|entry| *entry != id);
        if !on {
            settings.skills.disabled.push(id.clone());
        }
    })
}

// ----------------------------------------------------------------- the models ---

#[tauri::command]
pub fn ai_model_list(store: State<'_, Store>) -> Result<Value, String> {
    let settings = load()?.settings.ai;
    Ok(json!({
        "active": settings.active_model_id,
        "items": settings.models.iter().map(|model| json!({
            "id": model.id,
            "label": model.model,
            "detail": model.base_url,
            "removable": true,
            // Whether an endpoint has a key, never the key itself. The window is
            // drawn from this and nothing else, so a stored key has no path back
            // into the page.
            "hasKey": !store.get(&model_secret_key(&model.id)).unwrap_or_default().is_empty(),
        })).collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub fn ai_model_add(
    base_url: String,
    model: String,
    api_key: String,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<(), String> {
    let id = update_ai(&panels, &store, |settings| settings.add_model(&base_url, &model))?;
    // Written after the row exists, and only when one was given: re-entering an
    // endpoint to correct its name must not wipe the key that was working.
    if !api_key.is_empty() {
        store.set(&model_secret_key(&id), &api_key)?;
        // The key is part of what the provider is built from, so the panels take
        // it only on a second pass -- the first ran before it was stored.
        refresh_panels(&panels, &store)?;
    }
    Ok(())
}

#[tauri::command]
pub fn ai_model_delete(
    id: String,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<(), String> {
    update_ai(&panels, &store, |settings| settings.remove_model(&id))?;
    let _ = store.delete(&model_secret_key(&id));
    Ok(())
}

#[tauri::command]
pub fn ai_model_select(
    id: String,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<(), String> {
    update_ai(&panels, &store, |settings| {
        if settings.models.iter().any(|model| model.id == id) {
            settings.active_model_id = id.clone();
        }
    })
}

// --------------------------------------------------------------- the postures ---

#[tauri::command]
pub fn ai_set_mode(
    mode: String,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<(), String> {
    update_ai(&panels, &store, |settings| settings.agent.mode = AgentMode::parse(&mode))
}

#[tauri::command]
pub fn ai_set_thinking(
    enabled: Option<bool>,
    effort: Option<String>,
    show: Option<bool>,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<(), String> {
    // Whichever of the three the user just touched; the rest are left alone. A
    // caller spreading the object by hand would eventually send an effort that
    // quietly reset the switch beside it.
    update_ai(&panels, &store, |settings| {
        if let Some(enabled) = enabled {
            settings.thinking.enabled = enabled;
        }
        if let Some(effort) = effort {
            settings.thinking.effort = match effort.as_str() {
                "low" => ThinkingEffort::Low,
                "max" => ThinkingEffort::Max,
                _ => ThinkingEffort::High,
            };
        }
        if let Some(show) = show {
            settings.thinking.show = show;
        }
    })
}

// ------------------------------------------------------------ the settings page ---

#[tauri::command]
pub fn ai_settings_read() -> Result<AiSettings, String> {
    Ok(load()?.settings.ai)
}

/// One or more fields of it, named the way the page spells them.
///
/// Merged rather than replaced, so a page that knows about six fields cannot drop
/// the seventh this build added -- the same courtesy `Settings::rest` does for
/// the file.
#[tauri::command]
pub fn ai_settings_write(
    patch: Value,
    panels: State<'_, SharedPanels>,
    store: State<'_, Store>,
) -> Result<AiSettings, String> {
    update_ai(&panels, &store, |settings| {
        let mut current = serde_json::to_value(&*settings).unwrap_or_default();
        merge(&mut current, &patch);
        if let Ok(next) = serde_json::from_value::<AiSettings>(current) {
            *settings = next;
        }
    })?;
    Ok(load()?.settings.ai)
}

/// A recursive object merge: what the patch names wins, what it does not is kept.
fn merge(into: &mut Value, patch: &Value) {
    match (into, patch) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source.clone(),
    }
}

/// Opens one of the assistant's own files in whatever the system uses for it.
///
/// There is no editor in this window, and building one to show a five-line
/// markdown file would be a worse answer than the one every desktop already has.
#[tauri::command]
pub fn ai_reveal(
    what: String,
    pane: Option<String>,
    panels: State<'_, SharedPanels>,
    stores: State<'_, SharedStores>,
) -> Result<(), String> {
    let server = pane
        .and_then(|pane| panels.get(&pane))
        .map(|panel| panel.server_id())
        .unwrap_or_default();
    let path = match what.as_str() {
        "memoryGlobal" => stores.memory.file_for(MemoryScope::Global, None),
        "memoryServer" => stores.memory.file_for(MemoryScope::Server, Some(&server)),
        "trust" => stores.trust.path().clone(),
        "skills" => stores.skills.root().to_path_buf(),
        "logs" => stores.logs.dir().to_path_buf(),
        other => return Err(format!("Nothing here is called {other}.")),
    };
    /*
     * A file that has never been written has nothing to open, so it is created
     * empty rather than reported as an error the user can do nothing about --
     * "your memory file does not exist" is true and useless, and the reason they
     * pressed the button is to start writing one.
     */
    if !path.exists() {
        if path.extension().is_some() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, "");
        } else {
            let _ = std::fs::create_dir_all(&path);
        }
    }
    reveal(&path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_replaces_what_it_names_and_keeps_the_rest() {
        let mut current = json!({
            "enabled": true,
            "agent": { "mode": "ask", "maxSteps": 0 },
            "log": { "enabled": false, "keep": 20 },
        });
        merge(&mut current, &json!({ "agent": { "maxSteps": 12 } }));

        assert_eq!(current["agent"]["maxSteps"], 12);
        assert_eq!(current["agent"]["mode"], "ask", "a sibling is not dropped");
        assert_eq!(current["log"]["keep"], 20, "a section the patch never named is kept");
        assert_eq!(current["enabled"], true);
    }

    #[test]
    fn a_patch_may_replace_a_whole_section() {
        let mut current = json!({ "thinking": { "enabled": true, "effort": "high" } });
        merge(&mut current, &json!({ "thinking": { "effort": "low" } }));
        assert_eq!(current["thinking"]["effort"], "low");
        assert_eq!(current["thinking"]["enabled"], true);
    }

    #[test]
    fn the_scope_word_the_page_sends_is_read_the_way_it_is_meant() {
        assert_eq!(scope_of("global"), MemoryScope::Global);
        assert_eq!(scope_of("server"), MemoryScope::Server);
        assert_eq!(scope_of("anything else"), MemoryScope::Server);
    }

    #[test]
    fn removal_outcomes_cross_the_boundary_as_keys() {
        assert_eq!(removal_tag(RemoveOutcome::Ok), "ok");
        assert_eq!(removal_tag(RemoveOutcome::Ambiguous), "ambiguous");
    }
}
