//! Prompt presets (DESIGN.md, Prompt presets): ready-made prompts for the
//! Generate form, grouped by task, shared by every library. The built-in
//! ones are `default-presets.rsn` (embedded); the user's are files in the
//! `prompt-presets` folder next to `config.rsn`, one per preset in a folder
//! per task (and group), over the built-in ones.
//!
//! A prompt names reference images as `<imageN>` and leaves blanks for the
//! user as `{slot}` (e.g. `{scene}`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::error::{IoContext, Result};
use crate::{fsutil, rsnfmt};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PromptPresets", default)]
pub struct PromptPresets {
    pub tasks: Vec<PromptTask>,
}

/// What a group of presets does: Modify style, Remove background, ….
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PromptTask", default)]
pub struct PromptTask {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub presets: Vec<PromptPreset>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PromptPreset", default)]
pub struct PromptPreset {
    pub name: String,
    /// A heading within the task (empty: none).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// What the name means, loosely, and what you get.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub negative: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub favourite: bool,
}

impl PromptPresets {
    /// The `prompt-presets` folder next to the config file.
    pub fn dir_beside(config: &Path) -> PathBuf {
        config.with_file_name("prompt-presets")
    }

    /// The presets in `dir`, the built-in ones' copy there made current;
    /// with the preset files that didn't load (left alone), as messages. A
    /// `prompt-presets.rsn` beside it (the presets before the folder) is
    /// moved into a new folder and renamed `prompt-presets.rsn.old`. Presets
    /// of Modify style are moved into the style tasks, and saved there.
    pub fn load_dir(dir: &Path) -> Result<(PromptPresets, Vec<String>)> {
        let old = dir.with_file_name("prompt-presets.rsn");
        let fresh = !dir.exists();
        std::fs::create_dir_all(dir).at(dir)?;
        write_defaults(dir)?;
        if fresh && old.is_file() {
            let list: PromptPresets = rsnfmt::read(&old)?;
            list.save_dir(dir)?;
            std::fs::rename(&old, old.with_extension("rsn.old")).at(&old)?;
        }
        let state_path = dir.join(STATE_FILE);
        let mut state: UserState = if state_path.exists() { rsnfmt::read(&state_path)? } else { UserState::default() };
        let mut user = vec![];
        let mut bad = vec![];
        for task_dir in fsutil::subdirs(dir)? {
            let folder = task_dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
            for (group, path) in task_files(&task_dir)? {
                match read_preset_file(&path) {
                    Ok(f) => {
                        let stem = || path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                        let task = if f.task.is_empty() { folder.clone() } else { f.task };
                        let preset = PromptPreset {
                            name: if f.name.is_empty() { stem() } else { f.name },
                            group: if f.group.is_empty() { group } else { f.group },
                            description: f.description,
                            prompt: f.prompt,
                            negative: f.negative,
                            favourite: false,
                        };
                        user.push((task, preset));
                    }
                    Err(e) => bad.push(format!("prompt preset not loaded: {e}")),
                }
            }
        }
        let moved = split_old_style(&mut state, &mut user);
        let list = merge(&builtin(), &state, &user);
        if moved {
            list.save_dir(dir)?;
        }
        Ok((list, bad))
    }

    /// Writes the presets into `dir` as changes to the built-in ones: a
    /// file per changed or added preset (those of files that no longer
    /// match are removed; files that don't load are left alone), and
    /// `user-state.rsn`.
    pub fn save_dir(&self, dir: &Path) -> Result<()> {
        let (state, user) = split(self, &builtin());
        write_defaults(dir)?;
        let state_path = dir.join(STATE_FILE);
        if state == UserState::default() {
            if state_path.exists() {
                std::fs::remove_file(&state_path).at(&state_path)?;
            }
        } else {
            rsnfmt::write(&state_path, &state)?;
        }
        let mut want = std::collections::HashMap::new();
        for (task, p) in &user {
            let mut folder = dir.join(file_name(task));
            if !p.group.is_empty() {
                folder.push(file_name(&p.group));
            }
            let base = file_name(&p.name);
            let mut path = folder.join(format!("{base}.rsn"));
            // a file that doesn't load isn't written over
            let mut n = 1;
            while want.contains_key(&path) || (path.exists() && read_preset_file(&path).is_err()) {
                n += 1;
                path = folder.join(format!("{base} ({n}).rsn"));
            }
            let file = PresetFile {
                name: p.name.clone(),
                task: task.clone(),
                group: p.group.clone(),
                description: p.description.clone(),
                prompt: p.prompt.clone(),
                negative: p.negative.clone(),
            };
            want.insert(path, file);
        }
        for task_dir in fsutil::subdirs(dir)? {
            for (_, path) in task_files(&task_dir)? {
                if !want.contains_key(&path) && read_preset_file(&path).is_ok() {
                    std::fs::remove_file(&path).at(&path)?;
                }
            }
        }
        for (path, file) in &want {
            if read_preset_file(path).ok().as_ref() != Some(file) {
                rsnfmt::write(path, file)?;
            }
        }
        // folders left empty (deepest first)
        let mut dirs: Vec<_> =
            fsutil::walk(dir)?.into_iter().filter(|p| p.is_dir() && !fsutil::is_symlink(p)).collect();
        dirs.reverse();
        for d in dirs {
            if std::fs::read_dir(&d).at(&d)?.next().is_none() {
                std::fs::remove_dir(&d).at(&d)?;
            }
        }
        Ok(())
    }

    /// As saved and loaded again: the user's presets where [`merge`] puts
    /// them.
    pub fn normalized(&self) -> PromptPresets {
        let defaults = builtin();
        let (state, user) = split(self, &defaults);
        merge(&defaults, &state, &user)
    }

