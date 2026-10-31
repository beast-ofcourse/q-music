//! Installed-provider registry: grouping, selection order, health, settings.
//!
//! The registry answers four questions the rest of the host keeps asking:
//!
//! - **Who can do this job?** Providers are grouped by the kinds their manifest
//!   declares, in priority order.
//! - **Who does it next?** [`ProviderRegistry::select`] returns the first
//!   provider that is enabled, healthy and capable. The order is explicit, so
//!   two runs with the same state always pick the same provider.
//! - **Who is broken?** Three consecutive failures mark a provider unhealthy and
//!   it is skipped until a retry clears the flag. Health lives in memory only:
//!   a stale "unhealthy" verdict surviving a crash would silently disable a
//!   provider that is fine again after a restart.
//! - **What settings does that provider receive?** Manifest defaults, then the
//!   user's values, then the selected quality's overrides. Each layer only
//!   replaces the keys it names, so a quality override never wipes a global key.
//!
//! Everything the registry persists lives in [`ProviderState`], a plain
//! serialisable document. The registry never touches the filesystem itself; the
//! store layer decides where that document lives.

use crate::manifest::{
    Manifest, QualityOption, PROVIDER_DOWNLOAD, PROVIDER_LYRICS, PROVIDER_METADATA,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Consecutive failures that mark a provider unhealthy.
pub const FAILURES_BEFORE_UNHEALTHY: u32 = 3;

/// Quality id used when a manifest declares no `qualityOptions[]`.
pub const DEFAULT_QUALITY_ID: &str = "best";

/// Label shown for that synthetic option.
pub const DEFAULT_QUALITY_LABEL: &str = "Default quality";

/// One capability a provider can serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderKind {
    Metadata,
    Download,
    Lyrics,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 3] = [
        ProviderKind::Metadata,
        ProviderKind::Download,
        ProviderKind::Lyrics,
    ];

    /// The `type` string a manifest uses for this kind.
    pub fn manifest_kind(self) -> &'static str {
        match self {
            Self::Metadata => PROVIDER_METADATA,
            Self::Download => PROVIDER_DOWNLOAD,
            Self::Lyrics => PROVIDER_LYRICS,
        }
    }

    /// Short name for logs, settings and the UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Download => "download",
            Self::Lyrics => "lyrics",
        }
    }

    pub fn from_manifest_kind(kind: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.manifest_kind() == kind)
    }
}

/// Failure streak for one provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Health {
    consecutive_failures: u32,
}

impl Health {
    pub fn consecutive_failures(self) -> u32 {
        self.consecutive_failures
    }

    pub fn is_healthy(self) -> bool {
        self.consecutive_failures < FAILURES_BEFORE_UNHEALTHY
    }

    /// Count one failed call. Saturates rather than overflowing: the exact
    /// number past the threshold does not change any decision.
    pub fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Manual retry: forget the streak and try again.
    pub fn clear(&mut self) {
        self.consecutive_failures = 0;
    }
}

/// One package the host has installed, plus the decisions taken about it.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledProvider {
    pub manifest: Manifest,
    pub enabled: bool,
    pub health: Health,
}

