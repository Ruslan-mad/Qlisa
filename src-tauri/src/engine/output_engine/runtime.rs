//! Pure, instance-scoped output routing state.
//!
//! The registry deliberately contains no windowing or GL types.  This keeps
//! routing testable on CI and gives the native render backends a stable input:
//! one pipeline configuration per named output.  A pipeline owns its voice
//! namespace; voice ids are never resolved through a process-global "current"
//! slot.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::preferences::OutputDestination;

use super::{OutputTransform, VoiceId};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutputPipelineConfig {
    pub id: String,
    pub name: String,
    pub sink_kind: crate::preferences::OutputSinkKind,
    pub monitor: Option<u32>,
    pub floating_window: Option<crate::preferences::FloatingWindowGeometry>,
    pub enabled: bool,
    pub always_on_top: bool,
    pub hide_cursor: bool,
    pub transform: OutputTransform,
    pub fullscreen_locked: bool,
}

impl OutputPipelineConfig {
    pub fn from_destination(d: &OutputDestination) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            sink_kind: d.sink_kind.clone(),
            monitor: d.monitor,
            floating_window: d.floating_window.clone(),
            enabled: d.enabled,
            always_on_top: d.always_on_top,
            hide_cursor: d.hide_cursor,
            transform: d.transform,
            fullscreen_locked: d.fullscreen_locked,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OutputPipeline {
    pub config: OutputPipelineConfig,
    pub visible: bool,
    pub monitor_present: bool,
    voices: HashSet<VoiceId>,
}

impl OutputPipeline {
    fn new(config: OutputPipelineConfig) -> Self {
        Self {
            config,
            visible: false,
            monitor_present: true,
            voices: HashSet::new(),
        }
    }
    pub fn owns_voice(&self, voice: VoiceId) -> bool {
        self.voices.contains(&voice)
    }
    fn add_voice(&mut self, voice: VoiceId) {
        self.voices.insert(voice);
    }
    fn remove_voice(&mut self, voice: VoiceId) {
        self.voices.remove(&voice);
    }
}

/// Per OutputEngine registry.  It is intentionally not static: two engines
/// (for example a live workspace and a headless test engine) cannot affect one
/// another's destinations or voice ownership.
#[derive(Debug, Default)]
pub(crate) struct OutputRegistry {
    pipelines: BTreeMap<String, OutputPipeline>,
    default_id: Option<String>,
    voice_owner: HashMap<VoiceId, String>,
}

impl OutputRegistry {
    pub fn legacy_default() -> Self {
        Self::new(
            &[OutputDestination {
                id: "default".into(),
                name: "Main".into(),
                sink_kind: crate::preferences::OutputSinkKind::Display,
                network: Default::default(),
                monitor: None,
                floating_window: None,
                enabled: true,
                always_on_top: false,
                hide_cursor: false,
                transform: OutputTransform::default(),
                fullscreen_locked: true,
            }],
            "default",
        )
    }
    pub fn new(destinations: &[OutputDestination], default_id: &str) -> Self {
        let mut out = Self::default();
        out.update(destinations, default_id);
        out
    }

    /// Atomically reconcile additions, updates and removals.  Removed output
    /// ids release their voice mappings; an invalid default resolves to the
    /// first enabled destination, then to the first destination.
    pub fn update(&mut self, destinations: &[OutputDestination], default_id: &str) {
        let mut next = BTreeMap::new();
        for d in destinations.iter().filter(|d| !d.id.trim().is_empty()) {
            let cfg = OutputPipelineConfig::from_destination(d);
            let mut pipeline = self
                .pipelines
                .remove(&cfg.id)
                .unwrap_or_else(|| OutputPipeline::new(cfg.clone()));
            pipeline.config = cfg;
            next.insert(pipeline.config.id.clone(), pipeline);
        }
        self.pipelines = next;
        self.voice_owner
            .retain(|_, id| self.pipelines.contains_key(id));
        for p in self.pipelines.values_mut() {
            p.voices
                .retain(|v| self.voice_owner.get(v) == Some(&p.config.id));
        }
        self.default_id = (!default_id.trim().is_empty()
            && self.pipelines.contains_key(default_id))
        .then(|| default_id.to_owned())
        .or_else(|| {
            self.pipelines
                .values()
                .find(|p| p.config.enabled)
                .map(|p| p.config.id.clone())
        })
        .or_else(|| self.pipelines.keys().next().cloned());
    }

    pub fn resolve(&self, requested: Option<&str>) -> Option<String> {
        match requested {
            // An explicit destination is an operator choice.  Never silently
            // send a cue targeting a missing/disabled output to the default
            // monitor; report it as unavailable so the show cannot appear on
            // the wrong screen.
            Some(id) => self
                .pipelines
                .get(id)
                .filter(|p| p.config.enabled)
                .map(|_| id.to_owned()),
            None => self.default_id.clone(),
        }
    }
    pub fn default_id(&self) -> Option<String> {
        self.default_id.clone()
    }
    pub fn pipeline(&self, id: &str) -> Option<&OutputPipeline> {
        self.pipelines.get(id)
    }
    pub fn pipeline_mut(&mut self, id: &str) -> Option<&mut OutputPipeline> {
        self.pipelines.get_mut(id)
    }