    pub fn count(&self) -> usize {
        self.tasks.iter().map(|t| t.presets.len()).sum()
    }
}

/// The `{slot}` blanks of a prompt, as byte ranges (braces included). A
/// slot is a short name of letters, digits, spaces, `-` and `_`.
pub fn slots(prompt: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(s) = prompt[from..].find('{').map(|i| from + i) {
        let Some(e) = prompt[s + 1..].find(['{', '}']).map(|i| s + 1 + i) else { break };
        let inner = &prompt[s + 1..e];
        let ok = prompt.as_bytes()[e] == b'}'
            && !inner.trim().is_empty()
            && inner.len() <= 40
            && inner.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'));
        if ok {
            out.push(s..e + 1);
            from = e + 1;
        } else {
            from = e;
        }
    }
    out
}

/// A style sweep's settings (the Generate form in Sweep mode): which
/// presets get a run each.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "SweepSettings", default)]
pub struct Sweep {
    pub on: bool,
    pub source: SweepSource,
    /// 0: every preset of the source; else this many of them at random.
    #[serde(skip_serializing_if = "is_zero")]
    pub random: usize,
    /// Makes the random pick repeatable (**Reroll** changes it).
    #[serde(skip_serializing_if = "is_zero")]
    pub random_seed: u64,
    /// Ticked in Choose styles (source Selected).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selected: Vec<PresetKey>,
    /// Values for the presets' `{slot}`s, by slot (`{scene}` → `scene`).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub slots: std::collections::BTreeMap<String, String>,
    /// Styles' own values for their slots, over `slots`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub style_slots: Vec<StyleSlots>,
    /// Taken out of the pick, each leaving the rest of it as it was.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<Removed>,
    /// What gets a run each: a prompt preset (a style), or a setting's value.
    #[serde(skip_serializing_if = "is_zero")]
    pub vary: SweepVary,
    /// A setting sweep's values as typed ("8, 12, 20").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub values: String,
    /// A model sweep's model presets, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Prompt enhance's values for the blanks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<BlankValues>,
    /// **Suggest for blanks…** as last set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_ask: Option<BatchAsk>,
    /// **Enhance style prompts…** as last set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enhance_ask: Option<EnhanceAsk>,
    /// Styles' prompts enhanced or edited: used for their runs instead of
    /// the one built from the template, the style and the values.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub style_prompts: Vec<StylePrompt>,
}

/// A style's prompt as prompt enhance or the user left it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StylePrompt {
    pub preset: PresetKey,
    pub prompt: String,
    /// The prompt built when it was made: one built differently now was
    /// made from older values.
    pub from: String,
    /// Prompt enhance's (perhaps edited since), else the user's own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enhanced: bool,
}

/// The values prompt enhance offered for a blank: its shared value
/// (`style` None) or a style's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlankValues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<PresetKey>,
    pub slot: String,
    /// Shown to pick from.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
    /// Every one offered (rerolled ones too): asks avoid them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub offered: Vec<String>,
    /// Its **Suggest…** as last set (None: as by default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<SuggestAsk>,
}

/// How many values an ask for a blank's suggests, unless told otherwise.
pub const SUGGEST_COUNT: usize = 6;

/// How prompt enhance is asked for a blank's values: **Suggest…**'s
/// settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuggestAsk {
    /// Instructions sent with the ask ("only stones").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub how: String,
    pub count: usize,
    /// None: the server's, as set in Settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<crate::pe::Sampling>,
    /// Attach the reference and start images (with a vision server).
    pub images: bool,
    /// Name the values offered before, not to offer them again.
    pub avoid: bool,
    /// The answer replaces the values offered, rather than adding to them.
    pub replace: bool,
}

impl Default for SuggestAsk {
    fn default() -> Self {
        SuggestAsk {
            how: String::new(),
            count: SUGGEST_COUNT,
            sampling: None,
            images: true,
            avoid: true,
            replace: false,
        }
    }
}

/// How prompt enhance is asked to improve styles' prompts: **Enhance
/// style prompts…**'s settings, and the styles ticked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnhanceAsk {
    /// Instructions sent with each ask ("more cinematic").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub how: String,
    /// Improve the prompt as the row shows it (an edit, an enhancement)
    /// rather than the one built from the template and values.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub from_shown: bool,
    /// None: the server's, as set in Settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<crate::pe::Sampling>,
    /// Attach the reference and start images (with a vision server).
    pub images: bool,
    /// The longest answer, in tokens.
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<PresetKey>,
}

impl Default for EnhanceAsk {
    fn default() -> Self {
        EnhanceAsk {
            how: String::new(),
            from_shown: false,
            sampling: None,
            images: true,
            max_tokens: 400,
            styles: vec![],
        }
    }
}

/// **Suggest for blanks…**: the settings each ask gets, and the shared
/// blanks ticked (by slot).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchAsk {
    pub ask: SuggestAsk,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blanks: Vec<String>,
}

/// A style's own slot values in a sweep (an empty one: the shared value).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSlots {
    pub preset: PresetKey,
    pub slots: std::collections::BTreeMap<String, String>,
}

impl Sweep {
    /// The values offered for a blank, if any.
    pub fn blank_values(&self, style: Option<&PresetKey>, slot: &str) -> Option<&BlankValues> {
        self.suggestions.iter().find(|v| v.style.as_ref() == style && v.slot == slot)
    }

    /// Changes a blank's values offered (or its ask); an entry left empty
    /// is dropped.
    pub fn edit_blank_values(&mut self, style: Option<&PresetKey>, slot: &str, f: impl FnOnce(&mut BlankValues)) {
        let i = match self.suggestions.iter().position(|v| v.style.as_ref() == style && v.slot == slot) {
            Some(i) => i,
            None => {
                let v = BlankValues { style: style.cloned(), slot: slot.into(), ..Default::default() };
                self.suggestions.push(v);
                self.suggestions.len() - 1
            }
        };
        f(&mut self.suggestions[i]);
        let v = &self.suggestions[i];
        if v.shown.is_empty() && v.offered.is_empty() && v.ask.is_none() {
            self.suggestions.remove(i);
        }
    }