impl InstalledProvider {
    pub fn new(manifest: Manifest) -> Self {
        Self {
            manifest,
            enabled: true,
            health: Health::default(),
        }
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn display_name(&self) -> &str {
        self.manifest.display_name()
    }

    pub fn provides(&self, kind: ProviderKind) -> bool {
        self.manifest
            .kinds
            .iter()
            .any(|declared| declared == kind.manifest_kind())
    }

    /// Enabled and currently trusted to be called.
    pub fn is_selectable(&self) -> bool {
        self.enabled && self.health.is_healthy()
    }

    /// The quality menu for this provider.
    ///
    /// A package with no `qualityOptions[]` still has to be downloadable, so it
    /// gets exactly one synthetic option. Treating "no menu" as "no quality"
    /// would make the queue reject a perfectly valid provider.
    pub fn quality_options(&self) -> Vec<QualityChoice> {
        if self.manifest.quality_options.is_empty() {
            return vec![QualityChoice {
                id: DEFAULT_QUALITY_ID.to_string(),
                label: DEFAULT_QUALITY_LABEL.to_string(),
                description: None,
                kind: None,
                settings: BTreeMap::new(),
                is_default: true,
            }];
        }

        self.manifest
            .quality_options
            .iter()
            .enumerate()
            .map(|(index, option)| quality_choice(option, index == 0))
            .collect()
    }

    /// The quality id this provider will use: the persisted choice when it is
    /// still offered, otherwise the first declared option.
    pub fn quality_id(&self, chosen: Option<&str>) -> String {
        let options = self.quality_options();
        match chosen {
            Some(id) if options.iter().any(|option| option.id == id) => id.to_string(),
            _ => options
                .first()
                .map(|option| option.id.clone())
                .unwrap_or_else(|| DEFAULT_QUALITY_ID.to_string()),
        }
    }

    /// Settings ready for `initialize`, with `chosen` quality's overrides
    /// applied on top of manifest defaults and the user's values.
    pub fn resolved_settings(
        &self,
        chosen: Option<&str>,
        user_values: &BTreeMap<String, Value>,
    ) -> Map<String, Value> {
        let quality_id = self.quality_id(chosen);
        resolve_settings(&self.manifest, user_values, &quality_id)
    }
}

/// One entry of the quality menu, with its override map exposed.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityChoice {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: Option<String>,
    /// Settings this quality replaces on top of the global values.
    pub settings: BTreeMap<String, Value>,
    pub is_default: bool,
}

fn quality_choice(option: &QualityOption, is_default: bool) -> QualityChoice {
    QualityChoice {
        id: option.id.clone(),
        label: option
            .label
            .clone()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| option.id.clone()),
        description: option.description.clone(),
        kind: option.kind.clone(),
        settings: option.settings.clone(),
        is_default,
    }
}

/// Resolve one provider's settings from three layers.
///
/// Order, lowest precedence first:
/// 1. `settings[].default` from the manifest.
/// 2. The user's stored values, filtered to keys the manifest still declares so
///    a removed setting cannot be resurrected from old persistence.
/// 3. The selected quality's overrides, applied verbatim because they come from
///    the package itself.
///
/// An unknown `quality` applies no overrides instead of failing: the UI may
/// hold an id from a previous version of the package, and falling back to the
/// global values is the only behaviour that keeps working.
pub fn resolve_settings(
    manifest: &Manifest,
    user_values: &BTreeMap<String, Value>,
    quality: &str,
) -> Map<String, Value> {
    let declared: BTreeSet<&str> = manifest
        .settings
        .iter()
        .map(|setting| setting.key.as_str())
        .collect();

    let mut resolved = manifest.default_settings();

    for (key, value) in user_values {
        if declared.contains(key.as_str()) {
            resolved.insert(key.clone(), value.clone());
        }
    }

    if let Some(option) = manifest
        .quality_options
        .iter()
        .find(|option| option.id == quality)
    {
        for (key, value) in &option.settings {
            resolved.insert(key.clone(), value.clone());
        }
    }

    resolved
}

/// The persisted half of the registry. Health is deliberately absent.
///
/// `priority` is a full permutation of the installed provider names, highest
/// priority first. `values` holds the user's settings per provider. `quality`
/// holds the selected quality per provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderState {
    #[serde(default)]
    pub priority: Vec<String>,
    /// Names the user switched off. Absence means enabled.
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub quality: BTreeMap<String, String>,
    #[serde(default)]
    pub values: BTreeMap<String, BTreeMap<String, Value>>,
}

impl ProviderState {
    pub fn is_enabled(&self, name: &str) -> bool {
        !self.disabled.iter().any(|disabled| disabled == name)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("provider `{name}` is already installed")]
    Duplicate { name: String },

    #[error("provider `{name}` is not installed")]
    UnknownProvider { name: String },

    #[error("priority list is not a permutation of the installed providers")]
    InvalidPriority {
        missing: Vec<String>,
        unknown: Vec<String>,
        duplicated: Vec<String>,
    },