    /// Update only the monitor assignment of one registered destination.
    ///
    /// The native pipeline is moved by `OutputEngine`; this method keeps
    /// the pure registry in sync without reconciling or recreating any
    /// pipeline.
    pub fn set_monitor(&mut self, id: &str, monitor: Option<u32>) -> bool {
        let Some(pipeline) = self.pipelines.get_mut(id) else {
            return false;
        };
        pipeline.config.monitor = monitor;
        true
    }

    /// Update a batch only after every destination is known to the registry.
    /// This prevents a partial registry mutation if a caller supplies a stale
    /// output id during an assignment swap.
    pub fn set_monitors(&mut self, assignments: &[(String, Option<u32>)]) -> bool {
        if assignments
            .iter()
            .any(|(id, _)| !self.pipelines.contains_key(id))
        {
            return false;
        }
        for (id, monitor) in assignments {
            if let Some(pipeline) = self.pipelines.get_mut(id) {
                pipeline.config.monitor = *monitor;
            }
        }
        true
    }
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.pipelines.keys()
    }

    pub fn claim_voice(&mut self, voice: VoiceId, output_id: &str) -> bool {
        if !self
            .pipelines
            .get(output_id)
            .is_some_and(|p| p.config.enabled)
        {
            return false;
        }
        if let Some(old) = self.voice_owner.insert(voice, output_id.to_owned()) {
            if let Some(previous) = self.pipelines.get_mut(&old) {
                previous.remove_voice(voice);
            }
        }
        self.pipelines
            .get_mut(output_id)
            .expect("checked above")
            .add_voice(voice);
        true
    }
    pub fn release_voice(&mut self, voice: VoiceId) -> Option<String> {
        let id = self.voice_owner.remove(&voice)?;
        if let Some(p) = self.pipelines.get_mut(&id) {
            p.remove_voice(voice);
        }
        Some(id)
    }
    pub fn owner_of(&self, voice: VoiceId) -> Option<&str> {
        self.voice_owner.get(&voice).map(String::as_str)
    }
    pub fn clear_voices(&mut self) {
        self.voice_owner.clear();
        for p in self.pipelines.values_mut() {
            p.voices.clear();
        }
    }

    /// Release only voices owned by retired/disabled outputs.
    pub fn release_outputs(&mut self, ids: &HashSet<String>) {
        let voices: Vec<_> = self
            .voice_owner
            .iter()
            .filter(|(_, id)| ids.contains(*id))
            .map(|(voice, _)| *voice)
            .collect();
        for voice in voices {
            self.release_voice(voice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::{OutputDestination, OutputSinkKind};
    use uuid::Uuid;

    fn d(id: &str, enabled: bool) -> OutputDestination {
        OutputDestination {
            id: id.into(),
            name: id.into(),
            sink_kind: OutputSinkKind::Display,
            network: Default::default(),
            monitor: None,
            floating_window: None,
            enabled,
            always_on_top: false,
            hide_cursor: false,
            transform: OutputTransform::default(),
            fullscreen_locked: true,
        }
    }
    #[test]
    fn resolves_requested_then_enabled_default() {
        let r = OutputRegistry::new(&[d("a", true), d("b", true)], "b");
        assert_eq!(r.resolve(Some("a")).as_deref(), Some("a"));
        assert_eq!(r.resolve(Some("missing")), None);
        assert_eq!(r.resolve(None).as_deref(), Some("b"));
    }
    #[test]
    fn voice_ownership_is_instance_scoped_and_reconciles_removal() {
        let mut r = OutputRegistry::new(&[d("a", true), d("b", true)], "a");
        let v = Uuid::new_v4();
        assert!(r.claim_voice(v, "b"));
        assert_eq!(r.owner_of(v), Some("b"));
        r.update(&[d("a", true)], "a");
        assert_eq!(r.owner_of(v), None);
    }

    #[test]
    fn disabled_output_cannot_claim_voice() {
        let mut r = OutputRegistry::new(&[d("disabled", false)], "disabled");
        assert!(!r.claim_voice(Uuid::new_v4(), "disabled"));
    }

    #[test]
    fn window_preferences_are_copied_into_pipeline_config() {
        let mut destination = d("screen", true);
        destination.always_on_top = true;
        destination.hide_cursor = true;

        let config = OutputPipelineConfig::from_destination(&destination);
        assert!(config.always_on_top);
        assert!(config.hide_cursor);
    }

    #[test]
    fn monitor_update_changes_only_the_selected_registry_destination() {
        let mut registry =
            OutputRegistry::new(&[d("main", true), d("confidence", true)], "main");
        assert!(registry.set_monitor("confidence", Some(2)));
        assert_eq!(registry.pipeline("confidence").unwrap().config.monitor, Some(2));
        assert_eq!(registry.pipeline("main").unwrap().config.monitor, None);
        assert!(!registry.set_monitor("missing", Some(1)));
    }

    #[test]
    fn monitor_batch_update_is_atomic_for_unknown_destination() {
        let mut registry =
            OutputRegistry::new(&[d("main", true), d("confidence", true)], "main");
        let assignments = vec![("confidence".to_owned(), Some(2)), ("missing".to_owned(), None)];
        assert!(!registry.set_monitors(&assignments));
        assert_eq!(registry.pipeline("confidence").unwrap().config.monitor, None);
    }
}