    /// A style's own value for a slot (empty: none).
    pub fn own_slot(&self, preset: &PresetKey, slot: &str) -> &str {
        self.style_slots.iter().find(|s| s.preset == *preset).and_then(|s| s.slots.get(slot)).map_or("", String::as_str)
    }

    /// Sets (or with an empty value, clears) a style's own value for a slot.
    pub fn set_own_slot(&mut self, preset: &PresetKey, slot: &str, value: String) {
        let i = match self.style_slots.iter().position(|s| s.preset == *preset) {
            Some(i) => i,
            None => {
                self.style_slots.push(StyleSlots { preset: preset.clone(), slots: Default::default() });
                self.style_slots.len() - 1
            }
        };
        if value.is_empty() {
            self.style_slots[i].slots.remove(slot);
        } else {
            self.style_slots[i].slots.insert(slot.into(), value);
        }
        if self.style_slots[i].slots.is_empty() {
            self.style_slots.remove(i);
        }
    }

    /// A style's prompt enhanced or edited, if it has one.
    pub fn style_prompt(&self, preset: &PresetKey) -> Option<&StylePrompt> {
        self.style_prompts.iter().find(|s| s.preset == *preset)
    }

    /// Sets a style's prompt (None: back to the one built).
    pub fn set_style_prompt(&mut self, preset: &PresetKey, p: Option<StylePrompt>) {
        self.style_prompts.retain(|s| s.preset != *preset);
        self.style_prompts.extend(p);
    }

    /// The texts the chosen styles' blanks come from: each preset's prompt
    /// and, if it has one, the style's own (enhanced or edited, which can
    /// add blanks of its own).
    pub fn blank_texts<'a>(&'a self, chosen: &[(&'a PromptTask, &'a PromptPreset)]) -> Vec<&'a str> {
        let mut v = vec![];
        for (t, p) in chosen {
            v.push(p.prompt.as_str());
            if let Some(own) = self.style_prompt(&PresetKey { task: t.name.clone(), name: p.name.clone() }) {
                v.push(own.prompt.as_str());
            }
        }
        v
    }

    /// The prompt for a style's run: its own (enhanced or edited, any blank
    /// left filled from the values), else the one built.
    pub fn prompt_for(&self, template: &str, preset: &PresetKey, style: &str) -> String {
        match self.style_prompt(preset) {
            Some(own) => fill_slots(own.prompt.clone(), &self.slots_for(preset)),
            None => sweep_prompt(template, style, &self.slots_for(preset)),
        }
    }

    /// Styles' own slot values (kept from before style prompts) folded into
    /// their prompts: each style with any gets its prompt as they built it,
    /// an edit made from the shared values. The styles are (key, prompt).
    pub fn fold_style_slots(&mut self, template: &str, styles: &[(PresetKey, &str)]) -> bool {
        if self.style_slots.is_empty() {
            return false;
        }
        let mut made = vec![];
        for (key, style) in styles {
            if self.style_slots.iter().any(|s| s.preset == *key) && self.style_prompt(key).is_none() {
                made.push((key.clone(), sweep_prompt(template, style, &self.slots_for(key)), *style));
            }
        }
        self.style_slots.clear();
        for (key, prompt, style) in made {
            let from = sweep_prompt(template, style, &self.slots);
            if prompt != from {
                self.style_prompts.push(StylePrompt { preset: key, prompt, from, enhanced: false });
            }
        }
        true
    }

    /// The slot values for a style's run: its own, else the shared ones.
    pub fn slots_for(&self, preset: &PresetKey) -> std::collections::BTreeMap<String, String> {
        let mut m = self.slots.clone();
        if let Some(own) = self.style_slots.iter().find(|s| s.preset == *preset) {
            for (k, v) in &own.slots {
                if !v.trim().is_empty() {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        m
    }
}

/// What a sweep varies, run to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweepVary {
    /// The prompt: a prompt preset per run.
    #[default]
    Style,
    Steps,
    Cfg,
    FlowShift,
    /// img2img strength (with a start image).
    Strength,
    /// The model preset (each with its own steps, CFG and flow shift).
    Model,
}

impl SweepVary {
    pub const EVERY: [SweepVary; 6] = [
        SweepVary::Style,
        SweepVary::Steps,
        SweepVary::Cfg,
        SweepVary::FlowShift,
        SweepVary::Strength,
        SweepVary::Model,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SweepVary::Style => "Style",
            SweepVary::Steps => "Steps",
            SweepVary::Cfg => "CFG",
            SweepVary::FlowShift => "Flow shift",
            SweepVary::Strength => "Strength",
            SweepVary::Model => "Model",
        }
    }

    /// The [`crate::model::SweepParam`] name (empty for Style).
    pub fn key(self) -> &'static str {
        match self {
            SweepVary::Style => "",
            SweepVary::Steps => "steps",
            SweepVary::Cfg => "cfg",
            SweepVary::FlowShift => "flow_shift",
            SweepVary::Strength => "strength",
            SweepVary::Model => "model",
        }
    }
}

/// A preset taken out of a sweep's pick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removed {
    pub preset: PresetKey,
    /// A random pick takes the next preset in its order instead (else the
    /// sweep has one style fewer).
    pub replace: bool,
}

fn is_zero<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SweepSource {
    /// The presets ticked in Choose styles (older drafts: the others).
    #[default]
    Selected,
    Favourites,
    /// Every preset of a task.
    Task(String),
    /// Every preset of a group within a task.
    Group(String, String),
}

/// A preset by task and name (survives reordering and edits elsewhere).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename = "Preset")]
pub struct PresetKey {
    pub task: String,
    pub name: String,
}

