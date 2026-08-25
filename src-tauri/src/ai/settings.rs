//! What the assistant is configured to do, as it sits in `tshell.config.json`.
//!
//! Every field is normalised on load rather than trusted, because this file is
//! meant to be edited by hand. The rule throughout is the one the config layer
//! already follows: a value that makes no sense is replaced by the default and
//! the file still opens. The single exception is [`AgentSettings::mode`], where
//! an unrecognised value must not be read generously -- guessing `auto` from a
//! misspelling would turn every confirmation off on the strength of a bad
//! character.
//!
//! No key is stored here. `secrets.rs` holds them, under [`model_secret_key`],
//! for the same reason the server passwords moved out: the config file is
//! something users read, diff and paste into issues.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::config::make_id;

/// Reads an enum written as a string without letting an unrecognised one refuse
/// the whole file.
///
/// serde's derived reader rejects a variant it does not know, which would turn
/// one mistyped word into "your servers will not load". Both enums this is used
/// for have a defined answer for anything unrecognised, so the value is taken as
/// text and handed to that.
fn lenient_mode<'de, D: Deserializer<'de>>(deserializer: D) -> Result<AgentMode, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(AgentMode::parse(value.as_str().unwrap_or_default()))
}

fn lenient_effort<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ThinkingEffort, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(match value.as_str().unwrap_or_default() {
        "low" => ThinkingEffort::Low,
        "max" => ThinkingEffort::Max,
        _ => ThinkingEffort::High,
    })
}

/// How hard a reasoning model is asked to think, when it is asked to at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingEffort {
    Low,
    #[default]
    High,
    Max,
}

impl ThinkingEffort {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

/// Thinking, and whether you watch it. Two different questions that used to be
/// one setting.
///
/// `enabled` and `effort` are sent to the endpoint and decide what the model
/// actually does -- what a step costs and how long it takes. `show` decides
/// nothing but whether the panel draws the thinking it got back. Conflating them
/// meant the only way to stop paying for thinking was to stop reading it, which
/// is not a choice anyone was making.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct ThinkingSettings {
    /// Whether the model is asked to think at all. Defaults to on, as endpoints do.
    pub enabled: bool,
    /// Only meaningful while `enabled`. `high` is also the usual server default.
    #[serde(deserialize_with = "lenient_effort")]
    pub effort: ThinkingEffort,
    /// Whether the thinking is drawn in the thread, folded. Off by default: it is
    /// the longest thing in a step and the least often wanted.
    pub show: bool,
}

impl Default for ThinkingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            effort: ThinkingEffort::High,
            show: false,
        }
    }
}

/// How much the assistant asks before it acts. See [`crate::ai::types::AgentMode`].
pub use super::types::AgentMode;

impl Default for AgentMode {
    fn default() -> Self {
        Self::Ask
    }
}

