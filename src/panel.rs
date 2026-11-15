//! The tool drawer's panel (`place = "generator"`): what AI Retouch does
//! with the area painted, Remove or Replace, and the prompt for it. RAWmakase
//! draws it and sends each change; this program keeps the values, in its data
//! folder too, so a prompt outlives the program stopping while idle.
use rawmakase_extension::protocol::controls::{Choice, Control, ControlKind};
use rawmakase_extension::{Context, ErrorCode, Panel, RemoteError};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The manifest's `[[panel]]` id.
pub const PANEL: &str = "retouch";
/// The panel's mode control, [`REMOVE`] or [`REPLACE`].
pub const MODE: &str = "mode";
/// The panel's prompt control.
pub const PROMPT: &str = "prompt";
/// The mode that removes what is painted, or edits it as the prompt says.
pub const REMOVE: &str = "remove";
/// The mode that puts what the prompt describes where it is painted.
pub const REPLACE: &str = "replace";

/// The file in the data folder that keeps the panel's values.
const FILE: &str = "panel.json";

/// What the panel holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Asked {
    /// Replace, rather than Remove.
    pub(crate) replace: bool,
    /// The prompt, as typed.
    pub(crate) prompt: String,
}

impl Asked {
    /// The prompt, on one line and trimmed; none when it is blank.
    pub(crate) fn text(&self) -> Option<String> {
        let text = self.prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        (!text.is_empty()).then_some(text)
    }
}

/// The panel's values, shared by the panel and the generator, read from the
/// data folder the first time either needs them.
#[derive(Clone, Default)]
pub(crate) struct Shared(Arc<Mutex<Option<Asked>>>);

impl Shared {
    /// What the panel holds now.
    pub(crate) fn get(&self, dir: &Path) -> Asked {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.get_or_insert_with(|| read(dir)).clone()
    }
    fn set(&self, dir: &Path, asked: Asked) {
        write(dir, &asked);
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(asked);
    }
}

fn read(dir: &Path) -> Asked {
    let Ok(bytes) = std::fs::read(dir.join(FILE)) else {
        return Asked::default();
    };
    let json: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    Asked {
        replace: json[MODE].as_str() == Some(REPLACE),
        prompt: json[PROMPT].as_str().unwrap_or_default().to_string(),
    }
}

/// Keeps the values; one that can't be written lasts as long as the program.
fn write(dir: &Path, asked: &Asked) {
    let json = json!({
        MODE: if asked.replace { REPLACE } else { REMOVE },
        PROMPT: asked.prompt,
    });
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(FILE), json.to_string());
}

/// The panel, drawn by RAWmakase.
pub(crate) struct RetouchPanel(pub(crate) Shared);

impl RetouchPanel {
    fn controls(asked: &Asked) -> Vec<Control> {
        let (help, placeholder) = if asked.replace {
            (
                "Needed: what to put where you paint",
                "e.g. change the shoes to elegant high heels",
            )
        } else {
            (
                "Optional: without one, what you paint is removed",
                "e.g. remove the dogs, keep the people",
            )
        };
        vec![
            Control {
                id: Some(MODE.into()),
                label: Some("Mode".into()),
                help: None,
                visible_when: None,
                disabled: false,
                kind: ControlKind::Segmented {
                    options: vec![
                        Choice {
                            id: REMOVE.into(),
                            title: "AI Remove".into(),
                        },
                        Choice {
                            id: REPLACE.into(),
                            title: "AI Replace".into(),
                        },
                    ],
                    value: if asked.replace { REPLACE } else { REMOVE }.into(),
                },
            },
            Control {
                id: Some(PROMPT.into()),
                label: Some("Prompt".into()),
                help: Some(help.into()),
                visible_when: None,
                disabled: false,
                kind: ControlKind::Text {
                    value: asked.prompt.clone(),
                    multiline: true,
                    placeholder: Some(placeholder.into()),
                },
            },
        ]
    }
}

impl Panel for RetouchPanel {
    fn open(&mut self, cx: &Context) -> Result<Vec<Control>, RemoteError> {
        Ok(Self::controls(&self.0.get(cx.data_dir())))
    }

    fn changed(
        &mut self,
        cx: &Context,
        control: &str,
        value: Value,
    ) -> Result<Vec<Control>, RemoteError> {
        let mut asked = self.0.get(cx.data_dir());
        match (control, value.as_str()) {
            (MODE, Some(mode)) => asked.replace = mode == REPLACE,
            (PROMPT, Some(prompt)) => asked.prompt = prompt.to_string(),
            _ => {
                return Err(RemoteError::new(
                    ErrorCode::InvalidRequest,
                    format!("no control {control} that takes {value}"),
                ));
            }
        }
        self.0.set(cx.data_dir(), asked.clone());
        Ok(Self::controls(&asked))
    }

    fn clicked(&mut self, cx: &Context, _control: &str) -> Result<Vec<Control>, RemoteError> {
        Ok(Self::controls(&self.0.get(cx.data_dir())))
    }
}