impl Sweep {
    pub fn is_default(&self) -> bool {
        *self == Sweep::default()
    }
}

/// The presets a sweep makes runs for, in list order: the source's, or
/// `random` of them picked with `random_seed`, less the removed ones.
pub fn sweep_presets<'a>(list: &'a PromptPresets, sweep: &Sweep) -> Vec<(&'a PromptTask, &'a PromptPreset)> {
    let all = list.tasks.iter().flat_map(|t| t.presets.iter().map(move |p| (t, p)));
    let v: Vec<_> = all
        .filter(|(t, p)| match &sweep.source {
            SweepSource::Selected => sweep.selected.iter().any(|k| k.task == t.name && k.name == p.name),
            SweepSource::Favourites => p.favourite,
            SweepSource::Task(task) => t.name == *task,
            SweepSource::Group(task, group) => t.name == *task && p.group == *group,
        })
        .collect();
    let removed = |(t, p): &(&PromptTask, &PromptPreset)| {
        sweep.removed.iter().find(|r| r.preset.task == t.name && r.preset.name == p.name)
    };
    if sweep.random == 0 || sweep.random >= v.len() {
        return v.into_iter().filter(|x| removed(x).is_none()).collect();
    }
    // a Fisher–Yates shuffle with a splitmix64 stream; its first `random`
    // are the pick, a replaced preset handing its place to the next one
    let mut x = sweep.random_seed;
    let mut next = move || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut idx: Vec<usize> = (0..v.len()).collect();
    for i in 0..idx.len() {
        let j = i + (next() % (idx.len() - i) as u64) as usize;
        idx.swap(i, j);
    }
    let mut keep = vec![];
    let mut places = sweep.random;
    for i in idx {
        if places == 0 {
            break;
        }
        match removed(&v[i]) {
            Some(r) if r.replace => {}
            Some(_) => places -= 1,
            None => {
                keep.push(i);
                places -= 1;
            }
        }
    }
    keep.sort();
    keep.into_iter().map(|i| v[i]).collect()
}

/// Which of these presets have this `{slot}` (name without braces), in
/// their prompt or the style's own.
pub fn needing_slot<'a>(
    sweep: &Sweep,
    chosen: &[(&'a PromptTask, &'a PromptPreset)],
    slot: &str,
) -> Vec<(&'a PromptTask, &'a PromptPreset)> {
    chosen.iter().filter(|c| slot_names(sweep.blank_texts(&[**c])).iter().any(|n| n == slot)).copied().collect()
}

/// A sweep run's prompt: the preset's prompt at the template's `{preset}`
/// (the preset's own prompt when the template has none), then the slots
/// filled in.
pub fn sweep_prompt(template: &str, preset: &str, slots: &std::collections::BTreeMap<String, String>) -> String {
    let text = if template.contains(PRESET_SLOT) { template.replace(PRESET_SLOT, preset) } else { preset.to_string() };
    fill_slots(text, slots)
}

/// `text` with its `{slot}`s filled in (those with a value).
pub fn fill_slots(mut text: String, slots: &std::collections::BTreeMap<String, String>) -> String {
    for (k, v) in slots {
        if !v.trim().is_empty() {
            text = text.replace(&format!("{{{k}}}"), v.trim());
        }
    }
    text
}

/// The blanks `answer` has that `asked` didn't (names without braces).
pub fn added_slots(asked: &str, answer: &str) -> Vec<String> {
    let had = slot_names([asked]);
    slot_names([answer]).into_iter().filter(|n| !had.contains(n)).collect()
}

/// These blanks made plain words: `{scrap_metal}` → `scrap metal`.
pub fn unblank(mut text: String, names: &[String]) -> String {
    for n in names {
        text = text.replace(&format!("{{{n}}}"), &n.replace(['_', '-'], " "));
    }
    text
}

/// Where a sweep's template takes each preset's prompt.
pub const PRESET_SLOT: &str = "{preset}";

/// The `{slot}` names (without braces) of these texts, first-seen order,
/// `{preset}` left out.
pub fn slot_names<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in texts {
        for r in slots(t) {
            let name = t[r.start + 1..r.end - 1].to_string();
            if name != "preset" && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// The built-in style tasks: Lazy ones have no `{slot}`, Effortful ones at
/// least one, the Text ones one of [`TEXT_SLOTS`].
pub const LAZY_STYLE: &str = "Lazy Modify Style";
pub const EFFORTFUL_STYLE: &str = "Effortful Modify Style";
pub const TEXT_STYLE: &str = "Effortful Modify Style — Text";
const STYLE_TASKS: [&str; 3] = [LAZY_STYLE, EFFORTFUL_STYLE, TEXT_STYLE];

/// The slots for words drawn in the image, by the part they play: any
/// one makes a Text style.
pub const TEXT_SLOTS: [&str; 7] = ["text", "title", "slogan", "name", "dialogue", "sound", "caption"];

fn has_text_slot(names: &[String]) -> bool {
    names.iter().any(|n| TEXT_SLOTS.contains(&n.as_str()))
}

/// The one task the style tasks were split from.
const OLD_STYLE: &str = "Modify style";

/// Why a preset with this prompt can't be in this task (a style task's
/// rule for `{slot}`s), if it can't.
pub fn slot_rule(task: &str, prompt: &str) -> Option<&'static str> {
    let names = slot_names([prompt]);
    match task {
        LAZY_STYLE if !names.is_empty() => Some("can't have a {slot}: put it in Effortful Modify Style"),
        EFFORTFUL_STYLE if names.is_empty() => Some("needs a {slot} for the user to fill in"),
        TEXT_STYLE if !has_text_slot(&names) => {
            Some("needs a slot for its words: {text}, {title}, {slogan}, {name}, {dialogue}, {sound} or {caption}")
        }
        _ => None,
    }
}

/// The style task an old Modify style preset with this prompt goes to.
fn style_task_for(prompt: &str) -> &'static str {
    let names = slot_names([prompt]);
    if has_text_slot(&names) {
        TEXT_STYLE
    } else if names.is_empty() {
        LAZY_STYLE
    } else {
        EFFORTFUL_STYLE
    }
}