impl AgentMode {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Trust => "trust",
            Self::Auto => "auto",
        }
    }

    /// Anything unrecognised lands on `ask`. This is the one setting where a typo
    /// must not be read generously: guessing `auto` from a misspelling would turn
    /// every confirmation off on the strength of a bad character.
    pub fn parse(value: &str) -> Self {
        match value {
            "trust" => Self::Trust,
            "auto" => Self::Auto,
            _ => Self::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AgentSettings {
    /// Which confirmations are asked for. Global: the posture, not the machine.
    #[serde(deserialize_with = "lenient_mode")]
    pub mode: AgentMode,
    /// Steps one task may take before handing control back to the user. 0 is no limit.
    pub max_steps: u32,
    pub command_timeout_ms: u64,
    /// Extra command names treated as read-only, for in-house query tools.
    pub read_only_commands: Vec<String>,
    /// Characters of a command's output kept before the middle is elided.
    pub output_budget: usize,
    /// Characters of conversation carried into a request before the oldest
    /// command output is folded away. 0 carries everything, whatever it costs.
    pub context_budget: usize,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            mode: AgentMode::Ask,
            // 0 means the task runs until it is finished, refused or stopped. A
            // limit is something the user opts into, not something they are
            // handed halfway through a job.
            max_steps: 0,
            command_timeout_ms: 30_000,
            read_only_commands: Vec::new(),
            output_budget: 6_000,
            // Room for a long investigation without the request growing without
            // end. A task is mostly command output, and the oldest of it has
            // already been acted on by the time this bites.
            context_budget: 48_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct MemorySettings {
    /// Off means nothing is injected and the assistant is not offered the actions.
    pub enabled: bool,
    /// Characters each scope's file may reach. 0 makes that scope read-only.
    pub global_budget: usize,
    pub server_budget: usize,
}

impl Default for MemorySettings {
    fn default() -> Self {
        // Enough for a page of facts about a machine, and small enough that
        // carrying it on every request stays unnoticeable. The global scope gets
        // less: what is true of every machine is a handful of preferences, not
        // an inventory.
        Self {
            enabled: true,
            global_budget: 2_000,
            server_budget: 4_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct SkillSettings {
    /// Off means the manifest and the action are both kept out of the prompt.
    pub enabled: bool,
    /// Characters of one skill file read in a step before the middle is elided.
    pub max_chars: usize,
    /// Skills switched off from the panel, by directory name. They are neither
    /// offered in the manifest nor readable, so a model that remembers the name
    /// from an earlier conversation still cannot pull one in.
    pub disabled: Vec<String>,
}

impl Default for SkillSettings {
    fn default() -> Self {
        // Room for a long procedure in one step without a single file being able
        // to swallow the whole context budget on its own.
        Self {
            enabled: true,
            max_chars: 20_000,
            disabled: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct LogSettings {
    /// Off by default, and deliberately so. What this writes is the whole
    /// exchange with the model, unmasked -- which is what makes it worth having
    /// when a reply will not parse, and what makes it the wrong thing to leave
    /// running.
    pub enabled: bool,
    /// How many log files to keep. The oldest are removed as new ones are opened.
    pub keep: usize,
}

impl Default for LogSettings {
    fn default() -> Self {
        // Enough to still hold the session before the one being looked at, and
        // few enough that a forgotten setting does not fill a disk.
        Self {
            enabled: false,
            keep: 20,
        }
    }
}

/// One OpenAI-compatible endpoint the user has configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiModel {
    /// Stable across renames and reorders; what `active_model_id` points at.
    pub id: String,
    pub base_url: String,
    pub model: String,
}

/// Where one endpoint's key sits in the keychain.
pub fn model_secret_key(id: &str) -> String {
    format!("model:{id}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct AiSettings {
    pub enabled: bool,
    /// The endpoints the user has configured.
    pub models: Vec<AiModel>,
    /// Which of them answers. Empty when none is configured, which is the state
    /// a fresh install is in: the old build always had VS Code's own model to
    /// fall back on and this one has nothing, so the panel says so rather than
    /// pretending to have picked something.
    pub active_model_id: String,
    /// Whether recent terminal output may be sent along with the request.
    pub send_terminal_output: bool,
    pub output_lines: usize,
    /// How long one request to the model may take before it is abandoned.
    pub request_timeout_ms: u64,
    /// Whether the model thinks, how hard, and whether you watch it.
    pub thinking: ThinkingSettings,
    pub agent: AgentSettings,
    pub memory: MemorySettings,
    pub skills: SkillSettings,
    pub log: LogSettings,
    /// Whether the user has agreed to their terminal contents being sent to a
    /// third-party endpoint. Asked once, in the window, and remembered here.
    pub consented: bool,
    /// Anything under `ai` this build does not know, carried through untouched
    /// for the same reason `Settings::rest` exists.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            models: Vec::new(),
            active_model_id: String::new(),
            send_terminal_output: true,
            output_lines: 40,
            // Long enough for a reasoning model working through a large context,
            // short enough that a dead endpoint is reported rather than waited on.
            request_timeout_ms: 120_000,
            thinking: ThinkingSettings::default(),
            agent: AgentSettings::default(),
            memory: MemorySettings::default(),
            skills: SkillSettings::default(),
            log: LogSettings::default(),
            consented: false,
            rest: Map::new(),
        }
    }
}

fn clamp(value: usize, min: usize, max: usize) -> usize {
    value.clamp(min, max)
}

/// A name that may be used as a command or a skill directory.
fn is_plain_name(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

impl AiSettings {
    /// Bring a parsed file up to what the rest of the assistant may assume.
    pub fn normalize(&mut self) {
        self.models
            .retain(|model| !model.base_url.trim().is_empty() && !model.model.trim().is_empty());
        for model in &mut self.models {
            if model.id.trim().is_empty() {
                model.id = make_id();
            }
            model.base_url = model.base_url.trim().to_string();
            model.model = model.model.trim().to_string();
        }

        // Which model answers, resolved against the list so it always names
        // something that exists. A dangling id falls to the first configured
        // endpoint, and to nothing at all when there is none.
        let known = self
            .models
            .iter()
            .any(|model| model.id == self.active_model_id);
        if !known {
            self.active_model_id = self
                .models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_default();
        }

        self.output_lines = clamp(self.output_lines, 1, 200);
        self.request_timeout_ms = self.request_timeout_ms.clamp(10_000, 600_000);

        self.agent.max_steps = self.agent.max_steps.min(1_000);
        self.agent.command_timeout_ms = self.agent.command_timeout_ms.clamp(1_000, 600_000);
        self.agent.output_budget = clamp(self.agent.output_budget, 500, 60_000);
        self.agent.context_budget = clamp(self.agent.context_budget, 0, 400_000);
        self.agent
            .read_only_commands
            .retain(|name| is_plain_name(name));
        for name in &mut self.agent.read_only_commands {
            *name = name.trim().to_string();
        }

        self.memory.global_budget = clamp(self.memory.global_budget, 0, 20_000);
        self.memory.server_budget = clamp(self.memory.server_budget, 0, 20_000);

        self.skills.max_chars = clamp(self.skills.max_chars, 500, 200_000);
        self.skills.disabled.retain(|id| is_plain_name(id));
        for id in &mut self.skills.disabled {
            *id = id.trim().to_string();
        }

        self.log.keep = clamp(self.log.keep, 1, 500);
    }

    /// The model that answers, or `None` when the user has configured none.
    pub fn active_model(&self) -> Option<&AiModel> {
        self.models
            .iter()
            .find(|model| model.id == self.active_model_id)
    }

    /// Adds an endpoint and switches to it, which is what configuring one is for.
    ///
    /// Same endpoint and same model name means the user is re-entering one they
    /// already have -- usually to fix its key -- so that row is updated rather
    /// than duplicated, and its id survives so nothing pointing at it is orphaned.
    pub fn add_model(&mut self, base_url: &str, model: &str) -> String {
        let base_url = base_url.trim().to_string();
        let name = model.trim().to_string();
        let existing = self
            .models
            .iter()
            .position(|entry| entry.base_url == base_url && entry.model == name);
        let id = match existing {
            Some(at) => self.models[at].id.clone(),
            None => make_id(),
        };
        let entry = AiModel {
            id: id.clone(),
            base_url,
            model: name,
        };
        match existing {
            Some(at) => self.models[at] = entry,
            None => self.models.push(entry),
        }
        self.active_model_id = id.clone();
        id
    }

    /// Removes an endpoint. Removing the one in use moves to the first that is
    /// left, and to nothing when none is -- the menu never ends up pointing at
    /// something that is gone.
    pub fn remove_model(&mut self, id: &str) -> bool {
        let before = self.models.len();
        self.models.retain(|model| model.id != id);
        if self.models.len() == before {
            return false;
        }
        if self.active_model_id == id {
            self.active_model_id = self
                .models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_default();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_mode_lands_on_ask_rather_than_being_guessed_at() {
        assert_eq!(AgentMode::parse("auto"), AgentMode::Auto);
        assert_eq!(AgentMode::parse("trust"), AgentMode::Trust);
        assert_eq!(AgentMode::parse("Auto"), AgentMode::Ask);
        assert_eq!(AgentMode::parse("atuo"), AgentMode::Ask);
        assert_eq!(AgentMode::parse(""), AgentMode::Ask);
    }

    #[test]
    fn normalising_clamps_every_budget_into_range() {
        let mut settings = AiSettings {
            output_lines: 9999,
            request_timeout_ms: 1,
            agent: AgentSettings {
                max_steps: 99_999,
                command_timeout_ms: 1,
                output_budget: 1,
                context_budget: 9_999_999,
                ..Default::default()
            },
            memory: MemorySettings {
                global_budget: 99_999,
                ..Default::default()
            },
            skills: SkillSettings {
                max_chars: 1,
                ..Default::default()
            },
            log: LogSettings {
                keep: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.output_lines, 200);
        assert_eq!(settings.request_timeout_ms, 10_000);
        assert_eq!(settings.agent.max_steps, 1_000);
        assert_eq!(settings.agent.command_timeout_ms, 1_000);
        assert_eq!(settings.agent.output_budget, 500);
        assert_eq!(settings.agent.context_budget, 400_000);
        assert_eq!(settings.memory.global_budget, 20_000);
        assert_eq!(settings.skills.max_chars, 500);
        assert_eq!(settings.log.keep, 1);
    }

    #[test]
    fn a_model_missing_its_endpoint_or_name_is_dropped() {
        let mut settings = AiSettings {
            models: vec![
                AiModel {
                    id: "a".into(),
                    base_url: " ".into(),
                    model: "x".into(),
                },
                AiModel {
                    id: "b".into(),
                    base_url: "http://x".into(),
                    model: "".into(),
                },
                AiModel {
                    id: "c".into(),
                    base_url: " http://y ".into(),
                    model: " z ".into(),
                },
            ],
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.models.len(), 1);
        assert_eq!(settings.models[0].base_url, "http://y");
        assert_eq!(settings.models[0].model, "z");
    }

    #[test]
    fn a_dangling_active_id_falls_to_the_first_model() {
        let mut settings = AiSettings {
            models: vec![AiModel {
                id: "a".into(),
                base_url: "http://x".into(),
                model: "m".into(),
            }],
            active_model_id: "gone".into(),
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.active_model_id, "a");
    }

    #[test]
    fn with_no_models_there_is_nothing_to_point_at() {
        let mut settings = AiSettings {
            active_model_id: "gone".into(),
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.active_model_id, "");
        assert!(settings.active_model().is_none());
    }

    #[test]
    fn re_entering_an_endpoint_updates_the_row_rather_than_duplicating_it() {
        let mut settings = AiSettings::default();
        let first = settings.add_model("http://x", "m");
        let again = settings.add_model(" http://x ", " m ");
        assert_eq!(first, again);
        assert_eq!(settings.models.len(), 1);
    }

    #[test]
    fn removing_the_model_in_use_moves_to_one_that_is_left() {
        let mut settings = AiSettings::default();
        let first = settings.add_model("http://x", "m");
        let second = settings.add_model("http://y", "n");
        assert_eq!(settings.active_model_id, second);
        assert!(settings.remove_model(&second));
        assert_eq!(settings.active_model_id, first);
        assert!(settings.remove_model(&first));
        assert_eq!(settings.active_model_id, "");
        assert!(!settings.remove_model("gone"));
    }

    #[test]
    fn command_and_skill_names_that_are_not_names_are_dropped() {
        let mut settings = AiSettings {
            agent: AgentSettings {
                read_only_commands: vec![" myquery ".into(), "rm -rf".into(), "".into()],
                ..Default::default()
            },
            skills: SkillSettings {
                disabled: vec!["deploy".into(), "../etc".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.agent.read_only_commands, vec!["myquery"]);
        assert_eq!(settings.skills.disabled, vec!["deploy"]);
    }

    #[test]
    fn unknown_keys_under_ai_survive_a_round_trip() {
        let raw = r#"{"enabled":true,"somethingLater":{"x":1}}"#;
        let settings: AiSettings = serde_json::from_str(raw).unwrap();
        let back = serde_json::to_value(&settings).unwrap();
        assert_eq!(back["somethingLater"]["x"], 1);
    }

    #[test]
    fn the_stored_shape_carries_no_key() {
        let mut settings = AiSettings::default();
        settings.add_model("http://x", "m");
        let json = serde_json::to_string(&settings).unwrap();
        assert!(!json.contains("apiKey"), "{json}");
        assert!(!json.contains("encrypted"), "{json}");
    }
}