    #[error("provider `{name}` does not offer quality `{quality}`")]
    UnknownQuality { name: String, quality: String },
}

/// Providers grouped by capability, each group in priority order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderGroups<'a> {
    pub metadata: Vec<&'a InstalledProvider>,
    pub download: Vec<&'a InstalledProvider>,
    pub lyrics: Vec<&'a InstalledProvider>,
}

/// Installed providers plus the persisted decisions about them.
#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<InstalledProvider>,
    state: ProviderState,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild a registry from manifests and a previously saved state.
    ///
    /// The state's priority list is normalised rather than trusted: unknown
    /// names are dropped and installed names it forgot are appended in install
    /// order. A corrupt or outdated document therefore degrades to a working
    /// order instead of hiding providers.
    pub fn restore(manifests: Vec<Manifest>, state: ProviderState) -> Self {
        let mut registry = Self::new();
        for manifest in manifests {
            // Duplicates in the caller's list are a bug, not a user decision;
            // the first installation wins.
            if !registry.providers.iter().any(|entry| entry.name() == manifest.name) {
                registry.providers.push(InstalledProvider::new(manifest));
            }
        }

        registry.state = state;
        for provider in &mut registry.providers {
            provider.enabled = registry.state.is_enabled(provider.name());
        }
        registry.normalise_priority();
        registry
    }

    pub fn providers(&self) -> &[InstalledProvider] {
        &self.providers
    }

    pub fn get(&self, name: &str) -> Option<&InstalledProvider> {
        self.providers.iter().find(|entry| entry.name() == name)
    }

    pub fn state(&self) -> &ProviderState {
        &self.state
    }

    /// Install a package. New providers start last, so installing one never
    /// silently outranks a provider the user already trusts.
    pub fn install(&mut self, manifest: Manifest) -> Result<(), RegistryError> {
        if self.get(&manifest.name).is_some() {
            return Err(RegistryError::Duplicate {
                name: manifest.name,
            });
        }

        let name = manifest.name.clone();
        self.providers.push(InstalledProvider::new(manifest));
        self.state.priority.push(name);
        Ok(())
    }

    pub fn uninstall(&mut self, name: &str) -> bool {
        let before = self.providers.len();
        self.providers.retain(|entry| entry.name() != name);
        if self.providers.len() == before {
            return false;
        }

        self.state.priority.retain(|entry| entry != name);
        self.state.disabled.retain(|entry| entry != name);
        self.state.quality.remove(name);
        self.state.values.remove(name);
        true
    }

    /// Names in priority order, highest first.
    pub fn priority(&self) -> Vec<String> {
        self.effective_priority()
            .into_iter()
            .filter_map(|name| self.get(&name))
            .map(|entry| entry.name().to_string())
            .collect()
    }

    /// Replace the whole order. The list must name every installed provider
    /// exactly once: a partial list would leave selection dependent on install
    /// order, which is exactly the ambiguity priority exists to remove.
    pub fn set_priority(&mut self, names: &[String]) -> Result<(), RegistryError> {
        let mut duplicated = Vec::new();
        let mut seen = BTreeSet::new();
        for name in names {
            if !seen.insert(name.as_str()) {
                duplicated.push(name.clone());
            }
        }

        let installed: BTreeSet<&str> = self
            .providers
            .iter()
            .map(|entry| entry.name())
            .collect();

        let unknown: Vec<String> = seen
            .iter()
            .filter(|name| !installed.contains(**name))
            .map(|name| (*name).to_string())
            .collect();

        let missing: Vec<String> = installed
            .iter()
            .filter(|name| !seen.contains(**name))
            .map(|name| (*name).to_string())
            .collect();

        if !unknown.is_empty() || !missing.is_empty() || !duplicated.is_empty() {
            return Err(RegistryError::InvalidPriority {
                missing,
                unknown,
                duplicated,
            });
        }

        self.state.priority = names.to_vec();
        Ok(())
    }

    /// Move one provider to the front of the queue.
    pub fn promote(&mut self, name: &str) -> Result<(), RegistryError> {
        self.require(name)?;
        let mut order = self.priority();
        order.retain(|entry| entry != name);
        order.insert(0, name.to_string());
        self.set_priority(&order)
    }

    /// Move one provider one step towards the front.
    pub fn move_up(&mut self, name: &str) -> Result<(), RegistryError> {
        self.require(name)?;
        let mut order = self.priority();
        if let Some(index) = order.iter().position(|entry| entry == name) {
            if index > 0 {
                order.swap(index, index - 1);
            }
        }
        self.set_priority(&order)
    }

    /// Every provider that can serve `kind`, highest priority first, including
    /// ones that are disabled or unhealthy so the UI can list them with status.
    pub fn providers_for(&self, kind: ProviderKind) -> Vec<&InstalledProvider> {
        self.ordered()
            .into_iter()
            .filter(|entry| entry.provides(kind))
            .collect()
    }

    /// All three capability groups, each in priority order.
    pub fn groups(&self) -> ProviderGroups<'_> {
        ProviderGroups {
            metadata: self.providers_for(ProviderKind::Metadata),
            download: self.providers_for(ProviderKind::Download),
            lyrics: self.providers_for(ProviderKind::Lyrics),
        }
    }

    /// The provider that should handle `kind` right now.
    ///
    /// Disabled, unhealthy and incapable providers are skipped. `None` means
    /// the capability is genuinely unavailable, which callers must report
    /// rather than silently degrade.
    pub fn select(&self, kind: ProviderKind) -> Option<&InstalledProvider> {
        self.providers_for(kind)
            .into_iter()
            .find(|entry| entry.is_selectable())
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<(), RegistryError> {
        let provider = self.require_mut(name)?;
        provider.enabled = enabled;

        self.state.disabled.retain(|entry| entry != name);
        if !enabled {
            self.state.disabled.push(name.to_string());
        }
        Ok(())
    }

    pub fn record_failure(&mut self, name: &str) -> Result<(), RegistryError> {
        let provider = self.require_mut(name)?;
        provider.health.record_failure();
        Ok(())
    }

    pub fn record_success(&mut self, name: &str) -> Result<(), RegistryError> {
        let provider = self.require_mut(name)?;
        provider.health.record_success();
        Ok(())
    }

    /// Clear the failure streak after a manual retry.
    pub fn retry(&mut self, name: &str) -> Result<(), RegistryError> {
        let provider = self.require_mut(name)?;
        provider.health.clear();
        Ok(())
    }

    pub fn quality_options(&self, name: &str) -> Result<Vec<QualityChoice>, RegistryError> {
        Ok(self.require(name)?.quality_options())
    }

    pub fn selected_quality(&self, name: &str) -> Result<String, RegistryError> {
        let provider = self.require(name)?;
        Ok(provider.quality_id(self.state.quality.get(name).map(String::as_str)))
    }

    pub fn set_quality(&mut self, name: &str, quality: &str) -> Result<(), RegistryError> {
        let provider = self.require(name)?;
        if !provider
            .quality_options()
            .iter()
            .any(|option| option.id == quality)
        {
            return Err(RegistryError::UnknownQuality {
                name: name.to_string(),
                quality: quality.to_string(),
            });
        }

        self.state
            .quality
            .insert(name.to_string(), quality.to_string());
        Ok(())
    }

    /// Settings for the provider's currently selected quality.
    pub fn settings(&self, name: &str) -> Result<Map<String, Value>, RegistryError> {
        let provider = self.require(name)?;
        let quality = self.selected_quality(name)?;
        let empty = BTreeMap::new();
        let values = self.state.values.get(name).unwrap_or(&empty);
        Ok(provider.resolved_settings(Some(&quality), values))
    }

    fn require(&self, name: &str) -> Result<&InstalledProvider, RegistryError> {
        self.get(name).ok_or_else(|| RegistryError::UnknownProvider {
            name: name.to_string(),
        })
    }

    fn require_mut(&mut self, name: &str) -> Result<&mut InstalledProvider, RegistryError> {
        self.providers
            .iter_mut()
            .find(|entry| entry.name() == name)
            .ok_or_else(|| RegistryError::UnknownProvider {
                name: name.to_string(),
            })
    }

    fn ordered(&self) -> Vec<&InstalledProvider> {
        self.effective_priority()
            .into_iter()
            .filter_map(|name| self.get(&name))
            .collect()
    }

    /// Installed names in priority order. Installed providers absent from the
    /// saved list are appended in install order, so the result always contains
    /// every provider exactly once.
    fn effective_priority(&self) -> Vec<String> {
        let mut ordered: Vec<String> = Vec::with_capacity(self.providers.len());
        for name in &self.state.priority {
            if self.get(name).is_some() && !ordered.iter().any(|entry| entry == name) {
                ordered.push(name.clone());
            }
        }

        for provider in &self.providers {
            if !ordered.iter().any(|entry| entry == provider.name()) {
                ordered.push(provider.name().to_string());
            }
        }

        ordered
    }

    fn normalise_priority(&mut self) {
        self.state.priority = self.effective_priority();

        let installed: BTreeSet<&str> = self
            .providers
            .iter()
            .map(|entry| entry.name())
            .collect();

        self.state
            .disabled
            .retain(|name| installed.contains(name.as_str()));
        self.state
            .quality
            .retain(|name, _| installed.contains(name.as_str()));
        self.state
            .values
            .retain(|name, _| installed.contains(name.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY: &str = include_str!("../fixtures/sources/spotify-web.manifest.json");
    const APPLE_MUSIC: &str = include_str!("../fixtures/sources/apple-music.manifest.json");
    const AMAZON: &str = include_str!("../fixtures/sources/amazon.manifest.json");
    const TIDAL: &str = include_str!("../fixtures/sources/tidal-web.manifest.json");

    fn manifest(json: &str) -> Manifest {
        Manifest::parse(json).expect("fixture manifest parses")
    }

    /// The four lawful fixtures, installed in registry order.
    fn registry() -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        for json in [SPOTIFY, APPLE_MUSIC, AMAZON, TIDAL] {
            registry.install(manifest(json)).expect("installs");
        }
        registry
    }

    fn names(providers: Vec<&InstalledProvider>) -> Vec<&str> {
        providers
            .into_iter()
            .map(InstalledProvider::name)
            .collect()
    }

    #[test]
    fn providers_group_by_declared_kind_in_priority_order() {
        let registry = registry();
        let groups = registry.groups();

        assert_eq!(
            names(groups.metadata),
            vec!["spotify-web", "apple-music", "amazon", "tidal-web"]
        );
        assert_eq!(names(groups.download), vec!["amazon", "tidal-web"]);
        assert_eq!(names(groups.lyrics), vec!["apple-music"]);

        assert_eq!(
            registry
                .select(ProviderKind::Metadata)
                .map(InstalledProvider::name),
            Some("spotify-web")
        );
        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("amazon")
        );
        assert_eq!(
            registry
                .select(ProviderKind::Lyrics)
                .map(InstalledProvider::name),
            Some("apple-music")
        );
    }

    #[test]
    fn selection_is_deterministic_across_repeated_calls() {
        let registry = registry();
        let picks: Vec<&str> = (0..5)
            .map(|_| {
                registry
                    .select(ProviderKind::Download)
                    .expect("a download provider")
                    .name()
            })
            .collect();

        assert!(picks.iter().all(|name| *name == "amazon"));
    }

    #[test]
    fn reordering_priority_changes_the_winner_without_a_restart() {
        let mut registry = registry();
        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("amazon")
        );

        registry
            .set_priority(&[
                "tidal-web".to_string(),
                "amazon".to_string(),
                "spotify-web".to_string(),
                "apple-music".to_string(),
            ])
            .expect("valid permutation");

        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("tidal-web")
        );
        assert_eq!(registry.priority()[0], "tidal-web");
    }

    #[test]
    fn three_consecutive_failures_skip_a_provider_until_a_retry() {
        let mut registry = registry();

        registry.record_failure("amazon").expect("installed");
        registry.record_failure("amazon").expect("installed");
        assert_eq!(
            registry.get("amazon").expect("installed").health.consecutive_failures(),
            2
        );
        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("amazon"),
            "two failures are not enough to disqualify a provider"
        );

        registry.record_failure("amazon").expect("installed");
        assert!(!registry.get("amazon").expect("installed").health.is_healthy());
        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("tidal-web"),
            "the unhealthy provider is skipped, not fatal"
        );

        registry.retry("amazon").expect("installed");
        assert_eq!(
            registry.get("amazon").expect("installed").health.consecutive_failures(),
            0
        );
        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("amazon")
        );
    }

    #[test]
    fn a_success_clears_the_failure_streak() {
        let mut registry = registry();
        registry.record_failure("amazon").expect("installed");
        registry.record_failure("amazon").expect("installed");
        registry.record_success("amazon").expect("installed");

        assert_eq!(
            registry.get("amazon").expect("installed").health.consecutive_failures(),
            0
        );
    }

    #[test]
    fn a_disabled_provider_is_skipped_but_still_listed() {
        let mut registry = registry();
        registry.set_enabled("spotify-web", false).expect("installed");

        assert_eq!(
            registry
                .select(ProviderKind::Metadata)
                .map(InstalledProvider::name),
            Some("apple-music")
        );
        assert_eq!(
            names(registry.providers_for(ProviderKind::Metadata)),
            vec!["spotify-web", "apple-music", "amazon", "tidal-web"],
            "the UI still needs to see the disabled provider"
        );
        assert!(!registry.state().is_enabled("spotify-web"));

        registry.set_enabled("spotify-web", true).expect("installed");
        assert_eq!(
            registry
                .select(ProviderKind::Metadata)
                .map(InstalledProvider::name),
            Some("spotify-web")
        );
    }

    #[test]
    fn priority_must_name_every_installed_provider_exactly_once() {
        let mut registry = registry();

        let error = registry
            .set_priority(&["tidal-web".to_string(), "tidal-web".to_string()])
            .expect_err("partial, duplicated and unknown");
        let RegistryError::InvalidPriority {
            missing,
            unknown,
            duplicated,
        } = error
        else {
            panic!("expected InvalidPriority");
        };

        assert_eq!(duplicated, vec!["tidal-web"]);
        assert!(unknown.is_empty());
        assert_eq!(
            missing,
            vec!["amazon", "apple-music", "spotify-web"],
            "missing names are reported, not guessed"
        );

        let error = registry
            .set_priority(&[
                "ghost".to_string(),
                "amazon".to_string(),
                "tidal-web".to_string(),
                "spotify-web".to_string(),
                "apple-music".to_string(),
            ])
            .expect_err("unknown name");
        let RegistryError::InvalidPriority { unknown, .. } = error else {
            panic!("expected InvalidPriority");
        };
        assert_eq!(unknown, vec!["ghost"]);

        // The rejected calls left the order untouched.
        assert_eq!(registry.priority()[0], "spotify-web");
    }

    #[test]
    fn a_provider_without_quality_options_yields_one_default_option() {
        let registry = registry();
        let options = registry.quality_options("spotify-web").expect("installed");

        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, DEFAULT_QUALITY_ID);
        assert_eq!(options[0].label, DEFAULT_QUALITY_LABEL);
        assert!(options[0].is_default);
        assert!(options[0].settings.is_empty());

        assert_eq!(
            registry.selected_quality("spotify-web").expect("installed"),
            DEFAULT_QUALITY_ID
        );
    }

    #[test]
    fn declared_quality_options_keep_manifest_order_and_first_is_default() {
        let registry = registry();
        let options = registry.quality_options("tidal-web").expect("installed");

        assert_eq!(
            options.iter().map(|option| option.id.as_str()).collect::<Vec<_>>(),
            vec!["DOLBY_ATMOS", "HI_RES_LOSSLESS", "LOSSLESS", "HIGH", "LOW"]
        );
        assert!(options[0].is_default);
        assert!(options[1..].iter().all(|option| !option.is_default));
        assert_eq!(options[0].label, "Dolby Atmos");
        assert_eq!(options[0].kind.as_deref(), Some("spatial"));

        let mut registry = registry;
        registry.set_quality("tidal-web", "HI_RES_LOSSLESS").expect("offered");
        assert_eq!(
            registry.selected_quality("tidal-web").expect("installed"),
            "HI_RES_LOSSLESS"
        );
        assert_eq!(
            registry.set_quality("tidal-web", "cassette").unwrap_err(),
            RegistryError::UnknownQuality {
                name: "tidal-web".to_string(),
                quality: "cassette".to_string(),
            }
        );
    }

    #[test]
    fn a_quality_override_merges_without_clobbering_global_settings() {
        let manifest = manifest(
            r#"{
              "name": "quality-test",
              "version": "1.0.0",
              "type": ["download_provider"],
              "settings": [
                { "key": "apiKey", "label": "API key", "type": "string", "default": "global-key" },
                { "key": "region", "label": "Region", "type": "select", "default": "US", "options": ["US", "GB"] }
              ],
              "qualityOptions": [
                { "id": "PREMIUM_FLAC", "label": "Premium FLAC", "kind": "lossless", "settings": { "apiKey": "premium-key" } },
                { "id": "FREE", "label": "Free", "settings": {} }
              ]
            }"#,
        );

        let user_values: BTreeMap<String, Value> = [
            ("apiKey".to_string(), Value::String("user-key".to_string())),
            ("region".to_string(), Value::String("GB".to_string())),
            // A setting the manifest no longer declares must not come back.
            ("removedKey".to_string(), Value::Bool(true)),
        ]
        .into_iter()
        .collect();

        let premium = resolve_settings(&manifest, &user_values, "PREMIUM_FLAC");
        assert_eq!(premium["apiKey"], Value::String("premium-key".to_string()));
        assert_eq!(premium["region"], Value::String("GB".to_string()));
        assert!(!premium.contains_key("removedKey"));

        let free = resolve_settings(&manifest, &user_values, "FREE");
        assert_eq!(free["apiKey"], Value::String("user-key".to_string()));
        assert_eq!(free["region"], Value::String("GB".to_string()));

        // Unknown quality falls back to the global values instead of failing.
        let unknown = resolve_settings(&manifest, &user_values, "from-a-future-version");
        assert_eq!(unknown["apiKey"], Value::String("user-key".to_string()));
    }

    #[test]
    fn manifest_defaults_apply_when_the_user_has_no_values() {
        let registry = registry();
        let settings = registry.settings("apple-music").expect("installed");

        assert_eq!(settings["storefront"], Value::String("us".to_string()));
        assert_eq!(
            settings["lyricsTranslation"],
            Value::String(String::new())
        );
    }

    #[test]
    fn persisted_state_round_trips_and_restores_the_same_selection() {
        let mut registry = registry();
        registry
            .set_priority(&[
                "tidal-web".to_string(),
                "amazon".to_string(),
                "apple-music".to_string(),
                "spotify-web".to_string(),
            ])
            .expect("valid permutation");
        registry.set_quality("tidal-web", "LOSSLESS").expect("offered");
        registry.set_enabled("amazon", false).expect("installed");
        registry.record_failure("tidal-web").expect("installed");
        registry.record_failure("tidal-web").expect("installed");

        let document = serde_json::to_string(registry.state()).expect("serialises");
        let restored_state: ProviderState = serde_json::from_str(&document).expect("deserialises");

        assert!(
            !document.contains("consecutiveFailures"),
            "health must not survive a restart: {document}"
        );

        let restored = ProviderRegistry::restore(
            [SPOTIFY, APPLE_MUSIC, AMAZON, TIDAL]
                .into_iter()
                .map(manifest)
                .collect(),
            restored_state,
        );

        assert_eq!(
            restored.priority(),
            vec!["tidal-web", "amazon", "apple-music", "spotify-web"]
        );
        assert_eq!(
            restored.selected_quality("tidal-web").expect("installed"),
            "LOSSLESS"
        );
        assert!(!restored.state().is_enabled("amazon"));
        assert_eq!(
            restored.get("tidal-web").expect("installed").health.consecutive_failures(),
            0,
            "a fresh process starts with a clean health slate"
        );

        // Tidal is unhealthy-free now and first in order, but amazon is disabled
        // and tidal is not; metadata selection is unaffected by both.
        assert_eq!(
            restored
                .select(ProviderKind::Metadata)
                .map(InstalledProvider::name),
            Some("tidal-web")
        );
    }

    #[test]
    fn restoring_a_stale_state_keeps_every_installed_provider_visible() {
        let stale = ProviderState {
            priority: vec!["ghost".to_string(), "tidal-web".to_string()],
            disabled: vec!["ghost".to_string()],
            quality: BTreeMap::new(),
            values: BTreeMap::new(),
        };

        let registry = ProviderRegistry::restore(
            [SPOTIFY, AMAZON, TIDAL].into_iter().map(manifest).collect(),
            stale,
        );

        assert_eq!(
            registry.priority(),
            vec!["tidal-web", "spotify-web", "amazon"],
            "unknown names are dropped, unlisted installs are appended in install order"
        );
        assert!(registry.state().disabled.is_empty());
        assert_eq!(
            registry
                .select(ProviderKind::Download)
                .map(InstalledProvider::name),
            Some("tidal-web")
        );
    }

    #[test]
    fn installing_a_provider_places_it_last_and_duplicates_are_refused() {
        let mut registry = ProviderRegistry::new();
        registry.install(manifest(SPOTIFY)).expect("installs");
        registry.install(manifest(AMAZON)).expect("installs");

        assert_eq!(registry.priority(), vec!["spotify-web", "amazon"]);

        let error = registry.install(manifest(SPOTIFY)).unwrap_err();
        assert_eq!(
            error,
            RegistryError::Duplicate {
                name: "spotify-web".to_string()
            }
        );
        assert_eq!(registry.providers().len(), 2);
    }

    #[test]
    fn uninstalling_prunes_every_persisted_decision() {
        let mut registry = registry();
        registry.set_enabled("amazon", false).expect("installed");
        registry.set_quality("tidal-web", "HIGH").expect("offered");
        registry
            .state
            .values
            .insert("tidal-web".to_string(), BTreeMap::new());

        assert!(registry.uninstall("tidal-web"));
        assert!(!registry.uninstall("tidal-web"));

        assert!(!registry.priority().iter().any(|name| name == "tidal-web"));
        assert!(!registry.state().quality.contains_key("tidal-web"));
        assert!(!registry.state().values.contains_key("tidal-web"));
        assert_eq!(registry.providers().len(), 3);
    }

    #[test]
    fn promote_and_move_up_reorder_without_touching_other_providers() {
        let mut registry = registry();

        registry.move_up("tidal-web").expect("installed");
        assert_eq!(
            registry.priority(),
            vec!["spotify-web", "apple-music", "tidal-web", "amazon"]
        );
        registry.move_up("spotify-web").expect("installed");
        assert_eq!(
            registry.priority(),
            vec!["spotify-web", "apple-music", "tidal-web", "amazon"],
            "already first is a no-op"
        );

        registry.promote("amazon").expect("installed");
        assert_eq!(
            registry.priority(),
            vec!["amazon", "spotify-web", "apple-music", "tidal-web"]
        );
        assert_eq!(
            registry.move_up("ghost").unwrap_err(),
            RegistryError::UnknownProvider {
                name: "ghost".to_string()
            }
        );
    }

    #[test]
    fn provider_kind_round_trips_through_manifest_strings() {
        for kind in ProviderKind::ALL {
            assert_eq!(
                ProviderKind::from_manifest_kind(kind.manifest_kind()),
                Some(kind)
            );
        }
        assert_eq!(ProviderKind::from_manifest_kind("visualiser_provider"), None);
        assert_eq!(ProviderKind::Metadata.as_str(), "metadata");
    }
}