/// `key` with its task as it is since Modify style was split: the style
/// task of `list` that has a preset of its name, else Lazy.
pub fn current_key(key: &PresetKey, list: &PromptPresets) -> PresetKey {
    if key.task != OLD_STYLE {
        return key.clone();
    }
    let task = STYLE_TASKS
        .into_iter()
        .find(|t| list.tasks.iter().any(|x| x.name == *t && x.presets.iter().any(|p| p.name == key.name)))
        .unwrap_or(LAZY_STYLE);
    PresetKey { task: task.into(), name: key.name.clone() }
}

impl Sweep {
    /// The sweep with Modify style's presets in the style tasks (see
    /// [`current_key`]); a source of the whole task or a group becomes
    /// Lazy's.
    pub fn current(&self, list: &PromptPresets) -> Sweep {
        let mut s = self.clone();
        for k in &mut s.selected {
            *k = current_key(k, list);
        }
        for r in &mut s.removed {
            r.preset = current_key(&r.preset, list);
        }
        match &mut s.source {
            SweepSource::Task(t) | SweepSource::Group(t, _) if t == OLD_STYLE => *t = LAZY_STYLE.into(),
            _ => {}
        }
        s
    }
}

/// Moves what the user had in Modify style into the style tasks: each
/// preset by its slots ([`style_task_for`]), favourites and deletions with
/// their preset, a changed description to Lazy. Whether anything moved.
fn split_old_style(state: &mut UserState, user: &mut [(String, PromptPreset)]) -> bool {
    let mut moved = false;
    for (task, p) in user.iter_mut() {
        if task == OLD_STYLE {
            *task = style_task_for(&p.prompt).into();
            moved = true;
        }
    }
    let defaults = builtin();
    for k in state.favourites.iter_mut().chain(&mut state.hidden) {
        if k.task == OLD_STYLE {
            let mine = user.iter().find(|(t, p)| STYLE_TASKS.contains(&t.as_str()) && p.name == k.name);
            k.task = mine.map_or_else(|| current_key(k, &defaults).task, |(t, _)| t.clone());
            moved = true;
        }
    }
    for t in &mut state.tasks {
        if t.name == OLD_STYLE {
            t.name = LAZY_STYLE.into();
            moved = true;
        }
    }
    moved
}

const DEFAULTS: &str = include_str!("../default-presets.rsn");

/// The presets shipped with IncreArt (and **Restore defaults**), from
/// `default-presets.rsn`.
pub fn builtin() -> PromptPresets {
    static BUILTIN: LazyLock<PromptPresets> =
        // (it parses: `builtin_set` reads it)
        LazyLock::new(|| rsnfmt::from_str(DEFAULTS).unwrap_or_default());
    BUILTIN.clone()
}

/// What isn't in the user's preset files: `user-state.rsn` in the presets
/// folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "UserState", default)]
pub struct UserState {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub favourites: Vec<PresetKey>,
    /// Built-in presets deleted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<PresetKey>,
    /// Tasks that aren't built in, have a changed description, or are kept
    /// with no presets.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<UserTask>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Task", default)]
pub struct UserTask {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A user preset's file, `<task>/[<group>/]<name>.rsn`. A file dropped in
/// by hand may leave out the task and group: its folders give them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "PromptPreset", default)]
struct PresetFile {
    name: String,
    task: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    group: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    prompt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    negative: String,
}

const DEFAULTS_FILE: &str = "default-presets.rsn";
const STATE_FILE: &str = "user-state.rsn";

/// `list` as changes to `defaults`: the presets that differ from theirs or
/// aren't among them (by task), and the rest.
pub fn split(list: &PromptPresets, defaults: &PromptPresets) -> (UserState, Vec<(String, PromptPreset)>) {
    let mut state = UserState::default();
    let mut user = vec![];
    for t in &list.tasks {
        let dt = defaults.tasks.iter().find(|d| d.name == t.name);
        if dt.is_none_or(|d| d.description != t.description) || t.presets.is_empty() {
            state.tasks.push(UserTask { name: t.name.clone(), description: t.description.clone() });
        }
        for p in &t.presets {
            let key = PresetKey { task: t.name.clone(), name: p.name.clone() };
            if p.favourite {
                state.favourites.push(key);
            }
            let p = PromptPreset { favourite: false, ..p.clone() };
            if dt.and_then(|d| d.presets.iter().find(|d| d.name == p.name)) != Some(&p) {
                user.push((t.name.clone(), p));
            }
        }
    }
    for d in &defaults.tasks {
        let t = list.tasks.iter().find(|t| t.name == d.name);
        for p in &d.presets {
            if t.is_none_or(|t| !t.presets.iter().any(|x| x.name == p.name)) {
                state.hidden.push(PresetKey { task: d.name.clone(), name: p.name.clone() });
            }
        }
    }
    (state, user)
}

/// `defaults` with the user's changes. A user preset takes the place of the
/// built-in one of its name (when in the same group), else comes after its
/// group's last preset (or at the end), in name order; user tasks come after
/// the built-in ones, in name order.
pub fn merge(defaults: &PromptPresets, state: &UserState, user: &[(String, PromptPreset)]) -> PromptPresets {
    let lower = |s: &str| s.to_lowercase();
    let mut list = defaults.clone();
    for t in &mut list.tasks {
        t.presets.retain(|p| !state.hidden.iter().any(|k| k.task == t.name && k.name == p.name));
    }
    let mut extra: Vec<&str> =
        state.tasks.iter().map(|t| t.name.as_str()).chain(user.iter().map(|(t, _)| t.as_str())).collect();
    extra.retain(|n| !defaults.tasks.iter().any(|d| d.name == *n));
    extra.sort_by_key(|n| lower(n));
    extra.dedup();
    list.tasks.extend(extra.into_iter().map(|n| PromptTask { name: n.to_string(), ..Default::default() }));
    for u in &state.tasks {
        if let Some(t) = list.tasks.iter_mut().find(|t| t.name == u.name) {
            t.description = u.description.clone();
        }
    }
    let mut user: Vec<_> = user.iter().collect();
    user.sort_by_key(|(_, p)| (lower(&p.group), lower(&p.name)));
    for (task, p) in user {
        // (every user task was added above)
        let Some(t) = list.tasks.iter_mut().find(|t| t.name == *task) else { continue };
        let p = PromptPreset { favourite: false, ..p.clone() };
        match t.presets.iter().position(|x| x.name == p.name) {
            Some(i) if t.presets[i].group == p.group => t.presets[i] = p,
            old => {
                if let Some(i) = old {
                    t.presets.remove(i);
                }
                let at = t.presets.iter().rposition(|x| x.group == p.group).map_or(t.presets.len(), |i| i + 1);
                t.presets.insert(at, p);
            }
        }
    }
    list.tasks.retain(|t| !t.presets.is_empty() || state.tasks.iter().any(|u| u.name == t.name));
    for k in &state.favourites {
        let p =
            list.tasks.iter_mut().filter(|t| t.name == k.task).flat_map(|t| &mut t.presets).find(|p| p.name == k.name);
        if let Some(p) = p {
            p.favourite = true;
        }
    }
    list
}

/// A file or folder name for `name`: characters a file system may refuse
/// become `-`.
fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '-' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() || s.starts_with('.') { format!("_{s}") } else { s }
}

fn read_preset_file(path: &Path) -> Result<PresetFile> {
    rsnfmt::read(path)
}

/// The `.rsn` files of a task folder, with their group folder's name.
fn task_files(task_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = vec![];
    for p in fsutil::walk(task_dir)? {
        let Ok(rel) = p.strip_prefix(task_dir) else { continue };
        let depth = rel.components().count();
        if p.extension().is_some_and(|e| e == "rsn") && !fsutil::hidden(&p) && depth <= 2 && p.is_file() {
            // (none at the top: its parent is empty)
            let group = rel.parent().map(|g| g.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((group, p));
        }
    }
    Ok(out)
}

/// Writes the built-in presets' copy if it isn't current.
fn write_defaults(dir: &Path) -> Result<()> {
    let path = dir.join(DEFAULTS_FILE);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(DEFAULTS) {
        fsutil::write_atomic(&path, DEFAULTS.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voxel_asks_for_3d_and_not_pixel_art() {
        let b = builtin();
        let modify = b.tasks.iter().find(|t| t.name == LAZY_STYLE).unwrap();
        let voxel = modify.presets.iter().find(|p| p.name == "Voxel").unwrap();
        assert!(voxel.prompt.contains("3D voxel render") && !voxel.prompt.contains("isometric"));
        assert!(voxel.prompt.ends_with("Keep the subject, pose and identity unchanged."), "the camera may change");
        assert_eq!(voxel.negative, "pixel art, 2D, flat, sprite, pixelated");
        let robot = modify.presets.iter().find(|p| p.name == "Animatronic").unwrap();
        assert!(robot.prompt.contains("physical animatronic figure"));
        assert!(robot.prompt.ends_with("Keep the subject, pose and identity unchanged."));
        assert!(robot.negative.starts_with("drawing, illustration, 2D"));
        let other = modify.presets.iter().find(|p| p.name == "Claymation").unwrap();
        assert!(
            other.prompt.ends_with("Keep the composition, pose and identity unchanged.") && other.negative.is_empty()
        );
    }

    #[test]
    fn added_blanks_made_words() {
        let added = added_slots("a {material} cat", "a {material} cat on a {scrap_metal} {pole}");
        assert_eq!(added, ["scrap_metal", "pole"]);
        assert_eq!(
            unblank("on a {scrap_metal} {pole}, {material}".into(), &added),
            "on a scrap metal pole, {material}"
        );
    }

    #[test]
    fn the_defaults_read_with_crlf_line_ends() {
        let crlf: PromptPresets = rsnfmt::from_str(&DEFAULTS.replace('\n', "\r\n")).unwrap();
        assert_eq!(crlf.count(), builtin().count());
    }

    #[test]
    fn builtin_set() {
        let p = builtin();
        assert_eq!(p.tasks.len(), 11);
        assert!(p.count() >= 160, "{}", p.count());
        let names: Vec<_> = p.tasks.iter().take(3).map(|t| (t.name.as_str(), t.presets.len())).collect();
        assert_eq!(names, [(LAZY_STYLE, 68), (EFFORTFUL_STYLE, 53), (TEXT_STYLE, 46)]);
        for t in &p.tasks {
            for x in &t.presets {
                assert!(!x.description.is_empty() && x.prompt.contains("<image1>"), "{}", x.name);
                assert_eq!(slot_rule(&t.name, &x.prompt), None, "{}", x.name);
            }
        }
    }

    #[test]
    fn slot_ranges() {
        let p = "Add {object} to <image1>, {not a slot!} and {colour}{";
        let s: Vec<_> = slots(p).into_iter().map(|r| &p[r]).collect();
        assert_eq!(s, ["{object}", "{colour}"]);
        assert!(slots("no {} slots {").is_empty());
    }

    #[test]
    fn sweeps_pick_and_fill() {
        let list = builtin();
        let mut sw = Sweep { source: SweepSource::Group(LAZY_STYLE.into(), "3D & craft".into()), ..Default::default() };
        let names = |sw: &Sweep| sweep_presets(&list, sw).iter().map(|(_, p)| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&sw).len(), 18);
        sw.random = 4;
        sw.random_seed = 7;
        let a = names(&sw);
        assert_eq!(a.len(), 4);
        assert_eq!(a, names(&sw), "repeatable");
        sw.random_seed = 8;
        assert_ne!(a, names(&sw));
        let all = names(&Sweep { random: 0, ..sw.clone() });
        let pos: Vec<_> = a.iter().map(|n| all.iter().position(|x| x == n).unwrap()).collect();
        assert!(pos.windows(2).all(|w| w[0] < w[1]), "list order");

        sw = Sweep { selected: vec![PresetKey { task: "Text".into(), name: "Add sign".into() }], ..Default::default() };
        assert_eq!(names(&sw), ["Add sign"]);
        let mut slots = std::collections::BTreeMap::new();
        slots.insert("text".to_string(), " OPEN ".to_string());
        let p = sweep_presets(&list, &sw)[0].1;
        assert_eq!(sweep_prompt("", &p.prompt, &slots), p.prompt.replace("{text}", "OPEN"));
        assert_eq!(sweep_prompt("a fox, {preset}", "watercolour", &slots), "a fox, watercolour");
        assert_eq!(slot_names(["{preset} in {scene}", "{text} {scene}"]), ["scene", "text"]);
    }

    #[test]
    fn sweep_removals_keep_the_rest() {
        // a user's preset with a slot among Lazy's
        let mut list = builtin();
        let de = list.tasks[1].presets.iter().find(|p| p.name == "Double exposure").unwrap().clone();
        list.tasks[0].presets.push(de);
        let key = |n: &str| PresetKey { task: LAZY_STYLE.into(), name: n.into() };
        let names = |sw: &Sweep| sweep_presets(&list, sw).iter().map(|(_, p)| p.name.clone()).collect::<Vec<_>>();
        let mut sw = Sweep { source: SweepSource::Task(LAZY_STYLE.into()), random: 10, ..Default::default() };
        // a seed whose pick has Double exposure
        while !names(&sw).iter().any(|n| n == "Double exposure") {
            sw.random_seed += 1;
        }
        let a = names(&sw);
        let rest: Vec<_> = a.iter().filter(|n| *n != "Double exposure").cloned().collect();

        sw.removed = vec![Removed { preset: key("Double exposure"), replace: false }];
        assert_eq!(names(&sw), rest, "omitted");

        sw.removed[0].replace = true;
        let b = names(&sw);
        assert_eq!(b.len(), 10);
        assert!(!b.iter().any(|n| n == "Double exposure"));
        assert!(rest.iter().all(|n| b.contains(n)), "replaced, the rest kept");
        let chosen = sweep_presets(&list, &sw);
        assert!(slot_names(chosen.iter().map(|(_, p)| p.prompt.as_str())).is_empty());

        sw.random = 0;
        assert_eq!(names(&sw).len(), 68, "all but the removed one");
        let chosen = sweep_presets(&list, &Sweep { removed: vec![], ..sw.clone() });
        let need = needing_slot(&sw, &chosen, "backdrop");
        assert_eq!(need.iter().map(|(_, p)| p.name.as_str()).collect::<Vec<_>>(), ["Double exposure"]);

        // a style's own prompt can bring a blank its preset doesn't have
        let (t, p) = chosen[0];
        let key = PresetKey { task: t.name.clone(), name: p.name.clone() };
        let own = StylePrompt { preset: key, prompt: "a {metal} subject".into(), from: String::new(), enhanced: true };
        sw.set_style_prompt(&own.preset.clone(), Some(own));
        assert!(slot_names(sw.blank_texts(&chosen)).contains(&"metal".to_string()));
        let need = needing_slot(&sw, &chosen, "metal");
        assert_eq!(need.iter().map(|(_, p)| p.name.as_str()).collect::<Vec<_>>(), [p.name.as_str()]);
    }

    #[test]
    fn the_defaults_load_and_split_to_nothing() {
        let b = builtin();
        assert_eq!(split(&b, &b), (UserState::default(), vec![]));
        assert_eq!(merge(&b, &UserState::default(), &[]), b);
    }

    #[test]
    fn a_folder_of_changes_over_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let folder = PromptPresets::dir_beside(&dir.path().join("config.rsn"));
        let (p, bad) = PromptPresets::load_dir(&folder).unwrap();
        assert_eq!((p, bad.len()), (builtin(), 0));
        assert_eq!(std::fs::read_to_string(folder.join(DEFAULTS_FILE)).unwrap(), DEFAULTS);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "only the defaults' copy");

        // a favourite, an edit, a deletion, a new preset, a new task
        let mut p = builtin();
        let modify = &mut p.tasks[0].presets;
        let clay = modify.iter().position(|x| x.name == "Claymation").unwrap();
        modify[clay].favourite = true;
        modify[clay].prompt.push_str(" Extra.");
        modify.retain(|x| x.name != "Voxel");
        let mine = PromptPreset {
            name: "Mine: v1".into(),
            group: "3D & craft".into(),
            prompt: "x".into(),
            ..Default::default()
        };
        modify.push(mine);
        p.tasks.push(PromptTask { name: "Odd / task".into(), description: "d".into(), presets: vec![] });
        let normal = p.normalized();
        p.save_dir(&folder).unwrap();
        let (back, bad) = PromptPresets::load_dir(&folder).unwrap();
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(back, normal);
        let modify = &back.tasks[0].presets;
        let at = |n: &str| modify.iter().position(|x| x.name == n).unwrap();
        assert!(modify[at("Claymation")].favourite && modify[at("Claymation")].prompt.ends_with("Extra."));
        assert_eq!(at("Claymation"), clay, "an edited one keeps its place");
        assert!(!modify.iter().any(|x| x.name == "Voxel"));
        assert_eq!(modify[at("Mine: v1")].group, "3D & craft");
        assert_eq!(modify[at("Mine: v1") - 1].group, "3D & craft", "after its group's others");
        assert_eq!(back.tasks.last().unwrap().name, "Odd / task");
        assert!(folder.join("Lazy Modify Style/3D & craft/Mine- v1.rsn").is_file());
        assert!(folder.join("Lazy Modify Style/3D & craft/Claymation.rsn").is_file());

        // a file dropped in by hand: task and group from its folders; one
        // that doesn't load is reported and kept
        let drop = folder.join("Lazy Modify Style/Pasta/Fusilli.rsn");
        std::fs::create_dir_all(drop.parent().unwrap()).unwrap();
        std::fs::write(&drop, "PromptPreset { name: \"Fusilli\", prompt: \"twirls\" }\n").unwrap();
        std::fs::write(folder.join("Lazy Modify Style/broken.rsn"), "nope").unwrap();
        let (back, bad) = PromptPresets::load_dir(&folder).unwrap();
        assert_eq!(bad.len(), 1);
        let f = back.tasks[0].presets.iter().find(|x| x.name == "Fusilli").unwrap();
        assert_eq!((f.group.as_str(), f.prompt.as_str()), ("Pasta", "twirls"));

        // back to the defaults: only the copy and the broken file are left
        builtin().save_dir(&folder).unwrap();
        assert_eq!(PromptPresets::load_dir(&folder).unwrap().0, builtin());
        assert!(folder.join("Lazy Modify Style/broken.rsn").is_file());
        assert!(!folder.join(STATE_FILE).exists() && !folder.join("Lazy Modify Style/Pasta").exists());
    }

    #[test]
    fn modify_style_splits_into_the_style_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let folder = PromptPresets::dir_beside(&dir.path().join("config.rsn"));
        std::fs::create_dir_all(folder.join("Modify style/Mine")).unwrap();
        let file = |name: &str, prompt: &str| {
            let text = format!("PromptPreset {{ name: \"{name}\", prompt: \"{prompt}\" }}\n");
            std::fs::write(folder.join(format!("Modify style/Mine/{name}.rsn")), text).unwrap();
        };
        file("Plain", "Convert <image1> into felt.");
        file("Slotted", "Transform <image1> into {material}.");
        file("Lettered", "Transform <image1> into a sign saying {text}.");
        let old = |n: &str| PresetKey { task: OLD_STYLE.into(), name: n.into() };
        let state = UserState {
            favourites: vec![old("Claymation"), old("Double exposure"), old("Slotted")],
            hidden: vec![old("Voxel")],
            tasks: vec![],
        };
        rsnfmt::write(&folder.join(STATE_FILE), &state).unwrap();

        let (p, bad) = PromptPresets::load_dir(&folder).unwrap();
        assert!(bad.is_empty(), "{bad:?}");
        let task_of = |n: &str| p.tasks.iter().find(|t| t.presets.iter().any(|x| x.name == n)).unwrap();
        assert_eq!(task_of("Plain").name, LAZY_STYLE);
        assert_eq!(task_of("Slotted").name, EFFORTFUL_STYLE);
        assert_eq!(task_of("Lettered").name, TEXT_STYLE);
        assert!(!p.tasks.iter().any(|t| t.name == OLD_STYLE));
        let fav = |n: &str| task_of(n).presets.iter().find(|x| x.name == n).unwrap().favourite;
        assert!(fav("Claymation") && fav("Double exposure") && fav("Slotted"));
        assert_eq!(task_of("Double exposure").name, EFFORTFUL_STYLE);
        assert!(!p.tasks[0].presets.iter().any(|x| x.name == "Voxel"));

        // saved where they now are, and loaded the same again
        assert!(!folder.join(OLD_STYLE).exists());
        assert!(folder.join("Effortful Modify Style/Mine/Slotted.rsn").is_file());
        assert_eq!(PromptPresets::load_dir(&folder).unwrap().0, p);

        let sw = Sweep {
            source: SweepSource::Group(OLD_STYLE.into(), "Photo & film".into()),
            selected: vec![old("Claymation"), old("Double exposure"), old("Lettered")],
            ..Default::default()
        };
        let sw = sw.current(&p);
        assert_eq!(sw.source, SweepSource::Group(LAZY_STYLE.into(), "Photo & film".into()));
        let tasks: Vec<_> = sw.selected.iter().map(|k| k.task.as_str()).collect();
        assert_eq!(tasks, [LAZY_STYLE, EFFORTFUL_STYLE, TEXT_STYLE]);
    }

    #[test]
    fn style_tasks_keep_their_slot_rules() {
        assert_eq!(slot_rule(LAZY_STYLE, "a {preset} b"), None);
        assert!(slot_rule(LAZY_STYLE, "in {scene}").is_some());
        assert!(slot_rule(EFFORTFUL_STYLE, "plain").is_some());
        assert_eq!(slot_rule(EFFORTFUL_STYLE, "in {scene}"), None);
        assert!(slot_rule(TEXT_STYLE, "in {scene}").is_some());
        assert_eq!(slot_rule(TEXT_STYLE, "says {text}"), None);
        assert_eq!(slot_rule(TEXT_STYLE, "says {dialogue} in {place}"), None);
        assert_eq!(slot_rule("Remove background", "anything {x}"), None);
    }

    #[test]
    fn the_old_file_moves_into_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = PromptPresets::dir_beside(&dir.path().join("config.rsn"));
        let mut p = builtin();
        p.tasks[1].presets[0].favourite = true;
        p.tasks.truncate(2);
        rsnfmt::write(&dir.path().join("prompt-presets.rsn"), &p).unwrap();
        assert_eq!(PromptPresets::load_dir(&folder).unwrap().0, p);
        assert!(!dir.path().join("prompt-presets.rsn").exists());
        assert!(dir.path().join("prompt-presets.rsn.old").is_file());
    }
}
